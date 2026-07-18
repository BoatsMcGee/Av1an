use std::path::{Path, PathBuf};

use andean_condor::{
    core::sequence::target_quality::TargetQuality,
    models::sequence::target_quality::{
        types::{
            ProbeStatistic,
            ProbeStrategy,
            QualityMetric,
            SubsetProbeLength,
            SubsetProbePosition,
            DEFAULT_BUTTERAUGLI_TARGET_RANGE,
            DEFAULT_CVVDP_TARGET_RANGE,
            DEFAULT_SSIMULACRA2_TARGET_RANGE,
            DEFAULT_VMAF_TARGET_RANGE,
            DEFAULT_XPSNR_TARGET_RANGE,
        },
        TargetQualityConfig,
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::Result;

use crate::{
    commands::{
        handlers::{configure_input, configure_temp, load_configuration},
        DecoderMethod,
        TargetQualityMetric,
        TargetQualityProfile,
    },
    configuration::Configuration,
    utils::parameter_parser::EncoderParamsParser,
};

#[allow(clippy::too_many_arguments)]
pub fn target_quality_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    params: Option<&str>,
    metric: Option<&TargetQualityMetric>,
    target: Option<f64>,
    minimum_quantizer: Option<u8>,
    maximum_quantizer: Option<u8>,
    profile: Option<&TargetQualityProfile>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_temp(&mut configuration, temp_path)?;
    configure_target_quality(
        &mut configuration,
        input_path,
        decoder,
        filters,
        vs_args,
        params,
        metric,
        target,
        minimum_quantizer,
        maximum_quantizer,
        profile,
    )?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

#[allow(clippy::too_many_arguments)]
pub fn configure_target_quality(
    configuration: &mut Configuration,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    params: Option<&str>,
    metric: Option<&TargetQualityMetric>,
    target: Option<f64>,
    minimum_quantizer: Option<u8>,
    maximum_quantizer: Option<u8>,
    profile: Option<&TargetQualityProfile>,
) -> Result<()> {
    // Initialize target quality if it doesn't exist yet
    if configuration.condor.sequence_config.target_quality.is_none() && target.is_some() {
        configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            quantizer_range: TargetQuality::default_quantizer_range(
                &configuration.condor.encoder.base(),
            ),
            ..Default::default()
        });
    }

    if configuration.condor.sequence_config.target_quality.is_none() {
        // No target was ever specified, skip configuration
        return Ok(());
    }

    if input_path.is_some() || decoder.is_some() || filters.is_some() || vs_args.is_some() {
        let existing_input = if let Some(Some(input)) = configuration
            .condor
            .sequence_config
            .target_quality
            .as_ref()
            .map(|tq| tq.input.clone())
        {
            input
        } else {
            configuration.condor.input.clone()
        };
        let input = configure_input(
            configuration,
            &existing_input,
            input_path,
            decoder,
            vs_args,
            None,
        )?;
        if let Some(target_quality) = &mut configuration.condor.sequence_config.target_quality {
            target_quality.input = Some(input);
        }
    }

    if let Some(filters) = filters {
        configuration.tq_input_filters = filters.to_vec();
    }

    if let (Some(metric), Some(target_quality)) = (
        metric,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        target_quality.metric = match metric {
            TargetQualityMetric::VMAF => QualityMetric::VMAF {
                target_range: DEFAULT_VMAF_TARGET_RANGE,
                resolution:   None,
                scaler:       String::new(),
                filter:       None,
                threads:      1,
                model:        None,
                features:     vec![],
            },
            TargetQualityMetric::SSIMULACRA2 => QualityMetric::SSIMULACRA2 {
                target_range: DEFAULT_SSIMULACRA2_TARGET_RANGE,
                resolution:   None,
                threads:      None,
            },
            TargetQualityMetric::BUTTERAUGLI => QualityMetric::BUTTERAUGLI {
                target_range:         DEFAULT_BUTTERAUGLI_TARGET_RANGE,
                resolution:           None,
                threads:              None,
                intensity_multiplier: None,
                norm:                 None,
            },
            TargetQualityMetric::BUTTERAUGLI3Norm => QualityMetric::BUTTERAUGLI {
                target_range:         DEFAULT_BUTTERAUGLI_TARGET_RANGE,
                resolution:           None,
                threads:              None,
                intensity_multiplier: None,
                norm:                 Some(3),
            },
            TargetQualityMetric::XPSNR => QualityMetric::XPSNR {
                target_range: DEFAULT_XPSNR_TARGET_RANGE,
                resolution:   None,
            },
            TargetQualityMetric::CVVDP => QualityMetric::CVVDP {
                target_range:      DEFAULT_CVVDP_TARGET_RANGE,
                resolution:        None,
                display_model:     None,
                resize_to_display: None,
                disable_temporal:  None,
            },
        };
    }
    if let (Some(target), Some(target_quality)) = (
        target,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        let target_range = match target_quality.metric {
            QualityMetric::BUTTERAUGLI {
                ..
            }
            | QualityMetric::CVVDP {
                ..
            } => (target - 0.1, target + 0.1),
            _ => (target - 1.0, target + 1.0),
        };
        target_quality.metric.target_range_mut().0 = target_range.0;
        target_quality.metric.target_range_mut().1 = target_range.1;
    }
    if let (Some(minimum_quantizer), Some(target_quality)) = (
        minimum_quantizer,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        target_quality.quantizer_range.0 = minimum_quantizer as u32;
    }
    if let (Some(maximum_quantizer), Some(target_quality)) = (
        maximum_quantizer,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        target_quality.quantizer_range.1 = maximum_quantizer as u32;
    }
    if let (Some(params), Some(target_quality)) = (
        params,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        let mut parameters = configuration.condor.encoder.base().default_parameters();
        parameters.extend(EncoderParamsParser::parse_string(params)?);
        target_quality.probing.encoder_options = Some(parameters);
    }
    if let (Some(profile), Some(target_quality)) = (
        profile,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        match profile {
            TargetQualityProfile::Fast => {
                target_quality.probing.strategy = ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Frames(11),
                };
                target_quality.probing.statistic = ProbeStatistic::Mean;
            },
            TargetQualityProfile::Standard => {
                target_quality.probing.strategy = ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Percentage(25.0),
                };
                target_quality.probing.statistic = ProbeStatistic::RootMeanSquare;
            },
            TargetQualityProfile::Slow => {
                target_quality.probing.strategy = ProbeStrategy::Whole;
                if target_quality.metric.is_inverse_metric() {
                    target_quality.probing.statistic = ProbeStatistic::Percentile(90.0);
                } else {
                    target_quality.probing.statistic = ProbeStatistic::Percentile(10.0);
                }
            },
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use andean_condor::models::{
        encoder::{cli_parameter::CLIParameter, EncoderBase},
        input::{Input, VapourSynthImportMethod},
        sequence::target_quality::types::{InterpolationMethod, TargetQualityProbing},
    };

    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn target_quality_default_config() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();

        let expected_config = default_config(&test_video, &output, &temp_abs);

        init_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("init_handler should succeed");
        let (config, found_config_path) = target_quality_handler(
            Some(&config_path),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("target_quality_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    #[test]
    fn target_quality_custom_config() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let custom_filters = vec![VapourSynthFilter::Crop {
            top:    Some(140),
            bottom: Some(140),
            left:   None,
            right:  None,
        }];
        let custom_vs_args = vec!["method=target quality".to_string()];

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.tq_input_filters = custom_filters.clone();
        let mut tq_encoder_parameters = EncoderBase::SVTAV1.default_parameters();
        tq_encoder_parameters.extend(CLIParameter::new_numbers("--", " ", &[
            ("preset", 6.0),
            ("tune", 3.0),
        ]));
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            metric:          QualityMetric::BUTTERAUGLI {
                target_range:         (1.4, 1.6),
                resolution:           None,
                threads:              None,
                intensity_multiplier: None,
                norm:                 None,
            },
            maximum_probes:  4,
            quantizer_range: (8, 40),
            interpolators:   (InterpolationMethod::Natural, InterpolationMethod::Pchip),
            input:           Some(Input::VapourSynth {
                path:          input_abs.clone(),
                import_method: VapourSynthImportMethod::LSMASHWorks {
                    index: None
                },
                cache_path:    None,
            }),
            metric_input:    None,
            probing:         TargetQualityProbing {
                encoder_options: Some(tq_encoder_parameters),
                statistic:       ProbeStatistic::Mean,
                strategy:        ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Frames(11),
                },
            },
        });
        // immutable shadow
        let expected_config = expected_config;

        init_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("init_handler should succeed");
        let (config, found_config_path) = target_quality_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&test_video.path),
            Some(&DecoderMethod::LSMASHWorks),
            Some(&custom_filters),
            Some(&custom_vs_args),
            Some("--preset 6 --tune 3"),
            Some(&TargetQualityMetric::BUTTERAUGLI),
            Some(1.5),
            Some(8),
            Some(40),
            Some(&TargetQualityProfile::Fast),
        )
        .expect("detect_noise_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }
}
