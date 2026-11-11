#[path = "common.rs"]
mod common;
#[path = "config_helpers.rs"]
mod config_helpers;

use std::path::Path;

use andean_condor::models::sequence::scene_concatenator::ConcatMethod;
use california_condor::test_helpers::*;
use common::condor_cmd;
use config_helpers::{init_config, load};

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    #[test]
    fn c06_inline_cli_identity_survives_instantiation_for_final_recheck() {
        use andean_condor::{
            core::sequence::{Sequence, scene_concatenator::SceneConcatenator},
            models::input::{Input, VapourSynthScriptSource},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("declared-script.vpy");
        std::fs::write(&source, b"declared source bytes").expect("source");
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 16,
            frames: 2,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut config = default_config(&video, &source, temp.path());
        config.condor.input=Input::VapourSynthScript{source:VapourSynthScriptSource::Text("import vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=16,length=2,format=vs.YUV420P8).set_output()".into()),variables:Default::default(),index:0,filters:vec![],stream_concurrently:false};
        config.condor.output.overwrite = true;
        config.condor.sequence_config.scene_concatenator.method = ConcatMethod::Ivf;
        let mut condor = config
            .instantiate_condor(Box::new(|_| Ok(())))
            .expect("open inline source, retain declared identity");
        assert!(
            SceneConcatenator::default().validate(&mut condor).is_err(),
            "final core boundary protects CLI's duplicated inline source path"
        );
        assert_eq!(
            std::fs::read(source).expect("source preserved"),
            b"declared source bytes"
        );
    }

    #[test]
    fn c06_concatenate_rejects_before_discard_probe_or_save_and_persists_consent() {
        use california_condor::commands::handlers::concatenate::concatenate_handler;
        for case in ["alias", "distinct", "override", "missing-parent"] {
            let temp = tempfile::tempdir().expect("fixture");
            let source = temp.path().join("must-not-open.mkv");
            std::fs::write(&source, b"source sentinel").expect("source");
            let video = TestVideo {
                path: source.clone(),
                width: 32,
                height: 16,
                frames: 2,
                fps_rational: (24, 1),
                format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
                scenes: vec![],
            };
            let output = if case == "alias" {
                source.clone()
            } else if case == "missing-parent" {
                temp.path().join("missing/out.mkv")
            } else {
                temp.path().join("out.mkv")
            };
            let mut config = default_config(&video, &output, &temp.path().join("work"));
            if case == "override" {
                config.condor.sequence_config.scene_concatenator.output = Some(source.clone());
                config.condor.output.overwrite = true;
            }
            if case == "distinct" {
                std::fs::write(&output, b"old final").expect("old");
            }
            let file = temp.path().join("condor.json");
            config.save(&file).expect("save");
            let before = std::fs::read(&file).expect("before");
            let result = condor_cmd(&temp)
                .env("PATH", "")
                .args(["concatenate", "--discard", "audio"])
                .output()
                .expect("CLI");
            let message = String::from_utf8_lossy(&result.stderr);
            assert!(!result.status.success());
            assert!(
                message.contains("Output") && !message.contains("not installed"),
                "{case}: {message}"
            );
            assert_eq!(std::fs::read(&file).expect("after"), before);
            assert_eq!(std::fs::read(&source).expect("source"), b"source sentinel");
            if case == "distinct" {
                condor_cmd(&temp)
                    .env("PATH", "")
                    .args(["concatenate", "--overwrite"])
                    .assert()
                    .failure();
                assert!(load(&file).condor.output.overwrite);
                concatenate_handler(Some(&file), None, None, None, false)
                    .expect("omission retains saved true");
                assert!(load(&file).condor.output.overwrite);
                assert_eq!(std::fs::read(&output).expect("old"), b"old final");
            }
            assert!(!temp.path().join("missing").exists());
        }
    }

    /// `concatenate` switches tools the way the run flag does: keeping a tool
    /// keeps its settings, switching starts from the new tool's defaults.
    #[serial]
    #[test]
    fn concatenate_set_method_and_output() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args(["concatenate", "set", "method", "ffmpeg"])
            .assert()
            .success();
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        assert!(
            matches!(concat.method, ConcatMethod::FFmpeg(_)),
            "method is {}",
            concat.method
        );

        condor_cmd(&temp)
            .args(["concatenate", "set", "output", "final.mkv"])
            .assert()
            .success();
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        assert_eq!(
            concat.output.as_deref(),
            Some(Path::new("final.mkv")),
            "output is set"
        );

        condor_cmd(&temp).args(["concatenate", "reset", "output"]).assert().success();
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        assert!(concat.output.is_none(), "output was cleared");

        condor_cmd(&temp)
            .args(["concatenate", "set", "method", "mkvmerge"])
            .assert()
            .success();
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        assert!(
            matches!(concat.method, ConcatMethod::MKVMerge(_)),
            "method is {}",
            concat.method
        );
    }

    /// `--discard` probes the input and writes `copy: false` entries for the
    /// tracks it names, so `condor.json` describes the drop explicitly.
    #[serial]
    #[test]
    fn init_discard_audio_and_subtitle() {
        if !ffmpeg_is_available() || !mkvmerge_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let input = get_test_multitrack_video();
        let output = temp.path().join("out.mkv");

        condor_cmd(&temp)
            .args([
                "init",
                path_str(&input),
                path_str(&output),
                "--concat",
                "mkvmerge",
                "--discard",
                "audio,subtitle,chapters",
            ])
            .assert()
            .success();

        let config_path = temp.path().join("condor.json");
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        let ConcatMethod::MKVMerge(mkvmerge) = concat.method else {
            panic!("the method stays mkvmerge, but is {}", concat.method);
        };
        let tracks = mkvmerge.tracks.expect("a track map is written");
        assert_eq!(
            tracks.len(),
            3,
            "only the named tracks are written, not the whole map: {tracks:?}"
        );
        for (id, track) in &tracks {
            assert!(!track.copy(), "track {id} is dropped");
        }
        let ids = tracks.keys().copied().collect::<Vec<_>>();
        assert!(
            ids.contains(&1) && ids.contains(&2) && ids.contains(&3),
            "both audio tracks and the subtitle are dropped: {ids:?}"
        );
        assert!(!mkvmerge.chapters.expect("chapters are written").copy);
    }

    /// The FFmpeg selection map has to list every kept stream, because a stream
    /// that is absent from the map is not copied.
    #[serial]
    #[test]
    fn init_discard_audio_keeps_the_other_ffmpeg_streams() {
        if !ffmpeg_is_available() || !mkvmerge_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let input = get_test_multitrack_video();
        let output = temp.path().join("out.mkv");

        condor_cmd(&temp)
            .args([
                "init",
                path_str(&input),
                path_str(&output),
                "--concat",
                "ffmpeg",
                "--discard",
                "audio",
            ])
            .assert()
            .success();

        let config_path = temp.path().join("condor.json");
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        let ConcatMethod::FFmpeg(ffmpeg) = concat.method else {
            panic!("the method stays ffmpeg, but is {}", concat.method);
        };
        let tracks = ffmpeg.tracks.expect("a stream map is written");
        assert!(
            tracks.iter().any(|(_, track)| track.copy()),
            "the subtitle and attachment stay selected: {tracks:?}"
        );
        assert_eq!(
            tracks.values().filter(|track| !track.copy()).count(),
            2,
            "both audio streams are dropped: {tracks:?}"
        );
    }

    /// A video-only input has nothing to drop, so the flag leaves the settings
    /// unset rather than writing an empty map.
    #[serial]
    #[test]
    fn init_discard_without_matching_tracks_writes_nothing() {
        if !ffmpeg_is_available() || !mkvmerge_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let video = get_test_video();
        let output = temp.path().join("out.mkv");

        condor_cmd(&temp)
            .args([
                "init",
                path_str(&video.path),
                path_str(&output),
                "--concat",
                "mkvmerge",
                "--discard",
                "audio",
            ])
            .assert()
            .success();

        let config_path = temp.path().join("condor.json");
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        let ConcatMethod::MKVMerge(mkvmerge) = concat.method else {
            panic!("the method stays mkvmerge, but is {}", concat.method);
        };
        assert!(
            mkvmerge.tracks.is_none(),
            "no map is written for a source without audio: {:?}",
            mkvmerge.tracks
        );
    }
}
