#[path = "common.rs"]
mod common;

use andean_condor::models::sequence::quality_check::QualityCheckConfig;
use california_condor::{
    commands::handlers::load_configuration,
    test_helpers::*,
    utils::hash_path::hash_path,
};
use common::{condor_cmd, path_str};

#[cfg(test)]
mod tests {
    use andean_condor::{
        ffmpeg::FFPixelFormat,
        models::{
            encoder::cli_parameter::CLIParameter,
            input::{Input, VapourSynthImportMethod},
            sequence::target_quality::types::{
                DEFAULT_XPSNR_TARGET_RANGE,
                ProbeStatistic,
                ProbeStrategy,
                QualityMetric,
                SubsetProbeLength,
                SubsetProbePosition,
            },
        },
        vapoursynth::vapoursynth_filters::VapourSynthFilter,
    };
    use serial_test::serial;

    use super::*;

    #[serial]
    #[test]
    fn with_custom_options() {
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

        // Mock an existing config file with scenes
        let mut config = default_config(&test_video, &output, &temp_abs);
        // Allow testing with SVT Essential (does not support 8-bit)
        config.input_filters = vec![VapourSynthFilter::Resize {
            scaler: None,
            width:  None,
            height: None,
            format: Some(FFPixelFormat::YUV420P10LE),
        }];
        config.condor.encoder.parameters_mut().insert(
            "preset".to_owned(),
            CLIParameter::new_number("--", " ", 10.0),
        );
        config.condor.sequence_config.parallel_encoder.workers = Some(2);
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("configuration save should succeed");

        // Encode and concatenate first so the output file exists
        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["encode"])
            .assert()
            .success();

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["concatenate", "--method", "mkvmerge"])
            .assert()
            .success();

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "quality-check",
                "--input",
                path_str(&test_video.path),
                "--decoder",
                "vs-ffms2",
                "--filters",
                "resize:format=yuv420p10le;",
                "--metric",
                "xpsnr",
                "--profile",
                "fast",
            ])
            .assert()
            .success();

        let mut expected_config = config;
        let expected_frames = 11;
        expected_config.condor.sequence_config.quality_check = Some(QualityCheckConfig {
            metric:    QualityMetric::XPSNR {
                target_range: DEFAULT_XPSNR_TARGET_RANGE,
                resolution:   None,
            },
            strategy:  ProbeStrategy::Subset {
                position: SubsetProbePosition::Middle,
                length:   SubsetProbeLength::Frames(expected_frames),
            },
            statistic: ProbeStatistic::Mean,
            input:     Some(Input::VapourSynth {
                path:          input_abs,
                import_method: VapourSynthImportMethod::FFMS2 {
                    index: None
                },
                cache_path:    None,
            }),
        });
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
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            assert_eq!(
                scene.sequence_data.quality_check.quality.scores.len(),
                expected_frames as usize,
                "scene {} should have {} quality check scores",
                index,
                expected_frames
            );
        });
    }

    #[serial]
    #[test]
    fn resumes_partial_scenes() {
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

        // Mock an existing config file with scenes
        let mut config = default_config(&test_video, &output, &temp_abs);
        config.input_filters = vec![VapourSynthFilter::Resize {
            scaler: None,
            width:  None,
            height: None,
            format: Some(FFPixelFormat::YUV420P10LE),
        }];
        config.condor.encoder.parameters_mut().insert(
            "preset".to_owned(),
            CLIParameter::new_number("--", " ", 10.0),
        );
        config.condor.sequence_config.parallel_encoder.workers = Some(2);
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("configuration save should succeed");

        // Encode and concatenate first so the output file exists
        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["encode"])
            .assert()
            .success();

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["concatenate", "--method", "mkvmerge"])
            .assert()
            .success();

        // Simulate 2 scenes already with scores
        let mut config = load_configuration(Some(&config_path))
            .expect("load_configuration should succeed")
            .0;
        config.condor.scenes[0].sequence_data.quality_check.quality.scores = vec![45.0; 11];
        config.condor.scenes[1].sequence_data.quality_check.quality.scores = vec![45.0; 11];
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "quality-check",
                "--input",
                path_str(&test_video.path),
                "--decoder",
                "vs-ffms2",
                "--filters",
                "resize:format=yuv420p10le;",
                "--metric",
                "xpsnr",
                "--profile",
                "fast",
            ])
            .assert()
            .success();

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            let expected_frames = 11;
            assert_eq!(
                scene.sequence_data.quality_check.quality.scores.len(),
                expected_frames,
                "scene {} should have {} quality check scores",
                index,
                expected_frames
            );
        });
    }
}
