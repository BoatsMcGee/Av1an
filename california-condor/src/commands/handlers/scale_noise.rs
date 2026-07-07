use std::path::{Path, PathBuf};

use andean_condor::models::sequence::noise_scaler::NoiseScalerConfig;
use anyhow::{bail, Result};
use tracing::error;

use crate::{commands::handlers::load_configuration, configuration::Configuration, CondorCliError};

pub fn scale_noise_handler(
    config_path: Option<&Path>,
    threshold: Option<f64>,
    minimum_scaler: Option<f64>,
    maximum_scaler: Option<f64>,
    scale_chroma: bool,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    if configuration.condor.scenes.is_empty() {
        let err = CondorCliError::NoScenes;
        error!("{}", err);
        bail!(err);
    }

    configure_noise_scaler(
        &mut configuration,
        threshold,
        minimum_scaler,
        maximum_scaler,
        scale_chroma,
    )?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

pub fn configure_noise_scaler(
    configuration: &mut Configuration,
    threshold: Option<f64>,
    minimum_scaler: Option<f64>,
    maximum_scaler: Option<f64>,
    scale_chroma: bool,
) -> Result<()> {
    // Ensure noise_scaler config exists
    if configuration.condor.sequence_config.noise_scaler.is_none() {
        configuration.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig::default());
    }

    if let Some(scaler_config) = &mut configuration.condor.sequence_config.noise_scaler {
        if let Some(t) = threshold {
            scaler_config.threshold = t;
        }
        if let Some(min) = minimum_scaler {
            scaler_config.minimum_scaler = min;
        }
        if let Some(max) = maximum_scaler {
            scaler_config.maximum_scaler = max;
        }
        if scale_chroma {
            scaler_config.scale_chroma = true;
        }
    }

    Ok(())
}
