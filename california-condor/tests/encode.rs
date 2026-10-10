#[path = "common.rs"]
mod common;

use andean_condor::models::encoder::{
    Encoder,
    EncoderBase,
    EncoderPasses,
    cli_parameter::CLIParameter,
};
use california_condor::{
    commands::handlers::load_configuration,
    test_helpers::*,
    utils::hash_path::hash_path,
};
use common::condor_cmd;

#[cfg(test)]
mod tests {
    use andean_condor::{
        ffmpeg::FFPixelFormat,
        models::{
            input::Input,
            sequence::scene_concatenator::{ConcatMethod, FfmpegConfig, MkvmergeConfig},
        },
        vapoursynth::{plugins::resize::Scaler, vapoursynth_filters::VapourSynthFilter},
    };
    use serial_test::serial;

    use super::*;

    const WIDTH: usize = 960;
    const HEIGHT: usize = 540;

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

        // Mock an existing config file without scenes
        let mut config = default_config(&test_video, &output, &temp_abs);
        // Allow testing with SVT Essential (does not support 8-bit)
        if let Input::VapourSynth {
            filters, ..
        } = &mut config.condor.input
        {
            *filters = vec![VapourSynthFilter::Resize {
                scaler: None,
                width:  None,
                height: None,
                format: Some(FFPixelFormat::YUV420P10LE),
            }];
        }
        config.condor.encoder.parameters_mut().insert(
            "preset".to_owned(),
            CLIParameter::new_number("--", " ", 8.0),
        );
        // config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["encode"])
            .assert()
            .success();

        let expected_config = config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
        assert!(
            config.condor.sequence_config.parallel_encoder.scenes_directory.exists(),
            "scenes directory exists"
        );
        let encoded_chunks =
            std::fs::read_dir(&config.condor.sequence_config.parallel_encoder.scenes_directory)
                .expect("scenes read_dir should succeed");
        assert_eq!(
            encoded_chunks.count(),
            0,
            "scenes directory contains no encoded scenes",
        );
    }

    #[serial]
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

        // Mock an existing config file without scenes
        let mut config = default_config(&test_video, &output, &temp_abs);
        // Allow testing with SVT Essential (does not support 8-bit)
        if let Input::VapourSynth {
            filters, ..
        } = &mut config.condor.input
        {
            *filters = vec![VapourSynthFilter::Resize {
                scaler: None,
                width:  None,
                height: None,
                format: Some(FFPixelFormat::YUV420P10LE),
            }];
        }
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "encode",
                "--encoder",
                "x264",
                "--passes",
                "2",
                "--workers",
                "2",
                "--params",
                "--preset ultrafast --crf 18",
            ])
            .assert()
            .success();

        let mut expected_config = config;
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(2);
        let mut encoder_params = EncoderBase::X264.default_parameters();
        encoder_params.insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "ultrafast"),
        );
        encoder_params.insert("crf".to_owned(), CLIParameter::new_number("--", " ", 18.0));
        expected_config.condor.encoder = Encoder::X264 {
            executable: None,
            pass:       EncoderPasses::All(2),
            options:    encoder_params,
        };
        // immutable shadow
        let expected_config = expected_config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
        assert!(
            config.condor.sequence_config.parallel_encoder.scenes_directory.exists(),
            "scenes directory exists"
        );
        let encoded_chunks =
            std::fs::read_dir(&config.condor.sequence_config.parallel_encoder.scenes_directory)
                .expect("scenes read_dir should succeed");
        assert_eq!(
            encoded_chunks.count(),
            0,
            "scenes directory contains no encoded scenes",
        );
    }

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
        if let Input::VapourSynth {
            filters, ..
        } = &mut config.condor.input
        {
            *filters = vec![VapourSynthFilter::Resize {
                scaler: None,
                width:  Some(WIDTH),
                height: Some(HEIGHT),
                format: Some(FFPixelFormat::YUV420P10LE),
            }];
        }
        config.condor.encoder.parameters_mut().insert(
            "preset".to_owned(),
            CLIParameter::new_number("--", " ", 8.0),
        );
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "encode",
                "--workers",
                "2",
                "--filters",
                &format!("resize:scaler=bicubic;width={WIDTH};height={HEIGHT};format=yuv420p10le;"),
            ])
            .assert()
            .success();

        let mut expected_config = config;
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(2);
        if let Input::VapourSynth {
            filters, ..
        } = &mut expected_config.condor.input
        {
            *filters = vec![VapourSynthFilter::Resize {
                scaler: Some(Scaler::Bicubic),
                width:  Some(WIDTH),
                height: Some(HEIGHT),
                format: Some(FFPixelFormat::YUV420P10LE),
            }];
        }
        // immutable shadow
        let expected_config = expected_config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
        assert!(
            config.condor.sequence_config.parallel_encoder.scenes_directory.exists(),
            "scenes directory exists"
        );
        let encoded_chunks =
            std::fs::read_dir(&config.condor.sequence_config.parallel_encoder.scenes_directory)
                .expect("scenes read_dir should succeed");
        assert_eq!(
            encoded_chunks.count(),
            config.condor.scenes.len(),
            "scenes directory contains {} encoded scenes",
            config.condor.scenes.len()
        );

        // The encoded scenes are now on disk, so both concatenation methods can
        // be exercised against them. Concatenating is far cheaper than encoding,
        // and it leaves the scene files in place, so this needs no second encode.
        for label in ["mkvmerge", "ffmpeg"] {
            // Clear the previous run's output so each method starts clean. The
            // first iteration has none yet, since encoding only writes chunks.
            if output.exists() {
                std::fs::remove_file(&output).expect("concatenated output should be removable");
            }

            condor_cmd(&temp)
                .env("CONDOR_TEST_MODE", "1")
                .args(["concatenate", "--method", label])
                .assert()
                .success();

            // Switching to a method that is not the current one starts from that
            // method's defaults; the baseline config is mkvmerge, so the ffmpeg
            // run lands on default FFmpeg settings.
            let method = match label {
                "mkvmerge" => ConcatMethod::MKVMerge(MkvmergeConfig::default()),
                _ => ConcatMethod::FFmpeg(FfmpegConfig::default()),
            };

            let mut expected = expected_config.clone();
            expected.condor.sequence_config.scene_concatenator.method = method;

            let (config, _) =
                load_configuration(Some(&config_path)).expect("load_configuration should succeed");

            check_basic_config(&config, &expected);
            assert!(
                config
                    .condor
                    .sequence_config
                    .scene_concatenator
                    .output
                    .unwrap_or(config.condor.output.path)
                    .exists(),
                "{label} output exists"
            );
        }
    }
}
