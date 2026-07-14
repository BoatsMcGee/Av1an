#[path = "common.rs"]
mod common;

use andean_condor::models::{
    encoder::{photon_noise::PhotonNoise, Encoder},
    sequence::{noise_detector::NoiseDetectorData, noise_scaler::NoiseScalerConfig},
};
use california_condor::{
    commands::handlers::load_configuration,
    test_helpers::*,
    utils::hash_path::hash_path,
};
use std::{assert_matches, time::SystemTime};

use common::{condor_cmd, path_str};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_options_and_scenes() {
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
        expected_config.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso: 100, chroma_iso: None, width: None, height: None, c_y: None, ccb: None, ccr: None,
        }));
        expected_config.condor.scenes = test_video.mock_scenes(&expected_config.condor.encoder);
        expected_config.condor.scenes[0].encoder.set_photon_noise(Some(PhotonNoise {
            iso: 200, chroma_iso: None, width: None, height: None, c_y: None, ccb: None, ccr: None,
        }));
        expected_config.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig {
            threshold:      0.002,
            minimum_scaler: 1.0,
            maximum_scaler: 2.0,
            scale_chroma:   true,
        });
        let expected_config = expected_config;

        condor_cmd(&temp)
            .args(["init", path_str(&test_video.path), path_str(&output), "--photon-noise", "100"])
            .assert()
            .success();

        let (mut config, _) =
            load_configuration(Some(&config_path)).expect("load_config should succeed");
        config.condor.scenes = test_video
            .mock_scenes(&config.condor.encoder)
            .into_iter()
            .enumerate()
            .map(|(index, mut scene)| {
                scene.sequence_data.noise_detection = Some(NoiseDetectorData {
                    created_on: SystemTime::now(),
                    noise:      if index == 0 { 0.003 } else { 0.001 },
                    luminance:  0.0,
                });
                scene
            })
            .collect();
        config.save(&config_path).expect("save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["scale-noise", "--threshold", "0.002", "--minimum-scaler", "1.0", "--maximum-scaler", "2.0", "--scale-chroma"])
            .assert()
            .success();

        let (config, _) = load_configuration(Some(&config_path)).expect("config should load");

        check_basic_config(&config, &expected_config);
        assert!(!config.condor.scenes.is_empty(), "scenes are present");
        assert_eq!(
            config.condor.scenes.len(),
            test_video.scenes.len(),
            "scene count matches"
        );
        assert_matches!(
            config.condor.scenes[0].encoder,
            Encoder::SVTAV1 {
                photon_noise: Some(PhotonNoise { iso: 200, .. }),
                ..
            },
            "Scene 0 photon noise iso is 200"
        );
    }
}
