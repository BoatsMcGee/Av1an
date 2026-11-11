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

    #[cfg(unix)]
    #[test]
    fn c07_concatenate_discard_saves_fallback_indices_or_fails_atomically() {
        use std::{os::unix::fs::PermissionsExt, time::Duration};

        for case in ["rejected", "fallback", "warning", "invalid-ffprobe"] {
            let temp = tempfile::tempdir().expect("CLI fixture");
            let source = temp.path().join("source.mkv");
            let output = temp.path().join("out.mkv");
            std::fs::write(&source, b"source sentinel").expect("source");
            std::fs::write(&output, b"output sentinel").expect("output");
            let video = TestVideo {
                path: source.clone(),
                width: 2,
                height: 2,
                frames: 1,
                fps_rational: (24, 1),
                format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
                scenes: vec![],
            };
            let mut config = default_config(&video, &output, &temp.path().join("work"));
            config.condor.output.overwrite = true;
            if matches!(case, "fallback" | "invalid-ffprobe") {
                config.condor.sequence_config.scene_concatenator.method =
                    ConcatMethod::FFmpeg(Default::default());
            }
            let file = temp.path().join("condor.json");
            config.save(&file).expect("existing job");
            let before = std::fs::read(&file).expect("before");
            let tools = temp.path().join("tools");
            std::fs::create_dir_all(&tools).expect("isolated PATH");
            let mkvmerge = if matches!(case, "warning" | "invalid-ffprobe") {
                (
                    r#"{"container":{"recognized":true,"supported":true},"errors":[],"warnings":["recoverable warning"],"tracks":[{"id":0,"type":"video"},{"id":3,"type":"audio"}]}"#,
                    1,
                )
            } else {
                (r#"{"errors":["native identify error"]}"#, 2)
            };
            let ffprobe = if case == "invalid-ffprobe" {
                "{invalid"
            } else {
                r#"{"streams":[{"index":0,"codec_type":"video"},{"index":7,"codec_type":"audio"},{"index":11,"codec_type":"subtitle"},{"index":15,"codec_type":"attachment"}]}"#
            };
            for (name, json, status) in
                [("mkvmerge", mkvmerge.0, mkvmerge.1), ("ffprobe", ffprobe, 0)]
            {
                let script = format!(
                    "#!/bin/sh\nprintf '%s\\n' '{name}' \"$@\" >> \"$C07_CALLS\"\nprintf '%s\\n' '{json}'\nprintf '%s\\n' 'native diagnostic' >&2\nexit {status}\n"
                );
                let path = tools.join(name);
                std::fs::write(&path, script).expect("fake process");
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                    .expect("executable");
            }
            // The sentinel is deliberately undecodable: after saving, opening the
            // source must fail before a TUI/concatenation/encoding pipeline starts.
            let result = condor_cmd(&temp)
                .env("PATH", &tools)
                .env("C07_CALLS", temp.path().join("calls"))
                .env("RUST_LOG", "california_condor=debug")
                .args(["concatenate", "--discard", "audio,chapters,metadata"])
                .timeout(Duration::from_secs(20))
                .output()
                .expect("bounded CLI");
            assert!(
                !result.status.success(),
                "{case}: undecodable source must stop"
            );
            let message = String::from_utf8_lossy(&result.stderr);
            let calls = std::fs::read_to_string(temp.path().join("calls")).expect("probes called");
            if matches!(case, "rejected" | "invalid-ffprobe") {
                assert_eq!(
                    std::fs::read(&file).expect("after"),
                    before,
                    "{case}: {message}"
                );
                assert!(
                    message.contains(source.to_str().expect("path")),
                    "{case}: {message}"
                );
                if case == "rejected" {
                    assert!(
                        message.contains("mkvmerge") && message.contains("native diagnostic"),
                        "{message}"
                    );
                    assert!(!calls.contains("ffprobe"));
                } else {
                    assert!(message.contains("ffprobe"), "{message}");
                    assert!(
                        !calls.contains("mkvmerge"),
                        "invalid ffprobe cannot use MKV-only success"
                    );
                }
            } else {
                let concat = load(&file).condor.sequence_config.scene_concatenator;
                match concat.method {
                    ConcatMethod::FFmpeg(ffmpeg) => {
                        assert!(
                            message.contains("probing the FFmpeg streams only")
                                && message.contains("native identify error")
                                && message.contains("native diagnostic"),
                            "fallback retains native diagnostics: {message}"
                        );
                        let tracks = ffmpeg.tracks.expect("real fallback discard saved");
                        assert_eq!(tracks.len(), 3);
                        assert!(!tracks[&7].copy());
                        assert!(tracks[&11].copy());
                        assert!(tracks[&15].copy());
                        assert!(!ffmpeg.chapters.expect("chapters").copy);
                        assert!(!ffmpeg.metadata.expect("metadata").copy);
                        assert!(calls.contains("ffprobe\n-v\nerror\n-show_entries\nstream=index,codec_type\n-of\njson\n"));
                    },
                    ConcatMethod::MKVMerge(mkvmerge) => {
                        let tracks = mkvmerge.tracks.expect("valid warning discard saved");
                        assert_eq!(tracks.len(), 1);
                        assert!(!tracks[&3].copy());
                    },
                    ConcatMethod::Ivf => panic!("method changed"),
                }
            }
            assert_eq!(
                std::fs::read(&source).expect("source unchanged"),
                b"source sentinel"
            );
            assert_eq!(
                std::fs::read(&output).expect("output unchanged"),
                b"output sentinel"
            );
            eprintln!("C07 CLI {case}: {message}");
        }
    }

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
