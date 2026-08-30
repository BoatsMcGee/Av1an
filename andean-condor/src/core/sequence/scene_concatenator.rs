use std::{
    fs::File,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{self, Arc, Mutex, atomic::AtomicBool},
    thread,
};

use anyhow::{Context, Result, bail};
use av_format::{
    buffer::AccReader,
    demuxer::{Context as DemuxerContext, Event},
    muxer::{Context as MuxerContext, Writer},
    rational::{Ratio, Rational64},
};
use av_ivf::{demuxer::IvfDemuxer, muxer::IvfMuxer};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{error, trace};

use crate::{
    core::{
        Condor,
        input::Input,
        sequence::{
            Sequence,
            SequenceCompletion,
            SequenceDetails,
            SequenceStatus,
            Status,
            parallel_encoder::ParallelEncoder,
        },
    },
    models::sequence::{
        SequenceConfigHandler,
        SequenceDataHandler,
        scene_concatenator::{ConcatMethod, SceneConcatenatorConfigHandler},
    },
};

static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Scene Concatenator",
    description: "Concatenates encoded scenes into a single output file",
    version:     "0.0.1",
};

#[derive(Default)]
pub struct SceneConcatenator {}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for SceneConcatenator
where
    DataHandler: SequenceDataHandler,
    ConfigHandler: SequenceConfigHandler + SceneConcatenatorConfigHandler,
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
        let method = condor.sequence_config.scene_concatenator()?.method;
        match method {
            ConcatMethod::MKVMerge => {
                if which::which("mkvmerge").is_err() {
                    bail!(SceneConcatenatorError::MKVMergeNotInstalled);
                }
            },
            ConcatMethod::FFmpeg => {
                if which::which("ffmpeg").is_err() {
                    bail!(SceneConcatenatorError::FFmpegNotInstalled);
                }
            },
            ConcatMethod::Ivf => (),
        }

        Ok(((), vec![]))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];

        let scenes_directory = &condor.sequence_config.scene_concatenator()?.scenes_directory;
        if !scenes_directory.exists() {
            bail!(SceneConcatenatorError::ScenesDirectoryMissing {
                path: scenes_directory.clone(),
            });
        }
        if !scenes_directory.is_dir() {
            bail!(SceneConcatenatorError::ScenesDirectoryInvalid {
                path: scenes_directory.clone(),
            });
        }
        let scratch_directory = Self::scratch_directory(scenes_directory.as_path());
        if !scratch_directory.exists() {
            std::fs::create_dir_all(scratch_directory)?;
        }

        let scene_files = condor
            .scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| {
                let path = scenes_directory.join(format!(
                    "{}.{}",
                    ParallelEncoder::scene_id(index),
                    scene.encoder.output_extension()
                ));
                let exists = path.exists();

                (index, path, exists)
            })
            .filter(|(_, _, exists)| !*exists)
            .collect::<Vec<_>>();

        if !scene_files.is_empty() {
            warnings.push(anyhow::Error::new(
                SceneConcatenatorError::SceneFilesMissing {
                    scenes: scene_files.iter().map(|(index, _, _)| *index).collect(),
                },
            ));
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
        let warnings = vec![];

        let framerate = condor.input.clip_info()?.frame_rate;
        let input_path = {
            match &condor.input {
                Input::Video {
                    path, ..
                }
                | Input::VapourSynth {
                    path, ..
                } => Some(path.as_path()),
                Input::VapourSynthScript {
                    ..
                } => None, // May be invalid/Optional in the future
            }
        };
        let config = condor.sequence_config.scene_concatenator()?;
        let scenes = condor
            .scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| {
                let path = config.scenes_directory.join(format!(
                    "{}.{}",
                    ParallelEncoder::scene_id(index),
                    scene.encoder.output_extension()
                ));
                let exists = path.exists();

                (index, scene, path, exists)
            })
            .filter(|(_, _, _, exists)| *exists)
            .collect::<Vec<_>>();

        let total_frames = scenes.iter().fold(0, |acc, (_, scene, _, _)| {
            acc + (scene.end_frame - scene.start_frame)
        });
        let scene_paths = scenes.iter().map(|(_, _, path, _)| path.clone()).collect::<Vec<_>>();

        match config.method {
            ConcatMethod::MKVMerge => {
                Self::mkvmerge(
                    &config.scenes_directory,
                    &condor.output.path,
                    &scene_paths,
                    input_path,
                    framerate,
                    &progress_tx,
                    &cancelled,
                )?;
            },
            ConcatMethod::FFmpeg => {
                Self::ffmpeg(
                    &config.scenes_directory,
                    &condor.output.path,
                    &scene_paths,
                    total_frames,
                    framerate,
                    &progress_tx,
                    &cancelled,
                )?;
            },
            ConcatMethod::Ivf => {
                Self::ivf(&condor.output.path, &scene_paths, &progress_tx, &cancelled)?;
            },
        };

        progress_tx.send(SequenceStatus::Whole(Status::Completed {
            id: DETAILS.name.to_owned(),
        }))?;

        Ok(((), warnings))
    }
}

impl SceneConcatenator {
    pub const DETAILS: SequenceDetails = DETAILS;

    #[inline]
    fn send_progress(progress_tx: &sync::mpsc::Sender<SequenceStatus>, percentage: f64) {
        if !percentage.is_finite() {
            return;
        }
        let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id:         DETAILS.name.to_owned(),
            completion: SequenceCompletion::Percentage(percentage.clamp(0.0, 100.0)),
        }));
    }

    #[inline]
    pub fn mkvmerge(
        scenes_directory: &Path,
        output: &Path,
        scene_paths: &[PathBuf],
        input: Option<&Path>,
        duration: Ratio<i64>,
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<()> {
        #[cfg(windows)]
        const MAXIMUM_CHUNKS_PER_MERGE: usize = usize::MAX;
        #[cfg(not(windows))]
        const MAXIMUM_CHUNKS_PER_MERGE: usize = 512;

        // mkvmerge does not accept UNC paths on Windows
        #[cfg(windows)]
        fn fix_path<P: AsRef<Path>>(p: P) -> String {
            const UNC_PREFIX: &str = r#"\\?\"#;

            let p = p.as_ref().display().to_string();
            p.strip_prefix(UNC_PREFIX).map_or_else(
                || p.clone(),
                |path| {
                    path.strip_prefix("UNC")
                        .map_or_else(|| path.to_string(), |p2| format!("\\{p2}"))
                },
            )
        }

        #[cfg(not(windows))]
        fn fix_path<P: AsRef<Path>>(p: P) -> String {
            p.as_ref().display().to_string()
        }

        let scratch_directory = Self::scratch_directory(scenes_directory);
        if !scratch_directory.exists() {
            std::fs::create_dir_all(&scratch_directory)?;
        }
        let options_path = scratch_directory.join("options.json");
        let fixed_output = fix_path(output);
        let fixed_input = input.map(fix_path);

        let chunk_groups: Vec<Vec<PathBuf>> = scene_paths
            .chunks(MAXIMUM_CHUNKS_PER_MERGE)
            .map(|chunk| chunk.to_vec())
            .collect();

        if chunk_groups.len() == 1 {
            // Intermediate groups are unnecessary
            let options = MKVMergeOptions::new(
                &fixed_output,
                &scene_paths.iter().map(fix_path).collect::<Vec<_>>(),
                fixed_input.as_deref(),
                Some(duration),
            );
            options.write_to_disk(&options_path)?;
        } else {
            for (group_index, chunk_group) in chunk_groups.iter().enumerate() {
                if cancelled.load(sync::atomic::Ordering::Relaxed) {
                    return Ok(());
                }

                let group_options_path = scratch_directory.join(format!("{group_index:05}.json"));
                let group_output_path =
                    fix_path(scratch_directory.join(format!("{group_index:05}.mkv")));

                let group_options = MKVMergeOptions::new(
                    &group_output_path,
                    &chunk_group.iter().map(fix_path).collect::<Vec<_>>(),
                    None,
                    None,
                );
                group_options.write_to_disk(&group_options_path)?;

                let mut group_cmd = Command::new("mkvmerge");
                group_cmd.current_dir(scenes_directory);
                group_cmd.arg(format!("@./Scene Concatenator/{group_index:05}.json"));
                group_cmd.stdout(Stdio::piped());
                group_cmd.stderr(Stdio::piped());

                let mut group_child =
                    group_cmd.spawn().with_context(|| "Failed to concatenate with mkvmerge")?;
                let group_stdout = group_child.stdout.take().expect("mkvmerge should have STDOUT");
                let group_stderr = group_child.stderr.take().expect("mkvmerge should have STDERR");

                let group_stderr_output = Arc::new(Mutex::new(String::new()));
                let group_stderr_clone = Arc::clone(&group_stderr_output);
                let group_stderr_thread = thread::spawn(move || -> Result<()> {
                    let mut reader = BufReader::new(group_stderr);
                    let mut buf = Vec::with_capacity(256);
                    loop {
                        match reader.read_until(b'\n', &mut buf) {
                            Ok(0) => break,
                            Ok(_) => {
                                if let Ok(line) = simdutf8::basic::from_utf8(&buf) {
                                    group_stderr_clone
                                        .lock()
                                        .expect("mutex should acquire lock")
                                        .push_str(line);
                                }
                                buf.clear();
                            },
                            Err(e) => return Err(e.into()),
                        }
                    }
                    Ok(())
                });

                let (cancelled, group_stdout_output) = Self::stream_mkvmerge_progress(
                    group_stdout,
                    progress_tx,
                    Some((group_index, chunk_groups.len())),
                    cancelled,
                )?;
                if cancelled {
                    group_child.kill()?;
                    group_child.wait()?;
                    return Ok(());
                }

                let group_status =
                    group_child.wait().with_context(|| "Failed to wait for mkvmerge")?;
                group_stderr_thread
                    .join()
                    .map_err(|_| anyhow::anyhow!("mkvmerge STDERR thread panicked"))??;
                if !group_status.success() {
                    let error = SceneConcatenatorError::MkvmergeFailed {
                        status: group_status,
                        stdout: group_stdout_output,
                        stderr: group_stderr_output
                            .lock()
                            .expect("mutex should acquire lock")
                            .clone(),
                    };
                    error!("{}", error);
                    bail!(error);
                }
            }

            let chunk_group_options_names = chunk_groups
                .iter()
                .enumerate()
                .map(|(index, _)| format!("{index:05}.mkv"))
                .collect::<Vec<_>>();
            let options = MKVMergeOptions::new(
                &fixed_output,
                &chunk_group_options_names,
                fixed_input.as_deref(),
                Some(duration),
            );
            options.write_to_disk(&options_path)?;
        }

        let mut cmd = Command::new("mkvmerge");
        cmd.arg(format!("@{}", fix_path(options_path)));
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut child = cmd.spawn().with_context(|| "Failed to spawn mkvmerge")?;
        let stdout = child.stdout.take().expect("mkvmerge should have STDOUT");
        let stderr = child.stderr.take().expect("mkvmerge should have STDERR");

        let stderr_output = Arc::new(Mutex::new(String::new()));
        let stderr_clone = Arc::clone(&stderr_output);
        let stderr_thread = thread::spawn(move || -> Result<()> {
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::with_capacity(256);
            loop {
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Ok(line) = simdutf8::basic::from_utf8(&buf) {
                            stderr_clone.lock().expect("mutex should acquire lock").push_str(line);
                        }
                        buf.clear();
                    },
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(())
        });

        let (cancelled, stdout_output) =
            Self::stream_mkvmerge_progress(stdout, progress_tx, None, cancelled)?;
        if cancelled {
            child.kill()?;
            child.wait()?;
            return Ok(());
        }

        let status = child.wait().with_context(|| "Failed to wait for mkvmerge")?;
        stderr_thread
            .join()
            .map_err(|_| anyhow::anyhow!("mkvmerge STDERR thread panicked"))??;
        if !status.success() {
            let error = SceneConcatenatorError::MkvmergeFailed {
                status,
                stdout: stdout_output,
                stderr: stderr_output.lock().expect("mutex should acquire lock").clone(),
            };
            error!("{}", error);
            bail!(error);
        }

        Ok(())
    }

    /// Streams mkvmerge STDOUT, parse `Progress: N%`, and emit progress
    #[inline]
    fn stream_mkvmerge_progress(
        stdout: impl std::io::Read,
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        group: Option<(usize, usize)>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<(bool, String)> {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::with_capacity(256);
        let mut output = String::new();

        loop {
            if cancelled.load(sync::atomic::Ordering::Relaxed) {
                return Ok((true, output));
            }

            match reader.read_until(b'\r', &mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    if let Ok(line) = simdutf8::basic::from_utf8(&buf) {
                        output.push_str(line);
                        if let Some(percentage) = Self::parse_mkvmerge_progress(line) {
                            let percentage = match group {
                                Some((group_index, total_groups)) => {
                                    (group_index as f64 + percentage / 100.0) / total_groups as f64
                                        * 100.0
                                },
                                None => percentage,
                            };
                            Self::send_progress(progress_tx, percentage);
                        }
                    }
                    buf.clear();
                },
                Err(e) => return Err(e.into()),
            }
        }

        Ok((false, output))
    }

    /// Parses a single mkvmerge progress line (e.g. `Progress: 33%`)
    #[inline]
    fn parse_mkvmerge_progress(line: &str) -> Option<f64> {
        let line = line.trim();
        let progress = line.rfind("Progress: ")?;
        let (_, progress) = line.split_at(progress + "Progress: ".len());
        progress.trim().strip_suffix('%')?.trim().parse().ok()
    }

    #[inline]
    pub fn ffmpeg(
        scenes_directory: &Path,
        output: &Path,
        scene_paths: &[PathBuf],
        total_frames: usize,
        framerate: Ratio<i64>,
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<()> {
        let scratch_directory = scenes_directory.join("Scene Concatenator");
        let concat_file_path = scratch_directory.join("concat.txt");
        let concat_file = {
            let mut contents = String::with_capacity(24 * scene_paths.len());

            for scene_path in scene_paths {
                let fixed_path = scene_path
                    .display()
                    .to_string()
                    .replace('\\', r"\\")
                    .replace(' ', r"\ ")
                    .replace('\'', r"\'");
                contents.push_str("file ");
                contents.push_str(&fixed_path);
                contents.push('\n');
            }

            contents
        };
        File::create(&concat_file_path)?.write_all(concat_file.as_bytes())?;

        let mut cmd = Command::new("ffmpeg");

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        cmd.args(["-y", "-hide_banner", "-loglevel", "info", "-f", "concat", "-safe", "0", "-i"]);
        cmd.arg(concat_file_path);
        // todo: copy from input -i
        cmd.args(["-map", "0"]);
        // copy from input -i
        // cmd.args(["-map", "1", "-map", "-1:v"]);
        cmd.args(["-c", "copy"]);
        cmd.arg(output);

        let mut child = cmd.spawn().with_context(|| "Failed to concatenate with FFmpeg")?;
        let stdout = child.stdout.take().expect("FFmpeg should have STDOUT");
        let stderr = child.stderr.take().expect("FFmpeg should have STDERR");

        let stdout_output = Arc::new(Mutex::new(String::new()));
        let stdout_clone = Arc::clone(&stdout_output);
        let stdout_thread = thread::spawn(move || -> Result<()> {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::with_capacity(256);
            loop {
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Ok(line) = simdutf8::basic::from_utf8(&buf) {
                            stdout_clone.lock().expect("mutex should acquire lock").push_str(line);
                        }
                        buf.clear();
                    },
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(())
        });

        let mut reader = BufReader::new(stderr);
        let mut buf = Vec::with_capacity(256);
        let mut stderr_output = String::new();

        loop {
            if cancelled.load(sync::atomic::Ordering::Relaxed) {
                child.kill()?;
                child.wait()?;
                return Ok(());
            }

            match reader.read_until(b'\r', &mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    if let Ok(line) = simdutf8::basic::from_utf8(&buf) {
                        stderr_output.push_str(line);
                        if let Some(percentage) =
                            Self::parse_ffmpeg_progress(line, total_frames, framerate)
                        {
                            Self::send_progress(progress_tx, percentage);
                        }
                    }
                    buf.clear();
                },
                Err(e) => return Err(e.into()),
            }
        }

        let status = child.wait().with_context(|| "Failed to wait for FFmpeg")?;
        stdout_thread
            .join()
            .map_err(|_| anyhow::anyhow!("FFmpeg STDOUT thread panicked"))??;
        if !status.success() {
            let error = SceneConcatenatorError::FfmpegFailed {
                status,
                stdout: stdout_output.lock().expect("mutex should acquire lock").clone(),
                stderr: stderr_output,
            };
            error!("{}", error);
            bail!(error);
        }

        Ok(())
    }

    /// Parses a single ffmpeg progress line (e.g. `frame=23310 fps=4835 ...
    /// time=00:15:46.57 ...`)
    #[inline]
    fn parse_ffmpeg_progress(
        line: &str,
        total_frames: usize,
        framerate: Ratio<i64>,
    ) -> Option<f64> {
        if total_frames == 0 {
            return None;
        }

        let current_frames = Self::parse_ffmpeg_frames(line).or_else(|| {
            Self::parse_ffmpeg_time(line).map(|seconds| Self::time_to_frames(seconds, framerate))
        })?;

        Some(current_frames as f64 / total_frames as f64 * 100.0)
    }

    #[inline]
    fn parse_ffmpeg_frames(line: &str) -> Option<u64> {
        let frame = line.split_whitespace().find(|token| token.starts_with("frame="))?;
        frame.strip_prefix("frame=")?.parse().ok()
    }

    #[inline]
    fn parse_ffmpeg_time(line: &str) -> Option<f64> {
        let time = line.split_whitespace().find(|token| token.starts_with("time="))?;
        let time = time.strip_prefix("time=")?;

        // ffmpeg prints time as HH:MM:SS.ms or N.NN
        let seconds = match time.split(':').collect::<Vec<_>>().as_slice() {
            [hours, minutes, seconds] => {
                hours.parse::<f64>().ok()?.mul_add(3600.0, 0.0)
                    + minutes.parse::<f64>().ok()?.mul_add(60.0, 0.0)
                    + seconds.parse::<f64>().ok()?
            },
            [seconds] => seconds.parse::<f64>().ok()?,
            _ => return None,
        };

        Some(seconds)
    }

    #[inline]
    fn time_to_frames(seconds: f64, framerate: Ratio<i64>) -> u64 {
        // framerate = numer/denom frames per second
        (seconds * *framerate.numer() as f64 / *framerate.denom() as f64) as u64
    }

    #[inline]
    pub fn ivf(
        output: &Path,
        scene_paths: &[PathBuf],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<()> {
        let output = File::create(output)?;
        let mut muxer = MuxerContext::new(IvfMuxer::new(), Writer::new(output));
        let global_info = {
            let acc = AccReader::new(std::fs::File::open(&scene_paths[0])?);
            let mut demuxer = DemuxerContext::new(IvfDemuxer::new(), acc);
            demuxer.read_headers()?;
            // attempt to set the duration correctly
            let duration = demuxer.info.duration.unwrap_or(0)
                + scene_paths.iter().skip(1).try_fold(0u64, |sum, file| -> anyhow::Result<_> {
                    let acc = AccReader::new(std::fs::File::open(file)?);
                    let mut demuxer = DemuxerContext::new(IvfDemuxer::new(), acc);

                    demuxer.read_headers()?;
                    Ok(sum + demuxer.info.duration.unwrap_or(0))
                })?;

            let mut info = demuxer.info;
            info.duration = Some(duration);
            info
        };

        muxer.set_global_info(global_info)?;
        muxer.configure()?;
        muxer.write_header()?;

        let total_scenes = scene_paths.len();
        let mut pos_offset: usize = 0;
        for (index, file) in scene_paths.iter().enumerate() {
            if cancelled.load(sync::atomic::Ordering::Relaxed) {
                return Ok(());
            }

            Self::send_progress(
                progress_tx,
                (index + 1) as f64 / total_scenes as f64 * 100.0,
            );

            let mut last_pos: usize = 0;
            let input = std::fs::File::open(file)?;

            let acc = AccReader::new(input);

            let mut demuxer = DemuxerContext::new(IvfDemuxer::new(), acc);
            demuxer.read_headers()?;

            trace!("global info: {:#?}", demuxer.info);

            loop {
                match demuxer.read_event() {
                    Ok(event) => match event {
                        Event::MoreDataNeeded(sz) => panic!("needed more data: {sz} bytes"),
                        Event::NewStream(s) => panic!("new stream: {s:?}"),
                        Event::NewPacket(mut packet) => {
                            if let Some(p) = packet.pos.as_mut() {
                                last_pos = *p;
                                *p += pos_offset;
                            }

                            trace!("received packet with pos: {:?}", packet.pos);
                            muxer.write_packet(Arc::new(packet))?;
                        },
                        Event::Continue => {
                            // do nothing
                        },
                        Event::Eof => {
                            trace!("EOF received.");
                            break;
                        },
                        _ => unimplemented!(),
                    },
                    Err(e) => {
                        error!("{:?}", e);
                        break;
                    },
                }
            }
            pos_offset += last_pos + 1;
        }

        muxer.write_trailer()?;

        Ok(())
    }

    pub(crate) fn scratch_directory(scenes_directory: &Path) -> PathBuf {
        scenes_directory.join("Scene Concatenator")
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct MKVMergeOptions {
    output:           String,
    audio:            Option<String>,
    default_duration: Option<String>,
    chunks:           Vec<String>,
}

impl MKVMergeOptions {
    pub fn new(
        output: &str,
        chunks: &[String],
        audio: Option<&str>,
        default_duration: Option<Rational64>,
    ) -> Self {
        let default_duration = default_duration
            .map(|output_fps| format!("0:{}/{}fps", output_fps.numer(), output_fps.denom()));

        MKVMergeOptions {
            output: output.to_string(),
            audio: audio.map(|a| a.to_string()),
            default_duration,
            chunks: chunks.to_vec(),
        }
    }

    pub fn write_to_disk(&self, path: &Path) -> Result<()> {
        let args = self.generate_args();
        let mut file = File::create(path)?;
        file.write_all(serde_json::to_string_pretty(&args)?.as_bytes())?;
        Ok(())
    }

    pub fn generate_args(&self) -> Vec<&str> {
        let mut args = vec!["-o", &self.output];
        if let Some(audio) = &self.audio {
            args.push("--no-video");
            args.push(audio);
        }
        if let Some(default_duration) = &self.default_duration {
            args.push("--default-duration");
            args.push(default_duration);
        }
        args.push("[");
        for chunk in &self.chunks {
            args.push(chunk);
        }
        args.push("]");
        args
    }
}

#[derive(Debug, Error)]
pub enum SceneConcatenatorError {
    #[error("mkvmerge not installed")]
    MKVMergeNotInstalled,
    #[error("FFmpeg not installed")]
    FFmpegNotInstalled,
    #[error("Missing scene files: {scenes:?}")]
    SceneFilesMissing { scenes: Vec<usize> },
    #[error("Missing scenes directory: {path}")]
    ScenesDirectoryMissing { path: PathBuf },
    #[error("Scenes directory is not a directory: {path}")]
    ScenesDirectoryInvalid { path: PathBuf },
    #[error("Failed to concatenate with mkvmerge: {status}\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}")]
    MkvmergeFailed {
        status: std::process::ExitStatus,
        stdout: String,
        stderr: String,
    },
    #[error("Failed to concatenate with ffmpeg: {status}\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}")]
    FfmpegFailed {
        status: std::process::ExitStatus,
        stdout: String,
        stderr: String,
    },
}
