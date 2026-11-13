use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{self, Arc, atomic::AtomicBool},
    thread,
};

use anyhow::{Context, Result, bail, ensure};
use itertools::Itertools;
use thiserror::Error;
use tracing::{debug, error, trace};

use crate::models::sequence::provenance::{self, MetricBackend, ResultProvenance};
use crate::{
    core::{
        Condor,
        encoder::EncoderCapability,
        input::Input,
        sequence::{
            Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status,
            parallel_encoder::{ParallelEncoder, Task as ParallelEncoderTask},
            quality_check::{finish_temporal_compare, spawn_compare_progress},
            scene_concatenator::SceneConcatenator,
            zone_encoder::{ZoneEncoder, ZonePlan},
        },
    },
    metrics::{self, Engine, OutputIndexing},
    models::{
        encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
        input::{ImportMethod, Input as InputModel},
        sequence::{
            SequenceConfigHandler, SequenceDataHandler,
            parallel_encoder::ParallelEncoderConfigHandler,
            scene_concatenator::{ConcatMethod, SceneConcatenatorConfigHandler},
            target_quality::{
                TargetQualityConfig, TargetQualityConfigHandler, TargetQualityData,
                TargetQualityDataHandler,
                types::{
                    InterpolationMethod, MeasurementError, QualityMetric, QualityPass,
                    validate_scores,
                },
            },
        },
    },
    utils::interpolators,
    vapoursynth::{
        get_core,
        plugins::{
            MetricPluginFunction, PluginFunction,
            ffms2::Source,
            resize::bicubic::Bicubic,
            standard::{splice::Splice, trim::Trim},
            vship::{
                butteraugli::BUTTERAUGLI, cvvdp::CVVDP,
                ssimulacra2::SSIMULACRA2 as VSHIPSSIMULACRA2,
            },
            vszip::{ssimulacra2::SSIMULACRA2, xpsnr::XPSNR},
        },
    },
};

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Target Quality",
    description: "Determine the optimal quantizer for a given video quality metric score per \
                  scene.",
    version: "0.0.1",
};

pub struct TargetQuality {
    pub input: Option<Input>,
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

        Self::validate_config(condor)?;
        // Ensure all the scene encoders are validated
        for scene in &condor.scenes {
            scene.encoder.validate()?;
        }
        super::parallel_encoder::artifact::preflight(
            condor.sequence_config.parallel_encoder()?.validate_frame_count,
        )?;

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
        let Some(config) = config else {
            return Ok(((), warnings));
        };
        Self::validate_config(condor)?;
        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(input_model) = &parallel_encoder_config.input {
            &mut Input::from_data(input_model)?
        } else {
            &mut condor.input
        };
        let metric_input = if let Some(input) = self.metric_input.as_mut() {
            Some(input)
        } else if let Some(metric_input) = &config.metric_input {
            Some(&mut Input::from_data(metric_input)?)
        } else {
            None
        };
        // Initialize input by getting clip_info. For VapourSynth inputs, this
        // may begin a lengthy caching process, hence the separation
        // between validate and initialize.
        progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id: DETAILS.name.to_owned(),
            completion: SequenceCompletion::Custom {
                name: DETAILS.name.to_owned(),
                completed: 0.0,
                total: if metric_input.is_some() { 2.0 } else { 1.0 },
            },
        }))?;
        input.clip_info()?;
        if let Some(metric_input) = metric_input {
            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id: DETAILS.name.to_owned(),
                completion: SequenceCompletion::Custom {
                    name: DETAILS.name.to_owned(),
                    completed: 1.0,
                    total: 2.0,
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
        self.execute_cancel(condor, progress_tx, cancelled.into())
    }

    #[inline]
    fn execute_cancel(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: crate::utils::cancellation::Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)> {
        self.execute_with_probe(
            condor,
            &progress_tx,
            &cancelled,
            Self::probe_pass_with_options,
        )
    }
}

impl TargetQuality {
    fn execute_with_probe<DataHandler, ConfigHandler>(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &crate::utils::cancellation::Cancellation,
        mut probe: impl FnMut(
            u8,
            &Path,
            &TargetQualityConfig,
            &mut Input,
            Option<&mut Input>,
            u8,
            &ConcatMethod,
            &[Task],
            sync::mpsc::Sender<SequenceStatus>,
            &crate::utils::cancellation::Cancellation,
            super::parallel_encoder::ParallelEncoderOptions,
        ) -> Result<(Vec<Task>, Vec<anyhow::Error>)>,
    ) -> Result<((), Vec<anyhow::Error>)>
    where
        DataHandler: SequenceDataHandler + TargetQualityDataHandler,
        ConfigHandler: SequenceConfigHandler
            + ParallelEncoderConfigHandler
            + SceneConcatenatorConfigHandler
            + TargetQualityConfigHandler,
    {
        cancelled.check()?;
        let mut warnings = vec![];
        let parallel_encoder_config = condor.sequence_config.parallel_encoder()?;
        let scene_concatenator_config = condor.sequence_config.scene_concatenator()?;
        let config = condor.sequence_config.target_quality()?;
        let target_quality_directory = &parallel_encoder_config.scenes_directory.join(DETAILS.name);
        let workers = parallel_encoder_config.workers.unwrap_or(1);
        let condor_data = condor.as_data();

        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(TargetQualityError::ScenesEmpty));
            return Ok(((), warnings));
        }

        if config.is_none() {
            return Ok(((), warnings));
        }
        let config = config.clone().expect("TargetQualityConfig is Some");
        Self::validate_config(condor)?;
        let root_model = condor.input.as_data();
        let input_model = self.input.as_ref().map_or_else(
            || provenance::probe_input(&root_model, parallel_encoder_config, Some(&config)).clone(),
            Input::as_data,
        );
        let metric_model = self
            .metric_input
            .as_ref()
            .map(Input::as_data)
            .or_else(|| config.metric_input.clone());
        let mut admission = provenance::Admission::default();
        for (index, scene) in condor.scenes.iter().enumerate() {
            admission.admit_target(
                &input_model,
                metric_model.as_ref(),
                scene,
                Some(&config),
                index,
            )?;
        }
        let scene_frame_indices = condor
            .scenes
            .iter()
            .map(|scene| config.probing.strategy.frame_indices(scene.start_frame, scene.end_frame))
            .collect::<Result<Vec<_>>>()?;
        // Validate every claimed history before restoring even the first scene.
        for (index, (scene, indices)) in condor.scenes.iter().zip(&scene_frame_indices).enumerate()
        {
            if let Ok(data) = scene.sequence_data.get_target_quality() {
                Self::validate_history(&config, index, indices.len(), &data.passes)?;
            }
        }
        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(input_model) = &config.input {
            &mut Input::from_data(input_model)?
        } else if let Some(input_model) = &parallel_encoder_config.input {
            &mut Input::from_data(input_model)?
        } else {
            &mut condor.input
        };

        // The post-pass ordinal must represent 256 even though public passes stop at 255.
        let mut ordinal = 1_u16;

        // A resumed/exhausted search may schedule no tasks, but its final q must
        // still come from recorded measurements. Empty histories leave scenes untouched.
        let mut restored_quantizers = false;
        for scene in &mut condor.scenes {
            let Ok(data) = scene.sequence_data.get_target_quality() else {
                continue;
            };
            let history = Self::quantizer_score_history(&config, &data.passes)?;
            if let Some(q) = Self::measured_quantizer(&config.metric, &history)
                && scene.encoder.quantizer() != Some(q)
            {
                scene.encoder.set_quantizer(q);
                restored_quantizers = true;
            }
            if !history.is_empty() {
                let selection =
                    ResultProvenance::new(provenance::selection_context(scene, &config)?);
                let data = scene.sequence_data.get_target_quality_mut()?;
                if data.selection.as_ref() != Some(&selection) {
                    data.selection = Some(selection);
                    restored_quantizers = true;
                }
            }
        }

        loop {
            if ordinal > u16::from(config.maximum_probes) {
                break;
            }
            let pass = u8::try_from(ordinal).expect("ordinal is within the configured u8 budget");

            let pass_directory = target_quality_directory.join(pass.to_string());
            let tasks = condor
                .scenes
                .iter()
                .zip(&scene_frame_indices)
                .enumerate()
                .map(|(index, (scene, frame_indices))| -> Result<Option<Task>> {
                    let passes = scene
                        .sequence_data
                        .get_target_quality()
                        .map_or_else(|_| TargetQualityData::default(), |tq| tq.clone())
                        .passes;
                    // Admit only this scene's next ordinal, before capability probes or scratch.
                    // More-complete peers must never enter a lower whole-pass replacement.
                    if passes.len() != usize::from(pass - 1)
                        || passes.len() >= usize::from(config.maximum_probes)
                    {
                        return Ok(None);
                    }
                    let quantizer_score_history = Self::quantizer_score_history(&config, &passes)?;
                    if quantizer_score_history
                        .iter()
                        .any(|(_, score)| config.metric.score_within_target(*score))
                    {
                        return Ok(None);
                    }
                    let mut encoder = provenance::probe_encoder(scene, &config)?;
                    let Some(predicted_quantizer) = Self::next_probe_quantizer(
                        &config,
                        &encoder,
                        &quantizer_score_history,
                        cancelled,
                    )?
                    else {
                        return Ok(None);
                    };
                    encoder.set_quantizer(predicted_quantizer);
                    let output = pass_directory.join(format!(
                        "{}.{}",
                        ParallelEncoder::scene_id(index),
                        encoder.output_extension()
                    ));
                    Ok(Some(Task {
                        original_index: index,
                        frame_indices: frame_indices.clone(),
                        encoder,
                        output,
                        passes,
                    }))
                })
                .filter_map(|result: Result<Option<Task>>| result.transpose())
                .collect::<Result<Vec<_>>>()?;

            if tasks.is_empty() {
                // Empty early ordinals can precede resumable scenes with longer prefixes.
                if !condor.scenes.iter().any(|scene| {
                    scene.sequence_data.get_target_quality().is_ok_and(|data| {
                        data.passes.len() >= usize::from(pass)
                            && data.passes.len() < usize::from(config.maximum_probes)
                    })
                }) {
                    break;
                }
                ordinal += 1;
                continue;
            }

            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id: DETAILS.name.to_owned(),
                completion: SequenceCompletion::Passes {
                    completed: pass - 1,
                    total: pass,
                },
            }))?;
            if !pass_directory.exists() {
                std::fs::create_dir_all(&pass_directory)?;
            }

            debug!("Starting Pass {}", pass);

            let progress_tx_clone = progress_tx.clone();
            let (pass_progress_tx, pass_progress_rx) = sync::mpsc::channel();
            let relay = thread::spawn(move || -> Result<()> {
                for progress in pass_progress_rx {
                    match progress {
                        SequenceStatus::Whole(Status::Processing { id, completion })
                            if id == "Encode" =>
                        {
                            #[allow(clippy::collapsible_match)]
                            if let SequenceCompletion::Frames { completed, total } = completion {
                                progress_tx_clone.send(SequenceStatus::Subprocess {
                                    parent: Status::Processing {
                                        id: DETAILS.name.to_owned(),
                                        completion: SequenceCompletion::Passes {
                                            completed: pass,
                                            total: config.maximum_probes,
                                        },
                                    },
                                    child: Status::Processing {
                                        id,
                                        completion: SequenceCompletion::Frames { completed, total },
                                    },
                                })?;
                            }
                        },
                        SequenceStatus::Whole(Status::Completed { id }) if id == "Encode" => {
                            progress_tx_clone.send(SequenceStatus::Subprocess {
                                parent: Status::Processing {
                                    id: DETAILS.name.to_owned(),
                                    completion: SequenceCompletion::Passes {
                                        completed: pass,
                                        total: config.maximum_probes,
                                    },
                                },
                                child: Status::Completed { id },
                            })?;
                        },
                        SequenceStatus::Whole(Status::Processing { id, completion })
                            if id == "Compare" =>
                        {
                            #[allow(clippy::collapsible_match)]
                            if let SequenceCompletion::Frames { completed, total } = completion {
                                progress_tx_clone.send(SequenceStatus::Subprocess {
                                    parent: Status::Processing {
                                        id: DETAILS.name.to_owned(),
                                        completion: SequenceCompletion::Passes {
                                            completed: pass,
                                            total: config.maximum_probes,
                                        },
                                    },
                                    child: Status::Processing {
                                        id,
                                        completion: SequenceCompletion::Frames { completed, total },
                                    },
                                })?;
                            }
                        },
                        SequenceStatus::Whole(Status::Completed { id }) if id == "Compare" => {
                            progress_tx_clone.send(SequenceStatus::Subprocess {
                                parent: Status::Processing {
                                    id: DETAILS.name.to_owned(),
                                    completion: SequenceCompletion::Passes {
                                        completed: pass,
                                        total: config.maximum_probes,
                                    },
                                },
                                child: Status::Completed { id },
                            })?;
                        },
                        _ => (),
                    }
                }

                Ok(())
            });

            let before = admission.measurement_inputs(
                &input_model,
                metric_model.as_ref(),
                Some(&config.metric),
            )?;
            let mut configured_metric_input = if self.metric_input.is_none() {
                config.metric_input.as_ref().map(Input::from_data).transpose()?
            } else {
                None
            };
            let probed = probe(
                pass,
                target_quality_directory,
                &config,
                input,
                self.metric_input.as_mut().or(configured_metric_input.as_mut()),
                workers,
                &scene_concatenator_config.method,
                tasks.as_slice(),
                pass_progress_tx,
                cancelled,
                super::parallel_encoder::ParallelEncoderOptions {
                    max_tries: parallel_encoder_config.max_tries,
                    validate_frame_count: parallel_encoder_config.validate_frame_count,
                },
            );
            let relayed = crate::core::encoder::pipes::join_worker(
                relay,
                "Joining target-quality pass progress",
            );
            let (completed_tasks, pass_warnings) = probed?;
            relayed?;
            let mut publication = provenance::Admission::default();
            ensure!(
                before
                    == publication.measurement_inputs(
                        &input_model,
                        metric_model.as_ref(),
                        Some(&config.metric)
                    )?,
                "Target Quality inputs changed during measurement; recompute or use a new job"
            );
            if completed_tasks.is_empty() {
                warnings.extend(pass_warnings);
                break;
            }

            let previous_lengths = condor
                .scenes
                .iter()
                .map(|scene| scene.sequence_data.get_target_quality().map(|data| data.passes.len()))
                .collect::<Result<Vec<_>>>()?;
            let mut staged = condor.scenes.clone();
            let mut completions = Vec::with_capacity(completed_tasks.len());
            for mut completed_task in completed_tasks {
                let index = completed_task.original_index;
                publication.admit_target(
                    &input_model,
                    metric_model.as_ref(),
                    &staged[index],
                    Some(&config),
                    index,
                )?;
                let previous = staged[index].sequence_data.get_target_quality()?.passes.len();
                Self::validate_history(
                    &config,
                    index,
                    scene_frame_indices[index].len(),
                    &completed_task.passes,
                )?;
                // Only newly successful measurements are stamped; never certify a prefix.
                for quality in completed_task.passes.iter_mut().skip(previous) {
                    quality.provenance = Some(ResultProvenance::new(publication.target_context(
                        &input_model,
                        metric_model.as_ref(),
                        &staged[index],
                        &config,
                        quality,
                    )?));
                }
                if let Some(previous_backend) = staged[index]
                    .sequence_data
                    .get_target_quality()?
                    .passes
                    .first()
                    .and_then(|p| p.backend.as_ref())
                {
                    ensure!(
                        completed_task
                            .passes
                            .iter()
                            .all(|p| p.backend.as_ref() == Some(previous_backend)),
                        "scene {index} TQ backend changed; explicitly recompute history or use a new job"
                    );
                }
                // Update Target Quality Passes
                staged[completed_task.original_index]
                    .sequence_data
                    .get_target_quality_mut()?
                    .passes = completed_task.passes.clone();

                let quantizer_score_history =
                    Self::quantizer_score_history(&config, &completed_task.passes)?;
                let final_quantizer =
                    Self::measured_quantizer(&config.metric, &quantizer_score_history)
                        .ok_or(TargetQualityError::QualityMeasurementFailed)?;
                staged[completed_task.original_index].encoder.set_quantizer(final_quantizer);
                let selection =
                    ResultProvenance::new(provenance::selection_context(&staged[index], &config)?);
                staged[index].sequence_data.get_target_quality_mut()?.selection = Some(selection);

                let quality_pass = completed_task.passes.last().expect("passes is not empty");

                completions.push(SequenceStatus::Subprocess {
                    parent: Status::Processing {
                        id: DETAILS.name.to_owned(),
                        completion: SequenceCompletion::Passes {
                            completed: pass,
                            total: config.maximum_probes,
                        },
                    },
                    child: Status::Processing {
                        id: "Quality".to_owned(),
                        completion: Self::quality_completion(
                            &config,
                            completed_task.original_index,
                            quality_pass,
                        )?,
                    },
                });
            }
            admission = publication;
            condor.scenes = staged;
            for completion in completions {
                progress_tx.send(completion)?;
            }

            if !pass_warnings.is_empty() {
                warnings.extend(pass_warnings);
                break;
            }

            ordinal += 1;
            let mut data = condor_data.clone();
            data.scenes = condor.scenes.clone();
            if let Err(error) = (condor.save_callback)(data) {
                for (scene, previous) in condor.scenes.iter_mut().zip(previous_lengths) {
                    for quality in scene
                        .sequence_data
                        .get_target_quality_mut()?
                        .passes
                        .iter_mut()
                        .skip(previous)
                    {
                        quality.provenance = None;
                    }
                    scene.sequence_data.get_target_quality_mut()?.selection = None;
                }
                return Err(error).context("Saving measured Target Quality results");
            }
            restored_quantizers = false;

            cancelled.check_forced()?;
            if cancelled.is_cancelled() {
                break;
            }
        }

        cancelled.check_forced()?;
        if restored_quantizers && !cancelled.is_cancelled() {
            let mut data = condor_data;
            data.scenes = condor.scenes.clone();
            (condor.save_callback)(data)
                .context("Saving restored measured Target Quality quantizers")?;
        }

        Ok(((), warnings))
    }
}

impl TargetQuality {
    pub const DETAILS: SequenceDetails = DETAILS;

    fn next_probe_quantizer(
        config: &TargetQualityConfig,
        encoder: &Encoder,
        history: &[(f64, f64)],
        cancelled: &crate::utils::cancellation::Cancellation,
    ) -> Result<Option<f64>> {
        cancelled.check_forced()?;
        let target = config.metric.target_range().0.midpoint(config.metric.target_range().1);
        let bracket = Self::quantizer_bounds(
            config.quantizer_range,
            target,
            history,
            config.metric.is_inverse_metric(),
        );
        let bounds = (
            bracket.0.max(config.quantizer_range.0 as f64),
            bracket.1.min(config.quantizer_range.1 as f64),
        );
        let step = match encoder {
            Encoder::X264 { .. } | Encoder::X265 { .. } => 0.25,
            Encoder::SVTAV1 { .. }
                if encoder.supports_capability_cancel(
                    EncoderCapability::SvtAv1QuarterStepCrf,
                    cancelled,
                )? =>
            {
                0.25
            },
            _ => 1.0,
        };
        let predicted =
            Self::predict_quantizer(bounds, target, config.interpolators, history, step)?;
        Ok(Self::untested_quantizer(bounds, predicted, history, step))
    }

    fn untested_quantizer(
        bounds: (f64, f64),
        predicted: f64,
        history: &[(f64, f64)],
        step: f64,
    ) -> Option<f64> {
        // predict_quantizer validates these bounds and step before any tick arithmetic.
        let lower = bounds.0 / step;
        let upper = bounds.1 / step;
        let available = |tick: i64| {
            let q = tick as f64 * step;
            (bounds.0..=bounds.1).contains(&q) && !history.iter().any(|(tested, _)| *tested == q)
        };
        let interior = (lower.floor() as i64 + 1, upper.ceil() as i64 - 1);
        let candidate = (predicted / step).round();
        if candidate.is_finite()
            && (interior.0 as f64..=interior.1 as f64).contains(&candidate)
            && available(candidate as i64)
        {
            return Some(candidate * step);
        }
        let midpoint = lower.midpoint(upper);
        let nearest = |first: i64, last: i64| {
            let mut left = (midpoint.floor() as i64).min(last);
            let mut right = (left + 1).max(first);
            // ponytail: O(history²) exclusion checks, sorted ticks if histories grow;
            // never enumerate the grid, even for the full u32 configured range.
            for _ in 0..=history.len() {
                let tick = if left >= first
                    && (right > last || midpoint - left as f64 <= right as f64 - midpoint)
                {
                    let tick = left;
                    left -= 1;
                    tick
                } else if right <= last {
                    let tick = right;
                    right += 1;
                    tick
                } else {
                    return None;
                };
                if available(tick) {
                    return Some(tick as f64 * step);
                }
            }
            None
        };
        nearest(interior.0, interior.1).or_else(|| {
            // Only aligned endpoints can remain after interior exhaustion;
            // both are equally far from the midpoint, so try the lower first.
            [lower.ceil() as i64, upper.floor() as i64]
                .into_iter()
                .find(|tick| (*tick < interior.0 || *tick > interior.1) && available(*tick))
                .map(|tick| tick as f64 * step)
        })
    }

    fn validate_quantizer_grid(range: (f64, f64), step: f64) -> Result<()> {
        anyhow::ensure!(
            range.0.is_finite() && range.1.is_finite() && range.0 <= range.1,
            "Target Quality quantizer range must be finite and ordered: {range:?}"
        );
        anyhow::ensure!(
            step.is_finite() && step > 0.0,
            "Target Quality quantizer step must be positive and finite: {step}"
        );
        // Keep ticks and their immediate neighbors exactly representable, including
        // public callers with much larger ranges or smaller steps than encoder CRFs.
        let max_tick = ((1_u64 << 52) - 1) as f64;
        anyhow::ensure!(
            (range.0 / step).abs() <= max_tick && (range.1 / step).abs() <= max_tick,
            "Target Quality quantizer grid exceeds exact tick arithmetic: range {range:?}, step {step}"
        );
        Ok(())
    }

    /// Pure admission check shared with the TUI before it opens override inputs.
    /// Disabled/empty stages do no probing; cancellation is checked by their callers first.
    #[inline]
    pub fn validate_config<DataHandler, ConfigHandler>(
        condor: &Condor<DataHandler, ConfigHandler>,
    ) -> Result<()>
    where
        DataHandler: SequenceDataHandler,
        ConfigHandler: SequenceConfigHandler + TargetQualityConfigHandler,
    {
        if let Some(config) = condor.sequence_config.target_quality()? {
            config.validate(&condor.encoder)?;
            for (index, scene) in condor.scenes.iter().enumerate() {
                config
                    .validate(&scene.encoder)
                    .map_err(|error| anyhow::anyhow!("Target Quality scene {index}: {error:#}"))?;
                config.probing.strategy.validate_range(scene.start_frame, scene.end_frame)?;
            }
        }
        Ok(())
    }

    pub(crate) fn quantizer_score_history(
        config: &TargetQualityConfig,
        passes: &[QualityPass],
    ) -> Result<Vec<(f64, f64)>> {
        passes
            .iter()
            .enumerate()
            .map(|(index, pass)| {
                ensure!(
                    pass.quantizer.is_finite(),
                    MeasurementError::Record("quantizer")
                );
                Ok((
                    pass.quantizer,
                    config
                        .metric
                        .calculate_score(config.probing.statistic, &pass.scores)
                        .with_context(|| {
                            format!(
                                "Target Quality {} history pass {}",
                                config.metric.friendly_name(),
                                index + 1
                            )
                        })?,
                ))
            })
            .collect()
    }

    pub(crate) fn validate_history(
        config: &TargetQualityConfig,
        scene: usize,
        expected: usize,
        passes: &[QualityPass],
    ) -> Result<()> {
        for (index, pass) in passes.iter().enumerate() {
            validate_scores(&pass.scores, Some(expected))
                .and_then(|()| {
                    ensure!(
                        pass.quantizer.is_finite(),
                        MeasurementError::Record("quantizer")
                    );
                    ensure!(
                        pass.bitrate.is_finite(),
                        MeasurementError::Record("bitrate")
                    );
                    config
                        .metric
                        .calculate_score(config.probing.statistic, &pass.scores)
                        .map(|_| ())
                })
                .with_context(|| {
                    format!(
                        "Target Quality {} scene {scene} history pass {}",
                        config.metric.friendly_name(),
                        index + 1
                    )
                })?;
        }
        Ok(())
    }

    fn quantizer_bounds(
        quantizer_range: (u32, u32),
        target_score: f64,
        quantizer_score_history: &[(f64, f64)],
        inverse_metric: bool,
    ) -> (f64, f64) {
        let sorted = quantizer_score_history
            .iter()
            .sorted_by(|(_, score1), (_, score2)| {
                score1.partial_cmp(score2).unwrap_or(std::cmp::Ordering::Equal)
            })
            .collect::<Vec<_>>();
        // Exact target probes stay in the verified-quantizer flow, not the strict bracket.
        let above = sorted.iter().find(|(_, score)| *score > target_score);
        let below = sorted.iter().rfind(|(_, score)| *score < target_score);
        let (lower, upper) = if inverse_metric {
            (below, above)
        } else {
            (above, below)
        };
        let bounds = (
            lower.map_or(quantizer_range.0 as f64, |(quantizer, _)| *quantizer),
            upper.map_or(quantizer_range.1 as f64, |(quantizer, _)| *quantizer),
        );
        if bounds.0 > bounds.1 {
            // Either metric direction can cross under noise; use the safe configured range.
            (quantizer_range.0 as f64, quantizer_range.1 as f64)
        } else {
            bounds
        }
    }

    fn quality_completion(
        config: &TargetQualityConfig,
        index: usize,
        pass: &QualityPass,
    ) -> Result<SequenceCompletion> {
        Ok(SequenceCompletion::SceneQuality {
            index: index as u64,
            quantizer: pass.quantizer,
            score: config.metric.calculate_score(config.probing.statistic, &pass.scores)?,
            bitrate: pass.bitrate,
        })
    }

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
                ffmpeg_filter,
                append_color_data,
                ..
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
                    executable: executable.clone(),
                    pass: *pass,
                    options: sanitized_options,
                    photon_noise: None,
                    ffmpeg_filter: ffmpeg_filter.clone(),
                    append_color_data: *append_color_data,
                }
            },
            Encoder::RAV1E {
                executable,
                pass,
                options,
                ffmpeg_filter,
                append_color_data,
                ..
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
                    executable: executable.clone(),
                    pass: *pass,
                    options: sanitized_options,
                    photon_noise: None,
                    ffmpeg_filter: ffmpeg_filter.clone(),
                    append_color_data: *append_color_data,
                }
            },
            Encoder::VPX { .. } => encoder.clone(),
            Encoder::SVTAV1 {
                executable,
                pass,
                options,
                ffmpeg_filter,
                append_color_data,
                ..
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
                    executable: executable.clone(),
                    pass: *pass,
                    options: sanitized_options,
                    photon_noise: None,
                    ffmpeg_filter: ffmpeg_filter.clone(),
                    append_color_data: *append_color_data,
                }
            },
            Encoder::AVM {
                executable,
                pass,
                options,
                ffmpeg_filter,
                append_color_data,
                ..
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
                    executable: executable.clone(),
                    pass: *pass,
                    options: sanitized_options,
                    photon_noise: None,
                    ffmpeg_filter: ffmpeg_filter.clone(),
                    append_color_data: *append_color_data,
                }
            },
            Encoder::X264 { .. } => encoder.clone(),
            Encoder::X265 { .. } => encoder.clone(),
            Encoder::VVenC { .. } => encoder.clone(),
            Encoder::FFmpeg { .. } => encoder.clone(),
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
        concat_method: &ConcatMethod,
        tasks: &[Task],
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<(Vec<Task>, Vec<anyhow::Error>)> {
        Self::probe_pass_cancel(
            pass,
            target_quality_directory,
            config,
            input,
            metric_input,
            workers,
            concat_method,
            tasks,
            progress_tx,
            &crate::utils::cancellation::Cancellation::from(Arc::clone(cancelled)),
        )
    }

    #[inline]
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    pub fn probe_pass_cancel(
        pass: u8,
        target_quality_directory: &Path,
        config: &TargetQualityConfig,
        input: &mut Input,
        metric_input: Option<&mut Input>,
        workers: u8,
        concat_method: &ConcatMethod,
        tasks: &[Task],
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancellation: &crate::utils::cancellation::Cancellation,
    ) -> Result<(Vec<Task>, Vec<anyhow::Error>)> {
        Self::probe_pass_with_validation(
            pass,
            target_quality_directory,
            config,
            input,
            metric_input,
            workers,
            concat_method,
            tasks,
            progress_tx,
            cancellation,
            true,
        )
    }

    /// Probe pass with the owning job's count-only policy; old standalone APIs are strict.
    #[inline]
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    pub fn probe_pass_with_validation(
        pass: u8,
        target_quality_directory: &Path,
        config: &TargetQualityConfig,
        input: &mut Input,
        metric_input: Option<&mut Input>,
        workers: u8,
        concat_method: &ConcatMethod,
        tasks: &[Task],
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancellation: &crate::utils::cancellation::Cancellation,
        validate_frame_count: bool,
    ) -> Result<(Vec<Task>, Vec<anyhow::Error>)> {
        Self::probe_pass_with_options(
            pass,
            target_quality_directory,
            config,
            input,
            metric_input,
            workers,
            concat_method,
            tasks,
            progress_tx,
            cancellation,
            super::parallel_encoder::ParallelEncoderOptions {
                validate_frame_count,
                ..Default::default()
            },
        )
    }

    /// Explicit per-scene policy; whole-zone batches still make one attempt.
    /// Complete validated pass records return unchanged without native work.
    /// Incomplete passes require all earlier records and outputs directly under
    /// `<target_quality_directory>/<pass>`; stale outputs must unlink successfully
    /// before the whole batch is encoded and admitted. No per-file reuse occurs.
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    #[inline]
    pub fn probe_pass_with_options(
        pass: u8,
        target_quality_directory: &Path,
        config: &TargetQualityConfig,
        input: &mut Input,
        metric_input: Option<&mut Input>,
        workers: u8,
        concat_method: &ConcatMethod,
        tasks: &[Task],
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancellation: &crate::utils::cancellation::Cancellation,
        options: super::parallel_encoder::ParallelEncoderOptions,
    ) -> Result<(Vec<Task>, Vec<anyhow::Error>)> {
        anyhow::ensure!(
            options.max_tries >= 1,
            "max_tries must be at least 1 total attempt"
        );
        cancellation.check_forced()?;
        if cancellation.is_cancelled() {
            return Ok((Vec::new(), Vec::new()));
        }
        ensure!(pass > 0, "Target Quality pass must be at least 1");
        if tasks.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        for task in tasks {
            config.validate(&task.encoder)?;
            task.encoder.validate_settings()?;
            ensure!(
                !task.frame_indices.is_empty(),
                "Target Quality scene {} has no selected frames",
                task.original_index
            );
            Self::validate_history(
                config,
                task.original_index,
                task.frame_indices.len(),
                &task.passes,
            )?;
        }
        let temporal = metrics::scene_temporal(&config.metric);
        let scenes: Vec<_> = tasks.iter().map(|task| task.frame_indices.as_slice()).collect();
        if temporal {
            metrics::probe::validate_scene_selection(&scenes)?;
        }
        // Records, not scratch artifacts or native availability, establish reuse.
        if Self::pass_is_complete(tasks, pass) {
            return Ok((tasks.to_vec(), Vec::new()));
        }
        let previous_passes = usize::from(pass - 1);
        for task in tasks {
            ensure!(
                task.passes.len() >= previous_passes,
                "Target Quality scene {} pass {pass} is missing an earlier recorded pass",
                task.original_index
            );
            task.encoder.validate()?;
        }
        // Preserve early prerequisite diagnostics, but only for passes that
        // actually need work. These lookups launch no encoder or metric.
        super::parallel_encoder::artifact::preflight(options.validate_frame_count)?;
        let warnings: Vec<anyhow::Error> = vec![];
        let pass_directory = target_quality_directory.join(pass.to_string());
        let output = concat_method.with_extension(&target_quality_directory.join(pass.to_string()));
        // SECURITY: Only declared outputs in this pass belong to the replay. Do
        // not scan neighbors, recurse into directories, or follow a pass-dir link.
        match pass_directory.symlink_metadata() {
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "Target Quality pass {pass} scratch directory is not a directory: {}",
                pass_directory.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Inspecting Target Quality pass {pass} scratch {}",
                        pass_directory.display()
                    )
                });
            },
        }
        for task in tasks {
            ensure!(
                task.output.parent() == Some(pass_directory.as_path()),
                "Target Quality scene {} output is outside pass {pass} scratch: {}",
                task.original_index,
                task.output.display()
            );
        }
        for source in std::iter::once(&*input).chain(metric_input.as_deref()) {
            let path = match source {
                Input::Video { path, .. } | Input::VapourSynth { path, .. } => Some(path),
                Input::VapourSynthScript {
                    source: crate::models::input::VapourSynthScriptSource::Path(path),
                    ..
                } => Some(path),
                Input::VapourSynthScript { .. } => None,
            };
            if let Some(source) = path {
                let source = source.canonicalize().with_context(|| {
                    format!(
                        "Resolving Target Quality input before pass {pass} cleanup: {}",
                        source.display()
                    )
                })?;
                for candidate in
                    tasks.iter().map(|task| &task.output).chain(std::iter::once(&output))
                {
                    // Unlinking an output symlink is safe; only a regular entry
                    // could remove the input itself through an aliased parent.
                    if candidate.symlink_metadata().is_ok_and(|metadata| metadata.is_file()) {
                        ensure!(
                            candidate.canonicalize().with_context(|| format!(
                                "Resolving stale output {}",
                                candidate.display()
                            ))? != source,
                            "Target Quality pass {pass} output overlaps input {}; refusing stale cleanup",
                            candidate.display()
                        );
                    }
                }
            }
        }
        // An incomplete pass describes no single measurement. Remove every
        // candidate output before any native work, even if some have records.
        for path in tasks.iter().map(|task| &task.output).chain(std::iter::once(&output)) {
            match std::fs::remove_file(path) {
                Ok(()) => debug!("Discarding orphaned probe output {}", path.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "Discarding stale Target Quality pass {pass} output {}",
                            path.display()
                        )
                    });
                },
            }
        }
        let framerate = input.clip_info()?.frame_rate;
        let total_frames = tasks.iter().fold(0, |acc, task| acc + task.frame_indices.len());

        let encode_tasks = tasks
            .iter()
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
                                id: "Encode".to_owned(),
                                completion: SequenceCompletion::Frames {
                                    completed,
                                    total: total_frames as u64,
                                },
                            }))?;
                        }
                    },
                    SequenceStatus::Whole(Status::Completed { id: _ }) => {
                        progress_tx_clone.send(SequenceStatus::Whole(Status::Completed {
                            id: "Encode".to_owned(),
                        }))?;
                    },
                    _ => (),
                }
            }
            Ok(())
        });

        // Zone encoding needs one process for the whole pass, so it applies
        // only when every scene differs in its quantizer and all scenes share
        // one encoder; anything else keeps the per-scene encoders. Probe
        // encoders never carry an FFmpeg filter, so filters cannot make scenes
        // differ here.
        // ponytail: lose H.26x grouping for final-scene parity and crash-safe probes;
        // reenable only with native safety and probe/final parity proof.
        let zone_plan = if encode_tasks
            .iter()
            .any(|task| matches!(task.encoder.base(), EncoderBase::X264 | EncoderBase::X265))
        {
            None
        } else {
            match ZonePlan::try_build_cancel(&encode_tasks, cancellation) {
                Ok(plan) => Some(plan),
                Err(reason) => {
                    if cancellation.is_forced() {
                        drop(encode_progress_tx);
                        let relayed = crate::core::encoder::pipes::join_worker(
                            encode_thread,
                            "Joining cancelled probe progress",
                        );
                        relayed?;
                        return Err(reason);
                    }
                    debug!("Probe pass {pass} encoding scenes individually: {reason}");
                    None
                },
            }
        };
        let results = match zone_plan {
            Some(plan) => ZoneEncoder::encode_tasks_with_validation(
                input,
                encode_tasks.iter().cloned().collect::<VecDeque<_>>(),
                plan,
                encode_progress_tx,
                cancellation.clone(),
                options.validate_frame_count,
            ),
            None => ParallelEncoder::encode_tasks_with_options(
                input,
                workers,
                encode_tasks.iter().cloned().collect::<VecDeque<_>>(),
                encode_progress_tx,
                cancellation.clone(),
                options,
            ),
        };
        let relayed = crate::core::encoder::pipes::join_worker(
            encode_thread,
            "Joining probe encode progress",
        );
        let results = results?;
        relayed?;

        cancellation.check_forced()?;
        if cancellation.is_cancelled() {
            // No scene was scored, so no task is complete. Returning the input
            // tasks would report them as scored with the `passes` they had on
            // entry, empty on the first pass.
            return Ok((Vec::new(), warnings));
        }

        let scene_paths = tasks.iter().map(|task| task.output.clone()).collect::<Vec<_>>();
        match concat_method {
            ConcatMethod::MKVMerge(_) => {
                SceneConcatenator::mkvmerge_cancel(
                    &pass_directory,
                    &output,
                    &scene_paths,
                    None,
                    framerate,
                    None,
                    &progress_tx,
                    cancellation,
                )?;
            },
            ConcatMethod::FFmpeg(_) => {
                SceneConcatenator::ffmpeg_cancel(
                    &pass_directory,
                    &output,
                    &scene_paths,
                    None,
                    total_frames,
                    framerate,
                    None,
                    &progress_tx,
                    cancellation,
                )?;
            },
            ConcatMethod::Ivf => {
                SceneConcatenator::ivf_cancel(&output, &scene_paths, &progress_tx, cancellation)?;
            },
        }

        let cancelled = &cancellation.legacy_flag();
        let metric_input = metric_input.unwrap_or(input);
        // VMAF is scored by libvmaf over decoded frames, and its decoder borrow
        // would conflict with the one the node graph below needs. Score it
        // first, while `metric_input` is still free.
        //
        // The VapourSynth VMAF plugin cannot be used here at all: it emits no
        // per-frame scores and requires writing to and reading back a log file.
        //
        // Only the frames this pass actually probes are decoded and scored,
        // matching the plugin path, which trims each frame out of the
        // graph so VapourSynth never decodes the skipped ones.
        //
        // A native failure yields `None` and falls through to the plugin
        // branches; VMAF has no such branch and propagates its error.
        let is_vmaf = matches!(config.metric, QualityMetric::VMAF { .. });
        let engine = metrics::engine(&config.metric, !matches!(metric_input, Input::Video { .. }));
        let native = is_vmaf || engine.is_some();

        // Created and relayed here, before scoring, so the native path reports
        // compare progress through the same relay the plugin branches
        // use and the UI shows the same shape whichever path ran.
        let (compare_progress_tx, compare_thread) = spawn_compare_progress(progress_tx.clone());

        let native_scores = if native {
            let probed: Vec<usize> =
                tasks.iter().flat_map(|task| task.frame_indices.iter().copied()).collect();

            let reference = metric_input.decoder();
            let mut distorted = Input::from_video(&InputModel::Video {
                path: output.clone(),
                import_method: ImportMethod::FFMS2 {
                    index: None,
                    cache_path: None,
                },
                // The probe output is already in the metric input's format, so
                // it must not be converted again.
                filters: Vec::new(),
            })?;

            // The probe encode contains only the frames this pass selected, so
            // it is compacted and is read by position within the
            // selection. VMAF submission drives the same progress relay as the
            // other engines' score callbacks; final scores must not count twice.
            let scored = if is_vmaf {
                metrics::score_vmaf_frames_with_progress(
                    reference,
                    distorted.decoder(),
                    &config.metric,
                    &probed,
                    OutputIndexing::Compacted,
                    Some(cancelled),
                    |position| {
                        report_compare_progress(&compare_progress_tx, probed.len(), position + 1);
                    },
                    |_, _| {},
                )
            } else {
                // A name with no entry point here yields an error rather than a
                // `return`, so it takes the same path as any
                // other native failure and the progress
                // relay is joined on the way out.
                match engine {
                    Some(engine @ (Engine::Vship | Engine::Fmetrics)) => Self::score_native_scenes(
                        engine,
                        reference,
                        distorted.decoder(),
                        &config.metric,
                        &scenes,
                        &compare_progress_tx,
                        cancelled,
                    ),
                    // Unreachable: `engine` names only engines that score natively. A
                    // failure rather than a `return` so adding a fourth engine
                    // cannot quietly discard the pass.
                    Some(Engine::VapourSynth | Engine::Vmaf) | None => Err(anyhow::anyhow!(
                        "no native engine can score {}; it was chosen as {}",
                        config.metric.friendly_name(),
                        engine.map_or("none", Engine::as_str),
                    )),
                }
            };

            Some(scored)
        } else {
            None
        };

        // The relay thread spawned above ends only once every sender is
        // dropped. Each plugin branch moves its sender into
        // `get_scores`, so the channel disconnects as soon as that call
        // returns. The branch that reuses the native scores sends a
        // final completion and drops its sender instead; holding it
        // across the `join` below would leave the relay thread waiting
        // forever.
        // A metric scored natively above reuses those scores; otherwise the
        // plugin branches below run. For the vship metrics the plugin
        // branch is reached only when libvship was unavailable or
        // failed.
        let mut plugin_backend = None;
        let plugin = |compare_progress_tx| -> Result<Vec<f64>> {
            // The branches below score over a VapourSynth graph built from the
            // metric input, so everything here - the graph itself,
            // and for a natively decoded input the VapourSynth
            // script it is converted into - is created only on this
            // path. A host without VapourSynth scores every metric with a
            // native engine without touching it at all.
            //
            // Owned, so it outlives the `&mut v_input` borrow taken below.
            let mut v_input_owned = if matches!(metric_input, Input::Video { .. }) {
                metric_input.as_vapoursynth_script()?
            } else {
                None
            };
            let v_input = v_input_owned.as_mut();
            let decoder = match metric_input {
                Input::VapourSynth { decoder, .. } | Input::VapourSynthScript { decoder, .. } => {
                    decoder
                },
                Input::Video { .. } => v_input.expect("Video Input exists").decoder(),
            };
            let vapoursynth_decoder =
                decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
            let env = &vapoursynth_decoder.env;
            let reference_node = vapoursynth_decoder.get_output(
                vapoursynth_decoder.get_output_index(),
                vapoursynth_decoder.get_node_modifier(),
            )?;
            let core = get_core(env)?;
            plugin_backend = Some(MetricBackend::plugin_for_metric(core, &config.metric)?);

            let reference_node = if temporal {
                reference_node
            } else {
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

            Ok(match &config.metric {
                // Always scored above, since VMAF has no plugin branch here.
                QualityMetric::VMAF { .. } => {
                    unreachable!("VMAF is scored natively above, not here")
                },
                QualityMetric::SSIMULACRA2 {
                    resolution,
                    threads,
                    gpu_id,
                    ..
                } => {
                    let (reference_node, distorted_node) =
                        if let Some((width, height)) = *resolution {
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
                            gpu_id: gpu_id.map(u32::from),
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
                    gpu_id,
                    ..
                } => {
                    let (reference_node, distorted_node) =
                        if let Some((width, height)) = *resolution {
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
                        gpu_id: gpu_id.map(u32::from),
                        ..Default::default()
                    };
                    let node = plugin.invoke(core, &reference_node, &distorted_node)?;
                    BUTTERAUGLI::get_scores(
                        &node,
                        norm.and_then(|_| Some(BUTTERAUGLI::QNORM_PROPERTY_NAMES)),
                        compare_progress_tx,
                    )?
                },
                QualityMetric::XPSNR { resolution, .. } => {
                    let (reference_node, distorted_node) =
                        if let Some((width, height)) = *resolution {
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
                    // XPSNR returns a score per plane, combine them into the
                    // weighted XPSNR score.
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
                    gpu_id,
                    ..
                } => {
                    let (reference_node, distorted_node) =
                        if let Some((width, height)) = *resolution {
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
                        gpu_id: gpu_id.map(u32::from),
                        ..Default::default()
                    };
                    if temporal {
                        plugin.collect_scene_values(
                            core,
                            &reference_node,
                            &distorted_node,
                            &scenes,
                            OutputIndexing::Compacted,
                            compare_progress_tx,
                            cancelled,
                            |_, _| Ok(()),
                        )?
                    } else {
                        let node = plugin.invoke(
                            core,
                            &reference_node,
                            &distorted_node,
                            // The compacted encode/node indices cannot describe source gaps.
                            tasks.iter().flat_map(|task| task.frame_indices.iter().copied()),
                        )?;
                        CVVDP::get_scores(&node, None, compare_progress_tx)?
                    }
                },
            })
        };
        let Some((scores, actual)) = Self::route_compare_scores(
            native_scores,
            engine,
            &config.metric,
            compare_progress_tx,
            compare_thread,
            plugin,
            |scores| Self::validate_task_scores(config, tasks, scores),
        )?
        else {
            return Ok((Vec::new(), warnings));
        };
        let actual = if actual == Engine::VapourSynth {
            plugin_backend.context("successful plugin branch has no admission identity")?
        } else {
            MetricBackend::current(actual)
        };

        drop(progress_tx);

        // Zone and per-scene results both identify the original scene, not
        // its position in this candidate batch.
        let results = results
            .into_iter()
            .flatten()
            .map(|result| (result.scene, result))
            .collect::<HashMap<_, _>>();

        let grouped = Self::group_task_scores(config, tasks, scores)?;
        let tasks = tasks
            .iter()
            .zip(grouped)
            .map(|(task, scores)| -> Result<Task> {
                let mut completed_task = task.clone();
                let result = results.get(&task.original_index).with_context(|| {
                    format!(
                        "Target Quality scene {} pass {pass} has no completed encode",
                        task.original_index
                    )
                })?;
                // Keep the verified prefix; replace any partially recorded
                // current pass and its dependent suffix only after full admission.
                completed_task.passes.truncate(previous_passes);
                completed_task.passes.push(QualityPass {
                    provenance: None,
                    backend: Some(actual.clone()),
                    quantizer: task.encoder.quantizer().expect("quantizer exists"),
                    scores,
                    bitrate: result.bitrate,
                    started_on: result
                        .started
                        .duration_since(std::time::UNIX_EPOCH)
                        .context("Probe encode start precedes UNIX epoch")?
                        .as_millis(),
                    completed_on: result
                        .ended
                        .duration_since(std::time::UNIX_EPOCH)
                        .context("Probe encode completion precedes UNIX epoch")?
                        .as_millis(),
                });
                Self::validate_history(
                    config,
                    task.original_index,
                    task.frame_indices.len(),
                    &completed_task.passes,
                )?;
                Ok(completed_task)
            })
            .collect::<Result<Vec<_>>>()?;

        Ok((tasks, warnings))
    }

    fn group_task_scores(
        config: &TargetQualityConfig,
        tasks: &[Task],
        mut scores: Vec<f64>,
    ) -> Result<Vec<Vec<f64>>> {
        Self::validate_task_scores(config, tasks, &scores)?;
        Ok(tasks
            .iter()
            .map(|task| scores.drain(..task.frame_indices.len()).collect())
            .collect())
    }

    fn validate_task_scores(
        config: &TargetQualityConfig,
        tasks: &[Task],
        scores: &[f64],
    ) -> Result<()> {
        let expected = tasks.iter().map(|task| task.frame_indices.len()).sum();
        validate_scores(scores, Some(expected)).with_context(|| {
            format!(
                "Target Quality {} flattened measurement",
                config.metric.friendly_name()
            )
        })?;
        let mut offset = 0;
        for task in tasks {
            let end = offset + task.frame_indices.len();
            config
                .metric
                .calculate_score(config.probing.statistic, &scores[offset..end])
                .with_context(|| {
                    format!(
                        "Target Quality {} scene {} pass {}",
                        config.metric.friendly_name(),
                        task.original_index,
                        task.passes.len() + 1
                    )
                })?;
            offset = end;
        }
        Ok(())
    }

    /// Whether `pass` was recorded for every scene.
    ///
    /// A pass is scored for all its scenes at once, so a pass recorded for some
    /// scenes but not others never completed and its leftovers describe no
    /// single encode. An empty task list is incomplete: there is nothing to
    /// reuse.
    #[inline]
    fn pass_is_complete(tasks: &[Task], pass: u8) -> bool {
        !tasks.is_empty() && tasks.iter().all(|task| task.passes.len() >= usize::from(pass))
    }

    /// Own fallback, incomplete-pass routing and relay cleanup after scoring.
    pub(super) fn route_compare_scores(
        native: Option<Result<Vec<f64>>>,
        engine: Option<Engine>,
        metric: &QualityMetric,
        compare_progress_tx: sync::mpsc::Sender<SequenceStatus>,
        compare_thread: thread::JoinHandle<Result<()>>,
        plugin: impl FnOnce(sync::mpsc::Sender<SequenceStatus>) -> Result<Vec<f64>>,
        admit: impl Fn(&[f64]) -> Result<()>,
    ) -> Result<Option<(Vec<f64>, Engine)>> {
        let is_vmaf = matches!(metric, QualityMetric::VMAF { .. });
        let temporal = metrics::scene_temporal(metric);
        let mut terminal_error = None;
        let scores = match native {
            Some(Ok(scores)) => Some(scores),
            Some(Err(error)) if is_vmaf => {
                terminal_error = Some(error);
                None
            },
            Some(Err(error))
                if error.is::<metrics::probe::Cancelled>()
                    || error.is::<metrics::SceneScoringStarted>()
                    || error.is::<MeasurementError>() =>
            {
                terminal_error = Some(error);
                None
            },
            Some(Err(error)) => {
                log_native_fallback(&error, engine, metric);
                None
            },
            None => None,
        };
        let terminal = terminal_error.is_some();
        let actual = if scores.is_some() {
            if is_vmaf {
                Engine::Vmaf
            } else {
                engine.context("native success has no engine")?
            }
        } else {
            Engine::VapourSynth
        };
        let source = if scores.is_some() || terminal {
            engine.map_or("native", Engine::as_str)
        } else {
            "vapoursynth"
        };
        let completion_tx = compare_progress_tx.clone();
        let comparison = if let Some(error) = terminal_error {
            // Disconnect before joining; retaining this sender would deadlock.
            drop(compare_progress_tx);
            Err(error)
        } else if let Some(scores) = scores {
            drop(compare_progress_tx);
            Ok(scores)
        } else {
            plugin(compare_progress_tx)
        };
        let comparison = comparison
            .and_then(|scores| {
                validate_scores(&scores, None)?;
                admit(&scores)?;
                Ok(scores)
            })
            .with_context(|| format!("{source} {} measurement", metric.friendly_name()));
        if comparison.is_ok() {
            let _ = completion_tx.send(SequenceStatus::Whole(Status::Completed {
                id: "Compare".to_owned(),
            }));
        }
        drop(completion_tx);
        let comparison = if temporal || comparison.is_err() {
            finish_temporal_compare(comparison, compare_thread)
        } else {
            // Preserve the successful non-temporal pass's fatal relay-send policy.
            compare_thread.join().expect("compare progress thread should join")?;
            comparison
        };
        match comparison {
            Err(error) if (temporal || terminal) && error.is::<metrics::probe::Cancelled>() => {
                Ok(None)
            },
            result => result.map(|scores| Some((scores, actual))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn score_native_scenes(
        engine: Engine,
        reference: &mut av_decoders::Decoder,
        distorted: &mut av_decoders::Decoder,
        metric: &QualityMetric,
        scenes: &[&[usize]],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &AtomicBool,
    ) -> Result<Vec<f64>> {
        let total = scenes.iter().map(|scene| scene.len()).sum();
        metrics::score_scene_frames(
            engine,
            reference,
            distorted,
            metric,
            scenes,
            OutputIndexing::Compacted,
            Some(cancelled),
            |position, _| {
                report_compare_progress(progress_tx, total, position + 1);
            },
        )
    }

    /// Returns the highest quantizer whose probe scored within the target
    /// range, which is the smallest encode known to meet the target.
    #[inline]
    pub fn verified_quantizer(
        metric: &QualityMetric,
        quantizer_score_history: &[(f64, f64)],
    ) -> Option<f64> {
        quantizer_score_history
            .iter()
            .filter(|(_quantizer, score)| metric.score_within_target(*score))
            .map(|(quantizer, _score)| *quantizer)
            .max_by(f64::total_cmp)
    }

    // A closest measurement is a fallback, not evidence that the target was met.
    pub(crate) fn measured_quantizer(
        metric: &QualityMetric,
        history: &[(f64, f64)],
    ) -> Option<f64> {
        Self::verified_quantizer(metric, history).or_else(|| {
            let target = metric.target_range().0.midpoint(metric.target_range().1);
            history
                .iter()
                .min_by(|(qa, sa), (qb, sb)| {
                    (sa - target)
                        .abs()
                        .total_cmp(&(sb - target).abs())
                        .then_with(|| qb.total_cmp(qa))
                })
                .map(|(q, _)| *q)
        })
    }

    /// Interpolates and rounds a prediction; it may already have been measured.
    /// Task admission separately selects an untested interior grid point.
    ///
    /// With 0/1 records use the configured midpoint; with 2 use linear; with 3/4
    /// use the first/second configured method; with 5+ use PCHIP over all records.
    /// Linear needs 2 points, quadratic/Fritsch–Carlson/natural need 3, PCHIP
    /// needs 2, and Catmull/Akima/cubic polynomial need 4. Catmull only covers
    /// the inner interval. Insufficient points, rejection, or nonfinite results
    /// fall back to the nearest score-bracketing linear pair, then midpoint.
    /// Duplicate scores (including flat histories) or targets outside the observed
    /// score range use midpoint without pruning records or extrapolating.
    /// Finite estimates are rounded to the step and clamped to the requested range.
    ///
    /// # Errors
    /// Rejects nonfinite targets or history quantizers/scores before sorting or
    /// prediction, including empty/single histories. Also rejects nonfinite/reversed
    /// ranges, nonpositive/nonfinite steps, and grids whose tick magnitudes exceed
    /// 2^52 - 1, before rounding or clamping.
    #[inline]
    pub fn predict_quantizer(
        quantizer_range: (f64, f64),
        target_score: f64,
        interpolators: (InterpolationMethod, InterpolationMethod),
        quantizer_score_history: &[(f64, f64)],
        step: f64,
    ) -> Result<f64> {
        Self::validate_quantizer_grid(quantizer_range, step)?;
        ensure!(target_score.is_finite(), "target score must be finite");
        for (index, (quantizer, score)) in quantizer_score_history.iter().enumerate() {
            ensure!(
                quantizer.is_finite(),
                "history[{index}].quantizer must be finite"
            );
            ensure!(score.is_finite(), "history[{index}].score must be finite");
        }
        let midpoint = quantizer_range.0.midpoint(quantizer_range.1);

        let predicted_quantizer = match quantizer_score_history.len() {
            0..=1 => midpoint,
            n => {
                // Sort history by score.
                let mut sorted = quantizer_score_history.to_vec();
                sorted.sort_by(|(_, s1), (_, s2)| s1.total_cmp(s2));

                let (scores, quantizers): (Vec<f64>, Vec<f64>) =
                    sorted.iter().map(|(q, s)| (*s, *q)).unzip();

                if scores.windows(2).any(|pair| pair[0] == pair[1])
                    || !(scores[0]..=scores[n - 1]).contains(&target_score)
                {
                    midpoint
                } else {
                    let linear = || {
                        let k = scores
                            .windows(2)
                            .position(|pair| pair[0] <= target_score && target_score <= pair[1])?;
                        interpolators::linear(
                            scores[k..k + 2].try_into().ok()?,
                            quantizers[k..k + 2].try_into().ok()?,
                            target_score,
                        )
                    };
                    let method = match n {
                        2 => InterpolationMethod::Linear,
                        3 => interpolators.0,
                        4 => interpolators.1,
                        _ => InterpolationMethod::Pchip,
                    };
                    let result = match method {
                        InterpolationMethod::Linear => linear(),
                        InterpolationMethod::Quadratic | InterpolationMethod::FritschCarlson => {
                            // Use a contiguous triple containing the target, not always the first.
                            let k = usize::from(n == 4 && target_score > scores[2]);
                            let s = scores[k..k + 3].try_into()?;
                            let q = quantizers[k..k + 3].try_into()?;
                            if method == InterpolationMethod::Quadratic {
                                interpolators::quadratic(s, q, target_score)
                            } else {
                                interpolators::fritsch_carlson(s, q, target_score)
                            }
                        },
                        InterpolationMethod::Natural => {
                            interpolators::natural_cubic_spline(&scores, &quantizers, target_score)
                        },
                        InterpolationMethod::Pchip => {
                            interpolators::pchip(&scores, &quantizers, target_score)
                        },
                        InterpolationMethod::Catmull
                        | InterpolationMethod::Akima
                        | InterpolationMethod::CubicPolynomial
                            if n < 4 =>
                        {
                            None
                        },
                        InterpolationMethod::Catmull => interpolators::catmull_rom(
                            scores[..4].try_into()?,
                            quantizers[..4].try_into()?,
                            target_score,
                        ),
                        InterpolationMethod::Akima => interpolators::akima(
                            scores[..4].try_into()?,
                            quantizers[..4].try_into()?,
                            target_score,
                        ),
                        InterpolationMethod::CubicPolynomial => interpolators::cubic_polynomial(
                            scores[..4].try_into()?,
                            quantizers[..4].try_into()?,
                            target_score,
                        ),
                    };

                    result
                        .filter(|q| q.is_finite())
                        .or_else(|| linear().filter(|q| q.is_finite()))
                        .unwrap_or_else(|| {
                            trace!("Interpolation failed, falling back to configured midpoint");
                            midpoint
                        })
                }
            },
        };

        // Round the result of the interpolation to the nearest step.
        Ok(((predicted_quantizer / step).round() * step)
            .clamp(quantizer_range.0, quantizer_range.1))
    }
}

impl Default for TargetQuality {
    #[inline]
    fn default() -> Self {
        Self {
            input: None,
            metric_input: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub original_index: usize,
    pub frame_indices: Vec<usize>,
    pub encoder: Encoder,
    pub output: PathBuf,
    pub passes: Vec<QualityPass>,
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

/// The UI needs a zero report to switch from encoding to comparing. Emit it
/// with the first accepted frame; a closed UI receiver is not a scoring error.
fn report_compare_progress(
    progress_tx: &sync::mpsc::Sender<SequenceStatus>,
    total: usize,
    completed: usize,
) {
    if completed == 1 {
        let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id: "Compare".to_owned(),
            completion: SequenceCompletion::Frames {
                completed: 0,
                total: total as u64,
            },
        }));
    }
    let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
        id: "Compare".to_owned(),
        completion: SequenceCompletion::Frames {
            completed: completed as u64,
            total: total as u64,
        },
    }));
}

pub(super) fn log_native_fallback(
    error: &anyhow::Error,
    engine: Option<Engine>,
    metric: &QualityMetric,
) {
    // The unsupported field/value may be hidden behind several anyhow contexts.
    error!(
        error = %format_args!("{error:#}"),
        engine = ?engine.map(Engine::as_str),
        metric = metric.friendly_name(),
        "native metric scoring failed, falling back to the VapourSynth plugin"
    );
}

#[cfg(test)]
mod tests {
    use std::sync;

    use super::{
        SequenceCompletion, SequenceStatus, Status, TargetQuality, Task, spawn_compare_progress,
    };
    use crate::{
        metrics::Engine,
        models::sequence::target_quality::{
            TargetQualityConfig,
            types::{MeasurementError, QualityPass},
        },
    };

    use crate::models::{
        encoder::{Encoder, EncoderBase, photon_noise::PhotonNoise},
        sequence::target_quality::types::QualityMetric,
    };

    const Q08_METHODS: [crate::models::sequence::target_quality::types::InterpolationMethod; 8] = {
        use crate::models::sequence::target_quality::types::InterpolationMethod::*;
        [
            Linear,
            Quadratic,
            Natural,
            FritschCarlson,
            Pchip,
            Catmull,
            Akima,
            CubicPolynomial,
        ]
    };

    #[test]
    fn q08_three_record_pchip_runs_the_curve_not_midpoint() {
        use crate::models::sequence::target_quality::types::InterpolationMethod::Pchip;
        assert_eq!(
            TargetQuality::predict_quantizer(
                (0.0, 40.0),
                1.5,
                (Pchip, Pchip),
                &[(1.0, 0.0), (4.0, 1.0), (9.0, 2.0)],
                1.0 / 1024.0,
            )
            .expect("three-point PCHIP"),
            6.34375
        );
    }

    #[test]
    fn q08_three_record_four_point_methods_use_bracketing_linear_not_midpoint() {
        use crate::models::sequence::target_quality::types::InterpolationMethod::*;
        for method in [Catmull, Akima, CubicPolynomial] {
            assert_eq!(
                TargetQuality::predict_quantizer(
                    (0.0, 40.0),
                    1.5,
                    (method, Pchip),
                    &[(1.0, 0.0), (4.0, 1.0), (9.0, 2.0)],
                    0.25,
                )
                .expect("insufficient-point fallback"),
                6.5,
                "{method:?}"
            );
        }
    }

    #[test]
    fn q08_public_prediction_method_phase_matrix() {
        // q = (score + 1)^2. Literal curve evaluations, not a second dispatch implementation.
        // Hermite at t=1/2 is (q0+q1)/2 + (m0-m1)/8.
        let history = [(1.0, 0.0), (4.0, 1.0), (9.0, 2.0), (16.0, 3.0), (25.0, 4.0), (36.0, 5.0)];
        let cases = [
            (0, 0.5, [20.0; 8]),
            (1, 0.5, [20.0; 8]),
            (2, 0.5, [2.5; 8]),
            (3, 1.5, [6.5, 6.25, 6.3125, 6.34375, 6.34375, 6.5, 6.5, 6.5]),
            (
                4,
                1.5,
                [6.5, 6.25, 6.2, 6.34375, 6.239583333333333, 6.375, 6.25, 6.25],
            ),
            (5, 1.5, [6.239583333333333; 8]),
            (6, 1.5, [6.239583333333333; 8]),
        ];
        for (count, target, expected) in cases {
            // Deliberately unsorted: admission must order by score, not by quantizer or arrival.
            let mut samples = history[..count].to_vec();
            samples.reverse();
            for (method, want) in Q08_METHODS.into_iter().zip(expected) {
                let got = TargetQuality::predict_quantizer(
                    (0.0, 40.0),
                    target,
                    (method, method),
                    &samples,
                    1.0 / 1024.0,
                )
                .expect("finite prediction");
                assert!(
                    (got - want).abs() <= 1.0 / 2048.0,
                    "{method:?}, {count} records: {got} != {want}"
                );
            }
        }
    }

    #[test]
    fn q08_public_prediction_windows_and_tuple_positions() {
        use crate::models::sequence::target_quality::types::InterpolationMethod::*;
        let history = [(1.0, 0.0), (4.0, 1.0), (9.0, 2.0), (16.0, 3.0)];
        for (count, target, methods, want) in [
            (3, 1.5, (Quadratic, Natural), 6.25),
            (4, 1.5, (Quadratic, Natural), 6.2),
            (4, 2.5, (Linear, Linear), 12.5),
            (4, 2.5, (Linear, Quadratic), 12.25),
            (4, 2.5, (Linear, FritschCarlson), 12.354166666666666),
            (4, 2.5, (Linear, Natural), 12.35),
            (4, 2.5, (Linear, Pchip), 12.354166666666666),
            // Catmull rejects the outer interval: use its nearest bracketing pair.
            (4, 0.5, (Linear, Catmull), 2.5),
            (4, 2.5, (Linear, Catmull), 12.5),
            (4, 2.5, (Linear, Akima), 12.375),
            (4, 2.5, (Linear, CubicPolynomial), 12.25),
        ] {
            let got = TargetQuality::predict_quantizer(
                (0.0, 40.0),
                target,
                methods,
                &history[..count],
                1.0 / 1024.0,
            )
            .expect("window prediction");
            assert!(
                (got - want).abs() <= 1.0 / 2048.0,
                "{methods:?}, {count}/{target}: {got} != {want}"
            );
        }
        // The fifth and sixth records must participate, not just trigger a phase switch.
        for history in [
            &[(1.0, 0.0), (4.0, 1.0), (9.0, 2.0), (16.0, 3.0), (25.0, 4.0)][..],
            &[(1.0, 0.0), (4.0, 1.0), (9.0, 2.0), (16.0, 3.0), (25.0, 4.0), (36.0, 5.0)][..],
        ] {
            let got = TargetQuality::predict_quantizer(
                (0.0, 40.0),
                history.len() as f64 - 1.0,
                (Catmull, Quadratic),
                history,
                0.25,
            )
            .expect("whole history PCHIP");
            assert_eq!(got, if history.len() == 5 { 25.0 } else { 36.0 });
        }
    }

    #[test]
    fn q08_public_prediction_duplicate_flat_and_outside_fallback_matrix() {
        let history = [(1.0, 0.0), (4.0, 1.0), (9.0, 2.0), (16.0, 3.0), (25.0, 4.0), (36.0, 5.0)];
        for method in Q08_METHODS {
            for count in 2..=6 {
                let mut duplicate = history[..count].to_vec();
                duplicate[count - 1].1 = duplicate[count - 2].1;
                let flat: Vec<_> = history[..count].iter().map(|(q, _)| (*q, 0.0)).collect();
                for (samples, target) in [
                    (&duplicate[..], 0.5),
                    (&flat[..], 0.0),
                    (&history[..count], -1.0),
                    (&history[..count], 10.0),
                ] {
                    assert_eq!(
                        TargetQuality::predict_quantizer(
                            (0.0, 40.0),
                            target,
                            (method, method),
                            samples,
                            0.25,
                        )
                        .expect("bounded midpoint"),
                        20.0,
                        "{method:?}/{count}/{samples:?}/{target}"
                    );
                }
            }
        }
    }

    #[test]
    fn q08_public_prediction_keeps_noisy_measurements_and_clamps_finite_estimates() {
        let noisy = [(1.0, 0.0), (9.0, 1.0), (4.0, 2.0), (16.0, 3.0), (25.0, 4.0), (36.0, 5.0)];
        for method in Q08_METHODS {
            for count in 2..=6 {
                assert_eq!(
                    TargetQuality::predict_quantizer(
                        (0.0, 40.0),
                        1.0,
                        (method, method),
                        &noisy[..count],
                        0.25,
                    )
                    .expect("retain measured knot"),
                    9.0,
                    "{method:?}/{count}"
                );
                let got = TargetQuality::predict_quantizer(
                    (0.0, 40.0),
                    0.5,
                    (method, method),
                    &noisy[..count],
                    0.25,
                )
                .expect("noisy interior prediction");
                assert!(got.is_finite() && (0.0..=40.0).contains(&got));
                for (range, want) in [((0.0, 2.0), 2.0), ((10.0, 20.0), 10.0)] {
                    assert_eq!(
                        TargetQuality::predict_quantizer(
                            range,
                            1.0,
                            (method, method),
                            &noisy[..count],
                            0.25,
                        )
                        .expect("finite estimate is clamped"),
                        want,
                        "{method:?}/{count}"
                    );
                }
            }
        }
    }

    #[test]
    fn q08_public_prediction_rejects_nonfinite_inputs_even_before_midpoint() {
        let history = [(1.0, 0.0), (4.0, 1.0), (9.0, 2.0), (16.0, 3.0), (25.0, 4.0), (36.0, 5.0)];
        for method in Q08_METHODS {
            for count in 0..=6 {
                for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                    let error = TargetQuality::predict_quantizer(
                        (0.0, 40.0),
                        invalid,
                        (method, method),
                        &history[..count],
                        0.25,
                    )
                    .expect_err("nonfinite target must be rejected");
                    assert!(error.to_string().contains("target score"), "{error:#}");
                    for index in 0..count {
                        for field in ["quantizer", "score"] {
                            let mut samples = history[..count].to_vec();
                            if field == "quantizer" {
                                samples[index].0 = invalid;
                            } else {
                                samples[index].1 = invalid;
                            }
                            let error = TargetQuality::predict_quantizer(
                                (0.0, 40.0),
                                0.5,
                                (method, method),
                                &samples,
                                0.25,
                            )
                            .expect_err("every nonfinite record must be rejected");
                            let message = error.to_string();
                            assert!(
                                message.contains(&format!("history[{index}].{field}")),
                                "{message}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn q08_public_prediction_filters_algorithm_overflow_before_clamping() {
        use crate::models::sequence::target_quality::types::InterpolationMethod::*;
        let tiny = [(1.0, 0.0), (4.0, 1e-200), (9.0, 2e-200), (16.0, 3e-200)];
        // Lagrange products underflow to zero; the nearest linear pair still gives 6.5.
        for (count, methods) in [(3, (Quadratic, Linear)), (4, (Linear, CubicPolynomial))] {
            assert_eq!(
                TargetQuality::predict_quantizer(
                    (0.0, 40.0),
                    1.5e-200,
                    methods,
                    &tiny[..count],
                    0.25,
                )
                .expect("finite linear fallback"),
                6.5
            );
        }
        // Even the secant overflows: no infinity/NaN may reach the final clamp.
        let huge = [(f64::MAX, 0.0), (-f64::MAX, 1.0)];
        for method in Q08_METHODS {
            assert_eq!(
                TargetQuality::predict_quantizer((0.0, 40.0), 0.5, (method, method), &huge, 0.25,)
                    .expect("finite midpoint fallback"),
                20.0
            );
            let extreme = [
                (f64::MAX, 0.0),
                (-f64::MAX, 1.0),
                (f64::MAX, 2.0),
                (-f64::MAX, 3.0),
                (f64::MAX, 4.0),
                (-f64::MAX, 5.0),
            ];
            for count in 2..=6 {
                let got = TargetQuality::predict_quantizer(
                    (0.0, 40.0),
                    0.5,
                    (method, method),
                    &extreme[..count],
                    0.25,
                )
                .expect("overflow must stay bounded in every phase");
                assert!(
                    got.is_finite() && (0.0..=40.0).contains(&got),
                    "{method:?}/{count}: {got}"
                );
            }
        }
    }

    #[test]
    fn q04_duplicate_rounding_admits_an_untested_interior_probe() {
        use crate::models::sequence::target_quality::TargetQualityConfig;
        let config = TargetQualityConfig {
            quantizer_range: (26, 30),
            metric: QualityMetric::SSIMULACRA2 {
                target_range: (75.0, 75.0),
                resolution: None,
                threads: None,
                gpu_id: None,
            },
            ..Default::default()
        };
        let encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, false);
        let history = [(26.0, 76.05), (30.0, 67.5)];
        assert_eq!(
            TargetQuality::predict_quantizer(
                (26.0, 30.0),
                75.0,
                config.interpolators,
                &history,
                1.0
            )
            .expect("old interpolation remains public"),
            26.0
        );
        assert_eq!(
            TargetQuality::next_probe_quantizer(&config, &encoder, &history, &Default::default())
                .expect("next probe"),
            Some(28.0)
        );
        let single = TargetQualityConfig {
            quantizer_range: (26, 26),
            ..config
        };
        assert_eq!(
            TargetQuality::next_probe_quantizer(&single, &encoder, &[], &Default::default())
                .expect("single initial probe"),
            Some(26.0)
        );
        assert_eq!(
            TargetQuality::next_probe_quantizer(
                &single,
                &encoder,
                &[(26.0, 76.05)],
                &Default::default()
            )
            .expect("single exhausted probe"),
            None
        );
    }

    #[test]
    fn q04_grid_literal_matrix() {
        let cases = [
            (
                "quarter fallback",
                (26.0, 27.0),
                26.0,
                0.25,
                &[(26.0, 76.05), (27.0, 67.5)][..],
                Some(26.5),
            ),
            (
                "interior interpolation first",
                (26.0, 30.0),
                27.0,
                1.0,
                &[(26.0, 76.05), (30.0, 67.5)],
                Some(27.0),
            ),
            (
                "untested endpoint loses to interior",
                (26.0, 30.0),
                26.0,
                1.0,
                &[],
                Some(28.0),
            ),
            (
                "outside uses midpoint",
                (26.0, 30.0),
                40.0,
                1.0,
                &[],
                Some(28.0),
            ),
            (
                "unsorted repeated tied history",
                (26.0, 30.0),
                26.0,
                1.0,
                &[(30.0, 67.5), (28.0, 76.05), (26.0, 76.05), (28.0, 76.05)],
                Some(27.0),
            ),
            (
                "midpoint tested lower tie",
                (26.0, 30.0),
                28.0,
                1.0,
                &[(28.0, 75.0)],
                Some(27.0),
            ),
            (
                "midpoint and lower tested",
                (26.0, 30.0),
                28.0,
                1.0,
                &[(28.0, 75.0), (27.0, 76.0)],
                Some(29.0),
            ),
            ("initial", (26.0, 30.0), 28.0, 1.0, &[], Some(28.0)),
            ("equal initial", (26.0, 26.0), 26.0, 1.0, &[], Some(26.0)),
            (
                "equal measured",
                (26.0, 26.0),
                26.0,
                1.0,
                &[(26.0, 75.0)],
                None,
            ),
            (
                "adjacent measured",
                (26.0, 27.0),
                26.0,
                1.0,
                &[(26.0, 76.05), (27.0, 67.5)],
                None,
            ),
            (
                "fully exhausted",
                (26.0, 28.0),
                26.0,
                1.0,
                &[(26.0, 76.0), (28.0, 74.0), (27.0, 75.0)],
                None,
            ),
            (
                "missing side configured endpoint",
                (26.0, 28.0),
                27.0,
                1.0,
                &[(27.0, 76.0), (28.0, 74.0)],
                Some(26.0),
            ),
            (
                "bounds not grid aligned",
                (26.1, 27.9),
                26.1,
                1.0,
                &[],
                Some(27.0),
            ),
            ("no grid point", (26.1, 26.9), 26.5, 1.0, &[], None),
            (
                "large sparse range",
                (0.0, u32::MAX as f64),
                0.0,
                0.25,
                &[(0.0, 1.0), (2_147_483_647.5, 2.0)],
                Some(2_147_483_647.25),
            ),
        ];
        for (name, bounds, predicted, step, history, expected) in cases {
            let actual = TargetQuality::untested_quantizer(bounds, predicted, history, step);
            assert_eq!(actual, expected, "{name}");
            if let Some(q) = actual {
                assert!((bounds.0..=bounds.1).contains(&q), "{name}");
                assert_eq!(q / step, (q / step).round(), "{name}");
                assert!(!history.iter().any(|(tested, _)| *tested == q), "{name}");
            }
        }
    }

    #[test]
    fn q04_public_prediction_rejects_invalid_grid_before_arithmetic() {
        use crate::models::sequence::target_quality::types::InterpolationMethod;
        for (range, step, context) in [
            ((30.0, 26.0), 1.0, "range"),
            ((f64::NAN, 30.0), 1.0, "range"),
            ((26.0, f64::INFINITY), 1.0, "range"),
            ((26.0, 30.0), 0.0, "step"),
            ((26.0, 30.0), -1.0, "step"),
            ((26.0, 30.0), f64::NAN, "step"),
            ((26.0, 30.0), f64::INFINITY, "step"),
            ((26.0, 30.0), f64::MIN_POSITIVE, "grid"),
            ((-f64::MAX, f64::MAX), 1.0, "grid"),
        ] {
            let error = TargetQuality::predict_quantizer(
                range,
                75.0,
                (InterpolationMethod::Linear, InterpolationMethod::Linear),
                &[],
                step,
            )
            .expect_err("invalid public grid");
            assert!(format!("{error:#}").contains(context), "{error:#}");
        }
    }

    #[test]
    fn q04_final_selection_prefers_verified_then_closest_measured_with_higher_q_ties() {
        let metric = ssimulacra2();
        for (history, expected) in [
            (&[][..], None),
            (&[(28.0, 70.0)], Some(28.0)),
            (&[(26.0, 76.05), (30.0, 67.5)], Some(26.0)),
            (&[(24.0, 77.0), (28.0, 73.0)], Some(28.0)),
            (&[(28.0, 73.0), (24.0, 77.0)], Some(28.0)),
            (&[(26.0, 75.0), (28.0, 74.1), (30.0, 73.9)], Some(28.0)),
        ] {
            assert_eq!(
                TargetQuality::measured_quantizer(&metric, history),
                expected
            );
        }
        let inverse = QualityMetric::BUTTERAUGLI {
            target_range: (1.0, 1.0),
            resolution: None,
            threads: None,
            intensity_multiplier: None,
            norm: None,
            gpu_id: None,
        };
        assert_eq!(
            TargetQuality::measured_quantizer(&inverse, &[(20.0, 0.5), (28.0, 1.1), (30.0, 1.5)]),
            Some(28.0)
        );
    }

    // Finite explicit binaries exercise the actual cached capability probe;
    // they establish step routing, not native SVT acceptance.
    #[cfg(unix)]
    #[test]
    fn q04_actual_svt_step_resolver_uses_requested_binary_cache_and_forced_cancellation() {
        use crate::{
            models::sequence::target_quality::TargetQualityConfig,
            utils::cancellation::{Cancellation, Cancelled},
        };
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("executables");
        let config = TargetQualityConfig {
            quantizer_range: (26, 27),
            metric: QualityMetric::SSIMULACRA2 {
                target_range: (75.0, 75.0),
                resolution: None,
                threads: None,
                gpu_id: None,
            },
            ..Default::default()
        };
        for (name, exit, expected, next_expected) in
            [("quarter", 0, Some(26.5), Some(26.25)), ("integer", 1, None, None)]
        {
            let path = temp.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\nset -eu\n[ \"$3\" = --crf ]\n[ \"$4\" = 50.75 ]\nprintf probe >> \"$0.calls\"\n/bin/cat >/dev/null\nexit {exit}\n")).expect("fixture source");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("executable");
            let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
            encoder.set_executable(Some(path.clone()));
            let history = [(26.0, 76.05), (27.0, 67.5)];
            let cancellation = Cancellation::default();
            for _ in 0..2 {
                assert_eq!(
                    TargetQuality::next_probe_quantizer(&config, &encoder, &history, &cancellation)
                        .expect("actual resolver"),
                    expected
                );
            }
            let completed = [(26.0, 76.05), (27.0, 67.5), (26.5, 74.0)];
            assert_eq!(
                TargetQuality::next_probe_quantizer(&config, &encoder, &completed, &cancellation)
                    .expect("updated-history admission"),
                next_expected
            );
            assert_eq!(
                std::fs::read(path.with_extension("calls")).expect("capability invocation"),
                b"probe",
                "cache is keyed by the actual requested binary"
            );
            cancellation.request().expect("graceful");
            cancellation.request().expect("forced");
            let error =
                TargetQuality::next_probe_quantizer(&config, &encoder, &history, &cancellation)
                    .expect_err("force must not become integer-step fallback");
            assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
        }
        for base in [EncoderBase::X264, EncoderBase::X265] {
            let encoder = Encoder::default_from_base(&base, false);
            assert_eq!(
                TargetQuality::next_probe_quantizer(
                    &config,
                    &encoder,
                    &[(26.0, 76.05), (27.0, 67.5)],
                    &Default::default()
                )
                .expect("quarter step"),
                Some(26.5)
            );
        }
    }

    // Real FFmpeg encodes, FFMS2 decode and libvmaf measurement; the resume
    // history is produced by probe_pass, not a test score formula or fake backend.
    #[test]
    fn q04_execute_resumed_duplicate_measures_another_probe_and_final_quantizer() {
        use crate::{
            core::{Condor, input::Input, output::Output, sequence::Sequence},
            models::{
                encoder::cli_parameter::CLIParameter,
                input::{ImportMethod, Input as InputModel},
                scene::Scene,
                sequence::{
                    DefaultSequenceConfig, SequenceDataHandler,
                    parallel_encoder::ParallelEncoderConfig,
                    scene_concatenator::ConcatMethod,
                    target_quality::{
                        TargetQualityConfig, TargetQualityData, TargetQualityDataHandler,
                    },
                },
            },
            utils::cancellation::{Cancellation, Cancelled},
        };
        #[derive(Clone, Default, serde::Serialize)]
        struct AcceptedData {
            target_quality: TargetQualityData,
            #[serde(skip)]
            handoff: Option<(
                crossbeam_channel::Sender<()>,
                crossbeam_channel::Receiver<()>,
            )>,
        }
        impl SequenceDataHandler for AcceptedData {}
        impl TargetQualityDataHandler for AcceptedData {
            fn get_target_quality(&self) -> anyhow::Result<&TargetQualityData> {
                Ok(&self.target_quality)
            }
            fn get_target_quality_mut(&mut self) -> anyhow::Result<&mut TargetQualityData> {
                // This existing data-handler boundary runs after probe_pass accepts
                // real measurements. Pause only the armed test run, not production.
                if let Some((accepted, resume)) = self.handoff.take() {
                    accepted
                        .send_timeout((), std::time::Duration::from_secs(10))
                        .expect("accepted handoff");
                    resume
                        .recv_timeout(std::time::Duration::from_secs(10))
                        .expect("forced handoff");
                }
                Ok(&mut self.target_quality)
            }
        }
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("source.mkv");
        let generated = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=128x128:rate=24",
                "-frames:v",
                "4",
                "-c:v",
                "ffv1",
            ])
            .arg(&source)
            .output()
            .expect("existing FFmpeg fixture generator");
        assert!(generated.status.success(), "{generated:?}");
        let input_model = InputModel::Video {
            path: source,
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            filters: vec![],
        };
        let mut input = Input::from_data(&input_model).expect("FFMS2 input");
        let mut encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, false);
        encoder.parameters_mut().extend(CLIParameter::new_strings(
            "-",
            " ",
            &[("c:v", "libx264"), ("preset", "ultrafast")],
        ));
        let mut config = TargetQualityConfig {
            quantizer_range: (26, 30),
            maximum_probes: 3,
            metric: QualityMetric::VMAF {
                target_range: (75.0, 75.0),
                resolution: None,
                scaler: "bicubic".into(),
                threads: 0,
                model: None,
                features: vec![],
            },
            ..Default::default()
        };
        let directory = temp.path().join("Target Quality");
        // The existing FFmpeg concat entry point requires its caller's scratch directory.
        for pass in 1..=3 {
            std::fs::create_dir_all(directory.join(pass.to_string()).join("Scene Concatenator"))
                .expect("concat scratch");
        }
        let concat = ConcatMethod::FFmpeg(Default::default());
        let (tx, rx) = sync::mpsc::channel();
        let cancellation = Default::default();
        let mut task = Task {
            original_index: 0,
            frame_indices: vec![0, 1, 2, 3],
            encoder: encoder.clone(),
            output: Default::default(),
            passes: vec![],
        };
        for (pass, q) in [(1, 26.0), (2, 30.0)] {
            task.encoder.set_quantizer(q);
            task.output = directory.join(pass.to_string()).join("00000.mkv");
            let (completed, warnings) = TargetQuality::probe_pass_with_options(
                pass,
                &directory,
                &config,
                &mut input,
                None,
                1,
                &concat,
                &[task],
                tx.clone(),
                &cancellation,
                Default::default(),
            )
            .expect("real measured resume history");
            assert!(warnings.is_empty(), "{warnings:?}");
            task = completed.into_iter().next().expect("accepted task");
        }
        let history =
            TargetQuality::quantizer_score_history(&config, &task.passes).expect("history");
        assert!(
            history[0].1 > history[1].1,
            "fixture must distinguish quantizers: {history:?}"
        );
        // A target 5% of the way from q26 to q30 interpolates to q26.2,
        // rounding to the already measured integer q26.
        let target = history[1].1.mul_add(0.05, history[0].1 * 0.95);
        *config.metric.target_range_mut() = (target, target);
        assert_eq!(
            TargetQuality::predict_quantizer(
                (26.0, 30.0),
                target,
                config.interpolators,
                &history,
                1.0
            )
            .expect("prediction"),
            26.0
        );
        let scene = Scene {
            start_frame: 0,
            end_frame: 4,
            sub_scenes: None,
            encoder: encoder.clone(),
            sequence_data: AcceptedData {
                target_quality: TargetQualityData {
                    selection: None,
                    speed_transition: None,
                    passes: task.passes,
                },
                ..Default::default()
            },
        };
        let mut sequence_config = DefaultSequenceConfig {
            target_quality: Some(config.clone()),
            parallel_encoder: ParallelEncoderConfig::new(temp.path()),
            ..Default::default()
        };
        sequence_config.scene_concatenator.method = concat;
        let saved = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let snapshots = std::sync::Arc::clone(&saved);
        let mut condor = Condor::new(
            input,
            Output {
                path: temp.path().join("final.mkv"),
                overwrite: false,
                tags: Default::default(),
                video_tags: Default::default(),
            },
            encoder,
            vec![scene],
            Some(sequence_config),
            Box::new(move |data| {
                snapshots.lock().expect("snapshots").push(data.scenes);
                Ok(())
            }),
        );
        crate::core::sequence::c14_tests::current_target_fixture(&mut condor);
        let mut sequence = TargetQuality::default();
        let (_, warnings) = sequence
            .execute_cancel(&mut condor, tx, cancellation)
            .expect("actual orchestration");
        assert!(warnings.is_empty(), "{warnings:?}");
        let scene = &condor.scenes[0];
        let measured = &scene.sequence_data.target_quality.passes;
        assert_eq!(
            measured.len(),
            3,
            "duplicate prediction must not end the search"
        );
        assert_eq!(measured[2].quantizer, 28.0);
        assert_eq!(measured[2].scores.len(), 4);
        let final_q = scene.encoder.quantizer().expect("final quantizer");
        assert!(measured.iter().any(|pass| pass.quantizer == final_q));
        let accepted = TargetQuality::quantizer_score_history(&config, measured).expect("history");
        eprintln!("Q04 accepted={accepted:?}, target={target}, final={final_q}");
        assert_eq!(
            TargetQuality::verified_quantizer(&config.metric, &accepted),
            None,
            "target intentionally missed at maximum probes"
        );
        let closest = accepted
            .iter()
            .min_by(|(qa, sa), (qb, sb)| {
                (sa - target)
                    .abs()
                    .total_cmp(&(sb - target).abs())
                    .then_with(|| qb.total_cmp(qa))
            })
            .expect("measured closest")
            .0;
        assert_eq!(final_q, closest);
        assert_eq!(
            saved.lock().expect("saved").last().expect("persisted")[0].encoder.quantizer(),
            Some(final_q)
        );
        assert!(rx.try_iter().any(|status| matches!(
            status,
            SequenceStatus::Subprocess {
                child: Status::Processing {
                    completion: SequenceCompletion::SceneQuality {
                        quantizer: 28.0,
                        ..
                    },
                    ..
                },
                ..
            }
        )));

        // Also stop at the maximum immediately after an initial measurement:
        // an unmeasured next candidate must never become final.
        let one_probe = temp.path().join("one-probe");
        std::fs::create_dir_all(one_probe.join("Target Quality/1/Scene Concatenator"))
            .expect("scratch");
        condor.sequence_config.parallel_encoder.scenes_directory = one_probe;
        condor.sequence_config.target_quality.as_mut().expect("TQ").maximum_probes = 1;
        condor.scenes[0].sequence_data.target_quality.passes.clear();
        let (tx, _rx) = sync::mpsc::channel();
        sequence
            .execute_cancel(&mut condor, tx, Default::default())
            .expect("one real probe");
        let scene = &condor.scenes[0];
        assert_eq!(scene.sequence_data.target_quality.passes.len(), 1);
        assert_eq!(scene.sequence_data.target_quality.passes[0].quantizer, 28.0);
        assert_eq!(
            scene.encoder.quantizer(),
            Some(28.0),
            "never finalize the next unmeasured prediction after max probes"
        );

        let forced_probe = temp.path().join("forced-after-measurement");
        std::fs::create_dir_all(forced_probe.join("Target Quality/1/Scene Concatenator"))
            .expect("forced scratch");
        condor.sequence_config.parallel_encoder.scenes_directory = forced_probe;
        condor.scenes[0].sequence_data.target_quality.passes.clear();
        condor.scenes[0].encoder.set_quantizer(20.0);
        let before_saves = saved.lock().expect("saved").len();
        let cancellation = Cancellation::default();
        let force = cancellation.clone();
        let (accepted_tx, accepted_rx) = crossbeam_channel::bounded(0);
        let (resume_tx, resume_rx) = crossbeam_channel::bounded(0);
        condor.scenes[0].sequence_data.handoff = Some((accepted_tx, resume_rx));
        let (tx, rx) = sync::mpsc::channel();
        let progress = std::thread::spawn(move || {
            // Buffered progress alone cannot locate the cancellation window.
            // Rendezvous at accepted-history attachment after real Compare completes.
            while let Ok(status) = rx.recv_timeout(std::time::Duration::from_secs(10)) {
                if matches!(status, SequenceStatus::Subprocess { child: Status::Completed { id }, .. } if id == "Compare")
                {
                    accepted_rx
                        .recv_timeout(std::time::Duration::from_secs(10))
                        .expect("accepted measurement");
                    force.request().expect("graceful after measurement");
                    force.request().expect("forced after measurement");
                    resume_tx
                        .send_timeout((), std::time::Duration::from_secs(10))
                        .expect("resume accepted pass");
                    // Keep the real UI receiver alive until execute drops every sender.
                    for _ in rx {}
                    return;
                }
            }
            panic!("real comparison never completed");
        });
        let error = sequence
            .execute_cancel(&mut condor, tx, cancellation)
            .expect_err("force still propagates");
        progress.join().expect("progress handoff");
        assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
        let snapshots = saved.lock().expect("saved");
        assert_eq!(
            snapshots.len(),
            before_saves + 1,
            "accepted measurement must be saved before reporting Forced"
        );
        let persisted = &snapshots.last().expect("forced snapshot")[0];
        assert_eq!(persisted.sequence_data.target_quality.passes.len(), 1);
        assert_eq!(
            persisted.sequence_data.target_quality.passes[0].scores.len(),
            4
        );
        assert_eq!(
            persisted.sequence_data.target_quality.passes[0].quantizer,
            28.0
        );
        assert_eq!(
            persisted.encoder.quantizer(),
            Some(28.0),
            "save the measured final quantizer, not the old configured q"
        );
    }

    #[test]
    fn q03_inverse_bounds_use_nearest_probes() {
        let history = [(20.0, 0.5), (28.0, 0.9), (34.0, 1.2), (40.0, 1.6)];
        assert_eq!(
            TargetQuality::quantizer_bounds((5, 55), 1.0, &history, true),
            (28.0, 34.0)
        );
    }

    #[test]
    fn q03_bounds_preserve_direction_defaults_strictness_and_stable_ties() {
        let cases = [
            (
                "inverse unsorted",
                true,
                &[(34.0, 1.2), (20.0, 0.5), (40.0, 1.6), (28.0, 0.9)][..],
                (28.0, 34.0),
            ),
            (
                "higher mirror unsorted",
                false,
                &[(34.0, 0.8), (20.0, 1.5), (40.0, 0.4), (28.0, 1.1)],
                (28.0, 34.0),
            ),
            ("inverse empty", true, &[], (7.0, 51.0)),
            ("higher empty", false, &[], (7.0, 51.0)),
            (
                "inverse below singleton",
                true,
                &[(28.0, 0.9)],
                (28.0, 51.0),
            ),
            ("inverse above singleton", true, &[(34.0, 1.2)], (7.0, 34.0)),
            ("higher below singleton", false, &[(34.0, 0.8)], (7.0, 34.0)),
            (
                "higher above singleton",
                false,
                &[(28.0, 1.1)],
                (28.0, 51.0),
            ),
            ("inverse exact singleton", true, &[(30.0, 1.0)], (7.0, 51.0)),
            ("higher exact singleton", false, &[(30.0, 1.0)], (7.0, 51.0)),
            (
                "inverse below only",
                true,
                &[(20.0, 0.5), (28.0, 0.9)],
                (28.0, 51.0),
            ),
            (
                "inverse above only",
                true,
                &[(34.0, 1.2), (40.0, 1.6)],
                (7.0, 34.0),
            ),
            (
                "higher above only",
                false,
                &[(20.0, 1.5), (28.0, 1.1)],
                (28.0, 51.0),
            ),
            (
                "higher below only",
                false,
                &[(34.0, 0.8), (40.0, 0.4)],
                (7.0, 34.0),
            ),
            (
                "inverse stable ties and exact target",
                true,
                &[
                    (20.0, 0.5),
                    (28.0, 0.9),
                    (27.0, 0.9),
                    (30.0, 1.0),
                    (35.0, 1.2),
                    (34.0, 1.2),
                    (40.0, 1.6),
                ],
                (27.0, 35.0),
            ),
            (
                "higher stable ties and exact target",
                false,
                &[
                    (20.0, 1.5),
                    (28.0, 1.1),
                    (27.0, 1.1),
                    (30.0, 1.0),
                    (35.0, 0.8),
                    (34.0, 0.8),
                    (40.0, 0.4),
                ],
                (28.0, 34.0),
            ),
        ];
        for (name, inverse, history, expected) in cases {
            assert_eq!(
                TargetQuality::quantizer_bounds((7, 51), 1.0, history, inverse),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn q03_noisy_inverse_bounds_predict_within_configured_range() {
        use crate::models::sequence::target_quality::types::InterpolationMethod;

        let histories = [
            [(20.0, 0.5), (40.0, 1.6), (30.0, 1.02), (31.0, 0.98)],
            [(31.0, 0.98), (30.0, 1.02), (40.0, 1.6), (20.0, 0.5)],
            [(20.0, 0.5), (40.0, 1.6), (30.0, 1.0002), (30.25, 0.9998)],
        ];
        for history in histories {
            let bounds = TargetQuality::quantizer_bounds((7, 51), 1.0, &history, true);
            let predicted = TargetQuality::predict_quantizer(
                bounds,
                1.0,
                (InterpolationMethod::Linear, InterpolationMethod::Linear),
                &history,
                0.25,
            )
            .expect("noisy inverse history must remain predictable");
            assert_eq!(bounds, (7.0, 51.0));
            assert!(
                predicted.is_finite() && (7.0..=51.0).contains(&predicted),
                "prediction {predicted} for {history:?}"
            );
        }
    }

    #[test]
    fn q03_noisy_higher_bounds_use_q04_safe_configured_fallback() {
        // Q04 deliberately extends the Q03 inverse safety guard to either direction.
        let history = [(20.0, 1.6), (40.0, 0.5), (31.0, 1.02), (30.0, 0.98)];
        assert_eq!(
            TargetQuality::quantizer_bounds((7, 51), 1.0, &history, false),
            (7.0, 51.0)
        );
    }

    #[test]
    fn m27_target_history_gateway_and_verified_quantizer_use_scene_finals() {
        use crate::models::sequence::target_quality::{
            TargetQualityConfig,
            types::{ProbeStatistic, QualityPass},
        };
        let passes = [
            QualityPass {
                quantizer: 20.0,
                scores: vec![2.0, 9.0, 4.0],
                ..Default::default()
            },
            QualityPass {
                quantizer: 30.0,
                scores: vec![1.0, 8.0, 3.0],
                ..Default::default()
            },
        ];
        for disabled in [None, Some(false), Some(true)] {
            let config = TargetQualityConfig {
                metric: QualityMetric::CVVDP {
                    target_range: (3.9, 4.1),
                    resolution: None,
                    display_model: None,
                    resize_to_display: None,
                    disable_temporal: disabled,
                    gpu_id: None,
                },
                ..Default::default()
            };
            let history =
                TargetQuality::quantizer_score_history(&config, &passes).expect("history");
            match TargetQuality::quality_completion(&config, 7, &passes[0]).expect("completion") {
                SequenceCompletion::SceneQuality {
                    index,
                    quantizer,
                    score,
                    bitrate,
                } => {
                    assert_eq!(
                        (index, quantizer, score, bitrate),
                        (7, 20.0, if disabled == Some(true) { 5.0 } else { 4.0 }, 0.0)
                    );
                },
                other => panic!("unexpected completion {other:?}"),
            }
            assert_eq!(
                history,
                if disabled == Some(true) {
                    vec![(20.0, 5.0), (30.0, 4.0)]
                } else {
                    vec![(20.0, 4.0), (30.0, 3.0)]
                }
            );
            assert_eq!(
                TargetQuality::verified_quantizer(&config.metric, &history),
                Some(if disabled == Some(true) { 30.0 } else { 20.0 })
            );
            assert_eq!(config.probing.statistic, ProbeStatistic::Mean);
        }
    }

    #[test]
    fn m18_compare_submission_progress_declares_phase_once() {
        let (tx, rx) = sync::mpsc::channel();
        for completed in 1..=3 {
            super::report_compare_progress(&tx, 3, completed);
        }
        for expected in [0, 1, 2, 3] {
            assert!(
                matches!(rx.try_recv().expect("compare status"), SequenceStatus::Whole(Status::Processing {
                    id, completion: SequenceCompletion::Frames { completed, total }
                }) if id == "Compare" && completed == expected && total == 3),
                "compare declares zero once, then reports each accepted frame"
            );
        }
        assert!(
            rx.try_recv().is_err(),
            "final scores do not count submission twice"
        );
    }

    /// A `Task` with `passes` recorded passes; only the count is consulted.
    fn task_with_passes(index: usize, passes: usize) -> Task {
        Task {
            original_index: index,
            frame_indices: vec![0, 1],
            encoder: Default::default(),
            output: Default::default(),
            passes: vec![Default::default(); passes],
        }
    }

    fn ssimulacra2() -> QualityMetric {
        QualityMetric::SSIMULACRA2 {
            target_range: (74.0, 76.0),
            resolution: None,
            threads: None,
            gpu_id: None,
        }
    }

    mod q07 {
        use super::super::*;
        use crate::{
            core::{Condor, output::Output, sequence::Sequence},
            models::{
                scene::Scene,
                sequence::{
                    DefaultSequenceConfig, DefaultSequenceData,
                    parallel_encoder::ParallelEncoderConfig,
                },
            },
            utils::cancellation::{Cancellation, Cancelled},
        };

        // Real FFmpeg/FFMS2/libvmaf, shared with the Q04 acceptance fixture.
        fn fixture() -> (
            tempfile::TempDir,
            Input,
            Encoder,
            TargetQualityConfig,
            PathBuf,
        ) {
            let temp = tempfile::tempdir().expect("fixture");
            let source = temp.path().join("source.mkv");
            let generated = std::process::Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=128x128:rate=24",
                    "-frames:v",
                    "4",
                    "-c:v",
                    "ffv1",
                ])
                .arg(&source)
                .output()
                .expect("existing FFmpeg");
            assert!(generated.status.success(), "{generated:?}");
            let input = Input::from_video(&InputModel::Video {
                path: source,
                import_method: ImportMethod::FFMS2 {
                    index: None,
                    cache_path: None,
                },
                filters: vec![],
            })
            .expect("FFMS2");
            let mut encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, false);
            encoder.parameters_mut().extend(CLIParameter::new_strings(
                "-",
                " ",
                &[("c:v", "libx264"), ("preset", "ultrafast")],
            ));
            encoder.set_quantizer(26.0);
            let config = TargetQualityConfig {
                quantizer_range: (26, 34),
                maximum_probes: 2,
                metric: QualityMetric::VMAF {
                    target_range: (0.0, 0.0),
                    resolution: None,
                    scaler: "bicubic".into(),
                    threads: 0,
                    model: None,
                    features: vec![],
                },
                ..Default::default()
            };
            let directory = temp.path().join("Target Quality");
            for pass in 1..=2 {
                std::fs::create_dir_all(
                    directory.join(pass.to_string()).join("Scene Concatenator"),
                )
                .expect("scratch");
            }
            (temp, input, encoder, config, directory)
        }

        fn tasks(encoder: &Encoder, directory: &Path, pass: u8) -> Vec<Task> {
            [vec![0, 1], vec![2, 3]]
                .into_iter()
                .enumerate()
                .map(|(index, frame_indices)| Task {
                    original_index: index,
                    frame_indices,
                    encoder: encoder.clone(),
                    output: directory.join(pass.to_string()).join(format!("{index:05}.mkv")),
                    passes: vec![],
                })
                .collect()
        }

        fn probe(
            pass: u8,
            directory: &Path,
            config: &TargetQualityConfig,
            input: &mut Input,
            tasks: &[Task],
        ) -> Result<Vec<Task>> {
            let (tx, _rx) = sync::mpsc::channel();
            let (completed, warnings) = TargetQuality::probe_pass_with_options(
                pass,
                directory,
                config,
                input,
                None,
                1,
                &ConcatMethod::FFmpeg(Default::default()),
                tasks,
                tx,
                &Default::default(),
                Default::default(),
            )?;
            assert!(warnings.is_empty(), "{warnings:?}");
            Ok(completed)
        }

        type SavedScenes = std::rc::Rc<std::cell::RefCell<Vec<Vec<Scene<DefaultSequenceData>>>>>;

        #[test]
        fn c14_r1_publication_rechecks_model_source_and_backend_after_work() {
            for edit in ["model", "source", "backend", "matching"] {
                let (temp, mut condor, saved) = q13_fixture(&[0, 0], 1);
                let model = temp.path().join("custom.json");
                std::fs::write(&model, b"AAA").expect("model identity fixture");
                if let QualityMetric::VMAF {
                    model: configured, ..
                } = &mut condor.sequence_config.target_quality.as_mut().expect("TQ").metric
                {
                    *configured = Some(model.clone());
                }
                let source = condor.input.as_data().path().expect("declared source").to_path_buf();
                let before = serde_json::to_vec(&condor.scenes).expect("before");
                let (tx, _rx) = sync::mpsc::channel();
                provenance::take_identity_counts();
                let result = TargetQuality::default().execute_with_probe(
                    &mut condor,
                    &tx,
                    &Default::default(),
                    |_, _, _, _, _, _, _, tasks, _, _, _| {
                        if matches!(edit, "model" | "source") {
                            let path = if edit == "model" { &model } else { &source };
                            let modified = path.metadata()?.modified()?;
                            std::fs::write(
                                path,
                                if edit == "model" {
                                    b"BBB".as_slice()
                                } else {
                                    b"ordinary source replacement".as_slice()
                                },
                            )?;
                            if edit == "model" {
                                std::fs::File::open(path)?
                                    .set_times(std::fs::FileTimes::new().set_modified(modified))?;
                            }
                        }
                        let mut backend = MetricBackend::current(Engine::Vmaf);
                        if edit == "backend" {
                            backend.version = Some("changed during work".into());
                        }
                        let mut completed = tasks.to_vec();
                        for task in &mut completed {
                            task.passes.push(QualityPass {
                                quantizer: task.encoder.quantizer().expect("q"),
                                scores: vec![75.0; 2],
                                started_on: 1,
                                completed_on: 2,
                                backend: Some(backend.clone()),
                                ..Default::default()
                            });
                        }
                        Ok((completed, vec![]))
                    },
                );
                assert_eq!(
                    provenance::take_identity_counts(),
                    (2, 0),
                    "one model capture before work and one at publication, shared by both scenes"
                );
                if edit == "matching" {
                    result.expect("matching batch publication");
                    assert_eq!(saved.borrow().len(), 1);
                    assert!(condor.scenes.iter().all(|scene| {
                        scene.sequence_data.target_quality.passes[0].provenance.is_some()
                    }));
                } else {
                    let error =
                        result.expect_err("changed inputs cannot receive a cached certificate");
                    let expected = if edit == "backend" {
                        "backend version/plugin admission identity changed"
                    } else {
                        "inputs changed during measurement"
                    };
                    assert!(format!("{error:#}").contains(expected), "{error:#}");
                    assert!(saved.borrow().is_empty());
                    assert_eq!(
                        serde_json::to_vec(&condor.scenes).expect("no attached pass/selection"),
                        before
                    );
                }
            }
        }

        #[test]
        fn c14_fallback_router_stamps_actual_branch_at_owning_attachment() {
            let (_temp, mut condor, saved) = q13_fixture(&[0], 1);
            let config = condor.sequence_config.target_quality.as_mut().expect("TQ");
            config.metric = QualityMetric::default();
            *config.metric.target_range_mut() = (70.0, 80.0);
            let (tx, _rx) = sync::mpsc::channel();
            let mut calls = 0;
            TargetQuality::default()
                .execute_with_probe(
                    &mut condor,
                    &tx,
                    &Default::default(),
                    |_, _, config, _, _, _, _, tasks, progress, _, _| {
                        calls += 1;
                        let (compare, relay) = spawn_compare_progress(progress);
                        let (scores, actual) = TargetQuality::route_compare_scores(
                            Some(Err(anyhow::anyhow!(
                                "bounded native ABI failure before scores"
                            ))),
                            Some(Engine::Fmetrics),
                            &config.metric,
                            compare,
                            relay,
                            |sender| {
                                drop(sender);
                                Ok(vec![75.0; 2])
                            },
                            |scores| TargetQuality::validate_task_scores(config, tasks, scores),
                        )?
                        .context("successful fallback")?;
                        let mut completed = tasks.to_vec();
                        let quantizer = completed[0].encoder.quantizer().expect("q");
                        assert_eq!(actual, Engine::VapourSynth);
                        completed[0].passes.push(QualityPass {
                            quantizer,
                            scores,
                            started_on: 1,
                            completed_on: 2,
                            backend: Some(MetricBackend::available_plugin(&config.metric)?),
                            ..Default::default()
                        });
                        Ok((completed, vec![]))
                    },
                )
                .expect("owning fallback attachment");
            assert_eq!(calls, 1);
            assert_eq!(saved.borrow().len(), 1);
            let pass = &condor.scenes[0].sequence_data.target_quality.passes[0];
            assert_eq!(
                pass.backend.as_ref().expect("actual").engine,
                Engine::VapourSynth
            );
            assert_eq!(
                pass.provenance.as_ref().expect("stamp").context["backend"]["actual"]["engine"],
                "VapourSynth"
            );
            let (tx, _rx) = sync::mpsc::channel();
            TargetQuality::default()
                .execute_with_probe(
                    &mut condor,
                    &tx,
                    &Default::default(),
                    |_, _, _, _, _, _, _, _, _, _, _| {
                        panic!("matching actual fallback must resume without another measurement")
                    },
                )
                .expect("fallback matching resume");
        }

        fn q13_fixture(
            counts: &[usize],
            maximum_probes: u8,
        ) -> (
            tempfile::TempDir,
            Condor<DefaultSequenceData, DefaultSequenceConfig>,
            SavedScenes,
        ) {
            let (temp, input, encoder, mut config, directory) = fixture();
            config.maximum_probes = maximum_probes;
            for pass in 1..=maximum_probes {
                std::fs::create_dir_all(
                    directory.join(pass.to_string()).join("Scene Concatenator"),
                )
                .expect("concat scratch");
            }
            // Synthetic, admitted saved records; all appended records use real FFmpeg/FFMS2/VMAF.
            let scenes = counts
                .iter()
                .enumerate()
                .map(|(index, &count)| {
                    let mut data = DefaultSequenceData::default();
                    data.target_quality.passes = (0..count)
                        .map(|pass| QualityPass {
                            provenance: None,
                            backend: Some(MetricBackend::current(Engine::Vmaf)),
                            quantizer: [26.0, 30.0, 34.0][pass],
                            scores: vec![10.0 + pass as f64; 2],
                            bitrate: 100.0 + pass as f64,
                            started_on: 12 + pass as u128,
                            completed_on: 34 + pass as u128,
                        })
                        .collect();
                    Scene {
                        start_frame: index,
                        end_frame: index + 2,
                        sub_scenes: None,
                        encoder: encoder.clone(),
                        sequence_data: data,
                    }
                })
                .collect();
            let saved = SavedScenes::default();
            let snapshots = std::rc::Rc::clone(&saved);
            let mut condor = Condor::new(
                input,
                Output {
                    path: temp.path().join("final.mkv"),
                    overwrite: false,
                    tags: Default::default(),
                    video_tags: Default::default(),
                },
                encoder,
                scenes,
                Some(DefaultSequenceConfig {
                    target_quality: Some(config),
                    parallel_encoder: ParallelEncoderConfig::new(temp.path()),
                    ..Default::default()
                }),
                Box::new(move |data| {
                    snapshots.borrow_mut().push(data.scenes);
                    Ok(())
                }),
            );
            condor.sequence_config.scene_concatenator.method =
                ConcatMethod::FFmpeg(Default::default());
            crate::core::sequence::c14_tests::current_target_fixture(&mut condor);
            (temp, condor, saved)
        }

        fn q13_execute(
            condor: &mut Condor<DefaultSequenceData, DefaultSequenceConfig>,
        ) -> Vec<(u8, u64, f64, f64)> {
            let (tx, rx) = sync::mpsc::channel();
            let (_, warnings) = TargetQuality::default()
                .execute_cancel(condor, tx, Default::default())
                .expect("actual sequence execution");
            assert!(warnings.is_empty(), "{warnings:?}");
            rx.into_iter()
                .filter_map(|event| match event {
                    SequenceStatus::Subprocess {
                        parent:
                            Status::Processing {
                                completion: SequenceCompletion::Passes { completed, .. },
                                ..
                            },
                        child:
                            Status::Processing {
                                completion:
                                    SequenceCompletion::SceneQuality {
                                        index,
                                        quantizer,
                                        score,
                                        ..
                                    },
                                ..
                            },
                    } => Some((completed, index, quantizer, score)),
                    _ => None,
                })
                .collect()
        }

        #[test]
        fn q14_maximum_probe_execution_is_bounded() {
            if let Ok(case) = std::env::var("Q14_CASE") {
                let (maximum, expected, expected_calls) = match case.as_str() {
                    "max-1" => (1, 1, 1),
                    "max-2" => (2, 2, 2),
                    "max-255" => (255, 255, 255),
                    "early-hit" => (255, 1, 1),
                    "failure" | "cancel" => (255, 1, 2),
                    "forced" => (255, 2, 2),
                    _ => panic!("unknown Q14 case: {case}"),
                };
                let (temp, mut condor, saved) = q13_fixture(&[0], 1);
                let config = condor.sequence_config.target_quality.as_mut().expect("TQ");
                config.maximum_probes = maximum;
                // Synthetic model-only FFmpeg grid, not a native codec range claim.
                config.quantizer_range = (0, u32::MAX);
                *config.metric.target_range_mut() = (1.0, 1.0);
                let (tx, rx) = sync::mpsc::channel();
                let mut ordinals = Vec::new();
                let result = TargetQuality::default().execute_with_probe(
                    &mut condor,
                    &tx,
                    &Default::default(),
                    |pass, _, _, _, _, _, _, tasks, progress, cancellation, _| {
                        ordinals.push(pass);
                        assert_ne!(pass, 0, "never call a wrapped pass");
                        assert_eq!(tasks.len(), 1);
                        assert_eq!(tasks[0].passes.len(), usize::from(pass - 1));
                        eprintln!(
                            "Q14 {case}: callback={pass}, prefix={}, saves={}",
                            tasks[0].passes.len(),
                            saved.borrow().len()
                        );
                        if pass == 2 && case == "failure" {
                            bail!("Q14 injected probe failure");
                        }
                        if pass == 2 && case == "cancel" {
                            cancellation.request()?;
                            return Ok((Vec::new(), Vec::new()));
                        }
                        let mut completed = tasks.to_vec();
                        let task = &mut completed[0];
                        let quantizer = task.encoder.quantizer().expect("scheduled q");
                        assert!(!task.passes.iter().any(|p| p.quantizer == quantizer));
                        task.passes.push(QualityPass {
                            provenance: None,
                            backend: Some(MetricBackend::current(Engine::Vmaf)),
                            quantizer,
                            scores: vec![
                                if case == "early-hit" {
                                    1.0
                                } else if pass == 1 {
                                    0.0
                                } else {
                                    -f64::from(pass)
                                };
                                task.frame_indices.len()
                            ],
                            bitrate: 100.0 + f64::from(pass),
                            started_on: u128::from(pass) * 2,
                            completed_on: u128::from(pass) * 2 + 1,
                        });
                        for id in ["Encode", "Compare"] {
                            progress.send(SequenceStatus::Whole(Status::Completed {
                                id: id.to_owned(),
                            }))?;
                        }
                        if pass == 2 && case == "forced" {
                            cancellation.request()?;
                            cancellation.request()?;
                        }
                        // Dropping the real sender allows the owning progress relay to join.
                        Ok((completed, Vec::new()))
                    },
                );
                match case.as_str() {
                    "failure" => assert!(
                        result.expect_err("probe failure").to_string().contains("Q14 injected")
                    ),
                    "forced" => assert_eq!(
                        result
                            .expect_err("force after accepted record")
                            .downcast_ref::<Cancelled>(),
                        Some(&Cancelled::Forced)
                    ),
                    _ => assert!(result.expect("bounded owning execute").1.is_empty()),
                }
                assert_eq!(ordinals, (1..=expected_calls).collect::<Vec<u8>>());
                let passes = &condor.scenes[0].sequence_data.target_quality.passes;
                eprintln!(
                    "Q14 {case}: returned callbacks={}, history={}, saves={}",
                    ordinals.len(),
                    passes.len(),
                    saved.borrow().len()
                );
                assert_eq!(passes.len(), usize::from(expected));
                assert_eq!(saved.borrow().len(), usize::from(expected));
                for (ordinal, snapshot) in (1..=expected).zip(saved.borrow().iter()) {
                    assert_eq!(
                        snapshot[0].sequence_data.target_quality.passes.len(),
                        usize::from(ordinal)
                    );
                }
                assert_eq!(
                    serde_json::to_vec(saved.borrow().last().expect("last save")).expect("saved"),
                    serde_json::to_vec(&condor.scenes).expect("attached")
                );
                let final_q = condor.scenes[0].encoder.quantizer().expect("measured final q");
                assert_eq!(
                    final_q, passes[0].quantizer,
                    "initial score is closest to target"
                );
                assert!(passes.iter().any(|p| p.quantizer == final_q));
                let (rounds, quality) = q14_progress(rx.try_iter().collect(), maximum);
                assert_eq!(rounds, ordinals);
                assert_eq!(quality, (1..=expected).collect::<Vec<u8>>());
                assert!(!temp.path().join("Target Quality/0").exists());
                assert!(!temp.path().join("Target Quality/256").exists());
                return;
            }
            for case in ["max-1", "max-2", "max-255", "early-hit", "failure", "cancel", "forced"] {
                q14_watchdog(case, "q14_maximum_probe_execution_is_bounded");
            }
        }

        // Removing the normal wrapper's probe call or the last save must fail this test.
        #[test]
        fn q14_public_execute_saves_255th_native_record() {
            if std::env::var("Q14_CASE").as_deref() != Ok("native-255") {
                q14_watchdog("native-255", "q14_public_execute_saves_255th_native_record");
                return;
            }
            let (temp, mut condor, saved) = q13_fixture(&[0], 1);
            condor.sequence_config.target_quality.as_mut().expect("TQ").maximum_probes = 255;
            // Only the prefix is synthetic. The wrapper must encode/score the one new record.
            condor.scenes[0].sequence_data.target_quality.passes = vec![
                QualityPass {
                    provenance: None,
                    backend: Some(MetricBackend::current(Engine::Vmaf)),
                    quantizer: 30.0,
                    scores: vec![10.0; 2],
                    bitrate: 100.0,
                    started_on: 12,
                    completed_on: 34,
                };
                254
            ];
            condor.scenes[0].encoder.set_quantizer(30.0);
            crate::core::sequence::c14_tests::current_target_fixture(&mut condor);
            let before = serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes)
                .expect("synthetic prefix bytes");
            // Retain the existing concat-directory fixture workaround; do not fix it here.
            std::fs::create_dir_all(temp.path().join("Target Quality/255/Scene Concatenator"))
                .expect("last-pass concat scratch");
            let (tx, rx) = sync::mpsc::channel();
            eprintln!(
                "Q14 native-255: entering public Sequence::execute with 254 synthetic records"
            );
            let (_, warnings) = TargetQuality::default()
                .execute(&mut condor, tx, Arc::new(AtomicBool::new(false)))
                .expect("one real final pass through normal wrapper");
            assert!(warnings.is_empty(), "{warnings:?}");
            let passes = &condor.scenes[0].sequence_data.target_quality.passes;
            eprintln!(
                "Q14 native-255: returned history={}, saves={}",
                passes.len(),
                saved.borrow().len()
            );
            assert_eq!(passes.len(), 255);
            assert_eq!(serde_json::to_vec(&passes[..254]).expect("prefix"), before);
            let last = &passes[254];
            assert_eq!(last.quantizer, 32.0);
            assert_eq!(last.scores.len(), 2);
            assert!(last.scores.iter().all(|s| s.is_finite() && *s > 0.0));
            assert!(last.started_on > 34 && last.completed_on >= last.started_on);
            let bytes = std::fs::metadata(temp.path().join("Target Quality/255/00000.mkv"))
                .expect("one native encoded output")
                .len();
            let expected_bitrate = bytes as f64 * 96.0;
            assert!((last.bitrate - expected_bitrate).abs() < 1e-8);
            assert_eq!(
                saved.borrow().len(),
                1,
                "save actual 255th record before returning"
            );
            assert_eq!(
                serde_json::to_vec(&saved.borrow()[0]).expect("last save"),
                serde_json::to_vec(&condor.scenes).expect("attached last record")
            );
            let final_q = condor.scenes[0].encoder.quantizer().expect("measured q");
            assert!(passes.iter().any(|p| p.quantizer == final_q));
            let (rounds, quality) = q14_progress(rx.into_iter().collect(), 255);
            assert_eq!(rounds, [255]);
            assert_eq!(quality, [255]);
            assert!(!temp.path().join("Target Quality/0").exists());
            assert!(!temp.path().join("Target Quality/256").exists());
            eprintln!(
                "Q14 native-255: prefix byte-identical; one real q32 record retained/saved; progress255 only; final measured q={final_q}"
            );
        }

        fn q14_progress(events: Vec<SequenceStatus>, maximum: u8) -> (Vec<u8>, Vec<u8>) {
            let mut rounds = Vec::new();
            let mut quality = Vec::new();
            for event in events {
                match event {
                    SequenceStatus::Whole(Status::Processing {
                        completion: SequenceCompletion::Passes { completed, total },
                        ..
                    }) => {
                        assert!(total > 0 && total <= maximum);
                        assert_eq!(
                            completed,
                            total - 1,
                            "zero completed is valid only before pass1"
                        );
                        rounds.push(total);
                    },
                    SequenceStatus::Subprocess {
                        parent:
                            Status::Processing {
                                completion: SequenceCompletion::Passes { completed, total },
                                ..
                            },
                        child,
                    } => {
                        assert!(completed > 0 && completed <= maximum);
                        assert_eq!(total, maximum);
                        if matches!(
                            child,
                            Status::Processing {
                                completion: SequenceCompletion::SceneQuality { .. },
                                ..
                            }
                        ) {
                            quality.push(completed);
                        }
                    },
                    other => panic!("unexpected owning progress: {other:?}"),
                }
            }
            (rounds, quality)
        }

        fn q14_watchdog(case: &str, test: &str) {
            use std::{
                process::{Command, Stdio},
                time::{Duration, Instant},
            };
            // File-backed output cannot fill a pipe and masquerade as an overflow spin.
            // The parent owns child scratch, including cleanup after a killed child.
            let temp = tempfile::tempdir().expect("Q14 watchdog root");
            let stdout_path = temp.path().join("stdout.log");
            let stderr_path = temp.path().join("stderr.log");
            let mut child = Command::new(std::env::current_exe().expect("test binary"))
                .args([
                    "--exact",
                    &format!("core::sequence::target_quality::tests::q07::{test}"),
                    "--nocapture",
                ])
                .env("Q14_CASE", case)
                .env("TMPDIR", temp.path())
                .stdout(Stdio::from(
                    std::fs::File::create(&stdout_path).expect("child stdout"),
                ))
                .stderr(Stdio::from(
                    std::fs::File::create(&stderr_path).expect("child stderr"),
                ))
                .spawn()
                .expect("bounded Q14 child");
            let start = Instant::now();
            let mut timed_out = false;
            while child.try_wait().expect("child status").is_none() {
                if start.elapsed() > Duration::from_secs(60) {
                    timed_out = true;
                    child.kill().expect("kill stuck Q14 child");
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            let status = child.wait().expect("reap Q14 child");
            let stdout = std::fs::read_to_string(stdout_path).expect("child stdout log");
            let stderr = std::fs::read_to_string(stderr_path).expect("child stderr log");
            eprintln!(
                "Q14 {case}: watchdog timeout={timed_out}, status={status}\n{stdout}\n{stderr}"
            );
            assert!(
                !timed_out && status.success(),
                "Q14 {case}: timeout={timed_out}, {status}"
            );
            assert!(
                stdout.contains("1 passed; 0 failed"),
                "child must execute its oracle"
            );
        }

        #[test]
        fn q13_h1_maximum_budget_no_work_is_bounded() {
            use std::{
                process::{Command, Stdio},
                time::{Duration, Instant},
            };
            if let Ok(case) = std::env::var("Q13_H1_CASE") {
                let (temp, mut condor, saved) = q13_fixture(&[1], 1);
                let config = condor.sequence_config.target_quality.as_mut().expect("TQ");
                config.maximum_probes = 255;
                match case.as_str() {
                    "early-hit" => *config.metric.target_range_mut() = (10.0, 10.0),
                    "budget-255" => {
                        // Synthetic valid records: this case needs no 255 native encodes.
                        let passes = &mut condor.scenes[0].sequence_data.target_quality.passes;
                        passes.resize(255, passes[0].clone());
                    },
                    _ => panic!("unknown H1 case: {case}"),
                }
                let mut unavailable = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
                unavailable.set_executable(Some(temp.path().join("unavailable-custom-svt")));
                condor.scenes[0].encoder = unavailable;
                crate::core::sequence::c14_tests::current_target_fixture(&mut condor);
                let before =
                    serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes)
                        .expect("prefix bytes");
                let directory = temp.path().join("Target Quality");
                let sentinel = directory.join("1/00000.ivf");
                std::fs::write(&sentinel, b"completed probe scratch").expect("sentinel");
                eprintln!(
                    "Q13 H1 {case}: entering actual execute, saved count={}, max255",
                    condor.scenes[0].sequence_data.target_quality.passes.len()
                );
                for (configured_q, expected_saves) in [(26.0, 0), (20.0, 1), (26.0, 1)] {
                    condor.scenes[0].encoder.set_quantizer(configured_q);
                    let (tx, rx) = sync::mpsc::channel();
                    let (_, warnings) = TargetQuality::default()
                        .execute_cancel(&mut condor, tx, Default::default())
                        .expect("bounded no-work owning execute");
                    assert!(warnings.is_empty(), "{warnings:?}");
                    assert!(rx.try_iter().next().is_none(), "no skipped-round progress");
                    assert_eq!(condor.scenes[0].encoder.quantizer(), Some(26.0));
                    assert_eq!(saved.borrow().len(), expected_saves);
                    assert_eq!(
                        serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes)
                            .expect("unchanged history"),
                        before
                    );
                    assert_eq!(
                        std::fs::read(&sentinel).expect("no excluded cleanup"),
                        b"completed probe scratch"
                    );
                    assert_eq!(
                        std::fs::read_dir(&directory).expect("scratch").count(),
                        2,
                        "no new ordinal directories"
                    );
                }
                assert_eq!(saved.borrow()[0][0].encoder.quantizer(), Some(26.0));
                assert_eq!(
                    serde_json::to_vec(&saved.borrow()[0][0].sequence_data.target_quality.passes)
                        .expect("saved history"),
                    before
                );
                eprintln!(
                    "Q13 H1 {case}: all three actual executions returned; identical history/scratch, no capability/native work/progress, only one changed-q restoration save"
                );
                return;
            }

            let mut failures = Vec::new();
            for case in ["early-hit", "budget-255"] {
                // Parent owns all child temporary files even if the watchdog must kill it.
                let temp = tempfile::tempdir().expect("watchdog root");
                let mut child = Command::new(std::env::current_exe().expect("test binary"))
                    .args(["--exact", "core::sequence::target_quality::tests::q07::q13_h1_maximum_budget_no_work_is_bounded", "--nocapture"])
                    .env("Q13_H1_CASE", case)
                    .env("TMPDIR", temp.path())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("bounded owning-execute child");
                let start = Instant::now();
                let mut timed_out = false;
                while child.try_wait().expect("child status").is_none() {
                    if start.elapsed() > Duration::from_secs(10) {
                        timed_out = true;
                        child.kill().expect("kill stuck no-work child");
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                let output = child.wait_with_output().expect("reap child and collect evidence");
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!(
                    "Q13 H1 {case}: watchdog timeout={timed_out}, status={}\n{stdout}\n{stderr}",
                    output.status
                );
                if timed_out || !output.status.success() || !stdout.contains("1 passed; 0 failed") {
                    failures.push(format!("{case}: timeout={timed_out}, {}", output.status));
                }
            }
            assert!(failures.is_empty(), "{}", failures.join("\n"));
        }

        #[test]
        fn q13_execute_mixed_counts_preserves_prefixes_and_sparse_progress() {
            let (temp, mut condor, saved) = q13_fixture(&[3, 1, 0], 2);
            let before = condor
                .scenes
                .iter()
                .map(|scene| {
                    serde_json::to_vec(&scene.sequence_data.target_quality.passes)
                        .expect("saved bytes")
                })
                .collect::<Vec<_>>();
            let directory = temp.path().join("Target Quality");
            let sentinels = ["1/00000.mkv", "2/00000.mkv", "1/00001.mkv", "1/history.json"];
            for path in sentinels {
                std::fs::write(directory.join(path), b"verified peer scratch/history")
                    .expect("sentinel");
            }
            let events = q13_execute(&mut condor);
            assert_eq!(
                condor
                    .scenes
                    .iter()
                    .map(|s| s.sequence_data.target_quality.passes.len())
                    .collect::<Vec<_>>(),
                [3, 2, 2]
            );
            assert_eq!(
                events.iter().map(|(pass, index, ..)| (*pass, *index)).collect::<Vec<_>>(),
                [(1, 2), (2, 1), (2, 2)]
            );
            for &(ordinal, index, quantizer, score) in &events {
                let record = &condor.scenes[index as usize].sequence_data.target_quality.passes
                    [usize::from(ordinal - 1)];
                assert_eq!(quantizer, record.quantizer);
                assert_eq!(score, record.scores[0].midpoint(record.scores[1]));
            }
            for (index, (&prefix, scene)) in [3, 1, 0].iter().zip(&condor.scenes).enumerate() {
                let passes = &scene.sequence_data.target_quality.passes;
                assert_eq!(
                    serde_json::to_vec(&passes[..prefix]).expect("prefix bytes"),
                    before[index]
                );
                assert_eq!((scene.start_frame, scene.end_frame), (index, index + 2));
                for (ordinal, pass) in passes.iter().enumerate().skip(prefix) {
                    assert_eq!(pass.scores.len(), 2);
                    assert!(pass.scores.iter().all(|score| score.is_finite() && *score > 0.0));
                    let bytes = std::fs::metadata(
                        directory.join(format!("{}/{index:05}.mkv", ordinal + 1)),
                    )
                    .expect("real output")
                    .len();
                    let expected_bitrate = bytes as f64 * 96.0;
                    assert!((pass.bitrate - expected_bitrate).abs() < 1e-8);
                    assert!(pass.started_on > 34 && pass.completed_on >= pass.started_on);
                }
                let closest = passes
                    .iter()
                    .min_by(|a, b| {
                        let mean = |p: &QualityPass| p.scores[0].midpoint(p.scores[1]);
                        mean(a)
                            .total_cmp(&mean(b))
                            .then_with(|| b.quantizer.total_cmp(&a.quantizer))
                    })
                    .expect("measured final");
                assert_eq!(scene.encoder.quantizer(), Some(closest.quantizer));
            }
            for path in sentinels {
                assert_eq!(
                    std::fs::read(directory.join(path)).expect("excluded scratch"),
                    b"verified peer scratch/history"
                );
            }
            let snapshots = saved.borrow();
            assert_eq!(snapshots.len(), 2, "one save per accepted nonempty round");
            assert_eq!(
                snapshots[0]
                    .iter()
                    .map(|s| s.sequence_data.target_quality.passes.len())
                    .collect::<Vec<_>>(),
                [3, 1, 1]
            );
            assert_eq!(
                serde_json::to_vec(snapshots.last().expect("last save")).expect("saved"),
                serde_json::to_vec(&condor.scenes).expect("actual")
            );
            eprintln!(
                "Q13 mixed [3,1,0] max2 -> [3,2,2], real progress={events:?}; prefixes/timestamps/sentinels unchanged, 2 actual saves"
            );
        }

        #[test]
        fn q13_execute_empty_earlier_rounds_reach_later_ordinal() {
            let (_temp, mut condor, saved) = q13_fixture(&[2, 2], 3);
            let before = condor
                .scenes
                .iter()
                .map(|s| {
                    serde_json::to_vec(&s.sequence_data.target_quality.passes).expect("prefix")
                })
                .collect::<Vec<_>>();
            let events = q13_execute(&mut condor);
            assert_eq!(
                events.iter().map(|(pass, index, ..)| (*pass, *index)).collect::<Vec<_>>(),
                [(3, 0), (3, 1)]
            );
            for (index, scene) in condor.scenes.iter().enumerate() {
                let passes = &scene.sequence_data.target_quality.passes;
                assert_eq!(passes.len(), 3);
                assert_eq!(
                    serde_json::to_vec(&passes[..2]).expect("unchanged prefix"),
                    before[index]
                );
                assert_eq!(passes[2].scores.len(), 2);
                assert!(passes[2].scores.iter().all(|s| s.is_finite() && *s > 0.0));
            }
            assert_eq!(saved.borrow().len(), 1, "skipped ordinals never save");
            eprintln!(
                "Q13 [2,2] max3 -> [3,3], only real pass3 progress={events:?}, 1 actual save"
            );
        }

        #[test]
        fn q13_execute_any_earlier_hit_skips_missing_encoder_then_changed_target_resumes() {
            let (temp, mut condor, saved) = q13_fixture(&[3, 3], 4);
            let config = condor.sequence_config.target_quality.as_mut().expect("TQ");
            config.quantizer_range = (26, 38);
            *config.metric.target_range_mut() = (20.0, 20.0);
            condor.scenes[0].sequence_data.target_quality.passes[1].scores = vec![20.0; 2];
            let mut unavailable = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
            unavailable.set_executable(Some(temp.path().join("unavailable-custom-svt")));
            unavailable.set_quantizer(30.0);
            condor.scenes[0].encoder = unavailable.clone();
            crate::core::sequence::c14_tests::current_target_fixture(&mut condor);
            let prefix = serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes)
                .expect("current fixture prefix");
            let sentinel = temp.path().join("Target Quality/4/00000.ivf");
            std::fs::write(&sentinel, b"completed peer").expect("sentinel");
            let first = q13_execute(&mut condor);
            assert_eq!(
                first.iter().map(|(pass, index, ..)| (*pass, *index)).collect::<Vec<_>>(),
                [(4, 1)]
            );
            assert_eq!(
                serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes)
                    .expect("unchanged"),
                prefix
            );
            assert_eq!(condor.scenes[0].encoder.quantizer(), Some(30.0));
            assert_eq!(
                std::fs::read(&sentinel).expect("no excluded cleanup"),
                b"completed peer"
            );
            assert_eq!(saved.borrow().len(), 1);
            let peer_output = temp.path().join("Target Quality/4/00001.mkv");
            let peer_bytes = std::fs::read(&peer_output).expect("real peer output");
            condor.scenes[0].encoder = condor.encoder.clone();
            unavailable.set_quantizer(condor.scenes[1].encoder.quantizer().expect("measured q"));
            condor.scenes[1].encoder = unavailable;
            *condor
                .sequence_config
                .target_quality
                .as_mut()
                .expect("TQ")
                .metric
                .target_range_mut() = (0.0, 0.0);
            crate::core::sequence::c14_tests::current_target_fixture(&mut condor);
            let prefix = serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes)
                .expect("current fixture prefix");
            let peer_history =
                serde_json::to_vec(&condor.scenes[1].sequence_data.target_quality.passes)
                    .expect("current fixture peer");
            let second = q13_execute(&mut condor);
            assert_eq!(
                second.iter().map(|(pass, index, ..)| (*pass, *index)).collect::<Vec<_>>(),
                [(4, 0)]
            );
            assert_eq!(
                condor.scenes[0].sequence_data.target_quality.passes.len(),
                4
            );
            assert_eq!(
                serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes[..3])
                    .expect("retained old hit"),
                prefix
            );
            assert_eq!(
                serde_json::to_vec(&condor.scenes[1].sequence_data.target_quality.passes)
                    .expect("over-budget peer"),
                peer_history
            );
            assert_eq!(
                std::fs::read(peer_output).expect("over-budget output"),
                peer_bytes
            );
            assert_eq!(saved.borrow().len(), 2);
            assert!(
                first
                    .iter()
                    .chain(&second)
                    .all(|(_, _, _, score)| score.is_finite() && *score > 20.0)
            );
            eprintln!(
                "Q13 earlier middle hit excludes missing SVT; changed target resumes only that scene after its 3-record prefix, now-budget-complete missing peer excluded; progress={first:?}/{second:?}, saves=2"
            );
        }

        #[test]
        fn q13_execute_lowered_budget_preserves_history_without_scratch_or_extra_saves() {
            let (temp, mut condor, saved) = q13_fixture(&[3, 1], 1);
            let before = serde_json::to_vec(&condor.scenes).expect("scenes");
            let no_work = temp.path().join("no-work");
            condor.sequence_config.parallel_encoder.scenes_directory = no_work.clone();
            assert!(q13_execute(&mut condor).is_empty());
            assert_eq!(
                serde_json::to_vec(&condor.scenes).expect("unchanged"),
                before
            );
            assert!(saved.borrow().is_empty());
            assert!(
                !no_work.exists(),
                "budget admission precedes scratch creation"
            );
            condor.scenes[0].encoder.set_quantizer(20.0);
            assert!(q13_execute(&mut condor).is_empty());
            assert_eq!(condor.scenes[0].encoder.quantizer(), Some(26.0));
            assert!(q13_execute(&mut condor).is_empty());
            assert_eq!(
                saved.borrow().len(),
                1,
                "only changed measured-q restoration saves"
            );
            condor.sequence_config.parallel_encoder.scenes_directory = temp.path().to_owned();
            condor.sequence_config.target_quality.as_mut().expect("TQ").maximum_probes = 2;
            let raised = q13_execute(&mut condor);
            assert_eq!(
                raised.iter().map(|(pass, index, ..)| (*pass, *index)).collect::<Vec<_>>(),
                [(2, 1)]
            );
            assert_eq!(
                condor
                    .scenes
                    .iter()
                    .map(|s| s.sequence_data.target_quality.passes.len())
                    .collect::<Vec<_>>(),
                [3, 2]
            );
            let original: Vec<Scene<DefaultSequenceData>> =
                serde_json::from_slice(&before).expect("original scenes");
            for (scene, old) in condor.scenes.iter().zip(original) {
                let prefix = &scene.sequence_data.target_quality.passes
                    [..old.sequence_data.target_quality.passes.len()];
                assert_eq!(
                    serde_json::to_vec(prefix).expect("prefix"),
                    serde_json::to_vec(&old.sequence_data.target_quality.passes)
                        .expect("original prefix")
                );
            }
            assert_eq!(saved.borrow().len(), 2);
            eprintln!(
                "Q13 lowered max1 [3,1] no work/scratch/save, one changed-q restoration save only; raised max2 -> [3,2] with progress={raised:?}"
            );
        }

        #[cfg(unix)]
        fn executable(path: &Path, source: &str) {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(path, source).expect("script");
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                .expect("executable");
        }

        #[test]
        fn q09_actual_probe_retains_bps_in_serialized_history() {
            let (_temp, mut input, encoder, config, directory) = fixture();
            let completed = probe(
                1,
                &directory,
                &config,
                &mut input,
                &tasks(&encoder, &directory, 1),
            )
            .expect("real encode/metric pass");
            for task in completed {
                // Each scene selects two frames at 24 FPS: twelve scenes/second, eight bits/byte.
                let bytes = std::fs::metadata(&task.output).expect("real output").len();
                let expected = bytes as f64 * 96.0;
                assert!((task.passes[0].bitrate - expected).abs() < 1e-8);
                let saved = serde_json::to_value(&task.passes).expect("persistable history");
                assert_eq!(saved[0]["bitrate"].as_f64(), Some(task.passes[0].bitrate));
                assert_eq!(task.passes[0].scores.len(), 2);
            }
        }

        #[cfg(unix)]
        #[test]
        fn q07_cleanup_failure_aborts_before_encoder_with_path_context() {
            let (temp, mut input, mut encoder, config, directory) = fixture();
            let script = temp.path().join("encoder");
            executable(
                &script,
                "#!/bin/sh\nprintf spawned > \"$0.calls\"\nexit 1\n",
            );
            encoder.set_executable(Some(script.clone()));
            let tasks = tasks(&encoder, &directory, 1);
            std::fs::create_dir_all(&tasks[0].output).expect("directory collision");
            let sentinel = tasks[0].output.join("caller-data");
            std::fs::write(&sentinel, b"preserve").expect("sentinel");
            let error =
                probe(1, &directory, &config, &mut input, &tasks).expect_err("cleanup must fail");
            eprintln!("Q07 cleanup owning-path error: {error:#}");
            assert!(
                format!("{error:#}").contains(&tasks[0].output.display().to_string()),
                "{error:#}"
            );
            assert!(
                !script.with_extension("calls").exists(),
                "cleanup must precede any encoder spawn"
            );
            assert_eq!(
                std::fs::read(sentinel).expect("preserved directory"),
                b"preserve"
            );
            let mut escaped = tasks.clone();
            escaped[1].output = temp.path().join("source.mkv");
            assert!(probe(1, &directory, &config, &mut input, &escaped).is_err());
            assert!(
                escaped[1].output.exists(),
                "never delete outside the owned pass"
            );
            let detached = temp.path().join("caller-directory");
            std::fs::rename(directory.join("1"), &detached).expect("move scratch");
            std::os::unix::fs::symlink(&detached, directory.join("1")).expect("pass-dir link");
            assert!(probe(1, &directory, &config, &mut input, &tasks).is_err());
            assert_eq!(
                std::fs::read(detached.join("00000.mkv/caller-data"))
                    .expect("linked directory preserved"),
                b"preserve"
            );
            assert!(!script.with_extension("calls").exists());
        }

        #[cfg(unix)]
        #[test]
        fn q07_cleanup_never_deletes_an_input_even_inside_pass_scratch() {
            let (temp, mut input, mut encoder, config, directory) = fixture();
            let script = temp.path().join("encoder");
            executable(
                &script,
                "#!/bin/sh\nprintf spawned > \"$0.calls\"\nexit 1\n",
            );
            encoder.set_executable(Some(script.clone()));
            let tasks = tasks(&encoder, &directory, 1);
            std::fs::copy(temp.path().join("source.mkv"), &tasks[0].output)
                .expect("source inside scratch");
            if let Input::Video { path, .. } = &mut input {
                *path = directory.join("1/../1/00000.mkv");
            }
            let before = std::fs::read(&tasks[0].output).expect("source bytes");
            assert!(probe(1, &directory, &config, &mut input, &tasks).is_err());
            assert_eq!(
                std::fs::read(&tasks[0].output).expect("source must be preserved"),
                before
            );
            assert!(!script.with_extension("calls").exists());
        }

        #[cfg(unix)]
        #[test]
        fn q07_complete_pass_returns_unchanged_without_scratch_input_or_encoder() {
            let (temp, mut input, mut encoder, config, directory) = fixture();
            let script = temp.path().join("encoder");
            executable(
                &script,
                "#!/bin/sh\nprintf spawned > \"$0.calls\"\nexit 1\n",
            );
            encoder.set_executable(Some(script.clone()));
            let mut tasks = tasks(&encoder, &directory, 1);
            for task in &mut tasks {
                task.passes.push(QualityPass {
                    provenance: None,
                    backend: Some(MetricBackend::current(Engine::Vmaf)),
                    quantizer: 26.0,
                    scores: vec![75.0, 76.0],
                    bitrate: 100.0,
                    started_on: 12,
                    completed_on: 34,
                });
            }
            std::fs::remove_file(temp.path().join("source.mkv")).expect("no retained source");
            for available in [true, false] {
                if !available {
                    std::fs::remove_file(&script).expect("unavailable encoder");
                }
                let before = format!("{tasks:?}");
                let completed = probe(1, &directory, &config, &mut input, &tasks)
                    .expect("complete validated history is sufficient");
                assert_eq!(format!("{completed:?}"), before);
                assert!(!script.with_extension("calls").exists());
                eprintln!(
                    "Q07 complete reuse: records unchanged, missing outputs/source, encoder available={available}, no spawn"
                );
            }
            let mut invalid = tasks.clone();
            invalid[1].passes[0].scores.pop();
            assert!(
                probe(1, &directory, &config, &mut input, &invalid)
                    .expect_err("strict history")
                    .is::<MeasurementError>()
            );
            let mut invalid_config = config.clone();
            *invalid_config.metric.target_range_mut() = (f64::NAN, 75.0);
            assert!(probe(1, &directory, &invalid_config, &mut input, &tasks).is_err());
            let mut temporal = config.clone();
            temporal.metric = QualityMetric::CVVDP {
                target_range: (0.0, 1.0),
                resolution: None,
                display_model: None,
                resize_to_display: None,
                disable_temporal: None,
                gpu_id: None,
            };
            let mut invalid_selection = tasks.clone();
            invalid_selection[0].frame_indices.reverse();
            let error = probe(1, &directory, &temporal, &mut input, &invalid_selection)
                .expect_err("pure temporal selection validation still precedes reuse");
            assert!(
                format!("{error:#}").contains("strictly increasing"),
                "{error:#}"
            );
            assert!(probe(0, &directory, &config, &mut input, &tasks).is_err());
            assert!(
                probe(1, &directory, &config, &mut input, &[])
                    .expect("empty no-work")
                    .is_empty()
            );
        }

        #[cfg(unix)]
        #[test]
        fn q07_incomplete_pass_reencodes_current_quantizers_and_replaces_partial_records() {
            let (temp, mut input, encoder, config, directory) = fixture();
            let mut tasks = probe(
                1,
                &directory,
                &config,
                &mut input,
                &tasks(&encoder, &directory, 1),
            )
            .expect("verified prefix");
            let previous = tasks
                .iter()
                .map(|task| serde_json::to_value(&task.passes).expect("history"))
                .collect::<Vec<_>>();
            for task in &mut tasks {
                task.output = directory.join("2").join(format!("{:05}.mkv", task.original_index));
                task.encoder.set_quantizer(28.0);
            }
            let partial = probe(2, &directory, &config, &mut input, &tasks[..1])
                .expect("one scene has a current record");
            tasks[0].passes = partial[0].passes.clone();
            std::fs::write(&tasks[0].output, b"stale q28 marker").expect("abandoned output");
            let outside = temp.path().join("caller-directory");
            std::fs::create_dir_all(&outside).expect("outside directory");
            std::fs::write(outside.join("caller-data"), b"outside sentinel").expect("outside");
            std::os::unix::fs::symlink(&outside, &tasks[1].output).expect("stale output symlink");
            let unrelated = directory.join("2/unrelated.txt");
            std::fs::write(&unrelated, b"not a declared output").expect("caller neighbor");
            for (task, q) in tasks.iter_mut().zip([30.0, 34.0]) {
                task.encoder.set_quantizer(q);
            }
            let completed =
                probe(2, &directory, &config, &mut input, &tasks).expect("whole pass replay");
            eprintln!(
                "Q07 mixed replay: {:?}",
                completed
                    .iter()
                    .map(|task| (
                        task.original_index,
                        task.passes.iter().map(|pass| pass.quantizer).collect::<Vec<_>>()
                    ))
                    .collect::<Vec<_>>()
            );
            for (index, task) in completed.iter().enumerate() {
                assert_eq!(
                    task.passes.len(),
                    2,
                    "replace current record rather than appending a duplicate"
                );
                assert_eq!(
                    serde_json::to_value(&task.passes[..1]).expect("prefix"),
                    previous[index]
                );
                assert_eq!(task.passes[1].quantizer, [30.0, 34.0][index]);
                assert_eq!(task.passes[1].scores.len(), 2);
                assert!(task.passes[1].scores.iter().all(|score| score.is_finite()));
                assert!(
                    !task
                        .output
                        .symlink_metadata()
                        .expect("fresh artifact")
                        .file_type()
                        .is_symlink()
                );
                assert_ne!(
                    std::fs::read(&task.output).expect("fresh output"),
                    b"stale q28 marker"
                );
            }
            assert_eq!(
                std::fs::read(outside.join("caller-data"))
                    .expect("symlink directory target preserved"),
                b"outside sentinel"
            );
            assert_eq!(
                std::fs::read(unrelated).expect("caller data preserved"),
                b"not a declared output"
            );
            tasks[1].passes.clear();
            let before = std::fs::read(&tasks[0].output).expect("output");
            assert!(
                probe(2, &directory, &config, &mut input, &tasks).is_err(),
                "missing earlier prefix must not be invented"
            );
            assert_eq!(
                std::fs::read(&tasks[0].output).expect("not deleted"),
                before
            );
        }

        #[test]
        fn q07_execute_no_new_work_saves_only_changed_measured_restoration() {
            for target_hit in [true, false] {
                let (temp, mut input, encoder, mut config, directory) = fixture();
                let measured = probe(
                    1,
                    &directory,
                    &config,
                    &mut input,
                    &tasks(&encoder, &directory, 1)[..1],
                )
                .expect("real measured history");
                let history = measured[0].passes.clone();
                config.quantizer_range = (26, 26);
                if target_hit {
                    *config.metric.target_range_mut() = (0.0, 100.0);
                }
                let score = config
                    .metric
                    .calculate_score(config.probing.statistic, &history[0].scores)
                    .expect("real measured score");
                assert_eq!(config.metric.score_within_target(score), target_hit);
                let mut scene_encoder = encoder.clone();
                scene_encoder.set_quantizer(20.0);
                let mut data = DefaultSequenceData::default();
                data.get_target_quality_mut().expect("TQ").passes = history.clone();
                let scenes = vec![Scene {
                    start_frame: 0,
                    end_frame: 2,
                    sub_scenes: None,
                    encoder: scene_encoder,
                    sequence_data: data,
                }];
                let snapshots = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
                let saved = std::rc::Rc::clone(&snapshots);
                let mut condor = Condor::new(
                    input,
                    Output {
                        path: temp.path().join("final.mkv"),
                        overwrite: false,
                        tags: Default::default(),
                        video_tags: Default::default(),
                    },
                    encoder,
                    scenes,
                    Some(DefaultSequenceConfig {
                        target_quality: Some(config),
                        parallel_encoder: ParallelEncoderConfig::new(temp.path()),
                        ..Default::default()
                    }),
                    Box::new(move |data| {
                        saved.borrow_mut().push(data.scenes);
                        Ok(())
                    }),
                );
                crate::core::sequence::c14_tests::current_target_fixture(&mut condor);
                let history = condor.scenes[0].sequence_data.target_quality.passes.clone();
                for _ in 0..2 {
                    let (tx, _rx) = sync::mpsc::channel();
                    TargetQuality::default()
                        .execute_cancel(&mut condor, tx, Default::default())
                        .expect("no new work");
                    assert_eq!(condor.scenes[0].encoder.quantizer(), Some(26.0));
                    assert_eq!(
                        snapshots.borrow().len(),
                        1,
                        "actual callback once, no gratuitous repeated save"
                    );
                    let saved = snapshots.borrow();
                    assert_eq!(saved[0][0].encoder.quantizer(), Some(26.0));
                    assert_eq!(
                        serde_json::to_value(
                            &saved[0][0].sequence_data.get_target_quality().expect("TQ").passes
                        )
                        .expect("history"),
                        serde_json::to_value(&history).expect("history")
                    );
                }
                for forced in [false, true] {
                    let cancelled = Cancellation::default();
                    cancelled.request().expect("graceful");
                    if forced {
                        cancelled.request().expect("force");
                    }
                    let (tx, _rx) = sync::mpsc::channel();
                    let error = TargetQuality::default()
                        .execute_cancel(&mut condor, tx, cancelled)
                        .expect_err("cancel precedence");
                    assert_eq!(
                        error.downcast_ref::<Cancelled>(),
                        Some(if forced {
                            &Cancelled::Forced
                        } else {
                            &Cancelled::Graceful
                        })
                    );
                }
                condor.scenes[0].encoder.set_quantizer(20.0);
                condor.sequence_config.target_quality = None;
                let (tx, _rx) = sync::mpsc::channel();
                TargetQuality::default()
                    .execute_cancel(&mut condor, tx, Default::default())
                    .expect("disabled");
                assert_eq!(condor.scenes[0].encoder.quantizer(), Some(20.0));
                condor.scenes.clear();
                let (tx, _rx) = sync::mpsc::channel();
                TargetQuality::default()
                    .execute_cancel(&mut condor, tx, Default::default())
                    .expect("empty");
                assert_eq!(snapshots.borrow().len(), 1);
                eprintln!(
                    "Q07 no-work target_hit={target_hit}: measured q26 restored from configured q20, exactly one actual save across repeated execution; disabled/empty/cancel controls unchanged"
                );
            }
        }

        #[cfg(unix)]
        #[test]
        fn q07_cancelled_or_callback_failed_execute_keeps_history_then_replays_current_q() {
            use std::time::{Duration, Instant};
            for mode in [
                "graceful",
                "forced",
                "closed-progress",
                "native-graceful",
                "native-forced",
                "native-callback-fault",
                "cleanup-failure",
            ] {
                let (temp, mut input, encoder, mut config, directory) = fixture();
                let measured = probe(
                    1,
                    &directory,
                    &config,
                    &mut input,
                    &tasks(&encoder, &directory, 1),
                )
                .expect("verified prefix");
                config.quantizer_range = (30, 30);
                let script = temp.path().join("encoder");
                let ffmpeg = which::which("ffmpeg").expect("existing FFmpeg");
                // Finite rendezvous at actual encoder admission, not buffered UI timing.
                executable(
                    &script,
                    &format!(
                        "#!/bin/sh\nset -eu\nprintf started > \"$0.started\"\nn=0\nwhile [ \"$n\" -lt 500 ]; do\n n=$((n+1))\n if [ -f \"$0.release\" ]; then exec '{}' \"$@\"; fi\n sleep 0.01\ndone\nexit 1\n",
                        ffmpeg.display()
                    ),
                );
                let scenes = measured
                    .iter()
                    .map(|task| {
                        let mut data = DefaultSequenceData::default();
                        data.get_target_quality_mut().expect("TQ").passes = task.passes.clone();
                        let mut encoder = encoder.clone();
                        if !mode.starts_with("native-") {
                            encoder.set_executable(Some(script.clone()));
                        }
                        Scene {
                            start_frame: task.frame_indices[0],
                            end_frame: task.frame_indices[1] + 1,
                            sub_scenes: None,
                            encoder,
                            sequence_data: data,
                        }
                    })
                    .collect::<Vec<_>>();
                let snapshots = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
                let saved = std::rc::Rc::clone(&snapshots);
                let mut condor = Condor::new(
                    input,
                    Output {
                        path: temp.path().join("final.mkv"),
                        overwrite: false,
                        tags: Default::default(),
                        video_tags: Default::default(),
                    },
                    encoder,
                    scenes,
                    Some(DefaultSequenceConfig {
                        target_quality: Some(config),
                        parallel_encoder: ParallelEncoderConfig::new(temp.path()),
                        ..Default::default()
                    }),
                    Box::new(move |data| {
                        saved.borrow_mut().push(data.scenes);
                        Ok(())
                    }),
                );
                condor.sequence_config.scene_concatenator.method =
                    ConcatMethod::FFmpeg(Default::default());
                crate::core::sequence::c14_tests::current_target_fixture(&mut condor);
                let mut sequence = TargetQuality::default();
                if mode.starts_with("native-") {
                    // Pause frame 1 in the real native VMAF reference decoder.
                    // Frame 0 supplies metadata and one actual VMAF submission.
                    let callback = format!(
                        "import vapoursynth as vs\nimport pathlib, time\np = pathlib.Path({path:?})\nc = vs.core\nclip = c.std.BlankClip(width=128, height=128, length=4, format=vs.YUV420P8, fpsnum=24)\ndef frame(n, f):\n if n == 1:\n  p.with_suffix('.started').write_text('native')\n  deadline = time.monotonic() + 10\n  while not p.with_suffix('.release').exists():\n   if time.monotonic() > deadline: raise RuntimeError('Q07 native rendezvous timed out')\n   time.sleep(0.01)\n  if {fault}: raise RuntimeError('Q07 reference callback fault')\n return f\nclip.std.ModifyFrame(clip, frame).std.SetFrameProps(_Matrix=1, _Transfer=1, _Primaries=1, _Range=1).set_output()\n",
                        path = script.display().to_string(),
                        fault = if mode == "native-callback-fault" {
                            "True"
                        } else {
                            "False"
                        }
                    );
                    sequence.metric_input = Some(
                        Input::from_data(&InputModel::VapourSynthScript {
                            source: crate::models::input::VapourSynthScriptSource::Text(callback),
                            variables: Default::default(),
                            index: 0,
                            filters: vec![],
                            stream_concurrently: false,
                        })
                        .expect("real lazy native reference input"),
                    );
                    condor.sequence_config.target_quality.as_mut().expect("TQ").metric_input =
                        sequence.metric_input.as_ref().map(Input::as_data);
                    crate::core::sequence::c14_tests::current_target_fixture(&mut condor);
                }
                let before = serde_json::to_value(&condor.scenes).expect("current fixture before");
                let cancellation = Cancellation::default();
                let signal = cancellation.clone();
                let barrier = script.clone();
                let (tx, rx) = sync::mpsc::channel();
                let (result, events) = if mode == "cleanup-failure" {
                    let collision = directory.join("2/00000.mkv");
                    std::fs::create_dir_all(&collision).expect("stale directory");
                    std::fs::write(collision.join("sentinel"), b"preserve").expect("sentinel");
                    let result = sequence.execute_cancel(&mut condor, tx, cancellation);
                    assert!(
                        !script.with_extension("started").exists(),
                        "no native work before cleanup succeeds"
                    );
                    assert_eq!(
                        std::fs::read(collision.join("sentinel")).expect("preserved"),
                        b"preserve"
                    );
                    std::fs::remove_file(collision.join("sentinel")).expect("fixture cleanup");
                    std::fs::remove_dir(collision).expect("fixture cleanup");
                    (result, rx.try_iter().collect::<Vec<_>>())
                } else {
                    let controller = thread::spawn(move || {
                        let start = Instant::now();
                        while !barrier.with_extension("started").exists() {
                            assert!(
                                start.elapsed() < Duration::from_secs(10),
                                "encoder never admitted"
                            );
                            thread::sleep(Duration::from_millis(10));
                        }
                        if mode == "closed-progress" {
                            drop(rx);
                        } else {
                            if mode != "native-callback-fault" {
                                signal.request().expect("graceful");
                            }
                            if mode.ends_with("forced") {
                                signal.request().expect("forced");
                            }
                            // Keep UI alive through every production relay join.
                            std::fs::write(barrier.with_extension("release"), b"go")
                                .expect("release");
                            return rx.into_iter().collect::<Vec<_>>();
                        }
                        std::fs::write(barrier.with_extension("release"), b"go").expect("release");
                        vec![]
                    });
                    let result = sequence.execute_cancel(&mut condor, tx, cancellation);
                    (result, controller.join().expect("controller"))
                };
                if mode.ends_with("graceful") {
                    result.expect("graceful incomplete pass");
                } else {
                    let error = result.expect_err("force/callback error");
                    if mode.ends_with("forced") {
                        assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
                    }
                    if mode == "native-callback-fault" {
                        assert!(
                            format!("{error:#}").contains("Q07 reference callback fault"),
                            "{error:#}"
                        );
                    }
                    if mode == "cleanup-failure" {
                        assert!(format!("{error:#}").contains("00000.mkv"), "{error:#}");
                    }
                }
                assert_eq!(
                    serde_json::to_value(&condor.scenes).expect("after"),
                    before,
                    "no accepted current record"
                );
                assert!(snapshots.borrow().is_empty(), "no empty-pass save");
                assert!(!events.iter().any(|event| matches!(
                    event,
                    SequenceStatus::Subprocess {
                        child: Status::Processing {
                            completion: SequenceCompletion::SceneQuality { .. },
                            ..
                        },
                        ..
                    }
                )));
                if mode.starts_with("native-") {
                    assert!(events.iter().any(|event| matches!(event, SequenceStatus::Subprocess { child: Status::Processing { id, completion: SequenceCompletion::Frames { completed: 1, .. } }, .. } if id == "Compare")), "actual native submission preceded cancellation/fault: {events:?}");
                    assert!(!events.iter().any(|event| matches!(event, SequenceStatus::Subprocess { child: Status::Completed { id }, .. } if id == "Compare")));
                }
                for scene in &mut condor.scenes {
                    scene.encoder.set_executable(None);
                }
                condor.sequence_config.target_quality.as_mut().expect("TQ").quantizer_range =
                    (34, 34);
                condor.sequence_config.target_quality.as_mut().expect("TQ").metric_input = None;
                crate::core::sequence::c14_tests::current_target_fixture(&mut condor);
                let (tx, _rx) = sync::mpsc::channel();
                TargetQuality::default()
                    .execute_cancel(&mut condor, tx, Default::default())
                    .expect("resume complete pass");
                assert_eq!(snapshots.borrow().len(), 1);
                for (index, scene) in condor.scenes.iter().enumerate() {
                    let passes = &scene.sequence_data.get_target_quality().expect("TQ").passes;
                    assert_eq!(passes.len(), 2);
                    let mut raw_prefix = passes[..1].to_vec();
                    for pass in &mut raw_prefix {
                        pass.provenance = None;
                    }
                    assert_eq!(
                        serde_json::to_value(&raw_prefix).expect("raw measured prefix"),
                        serde_json::to_value(&measured[index].passes).expect("prefix")
                    );
                    assert_eq!(
                        passes[1].quantizer, 34.0,
                        "resume uses current quantizer, not abandoned q30"
                    );
                    assert_eq!(passes[1].scores.len(), 2);
                }
                eprintln!(
                    "Q07 {mode}: interrupted owning execute kept both verified prefixes and made zero saves; resumed both at current q34 with one actual save"
                );
            }
        }
    }

    /// An incomplete pass must not be reused, or a resumed pass is scored as a
    /// mixture of the abandoned attempt and the new one.
    #[test]
    fn a_pass_is_complete_only_when_every_scene_recorded_it() {
        assert!(!TargetQuality::pass_is_complete(
            &[task_with_passes(0, 0)],
            1
        ));
        // Recorded for one scene only, so its leftovers are purged.
        assert!(!TargetQuality::pass_is_complete(
            &[task_with_passes(0, 1), task_with_passes(1, 0)],
            1
        ));
        assert!(TargetQuality::pass_is_complete(
            &[task_with_passes(0, 1), task_with_passes(1, 1)],
            1
        ));
        // Completing an earlier pass says nothing about the current one.
        assert!(!TargetQuality::pass_is_complete(
            &[task_with_passes(0, 1)],
            2
        ));
    }

    #[test]
    fn q05_tq_owning_group_rejects_missing_extra_and_nonfinite() {
        let mut tasks = [task_with_passes(3, 0), task_with_passes(7, 0)];
        for task in &mut tasks {
            task.frame_indices = vec![task.original_index];
        }
        for scores in [
            vec![],
            vec![1.0],
            vec![1.0, 2.0, 3.0],
            vec![1.0, f64::NAN],
            vec![1.0, f64::INFINITY],
            vec![1.0, f64::NEG_INFINITY],
        ] {
            let result = std::panic::catch_unwind(|| {
                TargetQuality::group_task_scores(&Default::default(), &tasks, scores)
            });
            assert!(
                matches!(result, Ok(Err(_))),
                "missing/extra/nonfinite must return Err, never drain/panic: {result:?}"
            );
            assert!(tasks.iter().all(|task| task.passes.is_empty()));
        }
    }

    #[test]
    fn q05_tq_route_admits_all_scenes_before_completion_and_never_retries_corruption() {
        use crate::models::sequence::target_quality::types::ProbeStatistic;
        let mut tasks = [task_with_passes(3, 0), task_with_passes(7, 0)];
        for task in &mut tasks {
            task.frame_indices = vec![task.original_index, task.original_index + 1];
        }
        let mut config = TargetQualityConfig::default();
        config.probing.statistic = ProbeStatistic::Harmonic;
        for scores in [
            vec![],
            vec![2.0; 3],
            vec![2.0; 5],
            vec![2.0, 2.0, 1.0, f64::NAN],
            vec![2.0, 2.0, 1.0, f64::INFINITY],
            vec![2.0, 2.0, 1.0, f64::NEG_INFINITY],
            vec![2.0, 2.0, 1.0, -1.0],
        ] {
            for plugin in [false, true] {
                let (tx, rx) = sync::mpsc::channel();
                let (compare, relay) = spawn_compare_progress(tx);
                let native = (!plugin).then(|| Ok(scores.clone()));
                let error = TargetQuality::route_compare_scores(
                    native,
                    Some(Engine::Fmetrics),
                    &config.metric,
                    compare,
                    relay,
                    |tx| {
                        assert!(plugin, "corruption retried plugin");
                        drop(tx);
                        Ok(scores.clone())
                    },
                    |values| TargetQuality::validate_task_scores(&config, &tasks, values),
                )
                .expect_err("invalid batch");
                assert!(error.is::<MeasurementError>(), "{error:#}");
                if scores == [2.0, 2.0, 1.0, -1.0] {
                    assert!(format!("{error:#}").contains("scene 7 pass 1"), "{error:#}");
                }
                assert!(tasks.iter().all(|task| task.passes.is_empty()));
                assert!(
                    matches!(rx.try_recv(), Err(sync::mpsc::TryRecvError::Disconnected)),
                    "no completed event; relay joined/disconnected"
                );
            }
        }
        let (tx, rx) = sync::mpsc::channel();
        let (compare, relay) = spawn_compare_progress(tx);
        let mut fallbacks = 0;
        let scores = TargetQuality::route_compare_scores(
            Some(Err(anyhow::anyhow!(
                av_metrics_fmetrics::FmetricsError::LibraryNotFound {
                    reason: "Q05 test library absent".to_owned()
                }
            )
            .context("opening native library"))),
            Some(Engine::Fmetrics),
            &config.metric,
            compare,
            relay,
            |tx| {
                fallbacks += 1;
                drop(tx);
                Ok(vec![2.0; 4])
            },
            |values| TargetQuality::validate_task_scores(&config, &tasks, values),
        )
        .expect("availability fallback")
        .expect("complete");
        assert_eq!(fallbacks, 1);
        assert_eq!(scores.0, [2.0; 4]);
        assert_eq!(scores.1, Engine::VapourSynth);
        assert!(
            matches!(rx.try_recv(), Ok(SequenceStatus::Whole(Status::Completed { id })) if id == "Compare")
        );
    }

    #[test]
    fn q05_claimed_tq_history_rejects_bad_counts_values_and_reductions() {
        use crate::models::sequence::target_quality::types::ProbeStatistic;
        let mut config = TargetQualityConfig::default();
        config.probing.statistic = ProbeStatistic::Harmonic;
        for scores in [
            vec![],
            vec![1.0],
            vec![1.0; 3],
            vec![1.0, f64::NAN],
            vec![1.0, f64::INFINITY],
            vec![1.0, -1.0],
        ] {
            let error = TargetQuality::validate_history(
                &config,
                7,
                2,
                &[QualityPass {
                    scores,
                    ..Default::default()
                }],
            )
            .expect_err("claimed corrupt history");
            assert!(error.is::<MeasurementError>());
            assert!(format!("{error:#}").contains("scene 7 history pass 1"));
        }
        TargetQuality::validate_history(&config, 7, 2, &[]).expect("no history is unmeasured");
    }

    #[test]
    fn verified_quantizer_uses_single_in_range_probe() {
        // One probe inside the target must be used as-is
        let history = [(30.0, 74.32)];
        assert_eq!(
            TargetQuality::verified_quantizer(&ssimulacra2(), &history),
            Some(30.0)
        );
    }

    #[test]
    fn verified_quantizer_prefers_highest_in_range_quantizer() {
        let history = [(30.0, 71.96), (17.5, 79.76), (25.25, 75.31), (26.0, 74.1)];
        assert_eq!(
            TargetQuality::verified_quantizer(&ssimulacra2(), &history),
            Some(26.0)
        );
    }

    #[test]
    fn verified_quantizer_is_none_without_in_range_probe() {
        let history = [(30.0, 64.67), (17.5, 82.31), (22.75, 77.86), (25.25, 73.98)];
        assert_eq!(
            TargetQuality::verified_quantizer(&ssimulacra2(), &history),
            None
        );
        assert_eq!(TargetQuality::verified_quantizer(&ssimulacra2(), &[]), None);
    }

    #[test]
    fn verified_quantizer_handles_inverse_metric() {
        let butteraugli = QualityMetric::BUTTERAUGLI {
            target_range: (0.8, 1.2),
            resolution: None,
            threads: None,
            intensity_multiplier: None,
            norm: None,
            gpu_id: None,
        };
        let history = [(20.0, 0.6), (28.0, 0.95), (32.0, 1.15), (40.0, 1.6)];
        assert_eq!(
            TargetQuality::verified_quantizer(&butteraugli, &history),
            Some(32.0)
        );
    }

    /// A native pass must declare that comparing has begun.
    ///
    /// The UI picks the compare bar over the encode bar from a report whose
    /// `completed` is 0. A metric that only reports after scoring each pair
    /// never sends one, so the bar stayed on the encode count and then
    /// jumped to the next pass.
    #[test]
    fn a_native_pass_declares_the_compare_phase_before_reporting_scores() {
        let (tx, rx) = sync::mpsc::channel();
        let total = 44_usize;

        let mut declared = true;
        let mut report = |position: usize, _score: f64| {
            if declared {
                declared = false;
                tx.send(SequenceStatus::Whole(Status::Processing {
                    id: "Compare".to_owned(),
                    completion: SequenceCompletion::Frames {
                        completed: 0,
                        total: total as u64,
                    },
                }))
                .expect("send the phase declaration");
            }
            tx.send(SequenceStatus::Whole(Status::Processing {
                id: "Compare".to_owned(),
                completion: SequenceCompletion::Frames {
                    completed: (position + 1) as u64,
                    total: total as u64,
                },
            }))
            .expect("send progress");
        };

        for position in 0..total {
            report(position, 90.0);
        }
        drop(tx);

        let reports: Vec<(u64, u64)> = rx
            .iter()
            .filter_map(|status| match status {
                SequenceStatus::Whole(Status::Processing {
                    completion: SequenceCompletion::Frames { completed, total },
                    ..
                }) => Some((completed, total)),
                _ => None,
            })
            .collect();

        // The first report is the zero-count declaration, then one report per
        // frame, and the last reaches the total.
        assert_eq!(
            reports.first().map(|(completed, _)| *completed),
            Some(0),
            "the phase must be declared before any score is reported"
        );
        assert_eq!(
            reports.len(),
            total + 1,
            "one declaration plus one report per frame"
        );
        assert_eq!(
            reports.last(),
            Some(&(total as u64, total as u64)),
            "the last report must reach the total"
        );
    }

    /// Probes are scored against the unfiltered reference, so a filter would
    /// corrupt the metric the same way it does for Quality Check.
    #[test]
    fn a_probe_encoder_drops_the_ffmpeg_filter() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, false);
        encoder.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
        assert!(
            encoder.ffmpeg_filter().is_some(),
            "the scene encoder filters"
        );

        encoder.set_ffmpeg_filter(None);
        let probe = TargetQuality::remove_psychovisual_parameters(&encoder);

        assert_eq!(
            probe.ffmpeg_filter(),
            None,
            "the probe encoder must not filter"
        );
    }

    /// Probes are scored against the unfiltered reference, and a per-scene iso
    /// would also make scenes differ beyond the quantizer, so the probe drops
    /// the scene's photon noise while the final encode keeps it.
    #[test]
    fn a_probe_encoder_drops_the_photon_noise() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
        let Encoder::SVTAV1 { photon_noise, .. } = &mut encoder else {
            panic!("the base is SVT-AV1");
        };
        *photon_noise = Some(PhotonNoise {
            iso: 12000,
            chroma_iso: Some(4000),
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        });

        let Encoder::SVTAV1 {
            photon_noise: scene,
            ..
        } = &encoder
        else {
            panic!("the base is SVT-AV1");
        };
        assert!(scene.is_some(), "the scene encoder applies grain");

        let probe = TargetQuality::remove_psychovisual_parameters(&encoder);
        let Encoder::SVTAV1 {
            photon_noise: probe,
            ..
        } = &probe
        else {
            panic!("the base is SVT-AV1");
        };
        assert!(
            probe.is_none(),
            "the probe encoder must not apply photon noise"
        );
    }
}
