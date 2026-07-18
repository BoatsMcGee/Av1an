#[path = "common.rs"]
mod common;

use andean_condor::models::sequence::bitrate_optimizer::BitrateOptimizerConfig;
use california_condor::{
    commands::handlers::load_configuration,
    test_helpers::*,
    utils::hash_path::hash_path,
};

use common::{condor_cmd, path_str};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_with_scenes() {
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
            .args(["optimize-bitrate"])
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

    #[test]
    fn with_sigma_and_scenes() {
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
        expected_config.condor.sequence_config.bitrate_optimizer = BitrateOptimizerConfig {
            bitrate_sigma_threshold: Some(5),
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
            .args(["optimize-bitrate", "--sigma-threshold", "5"])
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
