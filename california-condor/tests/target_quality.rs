#[path = "common.rs"]
mod common;

use andean_condor::{
    core::sequence::target_quality::TargetQuality,
    models::{
        encoder::cli_parameter::CLIParameter,
        sequence::target_quality::{
            types::{InterpolationMethod, QualityMetric, TargetQualityProbing},
            TargetQualityConfig,
        },
    },
};
use andean_condor::models::encoder::EncoderBase;
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
    fn with_metric_target_and_no_scenes() {
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

        let target_value = 85.0;
        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        let mut tq_encoder_params = EncoderBase::SVTAV1.default_parameters();
        tq_encoder_params.extend(CLIParameter::new_numbers("--", " ", &[
            ("preset", 4.0),
            ("tune", 1.0),
        ]));
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            metric:          QualityMetric::SSIMULACRA2 {
                target_range: (target_value - 1.0, target_value + 1.0),
                resolution:   None,
                threads:      None,
            },
            maximum_probes:  4,
            quantizer_range: TargetQuality::default_quantizer_range(&EncoderBase::SVTAV1),
            interpolators:   (InterpolationMethod::Natural, InterpolationMethod::Pchip),
            input:           None,
            metric_input:    None,
            probing:         TargetQualityProbing {
                encoder_options: Some(tq_encoder_params),
                ..Default::default()
            },
        });
        let expected_config = expected_config;

        condor_cmd(&temp)
            .args(["init", path_str(&test_video.path), path_str(&output), "--target-metric", "ssimulacra2", "--target", "85"])
            .assert()
            .success();

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["target-quality", "--params", "--preset 4 --tune 1"])
            .assert()
            .success();

        let (config, _) = load_configuration(Some(&config_path)).expect("config should load");

        check_basic_config(&config, &expected_config);
    }

    #[test]
    fn with_all_metric_variants_and_no_scenes() {
        if !ffmpeg_is_available() {
            return;
        }
        let test_video = get_test_video();
        let target_value = 80.0;
        let test_cases: &[(&str, QualityMetric)] = &[
            (
                "vmaf",
                QualityMetric::VMAF {
                    target_range: (target_value - 1.0, target_value + 1.0),
                    resolution:   None,
                    scaler:       String::new(),
                    filter:       None,
                    threads:      1,
                    model:        None,
                    features:     vec![],
                },
            ),
            (
                "ssimulacra2",
                QualityMetric::SSIMULACRA2 {
                    target_range: (target_value - 1.0, target_value + 1.0),
                    resolution:   None,
                    threads:      None,
                },
            ),
            (
                "butteraugli",
                QualityMetric::BUTTERAUGLI {
                    target_range:         (target_value - 0.1, target_value + 0.1),
                    resolution:           None,
                    threads:              None,
                    intensity_multiplier: None,
                    norm:                 None,
                },
            ),
            (
                "butteraugli-3",
                QualityMetric::BUTTERAUGLI {
                    target_range:         (target_value - 0.1, target_value + 0.1),
                    resolution:           None,
                    threads:              None,
                    intensity_multiplier: None,
                    norm:                 Some(3),
                },
            ),
            (
                "xpsnr",
                QualityMetric::XPSNR {
                    target_range: (target_value - 1.0, target_value + 1.0),
                    resolution:   None,
                },
            ),
            (
                "cvvdp",
                QualityMetric::CVVDP {
                    target_range:      (target_value - 0.1, target_value + 0.1),
                    resolution:        None,
                    display_model:     None,
                    resize_to_display: None,
                    disable_temporal:  None,
                },
            ),
        ];

        for (metric_name, expected_metric) in test_cases {
            let temp = tempfile::tempdir().expect("failed to create temp dir");
            let config_path = temp.path().join("condor.json");

            condor_cmd(&temp)
                .args([
                    "init",
                    path_str(&test_video.path),
                    path_str(&temp.path().join("out.mkv")),
                    "--target-metric",
                    metric_name,
                    "--target",
                    "80",
                ])
                .assert()
                .success();

            condor_cmd(&temp)
                .env("CONDOR_TEST_MODE", "1")
                .args(["target-quality"])
                .assert()
                .success();

            let (config, _) = load_configuration(Some(&config_path)).expect("config should load");
            let tq = config
                .condor
                .sequence_config
                .target_quality
                .unwrap_or_else(|| panic!("target_quality is set for {metric_name}"));
            assert_eq!(
                std::mem::discriminant(&tq.metric),
                std::mem::discriminant(expected_metric),
                "target quality metric variant matches for {metric_name}",
            );
            let tr = tq.metric.target_range();
            let etr = expected_metric.target_range();
            assert!(
                (tr.0 - etr.0).abs() < 0.01 && (tr.1 - etr.1).abs() < 0.01,
                "target range for {metric_name}: expected ({}, {}), got ({}, {})",
                etr.0, etr.1, tr.0, tr.1,
            );
        }
    }
}
