use std::path::{Path, PathBuf};

use andean_condor::{
    models::{
        encoder::{photon_noise::PhotonNoise, Encoder, EncoderBase, EncoderPasses},
        input::{
            ImportMethod,
            Input as InputModel,
            VapourSynthImportMethod,
            VapourSynthScriptSource,
        },
        sequence::{
            scene_concatenator::ConcatMethod,
            target_quality::{
                types::{
                    ProbeStategy,
                    ProbeStatistic,
                    QualityMetric,
                    SubsetProbeLength,
                    SubsetProbePosition,
                },
                TargetQualityConfig,
            },
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
pub fn start_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    scd_input_path: Option<&Path>,
    tq_input_path: Option<&Path>,
    output_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    scd_filters: Option<&[VapourSynthFilter]>,
    tq_filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    scd_vs_args: Option<&[String]>,
    tq_vs_args: Option<&[String]>,
    concat: Option<&ConcatMethod>,
    workers: Option<u8>,
    encoder: Option<&EncoderBase>,
    passes: Option<u8>,
    params: Option<String>,
    tq_params: Option<String>,
    photon_noise: Option<u32>,
    chroma_noise: Option<u32>,
    target_metric: Option<TargetQualityMetric>,
    target: Option<f64>,
    minimum_quantizer: Option<u8>,
    maximum_quantizer: Option<u8>,
    target_profile: Option<TargetQualityProfile>,
) -> Result<(Configuration, PathBuf)> {
    if config_path.is_some_and(|p| !p.exists()) && input_path.is_none() && output_path.is_none() {
        let err = CondorCliError::NoConfigOrInputOrOutput;
        error!("{}", err);
        bail!(err);
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
            let path_err = || {
                let err = CondorCliError::NoConfigOrInputOrOutput;
                error!("{}", err);
                err
            };
            let input_path = input_path.ok_or_else(path_err)?;
            let output_path = output_path.ok_or_else(path_err)?;
            debug!("Creating new configuration");
            let input = path_abs::PathAbs::new(input_path)?.as_path().to_path_buf();
            let output = path_abs::PathAbs::new(output_path)?.as_path().to_path_buf();
            debug!("TEMP: {temp:?}", temp = temp_path);
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
    if let Some(input) = scd_input_path {
        configuration.condor.sequence_config.scene_detector.input =
            Some(Configuration::new_input_model(
                path_abs::PathAbs::new(input)?.as_path(),
                decoder,
                scd_vs_args,
            )?);
    }
    if let Some(filters) = filters {
        configuration.input_filters = filters.to_vec();
    }
    if let Some(filters) = scd_filters {
        configuration.scd_input_filters = filters.to_vec();
    }
    if let Some(filters) = tq_filters {
        configuration.tq_input_filters = filters.to_vec();
    }
    if let Some(output) = output_path {
        let output = path_abs::PathAbs::new(output)?.as_path().to_path_buf();
        configuration.condor.output.path = output;
    }
    if let Some(concat) = concat {
        configuration.condor.sequence_config.scene_concatenator.method = *concat;
    }
    if let Some(workers) = workers {
        configuration.condor.sequence_config.parallel_encoder.workers = Some(workers);
    }
    if let Some(encoder) = encoder {
        let options = encoder.default_parameters();
        let pass = encoder.default_passes();
        configuration.condor.encoder = match encoder {
            EncoderBase::AOM => Encoder::AOM {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::RAV1E => Encoder::RAV1E {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::VPX => Encoder::VPX {
                executable: None,
                pass,
                options,
            },
            EncoderBase::SVTAV1 => Encoder::SVTAV1 {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::AVM => Encoder::AVM {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::X264 => Encoder::X264 {
                executable: None,
                pass,
                options,
            },
            EncoderBase::X265 => Encoder::X265 {
                executable: None,
                pass,
                options,
            },
            EncoderBase::VVenC => Encoder::VVenC {
                executable: None,
                pass,
                options,
            },
            EncoderBase::FFmpeg => Encoder::FFmpeg {
                executable: None,
                options,
            },
        }
    }
    if let Some(passes) = passes
        && let Some(encoder_passes) = configuration.condor.encoder.passes_mut()
    {
        *encoder_passes = EncoderPasses::All(passes);
    }
    if let Some(params) = params {
        let parameters = EncoderParamsParser::parse_string(&params);
        configuration.condor.encoder.parameters_mut().extend(parameters);
    }
    if let Some(iso) = photon_noise {
        // TODO: Support chroma noise only
        configuration.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso,
            chroma_iso: chroma_noise,
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
    }
    if let Some(target) = target {
        if let Some(target_quality) = &mut configuration.condor.sequence_config.target_quality {
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
        } else {
            // Create a new target quality configuration
            configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig {
                metric: match target_metric.as_ref().unwrap_or(&TargetQualityMetric::SSIMULACRA2) {
                    TargetQualityMetric::VMAF => QualityMetric::VMAF {
                        target_range: (target - 1.0, target + 1.0),
                        resolution:   None,
                        scaler:       String::new(),
                        filter:       None,
                        threads:      1,
                        model:        None,
                        features:     vec![],
                    },
                    TargetQualityMetric::SSIMULACRA2 => QualityMetric::SSIMULACRA2 {
                        target_range: (target - 1.0, target + 1.0),
                        resolution:   None,
                        threads:      None,
                    },
                    TargetQualityMetric::BUTTERAUGLI => QualityMetric::BUTTERAUGLI {
                        target_range:         (target - 0.1, target + 0.1),
                        resolution:           None,
                        threads:              None,
                        intensity_multiplier: None,
                        norm:                 None,
                    },
                    TargetQualityMetric::BUTTERAUGLI3Norm => QualityMetric::BUTTERAUGLI {
                        target_range:         (target - 0.1, target + 0.1),
                        resolution:           None,
                        threads:              None,
                        intensity_multiplier: None,
                        norm:                 Some(3),
                    },
                    TargetQualityMetric::XPSNR => QualityMetric::XPSNR {
                        target_range: (target - 1.0, target + 1.0),
                        resolution:   None,
                    },
                    TargetQualityMetric::CVVDP => QualityMetric::CVVDP {
                        target_range:      (target - 0.1, target + 0.1),
                        resolution:        None,
                        display_model:     None,
                        resize_to_display: None,
                        disable_temporal:  None,
                    },
                },
                ..Default::default()
            });
        }
    }
    if let (Some(input), Some(target_quality)) = (
        tq_input_path,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        target_quality.input = Some(Configuration::new_input_model(
            path_abs::PathAbs::new(input)?.as_path(),
            decoder,
            tq_vs_args,
        )?);
    }
    if let (Some(metric), Some(target_quality)) = (
        target_metric,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        let previous_target = target_quality.metric.target_range();
        target_quality.metric = match metric {
            TargetQualityMetric::VMAF => QualityMetric::VMAF {
                target_range: previous_target,
                resolution:   None,
                scaler:       String::new(),
                filter:       None,
                threads:      1,
                model:        None,
                features:     vec![],
            },
            TargetQualityMetric::SSIMULACRA2 => QualityMetric::SSIMULACRA2 {
                target_range: previous_target,
                resolution:   None,
                threads:      None,
            },
            TargetQualityMetric::BUTTERAUGLI => QualityMetric::BUTTERAUGLI {
                target_range:         previous_target,
                resolution:           None,
                threads:              None,
                intensity_multiplier: None,
                norm:                 None,
            },
            TargetQualityMetric::BUTTERAUGLI3Norm => QualityMetric::BUTTERAUGLI {
                target_range:         previous_target,
                resolution:           None,
                threads:              None,
                intensity_multiplier: None,
                norm:                 Some(3),
            },
            TargetQualityMetric::XPSNR => QualityMetric::XPSNR {
                target_range: previous_target,
                resolution:   None,
            },
            TargetQualityMetric::CVVDP => QualityMetric::CVVDP {
                target_range:      previous_target,
                resolution:        None,
                display_model:     None,
                resize_to_display: None,
                disable_temporal:  None,
            },
        };
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
        tq_params,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        let parameters = EncoderParamsParser::parse_string(&params);
        target_quality.probing.encoder_options = Some(parameters);
    }
    if let (Some(profile), Some(target_quality)) = (
        target_profile,
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
