use std::sync::Arc;

use anyhow::{Result, bail, ensure};
use av1_grain::write_grain_table;
use tracing::{debug, error};

use crate::{
    core::{
        Condor,
        input::Input,
        sequence::{Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status},
    },
    models::sequence::{
        SequenceConfigHandler, SequenceDataHandler,
        parallel_encoder::{ParallelEncoderConfigHandler, ParallelEncoderDataHandler},
    },
};

pub(crate) mod artifact;
mod encode;
mod error;
mod lifecycle;
mod progress;
mod streaming;
mod task;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod e14_tests;

#[cfg(test)]
#[cfg(target_os = "linux")]
pub(super) mod e14_r1_tests;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod e11_tests;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod e13_tests;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod e28_tests;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod e15_tests;

pub use error::ParallelEncoderError;
pub(super) use lifecycle::TempOutput;
pub use task::{ParallelEncoderResult, Task};

/// Per-scene dispatch policy. A try includes every pass and output validation.
#[derive(Clone, Copy, Debug)]
pub struct ParallelEncoderOptions {
    pub max_tries: u32,
    pub validate_frame_count: bool,
}

impl Default for ParallelEncoderOptions {
    #[inline]
    fn default() -> Self {
        Self {
            max_tries: 3,
            validate_frame_count: true,
        }
    }
}

/// Frames each streaming worker keeps requested ahead and queued for its
/// encoder. 4-8 measured as fast as larger windows with less memory.
pub(super) const STREAM_WINDOW: usize = 8;

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Parallel Encoder",
    description: "Encodes a set of scenes in parallel until all scenes are encoded.",
    version: "0.0.1",
};

pub struct ParallelEncoder {
    pub input: Option<Input>,
}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for ParallelEncoder
where
    DataHandler: SequenceDataHandler + ParallelEncoderDataHandler,
    ConfigHandler: SequenceConfigHandler + ParallelEncoderConfigHandler,
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

        condor.sequence_config.parallel_encoder()?.validate()?;

        if condor.sequence_config.parallel_encoder()?.workers.is_some_and(|w| w == 0) {
            bail!(ParallelEncoderError::NoWorkers);
        }

        // Fail early instead of at the first scene's filter spawn.
        if condor.scenes.iter().any(|scene| scene.encoder.ffmpeg_filter().is_some())
            && which::which("ffmpeg").is_err()
        {
            bail!(ParallelEncoderError::FfmpegNotFound);
        }

        if let Some(input) = &self.input {
            Input::validate(&input.as_data())?;
        }

        // Ensure all the scene encoders are validated
        for scene in &condor.scenes {
            scene.encoder.validate()?;
        }

        artifact::preflight(condor.sequence_config.parallel_encoder()?.validate_frame_count)?;

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        condor.sequence_config.parallel_encoder()?.validate()?;
        let mut warnings = vec![];

        // Ensure scenes is not empty
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(ParallelEncoderError::ScenesEmpty));
        }
        if let Some(input) = &mut self.input {
            // VapourSynth inputs may cache here, which is why this is separate
            // from validate.
            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id: DETAILS.name.to_owned(),
                completion: SequenceCompletion::Custom {
                    name: DETAILS.name.to_owned(),
                    completed: 0.0,
                    total: 1.0,
                },
            }))?;
            input.clip_info()?;
            progress_tx.send(SequenceStatus::Whole(Status::Completed {
                id: DETAILS.name.to_owned(),
            }))?;
        }

        let scenes_directory = &condor.sequence_config.parallel_encoder()?.scenes_directory;
        if !scenes_directory.exists() {
            std::fs::create_dir_all(scenes_directory)?;
        }

        // Generate Photon Noise tables
        let input = self.input.as_mut().unwrap_or(&mut condor.input);
        let clip_info = input.clip_info()?;
        let transfer_function = clip_info.photon_noise_transfer_function();
        for scene in &mut condor.scenes {
            let params = scene.encoder.generate_photon_noise_table(
                clip_info.resolution.0,
                clip_info.resolution.1,
                transfer_function,
                clip_info.color_range,
            )?;

            if let Some((hashed_name, params)) = params {
                let output_directory = scenes_directory.join(DETAILS.name);
                let output = output_directory.join(format!("{}.tbl", hashed_name));
                let output_clone = output.clone();
                if !output.exists() {
                    if !output_directory.exists() {
                        std::fs::create_dir_all(&output_directory)?;
                    }
                    debug!("Writing a new photon noise table to {}", output.display());
                    write_grain_table(output, &[params])?;
                }
                scene.encoder.apply_photon_noise_parameters(&output_clone)?;
            }
        }

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        self.execute_cancel(condor, progress_tx, cancelled.into())
    }

    #[inline]
    fn execute_cancel(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: crate::utils::cancellation::Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)> {
        condor.sequence_config.parallel_encoder()?.validate()?;
        cancelled.check_forced()?;
        if cancelled.is_cancelled() {
            return Ok(((), Vec::new()));
        }
        // TODO:
        // handle subscenes

        let mut warnings = vec![];
        let config = condor.sequence_config.parallel_encoder()?.clone();
        let workers = config.workers.unwrap_or(1);
        let scenes_directory = &config.scenes_directory;
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(ParallelEncoderError::ScenesEmpty));
            return Ok(((), warnings));
        }

        let mut tasks = std::collections::VecDeque::new();
        let mut resumed = Vec::new();
        let mut resume_error = None;
        let mut metadata_changed = false;
        let scene_count = condor.scenes.len();
        for (original_index, scene) in condor.scenes.iter_mut().enumerate() {
            cancelled.check_forced()?;
            // Finish an active finite probe during Graceful, but admit neither
            // another candidate nor a new encode after it returns.
            if cancelled.is_cancelled() {
                break;
            }
            let output = scenes_directory.join(format!(
                "{}.{}",
                Self::scene_id(original_index),
                scene.encoder.output_extension()
            ));
            let data = scene.sequence_data.get_parallel_encoder_mut()?;
            let recorded =
                data.bytes.is_some() || data.started_on.is_some() || data.completed_on.is_some();
            let present = match output.symlink_metadata() {
                Ok(_) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                // Let the validator reject unreadable candidates through the same
                // metadata-withdrawal/save path as corrupt and missing chunks.
                Err(_) => true,
            };
            if present || recorded {
                let validation_progress = |completed: usize| {
                    let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
                        id: DETAILS.name.to_owned(),
                        completion: SequenceCompletion::Custom {
                            name: "validating".to_owned(),
                            completed: completed as f64,
                            total: scene_count as f64,
                        },
                    }));
                };
                validation_progress(original_index);
                let eligible = (|| -> Result<u64> {
                    ensure!(
                        data.bytes.is_some_and(|bytes| bytes > 0)
                            && data
                                .started_on
                                .zip(data.completed_on)
                                .is_some_and(|(start, end)| start <= end),
                        "artifact has no usable saved completed encoding record"
                    );
                    artifact::validate(
                        &output,
                        (scene.start_frame..scene.end_frame).len(),
                        config.validate_frame_count,
                        &cancelled,
                    )
                })();
                match eligible {
                    Ok(bytes) => {
                        metadata_changed |= data.bytes != Some(bytes);
                        data.bytes = Some(bytes);
                        resumed.push((original_index, bytes));
                    },
                    Err(error) if cancelled.is_forced() => return Err(error),
                    Err(error) => {
                        let conclusively_unusable = match output.metadata() {
                            Ok(metadata) => !metadata.is_file() || metadata.len() == 0,
                            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
                        };
                        // Probe/count/read failures withhold eligibility, not the
                        // historical record needed to retry after policy/tool repair.
                        if conclusively_unusable {
                            *data = Default::default();
                            metadata_changed |= recorded;
                        }
                        lifecycle::retain_error(&mut resume_error, error.context(format!("Scene {original_index} cannot resume {}; file preserved. Repair the probe or intentional count policy and retry; otherwise recompute in a new job or move the chunk aside and clear its saved completion record", output.display())));
                    },
                }
                validation_progress(original_index + 1);
                continue;
            }
            tasks.push_back(Task {
                original_index,
                index: tasks.len(),
                frame_indices: (scene.start_frame..scene.end_frame).collect::<Vec<_>>(),
                sub_scenes: scene.sub_scenes.clone(),
                encoder: scene.encoder.clone(),
                output,
            });
        }
        if metadata_changed {
            let saved = (condor.save_callback)(condor.as_data());
            if let Err(error) = saved {
                lifecycle::retain_error(
                    &mut resume_error,
                    error.context("Saving artifact eligibility metadata"),
                );
            }
        }
        if let Some(error) = resume_error {
            return Err(error);
        }
        for (scene, bytes) in resumed {
            cancelled.publish(|| {
                let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
                    id: scene.to_string(),
                    completion: SequenceCompletion::Custom {
                        name: "size".to_owned(),
                        completed: bytes as f64,
                        total: bytes as f64,
                    },
                }));
                Ok(())
            })?;
        }
        if tasks.is_empty() || cancelled.is_cancelled() {
            return Ok(((), warnings));
        }

        let Condor {
            input: condor_input,
            output,
            encoder,
            scenes,
            sequence_config,
            save_callback,
        } = condor;
        let input_data = condor_input.as_data();
        let input = self.input.as_mut().unwrap_or(condor_input);

        let (results_tx, results_rx) = crossbeam_channel::unbounded();
        let finished_scenes = Arc::new(crate::utils::semaphore::Semaphore::new(0));
        let mut on_results = |results: Vec<ParallelEncoderResult>| -> Result<()> {
            for encoder_result in results {
                if let Some(scene) = scenes.get_mut(encoder_result.scene)
                    && encoder_result.result.status.success()
                    && encoder_result.bytes != 0
                {
                    let parallel_encode_data = scene.sequence_data.get_parallel_encoder_mut()?;
                    parallel_encode_data.bytes = Some(encoder_result.bytes);
                    parallel_encode_data.started_on = Some(
                        encoder_result
                            .started
                            .duration_since(std::time::UNIX_EPOCH)
                            .expect("Time is valid")
                            .as_millis(),
                    );
                    parallel_encode_data.completed_on = Some(
                        encoder_result
                            .ended
                            .duration_since(std::time::UNIX_EPOCH)
                            .expect("Time is valid")
                            .as_millis(),
                    );
                }
            }

            let data = crate::models::Condor {
                input: input_data.clone(),
                output: output.as_data(),
                encoder: encoder.clone(),
                scenes: scenes.clone(),
                sequence_config: sequence_config.clone(),
            };
            (save_callback)(data)?;
            Ok(())
        };
        let mut stream = task::ResultStream {
            results_tx,
            results_rx: &results_rx,
            finished_scenes,
            on_results: &mut on_results,
        };

        let encoder_thread = ParallelEncoder::encode_tasks_with_sender(
            input,
            workers,
            tasks,
            progress_tx,
            cancelled,
            &mut stream,
            ParallelEncoderOptions {
                max_tries: config.max_tries,
                validate_frame_count: config.validate_frame_count,
            },
        );

        if let Err(e) = encoder_thread {
            error!("{}", e);
            return Err(e);
        }

        progress::drain_finished_results(&mut stream)?;

        Ok(((), warnings))
    }
}

impl Default for ParallelEncoder {
    #[inline]
    fn default() -> Self {
        Self { input: None }
    }
}

impl ParallelEncoder {
    pub const DETAILS: SequenceDetails = DETAILS;

    #[inline]
    pub fn new(input: Option<Input>) -> Self {
        ParallelEncoder { input }
    }

    #[inline]
    pub fn scene_id(index: usize) -> String {
        format!("{:>05}", index)
    }
}
