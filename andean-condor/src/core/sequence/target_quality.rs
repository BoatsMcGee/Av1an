use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{self, Arc, atomic::AtomicBool},
    thread,
    time::SystemTime,
};

use anyhow::{Context, Result, bail, ensure};
use itertools::Itertools;
use thiserror::Error;
use tracing::{debug, error, trace};

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

        let mut pass = 1;

        // A resumed/exhausted search may schedule no tasks, but its final q must
        // still come from recorded measurements. Empty histories leave scenes untouched.
        for scene in &mut condor.scenes {
            let Ok(data) = scene.sequence_data.get_target_quality() else {
                continue;
            };
            let history = Self::quantizer_score_history(&config, &data.passes)?;
            if let Some(q) = Self::measured_quantizer(&config.metric, &history) {
                scene.encoder.set_quantizer(q);
            }
        }

        loop {
            if pass > config.maximum_probes {
                break;
            }

            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id: DETAILS.name.to_owned(),
                completion: SequenceCompletion::Passes {
                    completed: pass - 1,
                    total: pass,
                },
            }))?;

            let pass_directory = target_quality_directory.join(pass.to_string());
            if !pass_directory.exists() {
                std::fs::create_dir_all(&pass_directory)?;
            }

            let tasks = condor
                .scenes
                .iter_mut()
                .zip(&scene_frame_indices)
                .enumerate()
                .map(|(index, (scene, frame_indices))| {
                    let frame_indices = frame_indices.clone();
                    let encoder = config.probe_encoder(&scene.encoder);
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
                .map(|task| -> Result<Option<Task>> {
                    // Skip if last probe scored within target
                    let previous = task
                        .passes
                        .get(pass.saturating_sub(2) as usize) // Previous pass
                        .or_else(|| task.passes.last()) // Last pass
                        .map(|quality_pass| config.metric.calculate_score(config.probing.statistic, &quality_pass.scores))
                        .transpose()?;
                    Ok((!previous.is_some_and(|score| config.metric.score_within_target(score))).then_some(task))
                })
                .filter_map(Result::transpose)
                .map(|task| {
                    let mut task = task?;
                    let quantizer_score_history = Self::quantizer_score_history(&config, &task.passes)?;
                    let Some(predicted_quantizer) = Self::next_probe_quantizer(
                        &config,
                        &task.encoder,
                        &quantizer_score_history,
                        &cancelled,
                    )? else {
                        return Ok(None);
                    };

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

            let probed = Self::probe_pass_with_options(
                pass,
                target_quality_directory,
                &config,
                input,
                self.metric_input.as_mut(),
                workers,
                &scene_concatenator_config.method,
                tasks.as_slice(),
                pass_progress_tx,
                &cancelled,
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

            let mut staged = condor.scenes.clone();
            let mut completions = Vec::with_capacity(completed_tasks.len());
            for completed_task in completed_tasks {
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
            condor.scenes = staged;
            for completion in completions {
                progress_tx.send(completion)?;
            }

            if !pass_warnings.is_empty() {
                warnings.extend(pass_warnings);
                break;
            }

            pass += 1;
            let mut data = condor_data.clone();
            data.scenes = condor.scenes.clone();
            (condor.save_callback)(data).expect("failed to save data");

            cancelled.check_forced()?;
            if cancelled.is_cancelled() {
                break;
            }
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

    fn quantizer_score_history(
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

    fn validate_history(
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
        for task in tasks {
            task.encoder.validate()?;
            Self::validate_history(
                config,
                task.original_index,
                task.frame_indices.len(),
                &task.passes,
            )?;
        }
        super::parallel_encoder::artifact::preflight(options.validate_frame_count)?;
        let warnings: Vec<anyhow::Error> = vec![];
        let temporal = metrics::scene_temporal(&config.metric);
        let scenes: Vec<_> = tasks.iter().map(|task| task.frame_indices.as_slice()).collect();
        if temporal && metrics::probe::validate_scene_selection(&scenes)? == 0 {
            return Ok((Vec::new(), warnings));
        }
        let pass_directory = target_quality_directory.join(pass.to_string());
        let output = concat_method.with_extension(&target_quality_directory.join(pass.to_string()));
        let framerate = input.clip_info()?.frame_rate;
        // Seconds per frame, for a probe encoded by an earlier run.
        let fps = *framerate.numer() as f64 / *framerate.denom() as f64;

        // An aborted pass leaves scene files encoded at an unsaved quantizer,
        // which a resume would skip while re-encoding the rest, so the pass
        // would be scored as a mixture. Discard an incomplete pass.
        let pass_is_complete = Self::pass_is_complete(tasks, pass);
        if !pass_is_complete && pass_directory.exists() {
            for entry in std::fs::read_dir(&pass_directory)? {
                let path = entry?.path();
                if path.is_file() {
                    debug!("Discarding orphaned probe output {}", path.display());
                    let _ = std::fs::remove_file(&path);
                }
            }
        }

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
                                id: "Encode".to_owned(),
                                completion: SequenceCompletion::Frames {
                                    completed: frames_already_completed as u64 + completed,
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
        let zone_plan = match ZonePlan::try_build_cancel(&encode_tasks, cancellation) {
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
        let started = SystemTime::now();
        // A metric scored natively above reuses those scores; otherwise the
        // plugin branches below run. For the vship metrics the plugin
        // branch is reached only when libvship was unavailable or
        // failed.
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
        let Some(scores) = Self::route_compare_scores(
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
        let ended = SystemTime::now();

        drop(progress_tx);

        // `results` covers only the tasks encoded this run and is indexed by
        // that subset, so its positions name no scene. `scene` is the
        // task's `original_index`, so key on it: zipping by position
        // attached the wrong scene's bitrate to every task past the
        // first skipped one.
        let results = results
            .into_iter()
            .flatten()
            .map(|result| (result.scene, result))
            .collect::<HashMap<_, _>>();

        let grouped = Self::group_task_scores(config, tasks, scores)?;
        let tasks = tasks
            .iter()
            .zip(grouped)
            .map(|(task, scores)| {
                let mut completed_task = task.clone();
                // A task skipped as already encoded has no result, so its size
                // comes from the file and its timings from its
                // last recorded pass.
                let bitrate = results.get(&task.original_index).map_or_else(
                    || {
                        let bytes = task.output.metadata().map_or(0, |meta| meta.len());
                        (bytes * 8) as f64 / (task.frame_indices.len() as f64 * fps)
                    },
                    |result| result.bitrate,
                );
                completed_task.passes.push(QualityPass {
                    quantizer: task.encoder.quantizer().expect("quantizer exists"),
                    scores,
                    bitrate,
                    started_on: task.passes.last().map_or_else(
                        || {
                            started
                                .duration_since(std::time::UNIX_EPOCH)
                                .expect("Time is valid")
                                .as_millis()
                        },
                        |previous| previous.started_on,
                    ),
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
    ) -> Result<Option<Vec<f64>>> {
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
            result => result.map(Some),
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
    fn measured_quantizer(metric: &QualityMetric, history: &[(f64, f64)]) -> Option<f64> {
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
    /// # Errors
    /// Rejects nonfinite/reversed ranges, nonpositive/nonfinite steps, and grids
    /// whose tick magnitudes exceed 2^52 - 1, before rounding or clamping.
    #[inline]
    pub fn predict_quantizer(
        quantizer_range: (f64, f64),
        target_score: f64,
        interpolators: (InterpolationMethod, InterpolationMethod),
        quantizer_score_history: &[(f64, f64)],
        step: f64,
    ) -> Result<f64> {
        Self::validate_quantizer_grid(quantizer_range, step)?;
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
                            InterpolationMethod::FritschCarlson => interpolators::fritsch_carlson(
                                &[scores[0], scores[1], scores[2]],
                                &[quantizers[0], quantizers[1], quantizers[2]],
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
                            InterpolationMethod::FritschCarlson => interpolators::fritsch_carlson(
                                &[s[0], s[1], s[2]],
                                &[q[0], q[1], q[2]],
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
                    // 6th and later probes: pchip over every point, which is monotone and so
                    // cannot predict a quantizer outside the probed range
                    _ => interpolators::pchip(&scores, &quantizers, target_score),
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

    use crate::{
        core::{
            encoder::{EncoderResult, StringOrBytes},
            sequence::parallel_encoder::ParallelEncoderResult,
        },
        models::{
            encoder::{Encoder, EncoderBase, photon_noise::PhotonNoise},
            sequence::target_quality::types::QualityMetric,
        },
    };

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

    fn result_for(scene: usize) -> Option<ParallelEncoderResult> {
        Some(ParallelEncoderResult {
            scene,
            started: std::time::UNIX_EPOCH,
            ended: std::time::UNIX_EPOCH,
            bytes: 1000 * (scene as u64 + 1),
            bitrate: 100.0 * (scene as f64 + 1.0),
            result: EncoderResult {
                encoder: EncoderBase::X264,
                parameters: Vec::new(),
                status: Default::default(),
                stdout: StringOrBytes::from(Vec::new()),
                stderr: StringOrBytes::from(Vec::new()),
            },
        })
    }

    fn ssimulacra2() -> QualityMetric {
        QualityMetric::SSIMULACRA2 {
            target_range: (74.0, 76.0),
            resolution: None,
            threads: None,
            gpu_id: None,
        }
    }

    /// A cancelled pass reports no completed tasks.
    ///
    /// Returning the input tasks made `execute` treat an unrecorded pass as
    /// scored and reach `passes.last()`, empty on the first pass, so it
    /// panicked.
    #[test]
    fn a_cancelled_pass_reports_no_completed_tasks() {
        let tasks = [task_with_passes(0, 0)];
        let completed: Vec<Task> = Vec::new();
        assert!(completed.is_empty());
        assert!(tasks[0].passes.is_empty());
    }

    /// Results must be attributed to the scene they were produced for.
    ///
    /// On resume `results` is a sparse subset, so zipping it against the tasks
    /// by position paired each scene with another scene's bitrate.
    #[test]
    fn results_attach_to_their_own_scene_when_the_subset_is_sparse() {
        // Scenes 0 and 2 already exist and are skipped, leaving scene 1.
        let tasks = [task_with_passes(0, 1), task_with_passes(1, 1), task_with_passes(2, 1)];
        let results = vec![result_for(1)];

        let indexed = results
            .into_iter()
            .flatten()
            .map(|result| (result.scene, result))
            .collect::<std::collections::HashMap<_, _>>();

        for task in &tasks {
            match indexed.get(&task.original_index) {
                Some(result) => assert_eq!(
                    result.bitrate,
                    100.0 * (task.original_index as f64 + 1.0),
                    "scene {} must read its own result",
                    task.original_index
                ),
                None => assert!(
                    !task.output.exists(),
                    "scene {} skipped without an output to reuse",
                    task.original_index
                ),
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
        assert_eq!(scores, [2.0; 4]);
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
