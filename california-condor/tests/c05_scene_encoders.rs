#[path = "common.rs"]
mod common;

#[cfg(test)]
mod tests {
    use super::common::condor_cmd;
    use andean_condor::models::{
        encoder::{Encoder, EncoderBase, photon_noise::PhotonNoise},
        input::{Input, VapourSynthScriptSource},
        sequence::target_quality::types::QualityPass,
    };
    use california_condor::{
        configuration::Configuration,
        test_helpers::{TestVideo, default_config},
    };
    use std::{fs, process::Output, time::Duration};

    fn fixture(temp: &tempfile::TempDir, saved_scenes: bool) -> Configuration {
        let source = temp.path().join("inert.vpy");
        fs::write(&source, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,length=4,format=YUV420P8).set_output()\n").expect("inert source");
        let video = TestVideo {
            path: source.clone(),
            width: 64,
            height: 64,
            frames: 4,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 2), (2, 4)],
        };
        let mut config = default_config(&video, &temp.path().join("out.mkv"), temp.path());
        config.condor.input = Input::VapourSynthScript {
            source: VapourSynthScriptSource::Path(source),
            variables: Default::default(),
            index: 0,
            filters: vec![],
            stream_concurrently: true,
        };
        config.condor.encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
        config.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso: 100,
            chroma_iso: None,
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
        if saved_scenes {
            config.condor.scenes =
                video.mock_scenes(&Encoder::default_from_base(&EncoderBase::X264, false));
            config.condor.scenes[0].sub_scenes = Some(vec![
                andean_condor::models::scene::SubScene {
                    start_frame: 0,
                    end_frame: 1,
                },
                andean_condor::models::scene::SubScene {
                    start_frame: 1,
                    end_frame: 2,
                },
            ]);
            config.condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(123);
            config.condor.scenes[0].sequence_data.target_quality.passes.push(QualityPass {
                quantizer: 17.0,
                scores: vec![50.0],
                bitrate: 1000.0,
                started_on: 100,
                completed_on: 200,
            });
        }
        config.save(&temp.path().join("condor.json")).expect("save fixture");
        config
    }

    fn run(temp: &tempfile::TempDir, args: &[&str]) -> Output {
        condor_cmd(temp)
            .env("PATH", "")
            .args(args)
            .timeout(Duration::from_secs(10))
            .output()
            .expect("CLI output")
    }

    fn load(temp: &tempfile::TempDir) -> Configuration {
        Configuration::load(&temp.path().join("condor.json"))
            .expect("load")
            .expect("saved configuration")
    }

    fn warning(output: &Output, expected: bool, args: &[&str]) {
        let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
        assert_eq!(
            stderr.contains("existing scenes unchanged"),
            expected,
            "{args:?}: {stderr}"
        );
        if expected {
            assert!(stderr.contains("future scenes"), "{stderr}");
            assert!(
                stderr.contains("condor scenes all reset encoder"),
                "{stderr}"
            );
        }
    }

    // Removing a flag-presence check or propagating global settings breaks this
    // real CLI/save oracle; an empty PATH stops native encoder resolution later.
    #[test]
    fn c05_global_flags_warn_and_preserve_saved_scene_json() {
        let temp = tempfile::tempdir().expect("fixture");
        let config = fixture(&temp, true);
        for command in [vec![], vec!["encode"]] {
            for flags in [
                vec!["--encoder", "rav1e"],
                vec!["--params", "--cpu-used=7"],
                vec!["--passes", "1"],
                vec!["--photon-noise", "800"],
                vec!["--chroma-noise", "400"],
                vec!["--ffmpeg-filter", "hflip"],
            ] {
                config.save(&temp.path().join("condor.json")).expect("restore");
                let args = [command.clone(), flags].concat();
                let output = run(&temp, &args);
                assert!(
                    !output.status.success(),
                    "empty PATH must prevent native encoding"
                );
                let saved = load(&temp);
                assert_ne!(
                    serde_json::to_value(&saved.condor.encoder).unwrap(),
                    serde_json::to_value(&config.condor.encoder).unwrap(),
                    "handler must save the requested global edit before native failure: {args:?}: {output:?}"
                );
                assert_eq!(
                    serde_json::to_value(&saved.condor.scenes).unwrap(),
                    serde_json::to_value(&config.condor.scenes).unwrap(),
                    "{args:?} must not change scene overrides, boundaries or history"
                );
                warning(&output, true, &args);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let warning_at = stderr.find("Existing scenes unchanged").unwrap();
                let failure_at = stderr.find("Error:").unwrap_or_else(|| {
                    panic!("expected the later native failure: {args:?}: {stderr}")
                });
                assert!(
                    warning_at < failure_at,
                    "warning must precede native failure: {stderr}"
                );
            }
        }
        config.save(&temp.path().join("condor.json")).expect("restore");
        let output = run(&temp, &["--disable-color-data"]);
        assert!(!load(&temp).condor.encoder.append_color_data());
        assert_eq!(
            serde_json::to_value(&load(&temp).condor.scenes).unwrap(),
            serde_json::to_value(&config.condor.scenes).unwrap()
        );
        warning(&output, true, &["--disable-color-data"]);
        config.save(&temp.path().join("condor.json")).expect("restore");
        warning(
            &run(&temp, &["encode", "--passes", "2"]),
            true,
            &["encode", "--passes", "2"],
        );
    }

    #[test]
    fn c05_no_warning_without_saved_scenes_or_global_encoder_request() {
        let temp = tempfile::tempdir().expect("fixture");
        let config = fixture(&temp, true);
        for args in [
            vec![],
            vec!["encode"],
            vec!["--workers", "2"],
            vec!["encode", "--workers", "2"],
            vec!["--max-tries", "2"],
            vec!["encode", "--max-tries", "2"],
            vec!["--target", "50"],
            vec!["--target-profile", "fast"],
            vec!["--tq-params", "--cpu-used=7"],
            vec!["encode", "--filters", "resize:format=yuv420p"],
            vec!["encode", "info"],
            vec!["encode", "set", "workers", "2"],
        ] {
            config.save(&temp.path().join("condor.json")).expect("restore");
            let output = run(&temp, &args);
            assert!(
                !["unexpected argument", "invalid value", "unrecognized subcommand"]
                    .iter()
                    .any(|error| String::from_utf8_lossy(&output.stderr).contains(error)),
                "test must reach the owning route: {args:?}: {output:?}"
            );
            warning(&output, false, &args);
            assert_eq!(
                load(&temp).condor.encoder,
                config.condor.encoder,
                "{args:?}"
            );
        }
        for args in [
            vec!["--params", "--cq-level=NaN"],
            vec!["encode", "--passes", "3"],
            vec!["--encoder", "x264", "--photon-noise", "100"],
            vec!["encode", "set", "passes", "3"],
            vec!["init", "set", "encoder", "--encoder", "x264", "--photon-noise", "100"],
        ] {
            config.save(&temp.path().join("condor.json")).expect("restore");
            let before = fs::read(temp.path().join("condor.json")).unwrap();
            let output = run(&temp, &args);
            assert!(!output.status.success());
            assert_eq!(fs::read(temp.path().join("condor.json")).unwrap(), before);
            warning(&output, false, &args);
        }
        let empty = fixture(&temp, false);
        for args in [
            vec!["--passes", "1"],
            vec!["encode", "--passes", "1"],
            vec!["encode", "set", "passes", "1"],
        ] {
            empty.save(&temp.path().join("condor.json")).expect("restore");
            warning(&run(&temp, &args), false, &args);
        }
    }

    #[test]
    fn c05_crud_guidance_and_real_explicit_reset_preserve_results_and_files() {
        let temp = tempfile::tempdir().expect("fixture");
        fixture(&temp, true);
        let scene_edit = run(&temp, &["scenes", "0", "set", "params", "--crf=17"]);
        assert!(scene_edit.status.success(), "{scene_edit:?}");
        let original = load(&temp);
        let chunk = temp.path().join("scenes/00000.mkv");
        fs::create_dir_all(chunk.parent().unwrap()).unwrap();
        fs::write(&chunk, b"historical chunk").unwrap();
        for args in [
            vec!["encode", "set", "encoder", "rav1e"],
            vec!["encode", "set", "params", "--cpu-used=7"],
            vec!["encode", "set", "passes", "1"],
            vec!["encode", "set", "executable", "/missing/c05-aomenc"],
            vec!["encode", "set", "photon-noise", "800", "--chroma-noise", "200"],
            vec!["encode", "set", "ffmpeg-filter", "hflip"],
            vec!["encode", "set", "append-color-data", "off"],
            vec!["encode", "reset", "encoder"],
            vec!["encode", "reset", "executable"],
            vec!["encode", "reset", "passes"],
            vec!["encode", "reset", "params"],
            vec!["encode", "reset", "photon-noise"],
            vec!["encode", "reset", "ffmpeg-filter"],
            vec!["encode", "reset", "append-color-data"],
            vec!["encode", "reset"],
            vec!["init", "set", "encoder", "--encoder", "rav1e"],
            vec!["init", "reset", "encoder"],
        ] {
            original.save(&temp.path().join("condor.json")).expect("restore");
            let output = run(&temp, &args);
            assert!(output.status.success(), "{args:?}: {output:?}");
            assert_eq!(
                serde_json::to_value(&load(&temp).condor.scenes).unwrap(),
                serde_json::to_value(&original.condor.scenes).unwrap()
            );
            warning(&output, true, &args);
        }
        let before_reset = load(&temp);
        let output = run(&temp, &["scenes", "all", "reset", "encoder"]);
        assert!(output.status.success(), "{output:?}");
        warning(&output, false, &["scenes", "all", "reset", "encoder"]);
        let saved = load(&temp);
        for (before, after) in before_reset.condor.scenes.iter().zip(&saved.condor.scenes) {
            assert_eq!(after.encoder, saved.condor.encoder);
            let mut expected = serde_json::to_value(before).unwrap();
            expected["encoder"] = serde_json::to_value(&saved.condor.encoder).unwrap();
            assert_eq!(serde_json::to_value(after).unwrap(), expected);
        }
        assert_eq!(fs::read(chunk).unwrap(), b"historical chunk");
    }
}
