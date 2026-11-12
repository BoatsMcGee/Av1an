use std::{
    ffi::OsString,
    fs::File,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{self, Arc, Mutex, atomic::AtomicBool},
    thread,
};

use crate::models::sequence::{
    parallel_encoder::{ParallelEncoderConfigHandler, ParallelEncoderDataHandler},
    provenance,
    target_quality::{TargetQualityConfigHandler, TargetQualityDataHandler},
};
use crate::{
    core::process::{InterruptedOutput, OwnedProcess},
    utils::cancellation::Cancellation,
};
use anyhow::{Context, Result, bail};
use av_format::{
    buffer::AccReader,
    demuxer::{Context as DemuxerContext, Event},
    muxer::{Context as MuxerContext, Writer},
    rational::Ratio,
};
use av_ivf::{demuxer::IvfDemuxer, muxer::IvfMuxer};
use thiserror::Error;
use tracing::{error, trace};

use crate::{
    core::{
        Condor,
        input::Input,
        sequence::{
            Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status,
            parallel_encoder::ParallelEncoder,
        },
    },
    models::sequence::{
        SequenceConfigHandler, SequenceDataHandler,
        scene_concatenator::{
            Chapters, ConcatMethod, FfmpegConfig, FfmpegTrack, FfmpegTrackMap, FfmpegTrackOptions,
            Metadata, MkvmergeConfig, MkvmergeExtraInput, SceneConcatenatorConfigHandler,
            mkvmerge::tags_xml,
        },
    },
};

mod ffmpeg;
mod ivf;
mod mkvmerge;
mod source;
#[cfg(test)]
mod x01_tests;

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Scene Concatenator",
    description: "Concatenates encoded scenes into a single output file",
    version: "0.0.1",
};
#[derive(Default)]
pub struct SceneConcatenator {}
impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for SceneConcatenator
where
    DataHandler: SequenceDataHandler + ParallelEncoderDataHandler + TargetQualityDataHandler,
    ConfigHandler: SequenceConfigHandler
        + SceneConcatenatorConfigHandler
        + ParallelEncoderConfigHandler
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
        Self::admit_final(condor)?;
        let concatenator = condor.sequence_config.scene_concatenator()?;
        let mut warnings = Vec::new();
        match &concatenator.method {
            ConcatMethod::MKVMerge(mkvmerge) => {
                if which::which("mkvmerge").is_err() {
                    bail!(SceneConcatenatorError::MKVMergeNotInstalled);
                }
                warnings.extend(mkvmerge.validate()?);
            },
            ConcatMethod::FFmpeg(ffmpeg) => {
                if which::which("ffmpeg").is_err() {
                    bail!(SceneConcatenatorError::FFmpegNotInstalled);
                }
                let has_input = matches!(
                    condor.input,
                    Input::Video { .. } | Input::VapourSynth { .. }
                );
                if !has_input && ffmpeg.extra_inputs.is_empty() {
                    warnings.push(SceneConcatenatorError::NoFfmpegInputs.into());
                }
                warnings.extend(ffmpeg.validate()?);
            },
            ConcatMethod::Ivf => (),
        }

        Ok(((), warnings))
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
        self.execute_cancel(condor, progress_tx, cancelled.into())
    }

    #[inline]
    fn execute_cancel(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let cancelled = cancelled.admit()?;
        Self::admit_final(condor)?;
        let warnings = vec![];

        let root = condor.input.as_data();
        let pe = condor.sequence_config.parallel_encoder()?;
        let tq = condor.sequence_config.target_quality()?.as_ref();
        let input = pe.input.as_ref().unwrap_or(&root);
        let probe = provenance::probe_input(&root, pe, tq);
        let metric = tq.and_then(|c| c.metric_input.as_ref());
        let mut admission = provenance::Admission::default();
        for (index, scene) in condor.scenes.iter().enumerate() {
            let config = condor.sequence_config.scene_concatenator()?;
            let path = config.scenes_directory.join(format!(
                "{}.{}",
                ParallelEncoder::scene_id(index),
                scene.encoder.output_extension()
            ));
            admission.admit_encode(input, probe, metric, scene, tq, &path, index)?;
            super::parallel_encoder::artifact::validate(
                &path,
                scene.end_frame - scene.start_frame,
                false, // PE owns decode/count validation; concat checks only identity/nonempty.
                &cancelled,
            )?;
        }

        let framerate = condor.input.clip_info()?.frame_rate;
        let input_path = {
            match &condor.input {
                Input::Video { path, .. } | Input::VapourSynth { path, .. } => Some(path.as_path()),
                Input::VapourSynthScript { .. } => None, // May be invalid/Optional in the future
            }
        };
        let config = condor.sequence_config.scene_concatenator()?;
        let destination = config.effective_output(&condor.output.path);
        let destination = std::env::current_dir()?.join(destination);
        // Native muxers can require a nonexistent path. A private owned sibling
        // directory reserves its namespace without an unlink/recreate race or prompt.
        let staging = tempfile::Builder::new()
            .prefix(".condor-output-")
            .tempdir_in(destination.parent().context("Output has no parent")?)?;
        let staged_path = staging.path().join(format!(
            "output.{}",
            destination
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or_else(|| config.method.extension())
        ));
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

        match &config.method {
            ConcatMethod::MKVMerge(mkvmerge) => {
                Self::mkvmerge_cancel(
                    &config.scenes_directory,
                    &staged_path,
                    &scene_paths,
                    input_path,
                    framerate,
                    Some(mkvmerge),
                    &progress_tx,
                    &cancelled,
                )?;
            },
            ConcatMethod::FFmpeg(ffmpeg) => {
                Self::ffmpeg_cancel(
                    &config.scenes_directory,
                    &staged_path,
                    &scene_paths,
                    input_path,
                    total_frames,
                    framerate,
                    Some(ffmpeg),
                    &progress_tx,
                    &cancelled,
                )?;
            },
            ConcatMethod::Ivf => {
                Self::ivf_cancel(&staged_path, &scene_paths, &progress_tx, &cancelled)?;
            },
        };

        cancelled.publish(|| {
            Self::admit_final(condor)?;
            crate::core::output::persist_output(
                tempfile::TempPath::try_from_path(staged_path)?,
                &destination,
                condor.output.overwrite,
            )?;
            progress_tx.send(SequenceStatus::Whole(Status::Completed {
                id: DETAILS.name.to_owned(),
            }))?;
            Ok(())
        })?;

        Ok(((), warnings))
    }
}
impl SceneConcatenator {
    pub const DETAILS: SequenceDetails = DETAILS;

    /// Declared root/stage/mux sources and every configured encoded chunk, including missing ones.
    /// Inline script reads and custom inputs omitted by the config handler cannot be inferred.
    #[inline]
    pub fn protected_sources<Data, Config: SceneConcatenatorConfigHandler>(
        input: &crate::models::input::Input,
        config: &Config,
        scenes: &[crate::models::scene::Scene<Data>],
    ) -> Result<Vec<PathBuf>> {
        let concat = config.scene_concatenator()?;
        let mut paths = config.protected_sources();
        paths.extend(input.path().map(Path::to_path_buf));
        paths.extend(concat.mux_sources());
        paths.extend(scenes.iter().enumerate().map(|(index, scene)| {
            concat.scenes_directory.join(format!(
                "{}.{}",
                ParallelEncoder::scene_id(index),
                scene.encoder.output_extension()
            ))
        }));
        Ok(paths)
    }

    fn admit_final<
        Data: SequenceDataHandler,
        Config: SequenceConfigHandler + SceneConcatenatorConfigHandler,
    >(
        condor: &Condor<Data, Config>,
    ) -> Result<()> {
        let sources = Self::protected_sources(
            &condor.input.as_data(),
            &condor.sequence_config,
            &condor.scenes,
        )?;
        let config = condor.sequence_config.scene_concatenator()?;
        crate::core::output::admit_output(
            config.effective_output(&condor.output.path),
            &sources,
            condor.output.overwrite,
            true,
        )
    }

    #[inline]
    fn send_progress(progress_tx: &sync::mpsc::Sender<SequenceStatus>, percentage: f64) {
        if !percentage.is_finite() {
            return;
        }
        let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id: DETAILS.name.to_owned(),
            completion: SequenceCompletion::Percentage(percentage.clamp(0.0, 100.0)),
        }));
    }

    pub(crate) fn scratch_directory(scenes_directory: &Path) -> PathBuf {
        scenes_directory.join("Scene Concatenator")
    }
}
#[derive(Debug, Error)]
pub enum SceneConcatenatorError {
    #[error("mkvmerge not installed")]
    MKVMergeNotInstalled,
    #[error("FFmpeg not installed")]
    FFmpegNotInstalled,
    #[error(
        "FFmpeg output settings configure no additional inputs; the output will contain only the \
         encoded video"
    )]
    NoFfmpegInputs,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        input::{Input as InputModel, VapourSynthScriptSource},
        output::Output as OutputModel,
        scene::Scene,
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    };

    fn c06_job(
        directory: &Path,
        output: &Path,
    ) -> Condor<DefaultSequenceData, DefaultSequenceConfig> {
        let input = Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Text("import vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=16,length=2,format=vs.YUV420P8).set_output()".into()),
            variables: Default::default(), index: 0, filters: vec![], stream_concurrently: false,
        }).expect("finite native input");
        let output: OutputModel = serde_json::from_value(
            serde_json::json!({"path":output,"tags":{},"video_tags":{},"overwrite":true}),
        )
        .expect("output model");
        let mut config = DefaultSequenceConfig::default();
        config.scene_concatenator.method = ConcatMethod::Ivf;
        config.scene_concatenator.scenes_directory = directory.into();
        Condor::new(
            input,
            crate::core::output::Output::new(&output).expect("output"),
            Default::default(),
            vec![],
            Some(config),
            Box::new(|_| Ok(())),
        )
    }

    #[test]
    fn c06_final_validation_rejects_source_alias() {
        let dir = tempfile::tempdir().expect("fixture");
        let source = dir.path().join("source.mkv");
        std::fs::write(&source, b"source sentinel").expect("source");
        let mut job = c06_job(dir.path(), &source);
        job.sequence_config.scene_detector.input = Some(InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(source.clone()),
            variables: Default::default(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        });
        assert!(
            SceneConcatenator::default().validate(&mut job).is_err(),
            "declared source must not be admitted as output"
        );
        assert_eq!(std::fs::read(source).expect("source"), b"source sentinel");
    }

    #[test]
    fn c06_default_config_enumerates_each_declared_stage_and_actual_destination() {
        use crate::models::sequence::{
            noise_detector::NoiseDetectorConfig, quality_check::QualityCheckConfig,
            target_quality::TargetQualityConfig,
        };
        for slot in [
            "root", "scd", "parallel", "noise", "tq", "metric", "qc", "chunk", "override",
            "existing",
        ] {
            let dir = tempfile::tempdir().expect("fixture");
            let source = dir.path().join("source");
            std::fs::write(&source, b"protected source").expect("source");
            let mut job = c06_job(dir.path(), &source);
            let input = InputModel::VapourSynthScript {
                source: VapourSynthScriptSource::Path(source.clone()),
                variables: Default::default(),
                index: 0,
                filters: vec![],
                stream_concurrently: false,
            };
            let config = &mut job.sequence_config;
            match slot {
                "root" => {
                    if let Input::VapourSynthScript { source: root, .. } = &mut job.input {
                        *root = VapourSynthScriptSource::Path(source.clone());
                    }
                },
                "scd" => config.scene_detector.input = Some(input),
                "parallel" => config.parallel_encoder.input = Some(input),
                "noise" => {
                    config.noise_detector = Some(NoiseDetectorConfig {
                        input: Some(input),
                        ..Default::default()
                    });
                },
                "tq" => {
                    config.target_quality = Some(TargetQualityConfig {
                        input: Some(input),
                        ..Default::default()
                    });
                },
                "metric" => {
                    config.target_quality = Some(TargetQualityConfig {
                        metric_input: Some(input),
                        ..Default::default()
                    });
                },
                "qc" => {
                    config.quality_check = Some(QualityCheckConfig {
                        input: Some(input),
                        ..Default::default()
                    });
                },
                "chunk" => {
                    job.scenes.push(Scene {
                        start_frame: 0,
                        end_frame: 2,
                        sub_scenes: None,
                        encoder: Default::default(),
                        sequence_data: Default::default(),
                    });
                    job.output.path = dir.path().join(format!(
                        "{}.{}",
                        ParallelEncoder::scene_id(0),
                        job.scenes[0].encoder.output_extension()
                    ));
                },
                "override" => {
                    config.scene_detector.input = Some(input);
                    config.scene_concatenator.output = Some(source.clone());
                    job.output.path = dir.path().join("fallback.ivf");
                },
                "existing" => job.output.overwrite = false,
                _ => unreachable!(),
            }
            assert!(
                SceneConcatenator::default().validate(&mut job).is_err(),
                "{slot}"
            );
            assert_eq!(std::fs::read(&source).expect("source"), b"protected source");
        }
    }

    #[test]
    fn c06_failed_final_concat_preserves_old_output() {
        let dir = tempfile::tempdir().expect("fixture");
        let output = dir.path().join("out.ivf");
        std::fs::write(&output, b"old final sentinel").expect("old final");
        let mut job = c06_job(dir.path(), &output);
        job.scenes.push(Scene {
            start_frame: 0,
            end_frame: 2,
            sub_scenes: None,
            encoder: Default::default(),
            sequence_data: Default::default(),
        });
        let chunk = dir.path().join(format!(
            "{}.{}",
            ParallelEncoder::scene_id(0),
            job.scenes[0].encoder.output_extension()
        ));
        std::fs::write(chunk, b"invalid IVF header").expect("invalid chunk");
        let (tx, rx) = sync::mpsc::channel();
        assert!(
            SceneConcatenator::default()
                .execute_cancel(&mut job, tx, Cancellation::default())
                .is_err()
        );
        assert_eq!(
            std::fs::read(&output).expect("old final"),
            b"old final sentinel"
        );
        assert!(
            !rx.try_iter()
                .any(|status| matches!(status, SequenceStatus::Whole(Status::Completed { .. })))
        );
    }

    #[cfg(unix)]
    #[test]
    fn c06_final_native_publication_and_races() {
        use std::os::unix::fs::PermissionsExt;
        let ffmpeg = which::which("ffmpeg").expect("required FFmpeg CLI");
        let python = which::which("python3").expect("required Python fixture CLI");
        for case in [
            "native",
            "native-new",
            "failure",
            "publication",
            "race",
            "alias",
            "force",
            "graceful",
        ] {
            let dir = tempfile::tempdir().expect("fixture");
            let root = std::fs::canonicalize(dir.path()).expect("root");
            let reference = root.join("reference.mkv");
            let output = Command::new(&ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=size=32x16:rate=24",
                    "-frames:v",
                    "2",
                    "-c:v",
                    "ffv1",
                    "-f",
                    "matroska",
                ])
                .arg(&reference)
                .output()
                .expect("real fixture encoder");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let old = Command::new(&ffmpeg)
                .args(["-v", "error", "-i"])
                .arg(&reference)
                .args(["-frames:v", "1", "-c", "copy"])
                .arg(root.join("old.mkv"))
                .output()
                .expect("valid distinct old output");
            assert!(
                old.status.success(),
                "{}",
                String::from_utf8_lossy(&old.stderr)
            );
            let bin = root.join("bin");
            std::fs::create_dir_all(&bin).expect("bin");
            let script = bin.join("ffmpeg");
            std::fs::write(
                &script,
                format!(
                    r#"#!{}
import os,sys,time
from pathlib import Path
r=Path(os.environ['C06_ROOT'])
c=os.environ['C06_CASE']
out=Path(sys.argv[-1])
(r/'actual-staging').write_text(str(out))
out.write_bytes(b'partial native output')
(r/'partial-written').write_bytes(out.read_bytes())
if c=='failure': sys.exit(42)
if c=='publication':
 out.unlink()
 sys.exit(0)
if c in ('force','graceful'):
 (r/'active').write_text('ready')
 deadline=time.monotonic()+8
 while not (r/'release').exists() and time.monotonic()<deadline: time.sleep(0.005)
 if not (r/'release').exists(): sys.exit(43)
if c=='race': (r/'out.mkv').write_bytes(b'racer owns final')
if c=='alias':
 (r/'out.mkv').unlink()
 os.link(r/'protected',r/'out.mkv')
out.write_bytes((r/'reference.mkv').read_bytes())
"#,
                    python.display()
                ),
            )
            .expect("finite mux process seam");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
                .expect("executable");
            let path = if case.starts_with("native") {
                ffmpeg.parent().expect("binary directory")
            } else {
                bin.as_path()
            };
            let child = Command::new(std::env::current_exe().expect("test executable"))
                .args([
                    "--exact",
                    "core::sequence::scene_concatenator::tests::c06_final_native_child",
                    "--nocapture",
                ])
                .env("C06_ROOT", &root)
                .env("C06_CASE", case)
                .env("PATH", path)
                .output()
                .expect("isolated owning Sequence");
            assert!(
                child.status.success(),
                "{case}\n{}\n{}",
                String::from_utf8_lossy(&child.stdout),
                String::from_utf8_lossy(&child.stderr)
            );
            eprintln!(
                "C06 final native case={case}: {}",
                String::from_utf8_lossy(&child.stdout)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn c06_final_native_child() {
        use std::time::{Duration, Instant};
        let Some(root) = std::env::var_os("C06_ROOT").map(PathBuf::from) else {
            return;
        };
        let case = std::env::var("C06_CASE").expect("case");
        let final_path = root.join("out.mkv");
        let reference = std::fs::read(root.join("reference.mkv")).expect("two-frame encoded scene");
        let old = std::fs::read(root.join("old.mkv")).expect("one-frame old output");
        if case != "race" && case != "native-new" {
            std::fs::write(&final_path, &old).expect("old valid final");
        }
        std::fs::write(root.join("protected"), b"protected original bytes").expect("source");
        let mut job = c06_job(&root, &root.join("fallback-must-not-be-written.mkv"));
        job.output.overwrite = case != "race" && case != "native-new";
        job.sequence_config.scene_concatenator.output = Some(final_path.clone());
        job.sequence_config.scene_concatenator.method =
            ConcatMethod::FFmpeg(FfmpegConfig::default());
        job.sequence_config.scene_detector.input = Some(InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(root.join("protected")),
            variables: Default::default(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        });
        job.scenes.push(Scene {
            start_frame: 0,
            end_frame: 2,
            sub_scenes: None,
            encoder: Default::default(),
            sequence_data: Default::default(),
        });
        let chunk = root.join(format!(
            "{}.{}",
            ParallelEncoder::scene_id(0),
            job.scenes[0].encoder.output_extension()
        ));
        std::fs::write(chunk, &reference).expect("real encoded scene");
        job.sequence_config.parallel_encoder.scenes_directory = root.clone();
        crate::core::sequence::c14_tests::current_encoding_fixture(&mut job);
        let token = Cancellation::default();
        let controller = (case == "force" || case == "graceful").then(|| {
            let root = root.clone();
            let token = token.clone();
            let force = case == "force";
            thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !root.join("active").exists() && Instant::now() < deadline {
                    thread::yield_now();
                }
                let observed = root.join("active").exists();
                token.request().expect("graceful");
                if force || !observed {
                    token.request().expect("force");
                }
                if !force {
                    std::fs::write(root.join("release"), b"finish admitted concat")
                        .expect("release");
                }
                assert!(observed, "bounded active process handshake");
            })
        });
        let mut concatenator = SceneConcatenator::default();
        let (tx, rx) = sync::mpsc::channel();
        concatenator.validate(&mut job).expect("final preflight");
        concatenator.initialize(&mut job, tx.clone()).expect("owning scratch setup");
        let result = concatenator.execute_cancel(&mut job, tx, token);
        if let Some(controller) = controller {
            controller.join().expect("controller joined");
        }
        let success = case.starts_with("native") || case == "graceful";
        let completed = rx
            .try_iter()
            .any(|status| matches!(status, SequenceStatus::Whole(Status::Completed { .. })));
        assert_eq!(
            completed, success,
            "owning boundary must not complete on admission/publication failure"
        );
        if success {
            result.expect("successful mux and publication");
            assert_ne!(
                std::fs::read(&final_path).expect("replacement"),
                old,
                "successful final mux must actually replace the distinct old output"
            );
            if case.starts_with("native") {
                let probe = Command::new("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-select_streams",
                        "v:0",
                        "-count_frames",
                        "-show_entries",
                        "stream=nb_read_frames",
                        "-of",
                        "csv=p=0",
                    ])
                    .arg(&final_path)
                    .output()
                    .expect("required FFprobe CLI");
                assert!(probe.status.success());
                assert_eq!(String::from_utf8_lossy(&probe.stdout).trim(), "2");
            } else {
                assert_eq!(std::fs::read(&final_path).expect("final"), reference);
            }
        } else {
            let error = result.expect_err("failure or late conflict");
            if case == "force" {
                assert_eq!(
                    error.downcast_ref::<crate::utils::cancellation::Cancelled>(),
                    Some(&crate::utils::cancellation::Cancelled::Forced)
                );
            }
            if case == "failure" {
                let Some(SceneConcatenatorError::FfmpegFailed { status, .. }) =
                    error.downcast_ref::<SceneConcatenatorError>()
                else {
                    panic!("typed FFmpeg failure: {error:#}")
                };
                assert_eq!(status.code(), Some(42));
            }
            if case == "publication" {
                assert!(
                    error.is::<tempfile::PathPersistError>(),
                    "actual persist failure: {error:#}"
                );
            }
            let expected = match case.as_str() {
                "race" => b"racer owns final".as_slice(),
                "alias" => b"protected original bytes".as_slice(),
                _ => old.as_slice(),
            };
            assert_eq!(
                std::fs::read(&final_path).expect("final preserved"),
                expected
            );
        }
        assert_eq!(
            std::fs::read(root.join("protected")).expect("source preserved"),
            b"protected original bytes"
        );
        assert!(
            !job.output.path.exists(),
            "actual override destination must be honored"
        );
        assert!(
            !std::fs::read_dir(&root).expect("cleanup listing").any(|entry| entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".condor-output-"))
        );
        if !case.starts_with("native") {
            assert_eq!(
                std::fs::read(root.join("partial-written"))
                    .expect("native partial output snapshot"),
                b"partial native output"
            );
            let staged =
                std::fs::read_to_string(root.join("actual-staging")).expect("child destination");
            assert!(staged.contains(".condor-output-") && staged.ends_with("output.mkv"));
        }
        println!(
            "final case={case} completed={completed}, final/source preserved or published, staging cleaned"
        );
    }
}
