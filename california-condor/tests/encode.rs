#[path = "common.rs"]
mod common;

use std::fs;

use andean_condor::{
    core::sequence::parallel_encoder::ParallelEncoder,
    models::encoder::{cli_parameter::CLIParameter, photon_noise::PhotonNoise, Encoder, EncoderBase, EncoderPasses},
};
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
            .args(["encode"])
            .assert()
            .success();

        let expected_config = default_config(&test_video, &output, &temp_abs);

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
    }

    #[test]
    fn with_options_and_no_scenes() {
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
            .args([
                "encode",
                "--encoder",
                "x264",
                "--workers",
                "6",
                "--passes",
                "2",
                "--params",
                "--preset ultrafast --crf 18",
                "--photon-noise",
                "800",
            ])
            .assert()
            .success();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);

        let mut params = EncoderBase::X264.default_parameters();
        params.insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "ultrafast"),
        );
        params.insert("crf".to_owned(), CLIParameter::new_number("--", " ", 18.0));
        expected_config.condor.encoder = Encoder::X264 {
            executable: None,
            pass:       EncoderPasses::All(2),
            options:    params,
        };
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(6);
        let expected_config = expected_config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
    }

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

        let mut params = EncoderBase::SVTAV1.default_parameters();
        params.extend(CLIParameter::new_numbers("--", " ", &[
            ("preset", 10.0),
            ("crf", 40.0),
        ]));
        expected_config.condor.encoder = Encoder::SVTAV1 {
            executable:   None,
            pass:         EncoderPasses::All(1),
            options:      params.clone(),
            photon_noise: Some(PhotonNoise {
                iso:        800,
                chroma_iso: None,
                width:      None,
                height:     None,
                c_y:        None,
                ccb:        None,
                ccr:        None,
            }),
        };
        let expected_config = expected_config;

        condor_cmd(&temp)
            .args([
                "init",
                path_str(&test_video.path),
                path_str(&output),
                "--params",
                "--preset 10 --crf 40",
                "--photon-noise",
                "800",
            ])
            .assert()
            .success();

        let (mut config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["encode"])
            .assert()
            .success();

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        assert_eq!(
            config.condor.encoder.base(),
            expected_config.condor.encoder.base(),
            "encoder base matches"
        );
        for (key, val) in expected_config.condor.encoder.parameters() {
            let actual = config.condor.encoder.parameters().get(key);
            assert!(
                actual.is_some_and(|a| a == val),
                "encoder option {key}={val:?} in actual config"
            );
        }
        check_basic_config(&config, &expected_config);
        assert_eq!(
            config.condor.scenes.len(),
            test_video.scenes.len(),
            "scenes contains {} scenes",
            test_video.scenes.len()
        );
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            check_encoder(
                &scene.encoder,
                &expected_config.condor.encoder,
                &format!("scene {} encoder", index),
            );
        });
        assert!(
            config.condor.sequence_config.parallel_encoder.scenes_directory.exists(),
            "Parallel Encoder scenes directory exists"
        );
        let files = fs::read_dir(&config.condor.sequence_config.parallel_encoder.scenes_directory)
            .expect("read_dir should succeed");
        assert_eq!(
            files.count() - 1,
            test_video.scenes.len(),
            "Parallel Encoder scenes directory contains {} files",
            test_video.scenes.len()
        );
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            let scene_path =
                config.condor.sequence_config.parallel_encoder.scenes_directory.join(format!(
                    "{}.{}",
                    ParallelEncoder::scene_id(index),
                    scene.encoder.output_extension()
                ));
            assert!(scene_path.exists(), "Scene {} encoded", index);
        });
    }
}
