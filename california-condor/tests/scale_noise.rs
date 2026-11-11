#[path = "common.rs"]
mod common;
#[path = "config_helpers.rs"]
mod config_helpers;

use std::time::SystemTime;

use andean_condor::models::{
    encoder::photon_noise::PhotonNoise,
    sequence::{noise_detector::NoiseDetectorData, noise_scaler::NoiseScalerConfig},
};
use california_condor::{
    commands::handlers::load_configuration, test_helpers::*, utils::hash_path::hash_path,
};
use common::condor_cmd;
use config_helpers::{init_config, load};

#[cfg(test)]
mod tests {
    use andean_condor::models::sequence::noise_detector::NoiseDetectorConfig;
    use serial_test::serial;

    use super::*;

    fn q12_fixture(temp: &std::path::Path) -> california_condor::configuration::Configuration {
        let video = TestVideo {
            path: temp.join("must-not-open.mkv"),
            width: 2,
            height: 2,
            frames: 2,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 2)],
        };
        let mut configuration =
            default_config(&video, &temp.join("out.mkv"), &temp.join("scratch"));
        configuration.condor.encoder.set_executable(Some(temp.join("must-not-run")));
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        configuration.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig {
            threshold: 0.2,
            minimum_scaler: 0.5,
            maximum_scaler: 1.5,
            scale_chroma: true,
        });
        configuration
    }

    #[test]
    fn q12_flags_and_crud_errors_preserve_memory_and_saved_bytes_offline() {
        use california_condor::commands::{
            config::ConfigCommand,
            handlers::scale_noise::{
                ScaleNoiseCrud, ScaleNoiseKey, ScaleNoiseSet, configure_noise_scaler,
                scale_noise_handler,
            },
        };
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut failures = vec![];
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1] {
            for field in 0..3 {
                let mut config = q12_fixture(temp.path());
                config.save(&path).expect("save");
                let before = serde_json::to_vec(&config).expect("snapshot");
                let bytes = std::fs::read(&path).expect("bytes");
                let args = match field {
                    0 => (Some(bad), None, None),
                    1 => (None, Some(bad), None),
                    _ => (None, None, Some(bad)),
                };
                let result = configure_noise_scaler(&mut config, args.0, args.1, args.2, false);
                if result.is_ok() || serde_json::to_vec(&config).expect("config") != before {
                    failures.push(format!("flags field {field} bad={bad}: {result:?}"));
                }
                config = q12_fixture(temp.path());
                let before = serde_json::to_vec(&config).expect("CRUD snapshot");
                let set = match field {
                    0 => ScaleNoiseSet::Threshold { value: bad },
                    1 => ScaleNoiseSet::MinimumScaler { value: bad },
                    _ => ScaleNoiseSet::MaximumScaler { value: bad },
                };
                let result = ScaleNoiseCrud::apply(&mut config, &set);
                if result.is_ok() || serde_json::to_vec(&config).expect("config") != before {
                    failures.push(format!("CRUD field {field} bad={bad}: {result:?}"));
                }
                if scale_noise_handler(Some(&path), args.0, args.1, args.2, false).is_ok()
                    || std::fs::read(&path).expect("bytes") != bytes
                {
                    failures.push(format!(
                        "saved flags field {field} bad={bad} accepted/changed bytes"
                    ));
                }
            }
        }
        let mut config = q12_fixture(temp.path());
        let before = serde_json::to_vec(&config).expect("snapshot");
        let result = ScaleNoiseCrud::restore(&mut config, Some(&ScaleNoiseKey::MaximumScaler));
        if result.is_ok() || serde_json::to_vec(&config).expect("config") != before {
            failures.push(format!("reset max below min accepted/mutated: {result:?}"));
        }
        config.condor.sequence_config.noise_scaler = None;
        let before = serde_json::to_vec(&config).expect("snapshot");
        let result = configure_noise_scaler(&mut config, Some(f64::NAN), None, None, false);
        if result.is_ok() || serde_json::to_vec(&config).expect("config") != before {
            failures.push("invalid flags materialized disabled section".into());
        }
        assert!(!temp.path().join("scratch").exists());
        assert!(
            failures.is_empty(),
            "invalid config accepted or mutated: {failures:?}"
        );
    }

    #[test]
    fn q12_actual_cli_invalid_flags_set_reset_preserve_file_bytes_offline() {
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let config = q12_fixture(temp.path());
        let mut failures = vec![];
        for args in [
            vec!["scale-noise", "--threshold", "NaN"],
            vec!["scale-noise", "--minimum-scaler", "inf"],
            vec!["scale-noise", "--maximum-scaler=-1"],
            vec!["scale-noise", "--minimum-scaler", "2"],
            vec!["scale-noise", "set", "threshold", "NaN"],
            vec!["scale-noise", "set", "maximum-scaler", "inf"],
            vec!["scale-noise", "set", "minimum-scaler", "2"],
            vec!["scale-noise", "reset", "maximum-scaler"],
        ] {
            config.save(&path).expect("save");
            let bytes = std::fs::read(&path).expect("bytes");
            let output =
                condor_cmd(&temp).env("PATH", temp.path()).args(&args).output().expect("CLI");
            let unchanged = std::fs::read(&path).expect("bytes") == bytes;
            eprintln!(
                "Q12 CLI {args:?}: status={} unchanged={unchanged} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            if output.status.success() || !unchanged {
                failures.push(format!(
                    "{args:?}: status={}, unchanged={unchanged}",
                    output.status
                ));
            }
        }
        assert!(!temp.path().join("scratch").exists());
        assert!(
            failures.is_empty(),
            "CLI accepted invalid edit or changed bytes: {failures:?}"
        );
    }

    #[test]
    fn q12_candidate_repair_defaults_and_crud_off_are_offline() {
        use california_condor::commands::{
            config::ConfigCommand,
            handlers::scale_noise::{
                ScaleNoiseCrud, ScaleNoiseKey, ScaleNoiseSet, configure_noise_scaler,
                scale_noise_handler,
            },
        };
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut config = q12_fixture(temp.path());
        config
            .condor
            .sequence_config
            .noise_scaler
            .as_mut()
            .expect("scaler")
            .minimum_scaler = 2.0;
        config.save(&path).expect("bad finite bounds");
        let (mut repaired, _) =
            scale_noise_handler(Some(&path), Some(0.2), Some(0.5), Some(1.5), false)
                .expect("complete flags repair");
        assert!(
            repaired
                .condor
                .sequence_config
                .noise_scaler
                .as_ref()
                .expect("scaler")
                .scale_chroma,
            "false flag retains true"
        );
        ScaleNoiseCrud::apply(
            &mut repaired,
            &ScaleNoiseSet::ScaleChroma {
                value: Some(california_condor::commands::config::Toggle::Off),
            },
        )
        .expect("CRUD off");
        assert!(
            !repaired
                .condor
                .sequence_config
                .noise_scaler
                .as_ref()
                .expect("scaler")
                .scale_chroma
        );
        config.save(&path).expect("bad bounds again");
        condor_cmd(&temp)
            .env("PATH", temp.path())
            .args(["scale-noise", "set", "maximum-scaler", "3"])
            .assert()
            .success();
        assert_eq!(
            load(&path).condor.sequence_config.noise_scaler.expect("scaler").maximum_scaler,
            3.0
        );
        config.save(&path).expect("bad bounds again");
        condor_cmd(&temp)
            .env("PATH", temp.path())
            .args(["scale-noise", "reset", "minimum-scaler"])
            .assert()
            .success();
        assert_eq!(
            load(&path).condor.sequence_config.noise_scaler.expect("scaler").minimum_scaler,
            0.0
        );
        config.save(&path).expect("bad bounds again");
        condor_cmd(&temp)
            .env("PATH", temp.path())
            .args(["scale-noise", "reset"])
            .assert()
            .success();
        assert!(load(&path).condor.sequence_config.noise_scaler.is_none());
        config.condor.sequence_config.noise_scaler.as_mut().expect("scaler").threshold = f64::NAN;
        configure_noise_scaler(&mut config, Some(0.2), Some(0.5), Some(1.5), false)
            .expect("repair before validation");
        ScaleNoiseCrud::restore(&mut config, None).expect("whole reset");
        assert!(config.condor.sequence_config.noise_scaler.is_none());
        ScaleNoiseCrud::restore(&mut config, Some(&ScaleNoiseKey::Threshold))
            .expect("disabled reset");
        assert!(config.condor.sequence_config.noise_scaler.is_none());
        configure_noise_scaler(&mut config, None, None, None, false).expect("defaults unchanged");
        let defaults = config.condor.sequence_config.noise_scaler.expect("defaults");
        assert_eq!(
            (
                defaults.threshold,
                defaults.minimum_scaler,
                defaults.maximum_scaler,
                defaults.scale_chroma
            ),
            (0.0, 0.0, 0.0, false)
        );
        assert!(!temp.path().join("scratch").exists());
        assert!(!temp.path().join("must-not-open.mkv").exists());
    }

    #[serial]
    #[test]
    fn default_and_no_noise_detector_data() {
        if !ffmpeg_is_available() {
            return;
        }
        let test_video = get_test_video();
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let output = temp.path().join("out.mkv");
        let input_abs = path_abs::PathAbs::new(test_video.path.clone())
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let config_path = temp.path().join("condor.json");

        condor_cmd(&temp)
            .args(["init", path_str(&test_video.path), path_str(&output)])
            .assert()
            .success();

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["scale-noise"])
            .assert()
            .success();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig::default());
        // immutable shadow
        let expected_config = expected_config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
    }

    #[serial]
    #[test]
    fn with_custom_options_and_iso_100() {
        if !ffmpeg_is_available() {
            return;
        }
        let test_video = get_test_video();
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let output = temp.path().join("out.mkv");
        let input_abs = path_abs::PathAbs::new(test_video.path.clone())
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let config_path = temp.path().join("condor.json");

        // Mock an existing config file with Noise Detector data
        let mut config = default_config(&test_video, &output, &temp_abs);
        config.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig::default());
        config.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso: 100,
            chroma_iso: None,
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
        let expected_noise = |index| match index {
            0 => 0.0,
            1 => 0.0003550650245139927,
            2 => 0.0002258349829248295,
            3 => 0.0001817853846724563,
            4 => 0.0000033027436124321854,
            _ => 0.0,
        };
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.condor.scenes.iter_mut().enumerate().for_each(|(index, scene)| {
            scene.encoder.set_photon_noise(Some(PhotonNoise {
                iso: 100,
                chroma_iso: None,
                width: None,
                height: None,
                c_y: None,
                ccb: None,
                ccr: None,
            }));
            scene.sequence_data.noise_detection = Some(NoiseDetectorData {
                created_on: SystemTime::now(),
                noise: expected_noise(index),
                luminance: 0.0,
            });
        });
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "scale-noise",
                "--threshold",
                "0.0002",
                "--minimum-scaler",
                "1.0",
                "--maximum-scaler",
                "2.0",
                "--scale-chroma",
            ])
            .assert()
            .success();

        let mut expected_config = config;
        let expected_isos = |index| match index {
            0 => 100,
            1 => 200,
            2 => 117,
            3 => 100,
            4 => 100,
            _ => 100,
        };
        expected_config.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig {
            threshold: 0.0002,
            minimum_scaler: 1.0,
            maximum_scaler: 2.0,
            scale_chroma: true,
        });
        expected_config.condor.scenes.iter_mut().enumerate().for_each(|(index, scene)| {
            scene.encoder.set_photon_noise(Some(PhotonNoise {
                iso: expected_isos(index),
                chroma_iso: None,
                width: None,
                height: None,
                c_y: None,
                ccb: None,
                ccr: None,
            }));
            scene.sequence_data.noise_detection = Some(NoiseDetectorData {
                created_on: SystemTime::now(),
                noise: expected_noise(index),
                luminance: 0.0,
            });
        });

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
        assert_eq!(
            config.condor.scenes.len(),
            test_video.scenes.len(),
            "scenes contains {} scenes",
            test_video.scenes.len()
        );
        config
            .condor
            .scenes
            .iter()
            .zip(expected_config.condor.scenes.iter())
            .enumerate()
            .for_each(|(index, (scene, expected_scene))| {
                check_encoder(
                    &scene.encoder,
                    &expected_scene.encoder,
                    &format!("scene {} encoder", index),
                );
            });
    }

    /// `scale-noise` materializes its optional section, validates the scaler
    /// bounds, and `reset` resets single settings.
    #[serial]
    #[test]
    fn scale_noise_set_reset_and_validation() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args(["scale-noise", "set", "threshold", "0.002"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["scale-noise", "set", "scale-chroma", "on"])
            .assert()
            .success();

        // A minimum above the default maximum (0) is refused.
        condor_cmd(&temp)
            .args(["scale-noise", "set", "minimum-scaler", "1.5"])
            .assert()
            .failure();

        condor_cmd(&temp)
            .args(["scale-noise", "set", "maximum-scaler", "2"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["scale-noise", "set", "minimum-scaler", "1.5"])
            .assert()
            .success();

        let scaler = load(&config_path)
            .condor
            .sequence_config
            .noise_scaler
            .expect("noise scaler is set");
        assert_eq!(scaler.threshold, 0.002, "threshold is set");
        assert_eq!(scaler.minimum_scaler, 1.5, "minimum scaler is set");
        assert_eq!(scaler.maximum_scaler, 2.0, "maximum scaler is set");
        assert!(scaler.scale_chroma, "chroma scaling is on");

        condor_cmd(&temp)
            .args(["scale-noise", "reset", "scale-chroma"])
            .assert()
            .success();
        let scaler = load(&config_path)
            .condor
            .sequence_config
            .noise_scaler
            .expect("noise scaler still exists");
        assert!(!scaler.scale_chroma, "chroma scaling is off again");
        assert_eq!(scaler.threshold, 0.002, "the other settings are untouched");
    }
}
