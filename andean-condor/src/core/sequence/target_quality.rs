use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{self, Arc, atomic::AtomicBool},
    thread,
    time::SystemTime,
};

use anyhow::{Result, bail};
use itertools::Itertools;
use thiserror::Error;
use tracing::{debug, error, trace};

use crate::{
    core::{
        Condor,
        encoder::EncoderCapability,
        input::Input,
        sequence::{
            Sequence,
            SequenceCompletion,
            SequenceDetails,
            SequenceStatus,
            Status,
            parallel_encoder::{ParallelEncoder, Task as ParallelEncoderTask},
            scene_concatenator::SceneConcatenator,
        },
    },
    models::{
        encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
        input::{Input as InputModel, VapourSynthScriptSource},
        sequence::{
            SequenceConfigHandler,
            SequenceDataHandler,
            parallel_encoder::{BufferStrategy, ParallelEncoderConfigHandler},
            scene_concatenator::{ConcatMethod, SceneConcatenatorConfigHandler},
            target_quality::{
                TargetQualityConfig,
                TargetQualityConfigHandler,
                TargetQualityData,
                TargetQualityDataHandler,
                types::{InterpolationMethod, QualityMetric, QualityPass},
            },
        },
    },
    utils::interpolators,
    vapoursynth::{
        get_core,
        plugins::{
            MetricPluginFunction,
            PluginFunction,
            ffms2::Source,
            resize::bicubic::Bicubic,
            standard::{splice::Splice, trim::Trim},
            vship::{
                butteraugli::BUTTERAUGLI,
                cvvdp::CVVDP,
                ssimulacra2::SSIMULACRA2 as VSHIPSSIMULACRA2,
            },
            vszip::{ssimulacra2::SSIMULACRA2, xpsnr::XPSNR},
        },
        script_builder::{VapourSynthPluginScript, script::VapourSynthScript},
    },
};

static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Target Quality",
    description: "Determine the optimal quantizer for a given video quality metric score per \
                  scene.",
    version:     "0.0.1",
};

pub struct TargetQuality {
    pub input:        Option<Input>,
    pub metric_input: Option<Input>,
}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for TargetQuality
where
    DataHandler: SequenceDataHandler + TargetQualityDataHandler,
    ConfigHandler: SequenceConfigHandler
        + ParallelEncoderConfigHandler
        + SceneConcatenatorConfigHandler
        + TargetQualityConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];

        // Ensure all the scene encoders are validated
        for scene in &condor.scenes {
            scene.encoder.validate()?;
        }

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];

        let parallel_encoder_config = condor.sequence_config.parallel_encoder()?;
        let config = condor.sequence_config.target_quality()?;

        // Ensure scenes is not empty
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(TargetQualityError::ScenesEmpty));
            return Ok(((), warnings));
        }
        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(input_model) = &parallel_encoder_config.input {
            &mut Input::from_data(input_model)?
        } else {
            &mut condor.input
        };
        let metric_input = if let Some(input) = self.metric_input.as_mut() {
            Some(input)
        } else if let Some(config) = config
            && let Some(metric_input) = &config.metric_input
        {
            Some(&mut Input::from_data(metric_input)?)
        } else {
            None
        };
        // Initialize input by getting clip_info. For VapourSynth inputs, this may begin
        // a lengthy caching process, hence the separation between validate and
        // initialize.
        progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id:         DETAILS.name.to_owned(),
            completion: SequenceCompletion::Custom {
                name:      DETAILS.name.to_owned(),
                completed: 0.0,
                total:     if metric_input.is_some() { 2.0 } else { 1.0 },
            },
        }))?;
        input.clip_info()?;
        if let Some(metric_input) = metric_input {
            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id:         DETAILS.name.to_owned(),
                completion: SequenceCompletion::Custom {
                    name:      DETAILS.name.to_owned(),
                    completed: 1.0,
                    total:     2.0,
                },
            }))?;
            metric_input.clip_info()?;
        }
        progress_tx.send(SequenceStatus::Whole(Status::Completed {
            id: DETAILS.name.to_owned(),
        }))?;

        let sequence_directory = &parallel_encoder_config.scenes_directory.join(DETAILS.name);
        if !sequence_directory.exists() {
            std::fs::create_dir_all(sequence_directory)?;
        }

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];
        let parallel_encoder_config = condor.sequence_config.parallel_encoder()?;
        let scene_concatenator_config = condor.sequence_config.scene_concatenator()?;
        let config = condor.sequence_config.target_quality()?;
        let target_quality_directory = &parallel_encoder_config.scenes_directory.join(DETAILS.name);
        let workers = parallel_encoder_config.workers.unwrap_or(1);
        let buffer_strategy = &parallel_encoder_config.buffer_strategy;
        let condor_data = condor.as_data();

        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(TargetQualityError::ScenesEmpty));
            return Ok(((), warnings));
        }

        if config.is_none() {
            return Ok(((), warnings));
        }
        let config = config.clone().expect("TargetQualityConfig is Some");
        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(input_model) = &config.input {
            &mut Input::from_data(input_model)?
        } else if let Some(input_model) = &parallel_encoder_config.input {
            &mut Input::from_data(input_model)?
        } else {
            &mut condor.input
        };

        let mut pass = 1;

        loop {
            if pass > config.maximum_probes {
                break;
            }

            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id:         DETAILS.name.to_owned(),
                completion: SequenceCompletion::Passes {
                    completed: pass - 1,
                    total:     pass,
                },
            }))?;

            let pass_directory = target_quality_directory.join(pass.to_string());
            if !pass_directory.exists() {
                std::fs::create_dir_all(&pass_directory)?;
            }

            let tasks = condor
                .scenes
                .iter_mut()
                .enumerate()
                .map(|(index, scene)| {
                    let frame_indices =
                        config.probing.strategy.frame_indices(scene.start_frame, scene.end_frame);
                    let mut encoder = scene.encoder.clone();
                    if let Some(parameters) = config.probing.encoder_options.as_ref() {
                        encoder.parameters_mut().clear();
                        encoder.parameters_mut().extend(parameters.clone());
                    }
                    let encoder = Self::remove_psychovisual_parameters(&encoder);
                    let output = pass_directory.join(format!(
                        "{}.{}",
                        ParallelEncoder::scene_id(index),
                        encoder.output_extension()
                    ));
                    let passes = scene
                        .sequence_data
                        .get_target_quality()
                        .map_or_else(|_| TargetQualityData::default(), |tq| tq.clone())
                        .passes;

                    Task {
                        original_index: index,
                        frame_indices,
                        encoder,
                        output,
                        passes,
                    }
                })
                .filter(|task| {
                    // Skip if last probe scored within target
                    !task
                        .passes
                        .get(pass.saturating_sub(2) as usize) // Previous pass
                        .or_else(|| task.passes.last()) // Last pass
                        .is_some_and(|quality_pass| {
                            config.metric.score_within_target(
                                config.probing.statistic.calculate(&quality_pass.scores),
                            )
                        })
                })
                .map(|mut task| {
                    let quantizer_score_history = task
                        .passes
                        .iter()
                        .map(|quality_pass| {
                            (
                                quality_pass.quantizer,
                                config.probing.statistic.calculate(&quality_pass.scores),
                            )
                        })
                        .collect::<Vec<_>>();
                    let sorted_quantizer_score_history = quantizer_score_history
                        .iter()
                        .sorted_by(|(_, score1), (_, score2)| {
                            score1.partial_cmp(score2).unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .collect::<Vec<_>>();
                    let inverse_metric = matches!(config.metric, QualityMetric::BUTTERAUGLI { .. });
                    let target_score =
                        config.metric.target_range().0.midpoint(config.metric.target_range().1);
                    let lower_quantizer_bound = sorted_quantizer_score_history
                        .iter()
                        .find(|(_quantizer, score)| {
                            if inverse_metric {
                                *score < target_score
                            } else {
                                *score > target_score
                            }
                        })
                        .map_or(config.quantizer_range.0 as f64, |(quantizer, _)| *quantizer);
                    let upper_quantizer_bound = sorted_quantizer_score_history
                        .iter()
                        .rfind(|(_quantizer, score)| {
                            if inverse_metric {
                                *score > target_score
                            } else {
                                *score < target_score
                            }
                        })
                        .map_or(config.quantizer_range.1 as f64, |(quantizer, _)| *quantizer);
                    let predicted_quantizer = TargetQuality::predict_quantizer(
                        (lower_quantizer_bound, upper_quantizer_bound),
                        target_score,
                        config.interpolators,
                        &quantizer_score_history,
                        match task.encoder {
                            Encoder::X264 {
                                ..
                            }
                            | Encoder::X265 {
                                ..
                            } => 0.25,
                            Encoder::SVTAV1 {
                                ..
                            } if task
                                .encoder
                                .supports_capability(EncoderCapability::SvtAv1QuarterStepCrf) =>
                            {
                                0.25
                            },
                            _ => 1.0,
                        },
                    )?;

                    // Skip already processed quantizer
                    if quantizer_score_history
                        .iter()
                        .any(|(quantizer, _)| *quantizer == predicted_quantizer)
                    {
                        return Ok(None);
                    }

                    // Modify encoder to use predicted quantizer
                    task.encoder.set_quantizer(predicted_quantizer);

                    Ok(Some(task))
                })
                .filter_map(|result: Result<Option<Task>>| result.transpose())
                .collect::<Result<Vec<_>>>()?;

            if tasks.is_empty() {
                break;
            }

            if tasks.iter().all(|task| task.passes.len() >= pass as usize) {
                // Skip already processed pass
                pass += 1;
                continue;
            }

            debug!("Starting Pass {}", pass);

            let progress_tx_clone = progress_tx.clone();
            let (pass_progress_tx, pass_progress_rx) = sync::mpsc::channel();
            thread::spawn(move || -> Result<()> {
                for progress in pass_progress_rx {
                    match progress {
                        SequenceStatus::Whole(Status::Processing {
                            id,
                            completion,
                        }) if id == "Encode" => {
                            #[allow(clippy::collapsible_match)]
                            if let SequenceCompletion::Frames {
                                completed,
                                total,
                            } = completion
                            {
                                progress_tx_clone.send(SequenceStatus::Subprocess {
                                    parent: Status::Processing {
                                        id:         DETAILS.name.to_owned(),
                                        completion: SequenceCompletion::Passes {
                                            completed: pass,
                                            total:     config.maximum_probes,
                                        },
                                    },
                                    child:  Status::Processing {
                                        id,
                                        completion: SequenceCompletion::Frames {
                                            completed,
                                            total,
                                        },
                                    },
                                })?;
                            }
                        },
                        SequenceStatus::Whole(Status::Completed {
                            id,
                        }) if id == "Encode" => {
                            progress_tx_clone.send(SequenceStatus::Subprocess {
                                parent: Status::Processing {
                                    id:         DETAILS.name.to_owned(),
                                    completion: SequenceCompletion::Passes {
                                        completed: pass,
                                        total:     config.maximum_probes,
                                    },
                                },
                                child:  Status::Completed {
                                    id,
                                },
                            })?;
                        },
                        SequenceStatus::Whole(Status::Processing {
                            id,
                            completion,
                        }) if id == "Compare" => {
                            #[allow(clippy::collapsible_match)]
                            if let SequenceCompletion::Frames {
                                completed,
                                total,
                            } = completion
                            {
                                progress_tx_clone.send(SequenceStatus::Subprocess {
                                    parent: Status::Processing {
                                        id:         DETAILS.name.to_owned(),
                                        completion: SequenceCompletion::Passes {
                                            completed: pass,
                                            total:     config.maximum_probes,
                                        },
                                    },
                                    child:  Status::Processing {
                                        id,
                                        completion: SequenceCompletion::Frames {
                                            completed,
                                            total,
                                        },
                                    },
                                })?;
                            }
                        },
                        SequenceStatus::Whole(Status::Completed {
                            id,
                        }) if id == "Compare" => {
                            progress_tx_clone.send(SequenceStatus::Subprocess {
                                parent: Status::Processing {
                                    id:         DETAILS.name.to_owned(),
                                    completion: SequenceCompletion::Passes {
                                        completed: pass,
                                        total:     config.maximum_probes,
                                    },
                                },
                                child:  Status::Completed {
                                    id,
                                },
                            })?;
                        },
                        _ => (),
                    }
                }

                Ok(())
            });

            let (completed_tasks, pass_warnings) = Self::probe_pass(
                pass,
                target_quality_directory,
                &config,
                input,
                self.metric_input.as_mut(),
                workers,
                buffer_strategy,
                &scene_concatenator_config.method,
                tasks.as_slice(),
                pass_progress_tx,
                &cancelled,
            )?;

            for completed_task in completed_tasks {
                // Update Target Quality Passes
                condor.scenes[completed_task.original_index]
                    .sequence_data
                    .get_target_quality_mut()?
                    .passes = completed_task.passes.clone();

                // Perform additional prediction
                let quantizer_score_history = completed_task
                    .passes
                    .iter()
                    .map(|quality_pass| {
                        (
                            quality_pass.quantizer,
                            config.probing.statistic.calculate(&quality_pass.scores),
                        )
                    })
                    .collect::<Vec<_>>();
                let sorted_quantizer_score_history = quantizer_score_history
                    .iter()
                    .sorted_by(|(_, score1), (_, score2)| {
                        score1.partial_cmp(score2).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .collect::<Vec<_>>();
                let inverse_metric = matches!(config.metric, QualityMetric::BUTTERAUGLI { .. });
                let target_score =
                    config.metric.target_range().0.midpoint(config.metric.target_range().1);
                let lower_quantizer_bound = sorted_quantizer_score_history
                    .iter()
                    .find(|(_quantizer, score)| {
                        if inverse_metric {
                            *score < target_score
                        } else {
                            *score > target_score
                        }
                    })
                    .map_or(config.quantizer_range.0 as f64, |(quantizer, _)| *quantizer);
                let upper_quantizer_bound = sorted_quantizer_score_history
                    .iter()
                    .rfind(|(_quantizer, score)| {
                        if inverse_metric {
                            *score > target_score
                        } else {
                            *score < target_score
                        }
                    })
                    .map_or(config.quantizer_range.1 as f64, |(quantizer, _)| *quantizer);
                let predicted_quantizer = TargetQuality::predict_quantizer(
                    (lower_quantizer_bound, upper_quantizer_bound),
                    target_score,
                    config.interpolators,
                    &quantizer_score_history,
                    match completed_task.encoder {
                        Encoder::X264 {
                            ..
                        }
                        | Encoder::X265 {
                            ..
                        } => 0.25,
                        Encoder::SVTAV1 {
                            ..
                        } => 1.0, // TODO: Implement svt_av1_supports_quarter_steps()
                        _ => 1.0,
                    },
                )?;

                // Update Scene Encoder quantizer
                condor.scenes[completed_task.original_index]
                    .encoder
                    .set_quantizer(predicted_quantizer);

                let quality_pass = completed_task.passes.last().expect("passes is not empty");

                progress_tx.send(SequenceStatus::Subprocess {
                    parent: Status::Processing {
                        id:         DETAILS.name.to_owned(),
                        completion: SequenceCompletion::Passes {
                            completed: pass,
                            total:     config.maximum_probes,
                        },
                    },
                    child:  Status::Processing {
                        id:         "Quality".to_owned(),
                        completion: SequenceCompletion::SceneQuality {
                            index:     completed_task.original_index as u64,
                            quantizer: quality_pass.quantizer,
                            score:     config.probing.statistic.calculate(&quality_pass.scores),
                            bitrate:   quality_pass.bitrate,
                        },
                    },
                })?;
            }

            if !pass_warnings.is_empty() {
                warnings.extend(pass_warnings);
                break;
            }

            pass += 1;
            let mut data = condor_data.clone();
            data.scenes = condor.scenes.clone();
            (condor.save_callback)(data).expect("failed to save data");

            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
        }

        Ok(((), warnings))
    }
}

impl TargetQuality {
    pub const DETAILS: SequenceDetails = DETAILS;

    #[inline]
    pub fn new(input: Option<Input>, metric_input: Option<Input>) -> Self {
        Self {
            input,
            metric_input,
        }
    }

    #[inline]
    pub fn default_quantizer_range(encoder: &EncoderBase) -> (u32, u32) {
        match encoder {
            EncoderBase::AOM | EncoderBase::VPX => (5, 55),
            EncoderBase::RAV1E => (50, 140),
            EncoderBase::SVTAV1 => (5, 55),
            EncoderBase::AVM => (5, 250),
            EncoderBase::X264 | EncoderBase::X265 => (5, 35),
            EncoderBase::VVenC => (5, 35),
            EncoderBase::FFmpeg => (15, 50),
        }
    }

    /// Remove known psychovisual parameters that reduce metric accuracy.
    #[inline]
    pub fn remove_psychovisual_parameters(encoder: &Encoder) -> Encoder {
        match encoder {
            Encoder::AOM {
                executable,
                pass,
                options,
                photon_noise,
            } => {
                let psychovisual_parameters: HashMap<String, CLIParameter> =
                    std::iter::once(("film-grain-table", CLIParameter::new_string("--", "=", "")))
                        .map(|(key, value)| (key.to_owned(), value))
                        .collect();

                let mut sanitized_options = options.clone();
                for (key, value) in psychovisual_parameters {
                    if let Some((_key, unsanitized_value)) = sanitized_options.get_key_value(&key)
                        && unsanitized_value.matches(&value)
                    {
                        sanitized_options.remove(&key);
                    }
                }

                Encoder::AOM {
                    executable:   executable.clone(),
                    pass:         *pass,
                    options:      sanitized_options,
                    photon_noise: photon_noise.clone(),
                }
            },
            Encoder::RAV1E {
                executable,
                pass,
                options,
                photon_noise,
            } => {
                let psychovisual_parameters: HashMap<String, CLIParameter> = std::iter::once((
                    "photon-noise-table",
                    CLIParameter::new_string("--", "=", ""),
                ))
                .map(|(key, value)| (key.to_owned(), value))
                .collect();

                let mut sanitized_options = options.clone();
                for (key, value) in psychovisual_parameters {
                    if let Some((_key, unsanitized_value)) = sanitized_options.get_key_value(&key)
                        && unsanitized_value.matches(&value)
                    {
                        sanitized_options.remove(&key);
                    }
                }

                Encoder::RAV1E {
                    executable:   executable.clone(),
                    pass:         *pass,
                    options:      sanitized_options,
                    photon_noise: photon_noise.clone(),
                }
            },
            Encoder::VPX {
                ..
            } => encoder.clone(),
            Encoder::SVTAV1 {
                executable,
                pass,
                options,
                photon_noise,
            } => {
                let psychovisual_parameters: HashMap<String, CLIParameter> = [
                    ("fgs-table", CLIParameter::new_string("--", " ", "")),
                    ("film-grain", CLIParameter::new_number("--", " ", 0.0)),
                    (
                        "film-grain-denoise",
                        CLIParameter::new_number("--", " ", 0.0),
                    ),
                    ("psy-rd", CLIParameter::new_number("--", " ", 0.0)),
                    ("ac-bias", CLIParameter::new_number("--", " ", 0.0)),
                    ("photon-noise", CLIParameter::new_number("--", " ", 0.0)),
                ]
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect();

                let mut sanitized_options = options.clone();
                for (key, value) in psychovisual_parameters {
                    if let Some((_key, unsanitized_value)) = sanitized_options.get_key_value(&key)
                        && unsanitized_value.matches(&value)
                    {
                        sanitized_options.remove(&key);
                    }
                }

                Encoder::SVTAV1 {
                    executable:   executable.clone(),
                    pass:         *pass,
                    options:      sanitized_options,
                    photon_noise: photon_noise.clone(),
                }
            },
            Encoder::AVM {
                executable,
                pass,
                options,
                photon_noise,
            } => {
                let psychovisual_parameters: HashMap<String, CLIParameter> =
                    std::iter::once(("film-grain-table", CLIParameter::new_string("--", "=", "")))
                        .map(|(key, value)| (key.to_owned(), value))
                        .collect();

                let mut sanitized_options = options.clone();
                for (key, value) in psychovisual_parameters {
                    if let Some((_key, unsanitized_value)) = sanitized_options.get_key_value(&key)
                        && unsanitized_value.matches(&value)
                    {
                        sanitized_options.remove(&key);
                    }
                }

                Encoder::AVM {
                    executable:   executable.clone(),
                    pass:         *pass,
                    options:      sanitized_options,
                    photon_noise: photon_noise.clone(),
                }
            },
            Encoder::X264 {
                ..
            } => encoder.clone(),
            Encoder::X265 {
                ..
            } => encoder.clone(),
            Encoder::VVenC {
                ..
            } => encoder.clone(),
            Encoder::FFmpeg {
                ..
            } => encoder.clone(),
        }
    }

    #[inline]
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    pub fn probe_pass(
        pass: u8,
        target_quality_directory: &Path,
        config: &TargetQualityConfig,
        input: &mut Input,
        metric_input: Option<&mut Input>,
        workers: u8,
        buffer_strategy: &BufferStrategy,
        concat_method: &ConcatMethod,
        tasks: &[Task],
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<(Vec<Task>, Vec<anyhow::Error>)> {
        let warnings: Vec<anyhow::Error> = vec![];
        let pass_directory = target_quality_directory.join(pass.to_string());
        let output = concat_method.with_extension(&target_quality_directory.join(pass.to_string()));
        let framerate = input.clip_info()?.frame_rate;

        let already_completed_tasks =
            tasks.iter().filter(|task| task.output.exists()).collect::<Vec<_>>();
        let frames_already_completed = already_completed_tasks
            .iter()
            .fold(0, |acc, task| acc + task.frame_indices.len());
        let total_frames = tasks.iter().fold(0, |acc, task| acc + task.frame_indices.len());

        let encode_tasks = tasks
            .iter()
            .filter(|task| !task.output.exists())
            .enumerate()
            .map(|(index, task)| ParallelEncoderTask {
                original_index: task.original_index,
                index,
                frame_indices: task.frame_indices.clone(),
                sub_scenes: None,
                encoder: task.encoder.clone(),
                output: task.output.clone(),
            })
            .collect::<Vec<_>>();

        let progress_tx_clone = progress_tx.clone();
        let (encode_progress_tx, encode_progress_rx) = sync::mpsc::channel();
        let encode_thread = thread::spawn(move || -> Result<()> {
            for progress in encode_progress_rx {
                match progress {
                    SequenceStatus::Whole(Status::Processing {
                        id: _id,
                        completion,
                    }) => {
                        #[allow(clippy::collapsible_match)]
                        if let SequenceCompletion::Frames {
                            completed,
                            total: _total,
                        } = completion
                        {
                            progress_tx_clone.send(SequenceStatus::Whole(Status::Processing {
                                id:         "Encode".to_owned(),
                                completion: SequenceCompletion::Frames {
                                    completed: frames_already_completed as u64 + completed,
                                    total:     total_frames as u64,
                                },
                            }))?;
                        }
                    },
                    SequenceStatus::Whole(Status::Completed {
                        id: _,
                    }) => {
                        progress_tx_clone.send(SequenceStatus::Whole(Status::Completed {
                            id: "Encode".to_owned(),
                        }))?;
                    },
                    _ => (),
                }
            }
            Ok(())
        });

        let results = ParallelEncoder::encode_tasks(
            input,
            workers,
            buffer_strategy,
            encode_tasks.iter().cloned().collect::<VecDeque<_>>(),
            encode_progress_tx,
            Arc::clone(cancelled),
        )?;

        if cancelled.load(sync::atomic::Ordering::Relaxed) {
            return Ok((tasks.to_vec(), warnings));
        }

        encode_thread.join().expect("encode progress thread should join")?;

        let scene_paths = tasks.iter().map(|task| task.output.clone()).collect::<Vec<_>>();
        match concat_method {
            ConcatMethod::MKVMerge => {
                SceneConcatenator::mkvmerge(
                    &pass_directory,
                    &output,
                    &scene_paths,
                    None,
                    framerate,
                    &progress_tx,
                    cancelled,
                )?;
            },
            ConcatMethod::FFmpeg => {
                SceneConcatenator::ffmpeg(
                    &pass_directory,
                    &output,
                    &scene_paths,
                    total_frames,
                    framerate,
                    &progress_tx,
                    cancelled,
                )?;
            },
            ConcatMethod::Ivf => {
                SceneConcatenator::ivf(&output, &scene_paths, &progress_tx, cancelled)?;
            },
        }

        let metric_input = metric_input.unwrap_or(input);
        let v_input = match metric_input.as_data() {
            InputModel::Video {
                path, ..
            } => {
                const SCRIPT_OUTPUT_INDEX: u8 = 0;
                const SCRIPT_NODE_NAME: &str = "clip";
                let mut script = VapourSynthScript::default();
                let script = {
                    let (dec_import_lines, dec_lines) =
                        Source::new(&path).generate_script(SCRIPT_NODE_NAME.to_owned())?;
                    if let Some(dec_import_lines) = dec_import_lines {
                        script.add_imports(dec_import_lines);
                    }
                    script.add_lines(dec_lines);

                    script.outputs.insert(SCRIPT_OUTPUT_INDEX, SCRIPT_NODE_NAME.to_owned());
                    script
                };
                let script_input_data = InputModel::VapourSynthScript {
                    source:    VapourSynthScriptSource::Text(script.to_string()),
                    variables: HashMap::new(),
                    index:     SCRIPT_OUTPUT_INDEX,
                };

                Some(&mut Input::from_vapoursynth(&script_input_data, None)?)
            },
            _ => None,
        };
        let decoder = match metric_input {
            Input::VapourSynth {
                decoder, ..
            }
            | Input::VapourSynthScript {
                decoder, ..
            } => decoder,
            Input::Video {
                ..
            } => v_input.expect("Video Input exists").decoder(),
        };
        let vapoursynth_decoder = decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
        let env = &vapoursynth_decoder.env;
        let reference_node = vapoursynth_decoder.get_output(
            vapoursynth_decoder.get_output_index(),
            vapoursynth_decoder.get_node_modifier(),
        )?;
        let core = get_core(env)?;

        let reference_node = {
            let frame_nodes: Vec<_> = tasks
                .iter()
                .map(|task| {
                    task.frame_indices
                        .iter()
                        .map(|index| {
                            Trim {
                                first: Some(*index as u32),
                                last: Some(*index as u32),
                                ..Default::default()
                            }
                            .invoke(core, &reference_node)
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect();

            Splice::invoke(core, &frame_nodes)?
        };
        let distorted_node = Source {
            source: output,
            ..Default::default()
        }
        .invoke(core)?;

        let progress_tx_clone = progress_tx.clone();
        let (compare_progress_tx, compare_progress_rx) = sync::mpsc::channel();
        let compare_thread = thread::spawn(move || -> Result<()> {
            for progress in compare_progress_rx {
                match progress {
                    SequenceStatus::Whole(Status::Processing {
                        id: _fn_name,
                        completion,
                    }) => {
                        #[allow(clippy::collapsible_match)]
                        if let SequenceCompletion::Frames {
                            completed,
                            total,
                        } = completion
                        {
                            progress_tx_clone.send(SequenceStatus::Whole(Status::Processing {
                                id:         "Compare".to_owned(),
                                completion: SequenceCompletion::Frames {
                                    completed,
                                    total,
                                },
                            }))?;
                        }
                    },
                    SequenceStatus::Whole(Status::Completed {
                        id: _,
                    }) => {
                        progress_tx_clone.send(SequenceStatus::Whole(Status::Completed {
                            id: "Compare".to_owned(),
                        }))?;
                    },
                    _ => (),
                }
            }
            Ok(())
        });

        let started = SystemTime::now();
        let mut scores = match &config.metric {
            QualityMetric::VMAF {
                ..
            } => {
                // let (reference_node, distorted_node) = if let Some((width, height)) =
                // *resolution {     let resize = Bicubic {
                //         width: Some(width),
                //         height: Some(height),
                //         ..Default::default()
                //     };
                //     (
                //         resize.invoke(core, &reference_node)?,
                //         resize.invoke(core, &distorted_node)?,
                //     )
                // } else {
                //     (reference_node, distorted_node)
                // };
                // The VapourSynth VMAF plugin does not provide real time scores and requires
                // writing to and reading from a file. Consider
                unimplemented!()
            },
            QualityMetric::SSIMULACRA2 {
                resolution,
                threads,
                ..
            } => {
                let (reference_node, distorted_node) = if let Some((width, height)) = *resolution {
                    let resize = Bicubic {
                        width: Some(width),
                        height: Some(height),
                        ..Default::default()
                    };
                    (
                        resize.invoke(core, &reference_node)?,
                        resize.invoke(core, &distorted_node)?,
                    )
                } else {
                    (reference_node, distorted_node)
                };
                if VSHIPSSIMULACRA2::plugin_is_installed(core) {
                    let plugin = VSHIPSSIMULACRA2 {
                        num_stream: threads.map_or(Some(4), |threads| Some(threads as u32)),
                        ..Default::default()
                    };
                    let node = plugin.invoke(core, &reference_node, &distorted_node)?;
                    VSHIPSSIMULACRA2::get_scores(&node, None, compare_progress_tx)?
                } else if SSIMULACRA2::plugin_is_installed(core) {
                    let node = SSIMULACRA2::invoke(core, &reference_node, &distorted_node)?;
                    SSIMULACRA2::get_scores(&node, None, compare_progress_tx)?
                } else {
                    error!("No VapourSynth SSIMULACRA2 plugin found");
                    bail!(TargetQualityError::QualityMeasurementFailed);
                }
            },
            QualityMetric::BUTTERAUGLI {
                resolution,
                threads,
                intensity_multiplier,
                norm,
                ..
            } => {
                let (reference_node, distorted_node) = if let Some((width, height)) = *resolution {
                    let resize = Bicubic {
                        width: Some(width),
                        height: Some(height),
                        ..Default::default()
                    };
                    (
                        resize.invoke(core, &reference_node)?,
                        resize.invoke(core, &distorted_node)?,
                    )
                } else {
                    (reference_node, distorted_node)
                };
                let plugin = BUTTERAUGLI {
                    num_stream: threads.map_or(Some(4), |threads| Some(threads as u32)),
                    intensity_multiplier: *intensity_multiplier,
                    q_norm: norm.map(|norm| norm as u32),
                    ..Default::default()
                };
                let node = plugin.invoke(core, &reference_node, &distorted_node)?;
                BUTTERAUGLI::get_scores(
                    &node,
                    norm.and_then(|_| Some(BUTTERAUGLI::QNORM_PROPERTY_NAMES)),
                    compare_progress_tx,
                )?
            },
            QualityMetric::XPSNR {
                resolution, ..
            } => {
                let (reference_node, distorted_node) = if let Some((width, height)) = *resolution {
                    let resize = Bicubic {
                        width: Some(width),
                        height: Some(height),
                        ..Default::default()
                    };
                    (
                        resize.invoke(core, &reference_node)?,
                        resize.invoke(core, &distorted_node)?,
                    )
                } else {
                    (reference_node, distorted_node)
                };
                let plugin = XPSNR {
                    temporal: Some(false),
                    verbose: Some(false),
                    ..Default::default()
                };
                let node = plugin.invoke(core, &reference_node, &distorted_node)?;
                // XPSNR returns a score per plane, combine them into the weighted XPSNR score.
                XPSNR::get_multiple_scores(&node, XPSNR::PROPERTY_NAMES, compare_progress_tx)?
                    .into_iter()
                    .map(|plane_scores| match plane_scores.as_slice() {
                        [y, u, v] => Ok(XPSNR::weight_xpsnr(*y, *u, *v)),
                        _ => Err(TargetQualityError::QualityMeasurementFailed),
                    })
                    .collect::<Result<Vec<f64>, _>>()?
            },
            QualityMetric::CVVDP {
                resolution,
                display_model,
                resize_to_display,
                disable_temporal,
                ..
            } => {
                let (reference_node, distorted_node) = if let Some((width, height)) = *resolution {
                    let resize = Bicubic {
                        width: Some(width),
                        height: Some(height),
                        ..Default::default()
                    };
                    (
                        resize.invoke(core, &reference_node)?,
                        resize.invoke(core, &distorted_node)?,
                    )
                } else {
                    (reference_node, distorted_node)
                };
                let plugin = CVVDP {
                    model_name: *display_model,
                    resize_to_display: *resize_to_display,
                    disable_temporal: *disable_temporal,
                    ..Default::default()
                };
                let node = plugin.invoke(core, &reference_node, &distorted_node)?;
                CVVDP::get_scores(&node, None, compare_progress_tx)?
            },
        };
        let ended = SystemTime::now();

        compare_thread.join().expect("compare progress thread should join")?;
        drop(progress_tx);

        let tasks = tasks
            .iter()
            .zip(results)
            .map(|(task, result)| {
                let result = result.expect("ParallelEncoder result exists");
                let mut completed_task = task.clone();
                completed_task.passes.push(QualityPass {
                    quantizer:    task.encoder.quantizer().expect("quantizer exists"),
                    scores:       scores.drain(0..task.frame_indices.len()).collect(),
                    bitrate:      result.bitrate,
                    started_on:   started
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("Time is valid")
                        .as_millis(),
                    completed_on: ended
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("Time is valid")
                        .as_millis(),
                });
                completed_task
            })
            .collect::<Vec<_>>();

        Ok((tasks, warnings))
    }

    #[inline]
    pub fn predict_quantizer(
        quantizer_range: (f64, f64),
        target_score: f64,
        interpolators: (InterpolationMethod, InterpolationMethod),
        quantizer_score_history: &[(f64, f64)],
        step: f64,
    ) -> Result<f64> {
        let midpoint = quantizer_range.0.midpoint(quantizer_range.1);

        let predicted_quantizer = match quantizer_score_history.len() {
            0..=1 => midpoint,
            n => {
                // Sort history by quantizer
                let mut sorted = quantizer_score_history.to_vec();
                sorted.sort_by(|(_, s1), (_, s2)| {
                    s1.partial_cmp(s2).unwrap_or(std::cmp::Ordering::Equal)
                });

                let (scores, quantizers): (Vec<f64>, Vec<f64>) =
                    sorted.iter().map(|(q, s)| (*s, *q)).unzip();

                let result = match n {
                    2 => {
                        // 3rd probe: linear interpolation
                        interpolators::linear(
                            &[scores[0], scores[1]],
                            &[quantizers[0], quantizers[1]],
                            target_score,
                        )
                    },
                    3 => {
                        // 4th probe: configurable method
                        match interpolators.0 {
                            InterpolationMethod::Linear => interpolators::linear(
                                &[scores[0], scores[1]],
                                &[quantizers[0], quantizers[1]],
                                target_score,
                            ),
                            InterpolationMethod::Quadratic => interpolators::quadratic(
                                &[scores[0], scores[1], scores[2]],
                                &[quantizers[0], quantizers[1], quantizers[2]],
                                target_score,
                            ),
                            InterpolationMethod::Natural => interpolators::natural_cubic_spline(
                                &scores,
                                &quantizers,
                                target_score,
                            ),
                            _ => None,
                        }
                    },
                    4 => {
                        // 5th probe: configurable method
                        let s: &[f64; 4] = &scores[..4].try_into()?;
                        let q: &[f64; 4] = &quantizers[..4].try_into()?;

                        match interpolators.1 {
                            InterpolationMethod::Linear => {
                                interpolators::linear(&[s[0], s[1]], &[q[0], q[1]], target_score)
                            },
                            InterpolationMethod::Quadratic => interpolators::quadratic(
                                &[s[0], s[1], s[2]],
                                &[q[0], q[1], q[2]],
                                target_score,
                            ),
                            InterpolationMethod::Natural => interpolators::natural_cubic_spline(
                                &scores,
                                &quantizers,
                                target_score,
                            ),
                            InterpolationMethod::Pchip => interpolators::pchip(s, q, target_score),
                            InterpolationMethod::Catmull => {
                                interpolators::catmull_rom(s, q, target_score)
                            },
                            InterpolationMethod::Akima => interpolators::akima(s, q, target_score),
                            InterpolationMethod::CubicPolynomial => {
                                interpolators::cubic_polynomial(s, q, target_score)
                            },
                        }
                    },
                    _ => None,
                };

                result.unwrap_or_else(|| {
                    trace!("Interpolation failed, falling back to binary search (midpoint)");
                    midpoint
                })
            },
        };

        // Round the result of the interpolation to the nearest integer
        Ok(((predicted_quantizer / step).round() * step)
            .clamp(quantizer_range.0, quantizer_range.1))
    }
}

impl Default for TargetQuality {
    #[inline]
    fn default() -> Self {
        Self {
            input:        None,
            metric_input: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub original_index: usize,
    pub frame_indices:  Vec<usize>,
    pub encoder:        Encoder,
    pub output:         PathBuf,
    pub passes:         Vec<QualityPass>,
}

#[derive(Debug, Clone, Error)]
pub enum TargetQualityError {
    #[error("No Scenes found")]
    ScenesEmpty,
    #[error("Parallel Encoder workers already configured")]
    WorkersAlreadyConfigured,
    #[error("Failed to encode")]
    EncoderFailed,
    #[error("Previous Pass data not found")]
    PreviousPassDataNotFound,
    #[error("Failed to measure quality")]
    QualityMeasurementFailed,
}
