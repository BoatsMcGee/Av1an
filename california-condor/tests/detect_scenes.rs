#[path = "common.rs"]
mod common;

use california_condor::{
    commands::handlers::load_configuration,
    test_helpers::*,
    utils::hash_path::hash_path,
};
use common::{condor_cmd, path_str};

#[cfg(test)]
mod tests {
    use andean_condor::{
        models::{
            input::{Input, VapourSynthImportMethod},
            sequence::scene_detector::{SceneDetectionMethod, ScenecutMethod},
        },
        vapoursynth::vapoursynth_filters::VapourSynthFilter,
    };

    use super::*;

    #[test]
    fn default() {
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
            .args(["detect-scenes"])
            .assert()
            .success();

        let expected_config = default_config(&test_video, &output, &temp_abs);

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
        assert_eq!(
            config.condor.scenes.len(),
            test_video.scenes.len(),
            "scenes contains {} scenes",
            test_video.scenes.len()
        );
        let scene_boundaries = config
            .condor
            .scenes
            .iter()
            .map(|scene| (scene.start_frame, scene.end_frame))
            .collect::<Vec<_>>();
        assert_eq!(
            scene_boundaries, test_video.scenes,
            "scene start and end frames are correct"
        );
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            check_encoder(
                &scene.encoder,
                &expected_config.condor.encoder,
                &format!("scene {} encoder", index),
            );
        });
    }

    #[test]
    fn with_custom_input() {
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
                "detect-scenes",
                "--input",
                path_str(&test_video.path),
                "--decoder",
                "vs-ffms2",
                "--filters",
                "resize:width=960;height=540;",
            ])
            .assert()
            .success();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.scene_detector.input = Some(Input::VapourSynth {
            path:          input_abs,
            import_method: VapourSynthImportMethod::FFMS2 {
                index: None
            },
            cache_path:    None,
        });
        expected_config.scd_input_filters = vec![VapourSynthFilter::Resize {
            scaler: None,
            width:  Some(960),
            height: Some(540),
            format: None,
        }];
        // immutable shadow
        let expected_config = expected_config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
        assert_eq!(
            config.condor.scenes.len(),
            test_video.scenes.len(),
            "scenes contains {} scenes",
            test_video.scenes.len()
        );
        let scene_boundaries = config
            .condor
            .scenes
            .iter()
            .map(|scene| (scene.start_frame, scene.end_frame))
            .collect::<Vec<_>>();
        assert_eq!(
            scene_boundaries, test_video.scenes,
            "scene start and end frames are correct"
        );
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            check_encoder(
                &scene.encoder,
                &expected_config.condor.encoder,
                &format!("scene {} encoder", index),
            );
        });
    }

    #[test]
    fn with_fast_max_scene_length() {
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
            .args(["detect-scenes", "--method", "fast", "--max-scene-seconds", "2"])
            .assert()
            .success();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.scene_detector.method =
            SceneDetectionMethod::AVSceneChange {
                minimum_length: test_video.fps().round() as usize,
                maximum_length: (test_video.fps() * 2.0).round() as usize,
                method:         ScenecutMethod::Fast,
            };
        // immutable shadow
        let expected_config = expected_config;
        let expected_boundaries = [
            (0, 48),
            (48, 96),
            (96, 130),
            (130, 178),
            (178, 226),
            (226, 274),
            (274, 322),
            (322, 370),
            (370, 418),
            (418, 466),
            (466, 514),
            (514, 562),
            (562, 610),
            (610, 658),
            (658, 706),
            (706, 720),
        ];

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);

        assert_eq!(
            config.condor.scenes.len(),
            expected_boundaries.len(),
            "scenes contains {} scenes",
            expected_boundaries.len()
        );
        let scene_boundaries = config
            .condor
            .scenes
            .iter()
            .map(|scene| (scene.start_frame, scene.end_frame))
            .collect::<Vec<_>>();
        assert_eq!(
            scene_boundaries, expected_boundaries,
            "scene start and end frames are correct"
        );
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            check_encoder(
                &scene.encoder,
                &expected_config.condor.encoder,
                &format!("scene {} encoder", index),
            );
        });
    }

    #[test]
    fn with_none() {
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
            .args(["detect-scenes", "--method", "none"])
            .assert()
            .success();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.scene_detector.method = SceneDetectionMethod::None {
            minimum_length: test_video.fps().round() as usize,
            maximum_length: (test_video.fps() * 10.0).round() as usize,
        };
        // immutable shadow
        let expected_config = expected_config;
        let expected_boundaries = [(0, 240), (240, 480), (480, 720)];

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);

        assert_eq!(
            config.condor.scenes.len(),
            expected_boundaries.len(),
            "scenes contains {} scenes",
            expected_boundaries.len()
        );
        let scene_boundaries = config
            .condor
            .scenes
            .iter()
            .map(|scene| (scene.start_frame, scene.end_frame))
            .collect::<Vec<_>>();
        assert_eq!(
            scene_boundaries, expected_boundaries,
            "scene start and end frames are correct"
        );
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            check_encoder(
                &scene.encoder,
                &expected_config.condor.encoder,
                &format!("scene {} encoder", index),
            );
        });
    }
}
