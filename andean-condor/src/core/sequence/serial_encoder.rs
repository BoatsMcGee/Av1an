use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        self,
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

use anyhow::Result;
use thiserror::Error;

use crate::{
    core::{
        Condor,
        encoder::EncodeProgress,
        input::Input,
        sequence::{Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status},
    },
    models::{
        encoder::Encoder,
        sequence::{
            SequenceConfigHandler,
            SequenceDataHandler,
            scene_detector::SceneDetectorDataHandler,
        },
    },
};

pub static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Serial Encoder",
    description: "Encodes every scene linearly with no threading or parallelism.",
    version:     "0.0.1",
};

pub struct SerialEncoder {
    pub input:            Option<Input>,
    pub encoder:          Option<Encoder>,
    pub scenes_directory: PathBuf,
}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for SerialEncoder
where
    DataHandler: SequenceDataHandler + SceneDetectorDataHandler,
    ConfigHandler: SequenceConfigHandler,
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

        let encoder = self.encoder.as_ref().map_or(&condor.encoder, |e| e);
        if let Some(input) = &self.input {
            Input::validate(&input.as_data())?;
        }

        encoder.validate()?;

        // Ensure all the scene encoders are validated
        for scene in &condor.scenes {
            scene.encoder.validate()?;
        }

        // TODO: Validate all scenes output the same codec

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];

        // Ensure scenes is not empty
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(SerialEncoderError::NoScenes));
        }
        // let encoder = self.encoder.as_ref().map_or(&condor.encoder, |e| e);
        if let Some(input) = &mut self.input {
            // Initialize input by getting clip_info. For VapourSynth inputs, this may begin
            // a lengthy caching process, hence the separation between validate and
            // initialize.
            input.clip_info()?;
        }

        if !self.scenes_directory.exists() {
            std::fs::create_dir_all(&self.scenes_directory)?;
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
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(SerialEncoderError::NoScenes));
            return Ok(((), warnings));
        }
        let encoder = self.encoder.as_ref().map_or(&condor.encoder, |e| e);
        let input: &mut Input = self.input.as_mut().map_or(&mut condor.input, |i| i);
        let clip_info = input.clip_info()?;
        let total_frames: usize = condor.scenes.iter().map(|s| s.end_frame - s.start_frame).sum();
        let frames_encoded = Arc::new(AtomicUsize::new(0));

        for (index, scene) in &mut condor.scenes.iter_mut().enumerate() {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }

            let scene_encoder = scene.encoder.clone();
            let scene_id = SerialEncoder::scene_id(index);
            let extension = encoder.output_extension();
            let output = self.scenes_directory.join(format!("{scene_id}.{extension}"));
            let scene_length = scene.end_frame - scene.start_frame;
            let scene_id_for_error = scene_id.clone();

            let (frames_tx, frames_rx) = crossbeam_channel::unbounded();
            let (encode_progress_tx, encode_progress_rx) = sync::mpsc::channel::<EncodeProgress>();
            let relay = thread::spawn({
                let frames_encoded = Arc::clone(&frames_encoded);
                let progress_tx = progress_tx.clone();
                move || -> Result<()> {
                    // Encoders report in batches, so the count advances by the
                    // difference from the last report rather than by one per event.
                    let mut last_frame = 0;
                    for progress in encode_progress_rx {
                        let advanced = progress.frame.saturating_sub(last_frame);
                        last_frame = progress.frame;
                        let completed =
                            frames_encoded.fetch_add(advanced, Ordering::Relaxed) + advanced;
                        let whole = Status::Processing {
                            id:         DETAILS.name.to_owned(),
                            completion: SequenceCompletion::Frames {
                                completed: completed as u64,
                                total:     total_frames as u64,
                            },
                        };

                        progress_tx.send(SequenceStatus::Whole(whole.clone()))?;
                        progress_tx.send(SequenceStatus::Subprocess {
                            parent: whole,
                            child:  Status::Processing {
                                id:         scene_id.clone(),
                                completion: SequenceCompletion::PassFrames {
                                    passes: progress.pass,
                                    frames: (progress.frame as u64, scene_length as u64),
                                },
                            },
                        })?;
                    }
                    Ok(())
                }
            });

            let encoder_thread = thread::spawn(move || -> Result<()> {
                let result = scene_encoder.encode_with_stream(
                    frames_rx,
                    &output,
                    encode_progress_tx,
                    &clip_info,
                )?;
                anyhow::ensure!(
                    result.status.success(),
                    "Scene {scene_id_for_error} failed to encode: {}\nSTDERR:\n{}",
                    result.status,
                    result.stderr
                );
                Ok(())
            });

            let y4m_header = input.y4m_header(Some(scene_length))?;
            frames_tx.send(Cursor::new(Vec::from(y4m_header.as_bytes())))?;
            let frame_indices = (scene.start_frame..scene.end_frame).collect::<Vec<_>>();
            input.y4m_frames(frames_tx, &frame_indices)?;

            encoder_thread
                .join()
                .map_err(|_| anyhow::anyhow!("Failed to join encoder thread"))??;
            relay.join().map_err(|_| anyhow::anyhow!("Failed to join progress thread"))??;
        }

        progress_tx.send(SequenceStatus::Whole(Status::Completed {
            id: DETAILS.name.to_owned(),
        }))?;

        Ok(((), warnings))
    }
}

impl SerialEncoder {
    pub const DETAILS: SequenceDetails = DETAILS;

    #[inline]
    pub fn new(scenes_directory: &Path) -> Self {
        SerialEncoder {
            input:            None,
            encoder:          None,
            scenes_directory: scenes_directory.to_path_buf(),
        }
    }

    #[inline]
    pub fn scene_id(index: usize) -> String {
        format!("{:>05}", index)
    }
}

#[derive(Debug, Clone, Copy, Error)]
pub enum SerialEncoderError {
    #[error("No scenes found")]
    NoScenes,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::models::{
        encoder::{EncoderBase, cli_parameter::CLIParameter},
        input::{ImportMethod, Input as InputModel},
        output::Output as OutputModel,
        scene::Scene,
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    };

    fn temp_directory(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "av1an-serial-encoder-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("temp directory created");
        directory
    }

    /// Writes a small y4m clip of `count` frames.
    fn write_test_y4m(path: &Path, count: usize) {
        const WIDTH: usize = 64;
        const HEIGHT: usize = 64;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(
            format!("YUV4MPEG2 W{WIDTH} H{HEIGHT} F30:1 Ip A1:1 C420\n").as_bytes(),
        );
        for index in 0..count {
            bytes.extend_from_slice(b"FRAME\n");
            for row in 0..HEIGHT {
                let mut line = vec![0u8; WIDTH];
                line[0] = ((index + row) % 256) as u8;
                bytes.extend_from_slice(&line);
            }
            let plane = (WIDTH / 2) * (HEIGHT / 2);
            bytes.extend(std::iter::repeat_n((index % 256) as u8, plane * 2));
        }
        fs::write(path, bytes).expect("y4m written");
    }

    /// A real encode through the Serial Encoder must finish and report
    /// progress.
    ///
    /// The old progress thread waited for exactly one report per frame, but
    /// x264 prints periodically, so it never saw the count it was waiting for
    /// and the call never returned. This runs the actual x264 binary when it is
    /// available.
    #[test]
    fn a_real_encode_finishes_and_reports_progress() {
        let Ok(_x264_binary) = which::which("x264") else {
            return; // no encoder available to test against
        };
        let directory = temp_directory("real");
        let source = directory.join("source.y4m");
        write_test_y4m(&source, 30);

        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, false);
        encoder.set_quantizer(30.0);
        // x264 prints a progress line every few frames, which is what exposed
        // the hang.
        encoder.parameters_mut().insert(
            "log-level".to_owned(),
            CLIParameter::new_string("--", "=", "info"),
        );

        let input = Input::from_video(&InputModel::Video {
            path:          source,
            import_method: ImportMethod::FFMS2 {
                index:      None,
                cache_path: None,
            },
            filters:       Vec::new(),
        })
        .expect("input opens");

        let scenes = vec![
            Scene {
                start_frame:   0,
                end_frame:     10,
                sub_scenes:    None,
                encoder:       encoder.clone(),
                sequence_data: DefaultSequenceData::default(),
            },
            Scene {
                start_frame:   10,
                end_frame:     20,
                sub_scenes:    None,
                encoder:       encoder.clone(),
                sequence_data: DefaultSequenceData::default(),
            },
            Scene {
                start_frame: 20,
                end_frame: 30,
                sub_scenes: None,
                encoder,
                sequence_data: DefaultSequenceData::default(),
            },
        ];

        let mut condor = Condor::new(
            input,
            crate::core::output::Output::new(&OutputModel {
                path:       directory.join("output.mkv"),
                tags:       Default::default(),
                video_tags: Default::default(),
            })
            .expect("output"),
            Encoder::default_from_base(&EncoderBase::X264, false),
            scenes,
            None,
            Box::new(|_: crate::models::Condor<DefaultSequenceData, DefaultSequenceConfig>| Ok(())),
        );

        let mut serial = SerialEncoder::new(&directory.join("scenes"));
        // `initialize` normally creates this.
        fs::create_dir_all(directory.join("scenes")).expect("scenes directory created");
        let (progress_tx, progress_rx) = sync::mpsc::channel();
        let collector = thread::spawn(move || {
            let mut frames = 0u64;
            let mut completed = false;
            for status in progress_rx {
                match status {
                    SequenceStatus::Whole(Status::Processing {
                        id: _,
                        completion:
                            SequenceCompletion::Frames {
                                completed, ..
                            },
                    }) => frames = frames.max(completed),
                    SequenceStatus::Whole(Status::Completed {
                        ..
                    }) => completed = true,
                    _ => (),
                }
            }
            (frames, completed)
        });

        let result = serial.execute(&mut condor, progress_tx, Arc::new(AtomicBool::new(false)));
        drop(serial);

        let (frames, completed) = collector.join().expect("collector joins");
        assert!(result.is_ok(), "the encode should succeed: {result:?}");
        // x264 batches its progress output, so the count is a lower bound on
        // the frames actually encoded; what matters is that the sequence
        // reports progress at all and then finishes.
        assert!(frames > 0, "progress should be reported, got {frames}");
        assert!(completed, "the sequence should report completion");
        for index in 0..3 {
            let output = directory.join("scenes").join(format!("{:>05}.264", index));
            assert!(output.exists(), "scene {index} should be written");
        }
    }
}
