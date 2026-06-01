use std::path::{Path, PathBuf};

use andean_condor::{
    models::{
        input::{
            ImportMethod,
            Input as InputModel,
            VapourSynthImportMethod,
            VapourSynthScriptSource,
        },
        sequence::target_quality::{
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
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::{bail, Result};
use tracing::{debug, error, trace};

use crate::{
    commands::{DecoderMethod, TargetQualityMetric, TargetQualityProfile},
    configuration::{ConfigError, Configuration},
    utils::parameter_parser::EncoderParamsParser,
    CondorCliError,
    DEFAULT_CONFIG_PATH,
};

#[allow(clippy::too_many_arguments)]
pub fn target_quality_handler(
    temp_path: Option<&Path>,
    config_path: Option<&Path>,
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
    if config_path.is_some_and(|p| !p.exists()) && input_path.is_none() {
        bail!(CondorCliError::NoConfigOrInput);
    }
    let config_path =
        path_abs::PathAbs::new(config_path.unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH)))?
            .as_path()
            .to_path_buf();
    let config_already_existed = config_path.exists();

    let mut configuration = {
        if config_already_existed {
            debug!("Loading existing configuration");
            match Configuration::load(&config_path) {
                Ok(config) => config.expect("Config should exist"),
                Err(err) => match err {
                    ConfigError::Load(path) => {
                        let err = CondorCliError::ConfigLoadError(path);
                        error!("{}", err);
                        bail!(err);
                    },
                    _ => unreachable!("ConfigError should be LoadError"),
                },
            }
        } else {
            trace!("No existing configuration found");
            let input_path = input_path.ok_or_else(|| {
                let err = CondorCliError::NoConfigOrInput;
                error!("{}", err);
                err
            })?;
            debug!("Creating new temporary configuration");
            let input = path_abs::PathAbs::new(input_path)?.as_path().to_path_buf();
            // Won't be used
            let output = input.with_file_name(format!(
                "{}.mkv",
                input.file_stem().expect("input is a file").display()
            ));
            let output = path_abs::PathAbs::new(output)?.as_path().to_path_buf();
            Configuration::new(&input, &output, temp_path, vs_args, decoder)?
        }
    };

    if let Some(temp) = temp_path {
        configuration.temp = temp.to_path_buf();
    }
    if let Some(decoder) = &decoder {
        let existing_input_path = match configuration.condor.input {
            InputModel::Video {
                path, ..
            } => Some(path),
            InputModel::VapourSynth {
                path, ..
            } => Some(path),
            InputModel::VapourSynthScript {
                source, ..
            } => match source {
                VapourSynthScriptSource::Path(source_path) => Some(source_path),
                _ => input_path.map(|p| p.to_path_buf()), // Default to provided input path
            },
        };
        let existing_input_path = existing_input_path.ok_or_else(|| {
            let err = CondorCliError::DecoderWithoutInput;
            error!("{}", err);
            err
        })?;
        let existing_input_path =
            path_abs::PathAbs::new(existing_input_path)?.as_path().to_path_buf();
        match decoder {
            DecoderMethod::FFMS2 => {
                configuration.condor.input = InputModel::Video {
                    path:          existing_input_path,
                    import_method: ImportMethod::FFMS2 {
                        index: None
                    },
                };
            },
            vs_decoders => {
                configuration.condor.input = InputModel::VapourSynth {
                    path:          existing_input_path,
                    import_method: match vs_decoders {
                        DecoderMethod::BestSource => VapourSynthImportMethod::BestSource {
                            index: None,
                        },
                        DecoderMethod::VSFFMS2 => VapourSynthImportMethod::FFMS2 {
                            index: None
                        },
                        DecoderMethod::LSMASHWorks => VapourSynthImportMethod::LSMASHWorks {
                            index: None,
                        },
                        DecoderMethod::DGDecodeNV => VapourSynthImportMethod::DGDecNV {
                            dgindexnv_executable: None,
                        },
                        DecoderMethod::FFMS2 => unreachable!(),
                    },
                    cache_path:    None,
                };
            },
        };
    }
    if let Some(input) = input_path {
        configuration.condor.input = Configuration::new_input_model(
            path_abs::PathAbs::new(input)?.as_path(),
            decoder,
            vs_args,
        )?;
    }
    if let Some(filters) = filters {
        configuration.tq_input_filters = filters.to_vec();
    }
    // Initialize target quality if it doesn't exist yet
    if configuration.condor.sequence_config.target_quality.is_none() {
        configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig::default());
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

    if !config_already_existed {
        debug!("Saving new Configuration to {}", config_path.display());
        configuration.save(&config_path)?;
    }

    Ok((configuration, config_path))
}
