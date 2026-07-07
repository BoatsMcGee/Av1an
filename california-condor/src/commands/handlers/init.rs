use std::path::Path;

use andean_condor::vapoursynth::vapoursynth_filters::VapourSynthFilter;
use anyhow::{bail, Result};
use tracing::{error, info};

use crate::{
    commands::{
        handlers::{
            concatenate::configure_concatenate,
            configure_temp,
            encode::configure_parallel_encoder,
            target_quality::configure_target_quality,
        },
        ConcatenationMethod,
        DecoderMethod,
        EncoderMethod,
        TargetQualityMetric,
    },
    configuration::Configuration,
    CondorCliError,
    DEFAULT_CONFIG_PATH,
};

#[allow(clippy::too_many_arguments)]
pub fn init_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: &Path,
    output_path: &Path,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    concatenator: Option<&ConcatenationMethod>,
    workers: Option<u8>,
    encoder: Option<&EncoderMethod>,
    params: Option<String>,
    photon_noise: Option<u32>,
    target_metric: Option<TargetQualityMetric>,
    target: Option<f64>,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let input = path_abs::PathAbs::new(input_path)?.as_path().to_path_buf();
    let output = path_abs::PathAbs::new(output_path)?.as_path().to_path_buf();
    let config_path = path_abs::PathAbs::new(
        config_path.map_or_else(|| cwd.join(DEFAULT_CONFIG_PATH), |p| p.to_path_buf()),
    )?
    .as_path()
    .to_path_buf();

    if config_path.exists() {
        let err = CondorCliError::ConfigFileAlreadyExists(config_path);
        error!("{}", err);
        bail!(err);
    }

    let mut configuration = Configuration::new(&input, &output, temp_path, vs_args, decoder)?;

    configure_temp(&mut configuration, temp_path)?;
    if let Some(filters) = filters {
        configuration.input_filters = filters.to_vec();
    }

    // configure_scene_detector(
    //     &mut configuration,
    //     None,
    //     None,
    //     None,
    //     None,
    //     None,
    //     None,
    //     None,
    // )?;
    // configure_benchmarker(
    //     &mut configuration,
    //     None,
    //     None,
    //     None,
    //     None,
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
        None,
        None,
        None,
        None,
        None,
        target_metric,
        target,
        None,
        None,
        None,
    )?;
    // configure_optimize_bitrate_handler(&mut configuration, sigma_threshold)?;
    // configure_scale_speed(&mut configuration, quantizers, speeds)?;
    configure_parallel_encoder(
        &mut configuration,
        None,
        None,
        None,
        None,
        workers,
        encoder,
        None,
        params,
        photon_noise,
        None,
    )?;
    configure_concatenate(&mut configuration, concatenator)?;

    configuration.save(&config_path)?;

    info!(
        "Initialized Condor configuration at: {}",
        config_path.display()
    );
    info!("Run \"condor\" to start or \"condor config\" to further modify the configuration.");

    Ok(())
}
