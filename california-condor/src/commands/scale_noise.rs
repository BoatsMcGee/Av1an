use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use tracing::{debug, error};

use crate::{
    configuration::{ConfigError, Configuration},
    CondorCliError,
    DEFAULT_CONFIG_PATH,
};

pub fn scale_noise_handler(
    config_path: Option<&Path>,
    threshold: Option<f64>,
    minimum_scaler: Option<f64>,
    maximum_scaler: Option<f64>,
    scale_chroma: bool,
) -> Result<(Configuration, PathBuf)> {
    let config_path =
        path_abs::PathAbs::new(config_path.unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH)))?
            .as_path()
            .to_path_buf();

    if !config_path.exists() {
        let err = CondorCliError::NoConfig;
        error!("{}", err);
        bail!(err);
    }

    debug!("Loading existing configuration");
    let mut configuration = match Configuration::load(&config_path) {
        Ok(config) => config.expect("Config should exist"),
        Err(err) => match err {
            ConfigError::Load(path) => {
                let err = CondorCliError::ConfigLoadError(path);
                error!("{}", err);
                bail!(err);
            },
            _ => unreachable!("ConfigError should be LoadError"),
        },
    };

    if configuration.condor.scenes.is_empty() {
        let err = CondorCliError::NoScenes;
        error!("{}", err);
        bail!(err);
    }

    // Ensure noise_scaler config exists
    if configuration.condor.sequence_config.noise_scaler.is_none() {
        configuration.condor.sequence_config.noise_scaler =
            Some(andean_condor::models::sequence::noise_scaler::NoiseScalerConfig::default());
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

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}
