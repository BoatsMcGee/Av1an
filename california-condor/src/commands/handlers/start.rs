use std::path::{Path, PathBuf};

use andean_condor::vapoursynth::vapoursynth_filters::VapourSynthFilter;
use anyhow::{bail, Result};
use tracing::{debug, error, trace};

use crate::{
    commands::{
        handlers::{
            concatenate::configure_concatenate,
            configure_input,
            configure_temp,
            detect_scenes::configure_scene_detector,
            encode::configure_parallel_encoder,
            target_quality::configure_target_quality,
        },
        ConcatenationMethod,
        DecoderMethod,
        EncoderMethod,
        TargetQualityMetric,
        TargetQualityProfile,
    },
    configuration::{ConfigError, Configuration},
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
    scd_decoder: Option<&DecoderMethod>,
    tq_decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    scd_filters: Option<&[VapourSynthFilter]>,
    tq_filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    scd_vs_args: Option<&[String]>,
    tq_vs_args: Option<&[String]>,
    concatenator: Option<&ConcatenationMethod>,
    workers: Option<u8>,
    encoder: Option<&EncoderMethod>,
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

    configure_temp(&mut configuration, temp_path)?;
    if let Some(output) = output_path {
        let output = path_abs::PathAbs::new(output)?.as_path().to_path_buf();
        configuration.condor.output.path = output;
    }
    if let Some(filters) = filters {
        configuration.input_filters = filters.to_vec();
    }

    let existing_input = configuration.condor.input.clone();
    configuration.condor.input = configure_input(
        &configuration,
        &existing_input,
        input_path.filter(|_| config_already_existed),
        decoder.filter(|_| config_already_existed),
        vs_args.filter(|_| config_already_existed),
        None,
    )?;
    configure_scene_detector(
        &mut configuration,
        scd_input_path,
        scd_decoder,
        scd_filters,
        scd_vs_args,
        None,
        None,
        None,
    )?;
    // configure_benchmarker(
    //     &mut configuration,
    //     input_path.filter(|_| config_already_existed),
    //     decoder.filter(|_| config_already_existed),
    //     filters.filter(|_| config_already_existed),
    //     vs_args.filter(|_| config_already_existed),
    //     encoder,
    //     passes,
    //     params,
    //     threshold,
    //     max_memory,
    // )?;
    // configure_noise_detector(&mut configuration,
    // nd_input_path,
    // nd_vs_args)?; configure_noise_scaler(&mut
    // configuration, threshold, minimum_scaler, maximum_scaler, scale_chroma)?;
    configure_target_quality(
        &mut configuration,
        tq_input_path,
        tq_decoder,
        tq_filters,
        tq_vs_args,
        tq_params,
        target_metric,
        target,
        minimum_quantizer,
        maximum_quantizer,
        target_profile,
    )?;
    // configure_optimize_bitrate_handler(&mut configuration, sigma_threshold)?;
    // configure_scale_speed(&mut configuration, quantizers, speeds)?;
    configure_parallel_encoder(
        &mut configuration,
        input_path.filter(|_| config_already_existed),
        decoder.filter(|_| config_already_existed),
        filters.filter(|_| config_already_existed),
        vs_args.filter(|_| config_already_existed),
        workers,
        encoder,
        passes,
        params,
        photon_noise,
        chroma_noise,
    )?;
    configure_concatenate(&mut configuration, concatenator)?;

    if !config_already_existed {
        debug!("Saving new Configuration to {}", config_path.display());
    }
    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}
