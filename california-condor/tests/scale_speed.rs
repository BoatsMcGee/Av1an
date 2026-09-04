#[path = "common.rs"]
mod common;

use andean_condor::models::sequence::speed_scaler::SpeedScalerConfig;
use california_condor::{
    commands::handlers::load_configuration,
    test_helpers::*,
    utils::hash_path::hash_path,
};
use common::condor_cmd;

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    #[serial]
    #[test]
    fn default_and_no_scenes() {
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
            .args(["scale-speed"])
            .assert()
            .success();

        let expected_config = default_config(&test_video, &output, &temp_abs);

        let (config, _) = load_configuration(Some(&config_path)).expect("config should load");

        check_basic_config(&config, &expected_config);
    }

    #[serial]
    #[test]
    fn with_pairs_and_no_scenes() {
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

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.speed_scaler = SpeedScalerConfig {
            speed_quantizers: vec![(6, 20.0), (4, 35.0), (2, 50.0)],
        };
        let expected_config = expected_config;

        condor_cmd(&temp)
            .args(["init", path_str(&test_video.path), path_str(&output)])
            .assert()
            .success();

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "scale-speed",
                "--quantizers",
                "20",
                "--speeds",
                "6",
                "--quantizers",
                "35",
                "--speeds",
                "4",
                "--quantizers",
                "50",
                "--speeds",
                "2",
            ])
            .assert()
            .success();

        let (config, _) = load_configuration(Some(&config_path)).expect("config should load");

        check_basic_config(&config, &expected_config);
    }

    #[serial]
    #[test]
    fn with_pairs_and_scenes() {
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

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.scenes = test_video.mock_scenes(&expected_config.condor.encoder);
        expected_config.condor.sequence_config.speed_scaler = SpeedScalerConfig {
            speed_quantizers: vec![(8, 10.0), (5, 25.0), (3, 35.0)],
        };
        let expected_config = expected_config;

        condor_cmd(&temp)
            .args(["init", path_str(&test_video.path), path_str(&output)])
            .assert()
            .success();

        {
            let (mut config, _) =
                load_configuration(Some(&config_path)).expect("load_config should succeed");
            config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
            config.save(&config_path).expect("save should succeed");
        }

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "scale-speed",
                "--quantizers",
                "10",
                "--speeds",
                "8",
                "--quantizers",
                "25",
                "--speeds",
                "5",
                "--quantizers",
                "35",
                "--speeds",
                "3",
            ])
            .assert()
            .success();

        let (config, _) = load_configuration(Some(&config_path)).expect("config should load");

        check_basic_config(&config, &expected_config);
        assert_eq!(
            config.condor.scenes.len(),
            test_video.scenes.len(),
            "scene count matches"
        );
    }
}
