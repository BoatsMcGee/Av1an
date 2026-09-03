#[path = "common.rs"]
mod common;

use andean_condor::models::{
    encoder::{EncoderBase, cli_parameter::CLIParameter},
    sequence::target_quality::{
        TargetQualityConfig,
        types::{QualityMetric, TargetQualityProbing},
    },
};
use california_condor::{
    commands::handlers::load_configuration,
    test_helpers::*,
    utils::hash_path::hash_path,
};
use common::{condor_cmd, path_str};

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use andean_condor::{
        ffmpeg::FFPixelFormat,
        models::{
            input::{Input, VapourSynthImportMethod},
            sequence::target_quality::types::{
                ProbeStatistic,
                ProbeStrategy,
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
        let mut tq_encoder_params = EncoderBase::SVTAV1.default_parameters();
        tq_encoder_params.extend(CLIParameter::new_numbers("--", " ", &[
            ("preset", 8.0),
            ("tune", 1.0),
        ]));

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
            CLIParameter::new_number("--", " ", 6.0),
        );
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "target-quality",
                "--input",
                path_str(&test_video.path),
                "--decoder",
                "vs-ffms2",
                "--filters",
                "resize:format=yuv420p10le;",
                "--metric",
                "xpsnr",
                "--target",
                "45",
                "--min-q",
                "20",
                "--max-q",
                "40",
                "--profile",
                "fast",
                "--params",
                "--preset 8 --tune 1",
            ])
            .assert()
            .success();

        let mut expected_config = config.clone();
        expected_config.tq_input_filters = expected_config.input_filters.clone();
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            metric: QualityMetric::XPSNR {
                target_range: (44.0, 46.0),
                resolution:   None,
            },
            quantizer_range: (20, 40),
            input: Some(Input::VapourSynth {
                path:          input_abs,
                import_method: VapourSynthImportMethod::FFMS2 {
                    index: None
                },
                cache_path:    None,
            }),
            probing: TargetQualityProbing {
                encoder_options: Some(tq_encoder_params),
                strategy:        ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Frames(11),
                },
                statistic:       ProbeStatistic::Mean,
            },
            ..Default::default()
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
        let quantizer_range = config
            .condor
            .sequence_config
            .target_quality
            .as_ref()
            .expect("Target Quality sequence_config should exist")
            .quantizer_range;
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            assert!(
                !scene.sequence_data.target_quality.passes.is_empty(),
                "scene {} should have Target Quality passes",
                index
            );
            let maximum_probes = config
                .condor
                .sequence_config
                .target_quality
                .as_ref()
                .expect("Target Quality sequence_config should exist")
                .maximum_probes;
            assert!(
                scene.sequence_data.target_quality.passes.len() <= maximum_probes as usize,
                "scene {} should have at most {} Target Quality passes",
                index,
                maximum_probes
            );
            let crf = scene.encoder.parameters().get("crf").expect("crf should exist");
            assert_matches!(
                crf,
                CLIParameter::Number { .. },
                "scene {} crf should be a number",
                index
            );
            let crf_value = match crf {
                CLIParameter::Number {
                    value, ..
                } => *value,
                _ => panic!("scene {} crf should be a number", index),
            };
            assert!(
                crf_value >= quantizer_range.0 as f64 && crf_value <= quantizer_range.1 as f64,
                "scene {} crf {} should be within {} and {} CRF",
                index,
                crf_value,
                quantizer_range.0,
                quantizer_range.1
            );
        });
    }
}
