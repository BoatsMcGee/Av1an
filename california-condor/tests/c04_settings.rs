#[path = "common.rs"]
mod common;

#[cfg(test)]
mod tests {
    use super::common::condor_cmd;
    use andean_condor::models::{
        encoder::{Encoder, EncoderBase},
        input::{ImportMethod, Input},
    };
    use california_condor::{configuration::Configuration, test_helpers::TestVideo};

    #[test]
    fn c04_f2_legacy_jobs_can_be_repaired_in_either_order_without_running() {
        use andean_condor::models::encoder::EncoderPasses;
        let temp = tempfile::tempdir().expect("legacy fixture");
        let script = temp.path().join("blank.vpy");
        std::fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,format=YUV420P8).set_output()\n").expect("script");
        let mut legacy = Configuration::new(
            &script,
            &temp.path().join("out.mkv"),
            Some(temp.path()),
            None,
            None,
        )
        .expect("seed config");
        let source = temp.path().join("must-not-open.mkv");
        std::fs::write(&source, b"not a video").expect("source");
        legacy.condor.input = Input::Video {
            path: source,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(temp.path().join("forbidden.ffindex")),
            },
            filters: vec![],
        };
        legacy.condor.encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
        *legacy.condor.encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
        let video = TestVideo {
            path: script,
            width: 64,
            height: 64,
            frames: 2,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 1), (1, 2)],
        };
        legacy.condor.scenes = video.mock_scenes(&legacy.condor.encoder);
        let path = temp.path().join("condor.json");
        for global_first in [true, false] {
            legacy.save(&path).expect("legacy JSON");
            condor_cmd(&temp)
                .args(["init", "set", "output", "changed.mkv"])
                .assert()
                .success();
            condor_cmd(&temp).args(["encode", "set", "workers", "2"]).assert().success();
            condor_cmd(&temp)
                .args(["scenes", "all", "reset", "parallel-encoder"])
                .assert()
                .success();
            let global = ["encode", "set", "passes", "1"];
            let scenes = ["scenes", "all", "set", "passes", "1"];
            let (first, second) = if global_first {
                (&global[..], &scenes[..])
            } else {
                (&scenes[..], &global[..])
            };
            condor_cmd(&temp).args(first).assert().success();
            let before = std::fs::read(&path).expect("partially repaired");
            for run in [vec![], vec!["encode"]] {
                let output =
                    condor_cmd(&temp).args(run).assert().failure().get_output().stderr.clone();
                assert!(
                    String::from_utf8_lossy(&output).contains("multi-pass requires VBR"),
                    "{}",
                    String::from_utf8_lossy(&output)
                );
                assert_eq!(
                    std::fs::read(&path).expect("failed execution snapshot"),
                    before
                );
            }
            condor_cmd(&temp).args(second).assert().success();
            let repaired = Configuration::load(&path).expect("load").expect("config");
            assert_eq!(repaired.condor.encoder.validate_settings(), Ok(()));
            assert!(
                repaired
                    .condor
                    .scenes
                    .iter()
                    .all(|scene| scene.encoder.validate_settings() == Ok(()))
            );
            let before = std::fs::read(&path).expect("repaired snapshot");
            for invalid in [
                vec!["encode", "set", "passes", "2"],
                vec!["scenes", "all", "set", "passes", "2"],
            ] {
                let output =
                    condor_cmd(&temp).args(invalid).assert().failure().get_output().stderr.clone();
                assert!(String::from_utf8_lossy(&output).contains("multi-pass requires VBR"));
                assert_eq!(
                    std::fs::read(&path).expect("rejected edit snapshot"),
                    before
                );
            }
        }
        legacy.save(&path).expect("legacy reset fixture");
        let before = std::fs::read(&path).expect("snapshot");
        for targeted in [
            vec!["scenes", "all", "reset"],
            vec!["scenes", "add", "2", "3"],
            vec!["encode", "set", "append-color-data", "off"],
            vec!["encode", "set", "executable", "/missing/c04"],
            vec!["scenes", "all", "set", "append-color-data", "off"],
            vec!["scenes", "all", "set", "executable", "/missing/c04"],
        ] {
            let output =
                condor_cmd(&temp).args(&targeted).assert().failure().get_output().stderr.clone();
            assert!(
                String::from_utf8_lossy(&output).contains("multi-pass requires VBR"),
                "invalid target must not be changed/copied: {targeted:?}: {}",
                String::from_utf8_lossy(&output)
            );
            assert_eq!(
                std::fs::read(&path).expect("unchanged targeted edit"),
                before
            );
        }
        condor_cmd(&temp).args(["encode", "reset", "passes"]).assert().success();
        condor_cmd(&temp).args(["scenes", "all", "reset"]).assert().success();
        legacy.save(&path).expect("legacy clear fixture");
        condor_cmd(&temp).args(["scenes", "reset"]).assert().success();
        condor_cmd(&temp).args(["init", "reset", "encoder"]).assert().success();
        assert!(!temp.path().join("forbidden.ffindex").exists());
    }

    #[test]
    fn c04_routes_reject_before_save_or_index() {
        let temp = tempfile::tempdir().expect("fixture");
        let script = temp.path().join("blank.vpy");
        std::fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,format=YUV420P8).set_output()\n").expect("script");
        let mut config = Configuration::new(
            &script,
            &temp.path().join("out.mkv"),
            Some(temp.path()),
            None,
            None,
        )
        .expect("config");
        let source = temp.path().join("must-not-open.mkv");
        std::fs::write(&source, b"not a video").expect("source");
        config.condor.input = Input::Video {
            path: source,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(temp.path().join("forbidden.ffindex")),
            },
            filters: vec![],
        };
        config.condor.encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
        let video = TestVideo {
            path: script,
            width: 64,
            height: 64,
            frames: 1,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 1)],
        };
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        let path = temp.path().join("condor.json");
        config.save(&path).expect("save");
        let before = std::fs::read(&path).expect("snapshot");
        for args in [
            vec!["--encoder", "x264", "--photon-noise", "100"],
            vec!["encode", "--encoder", "x264", "--photon-noise", "100"],
            vec!["init", "set", "encoder", "--encoder", "x264", "--photon-noise", "100"],
            vec!["encode", "set", "passes", "3"],
            vec!["encode", "--encoder", "svt-av1", "--passes", "2"],
            vec!["encode", "--encoder", "ffmpeg", "--ffmpeg-filter", "hflip"],
            vec!["encode", "--chroma-noise", "100"],
            vec!["encode", "set", "params", "--cq-level NaN"],
            vec!["scenes", "0", "set", "encoder", "x264", "--iso", "100"],
            vec!["scenes", "0", "set", "passes", "3"],
        ] {
            let output = condor_cmd(&temp)
                .args(&args)
                .timeout(std::time::Duration::from_secs(10))
                .assert()
                .failure()
                .get_output()
                .stderr
                .clone();
            let stderr = String::from_utf8_lossy(&output);
            assert!(
                stderr.contains("Photon Noise")
                    || stderr.contains("pass")
                    || stderr.contains("ffmpeg-filter")
                    || stderr.contains("finite"),
                "{args:?}: {stderr}"
            );
            assert!(
                !stderr.contains("unexpected argument"),
                "not a validation failure: {stderr}"
            );
            assert_eq!(
                std::fs::read(&path).expect("unchanged config"),
                before,
                "{args:?}"
            );
            assert!(!temp.path().join("forbidden.ffindex").exists());
        }
        let new = temp.path().join("new.json");
        for args in [
            vec![
                "init",
                "must-not-open.mkv",
                "out.mkv",
                "--encoder",
                "x264",
                "--photon-noise",
                "100",
            ],
            vec![
                "--input",
                "must-not-open.mkv",
                "--output",
                "out.mkv",
                "--encoder",
                "x264",
                "--photon-noise",
                "100",
            ],
        ] {
            let output = condor_cmd(&temp)
                .args(["--config-file", new.to_str().expect("path")])
                .args(args)
                .assert()
                .failure()
                .get_output()
                .stderr
                .clone();
            assert!(
                String::from_utf8_lossy(&output).contains("does not support Photon Noise"),
                "{output:?}"
            );
            assert!(!new.exists(), "invalid new config must not save");
        }
        condor_cmd(&temp)
            .args(["encode", "set", "encoder", "svt-av1"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["encode", "set", "params", "--rc 1 --tbr 1000 --keyint 48"])
            .assert()
            .success();
        condor_cmd(&temp).args(["encode", "set", "passes", "2"]).assert().success();
        let before = std::fs::read(&path).expect("VBR snapshot");
        condor_cmd(&temp).args(["encode", "reset", "params"]).assert().failure();
        assert_eq!(std::fs::read(&path).expect("unchanged reset"), before);

        *config.condor.scenes[0].encoder.passes_mut().expect("passes") =
            andean_condor::models::encoder::EncoderPasses::All(3);
        config.save(&path).expect("loaded invalid scene fixture");
        let before = std::fs::read(&path).expect("invalid scene snapshot");
        for args in [vec![], vec!["encode"]] {
            let output =
                condor_cmd(&temp).args(args).assert().failure().get_output().stderr.clone();
            assert!(
                String::from_utf8_lossy(&output).contains("does not support pass"),
                "{}",
                String::from_utf8_lossy(&output)
            );
            assert_eq!(
                std::fs::read(&path).expect("unchanged loaded config"),
                before
            );
        }
    }
}
