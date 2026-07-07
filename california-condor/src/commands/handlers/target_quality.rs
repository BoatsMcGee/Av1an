use std::path::{Path, PathBuf};

use andean_condor::{
    models::sequence::target_quality::{
        types::{
            ProbeStategy,
            ProbeStatistic,
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
use anyhow::{bail, Result};
use tracing::error;

use crate::{
    commands::{
        handlers::{configure_input, configure_temp, load_configuration},
        DecoderMethod,
        TargetQualityMetric,
        TargetQualityProfile,
    },
    configuration::Configuration,
    utils::parameter_parser::EncoderParamsParser,
    CondorCliError,
};

#[allow(clippy::too_many_arguments)]
pub fn target_quality_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    params: Option<String>,
    metric: Option<TargetQualityMetric>,
    target: Option<f64>,
    minimum_quantizer: Option<u8>,
    maximum_quantizer: Option<u8>,
    profile: Option<TargetQualityProfile>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    if configuration.condor.scenes.is_empty() {
        let err = CondorCliError::NoScenes;
        error!("{}", err);
        bail!(err);
    }

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
    params: Option<String>,
    metric: Option<TargetQualityMetric>,
    target: Option<f64>,
    minimum_quantizer: Option<u8>,
    maximum_quantizer: Option<u8>,
    profile: Option<TargetQualityProfile>,
) -> Result<()> {
    // Initialize target quality if it doesn't exist yet
    if configuration.condor.sequence_config.target_quality.is_none() {
        configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig::default());
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
        let parameters = EncoderParamsParser::parse_string(&params);
        target_quality.probing.encoder_options = Some(parameters);
    }
    if let (Some(profile), Some(target_quality)) = (
        profile,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        match profile {
            TargetQualityProfile::Fast => {
                target_quality.probing.strategy = ProbeStategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Frames(11),
                };
                target_quality.probing.statistic = ProbeStatistic::Mean;
            },
            TargetQualityProfile::Standard => {
                target_quality.probing.strategy = ProbeStategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Percentage(25.0),
                };
                target_quality.probing.statistic = ProbeStatistic::RootMeanSquare;
            },
            TargetQualityProfile::Slow => {
                target_quality.probing.strategy = ProbeStategy::Whole;
                target_quality.probing.statistic = ProbeStatistic::Percentile(10.0);
            },
        }
    }

    Ok(())
}
