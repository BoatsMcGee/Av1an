use std::path::{Path, PathBuf};

use andean_condor::{
    models::sequence::target_quality::types::{
        DEFAULT_BUTTERAUGLI_TARGET_RANGE,
        DEFAULT_CVVDP_TARGET_RANGE,
        DEFAULT_SSIMULACRA2_TARGET_RANGE,
        DEFAULT_VMAF_TARGET_RANGE,
        DEFAULT_XPSNR_TARGET_RANGE,
        ProbeStatistic,
        ProbeStrategy,
        QualityMetric,
        SubsetProbeLength,
        SubsetProbePosition,
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::Result;

use crate::{
    commands::{
        DecoderMethod,
        QualityMetric as QualityMetricBase,
        QualityProfile,
        handlers::{configure_input, configure_temp, load_configuration},
    },
    configuration::Configuration,
};

#[allow(clippy::too_many_arguments)]
pub fn quality_check_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    metric: Option<&QualityMetricBase>,
    profile: Option<&QualityProfile>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_temp(&mut configuration, temp_path)?;
    configure_quality_check(
        &mut configuration,
        input_path,
        decoder,
        filters,
        vs_args,
        metric,
        profile,
    )?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

#[allow(clippy::too_many_arguments)]
pub fn configure_quality_check(
    configuration: &mut Configuration,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    metric: Option<&QualityMetricBase>,
    profile: Option<&QualityProfile>,
) -> Result<()> {
    // Initialize quality check if it doesn't exist yet
    if configuration.condor.sequence_config.quality_check.is_none() {
        configuration.condor.sequence_config.quality_check = Some(Default::default());
    }

    if input_path.is_some() || decoder.is_some() || filters.is_some() || vs_args.is_some() {
        let existing_input = if let Some(Some(input)) = configuration
            .condor
            .sequence_config
            .quality_check
            .as_ref()
            .map(|qc| qc.input.clone())
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
        if let Some(quality_check) = &mut configuration.condor.sequence_config.quality_check {
            quality_check.input = Some(input);
        }
    }

    if let Some(filters) = filters {
        configuration.input_filters = filters.to_vec();
    }

    if let (Some(metric), Some(quality_check)) = (
        metric,
        &mut configuration.condor.sequence_config.quality_check,
    ) {
        quality_check.metric = match metric {
            QualityMetricBase::VMAF => QualityMetric::VMAF {
                target_range: DEFAULT_VMAF_TARGET_RANGE,
                resolution:   None,
                scaler:       String::new(),
                filter:       None,
                threads:      1,
                model:        None,
                features:     vec![],
            },
            QualityMetricBase::SSIMULACRA2 => QualityMetric::SSIMULACRA2 {
                target_range: DEFAULT_SSIMULACRA2_TARGET_RANGE,
                resolution:   None,
                threads:      None,
            },
            QualityMetricBase::BUTTERAUGLI => QualityMetric::BUTTERAUGLI {
                target_range:         DEFAULT_BUTTERAUGLI_TARGET_RANGE,
                resolution:           None,
                threads:              None,
                intensity_multiplier: None,
                norm:                 None,
            },
            QualityMetricBase::BUTTERAUGLI3Norm => QualityMetric::BUTTERAUGLI {
                target_range:         DEFAULT_BUTTERAUGLI_TARGET_RANGE,
                resolution:           None,
                threads:              None,
                intensity_multiplier: None,
                norm:                 Some(3),
            },
            QualityMetricBase::XPSNR => QualityMetric::XPSNR {
                target_range: DEFAULT_XPSNR_TARGET_RANGE,
                resolution:   None,
            },
            QualityMetricBase::CVVDP => QualityMetric::CVVDP {
                target_range:      DEFAULT_CVVDP_TARGET_RANGE,
                resolution:        None,
                display_model:     None,
                resize_to_display: None,
                disable_temporal:  None,
            },
        };
    }
    if let (Some(profile), Some(quality_check)) = (
        profile,
        &mut configuration.condor.sequence_config.quality_check,
    ) {
        match profile {
            QualityProfile::Fast => {
                quality_check.strategy = ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Frames(11),
                };
                quality_check.statistic = ProbeStatistic::Mean;
            },
            QualityProfile::Standard => {
                quality_check.strategy = ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Percentage(25.0),
                };
                quality_check.statistic = ProbeStatistic::RootMeanSquare;
            },
            QualityProfile::Slow => {
                quality_check.strategy = ProbeStrategy::Whole;
                if quality_check.metric.is_inverse_metric() {
                    quality_check.statistic = ProbeStatistic::Percentile(90.0);
                } else {
                    quality_check.statistic = ProbeStatistic::Percentile(10.0);
                }
            },
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use andean_condor::models::{
        input::{Input, VapourSynthImportMethod},
        sequence::quality_check::QualityCheckConfig,
    };

    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn quality_check_default_config() {
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
        let (config, found_config_path) =
            quality_check_handler(Some(&config_path), None, None, None, None, None, None, None)
                .expect("quality_check_handler should succeed");

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
    fn quality_check_custom_config() {
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
        let custom_vs_args = vec!["method=quality check".to_string()];

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.quality_check = Some(QualityCheckConfig {
            input:     Some(Input::VapourSynth {
                path:          input_abs.clone(),
                import_method: VapourSynthImportMethod::LSMASHWorks {
                    index: None
                },
                cache_path:    None,
            }),
            metric:    QualityMetric::BUTTERAUGLI {
                target_range:         DEFAULT_BUTTERAUGLI_TARGET_RANGE,
                resolution:           None,
                threads:              None,
                intensity_multiplier: None,
                norm:                 None,
            },
            statistic: ProbeStatistic::Mean,
            strategy:  ProbeStrategy::Subset {
                position: SubsetProbePosition::Middle,
                length:   SubsetProbeLength::Frames(11),
            },
        });
        expected_config.input_filters = custom_filters.clone();
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
        let (config, found_config_path) = quality_check_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&test_video.path),
            Some(&DecoderMethod::LSMASHWorks),
            Some(&custom_filters),
            Some(&custom_vs_args),
            Some(&QualityMetricBase::BUTTERAUGLI),
            Some(&QualityProfile::Fast),
        )
        .expect("quality_check_handler should succeed");

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
