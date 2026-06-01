use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use tracing::{debug, error};

use crate::{
    configuration::{ConfigError, Configuration},
    CondorCliError,
    DEFAULT_CONFIG_PATH,
};

pub fn optimize_bitrate_handler(
    config_path: Option<&Path>,
    sigma_threshold: Option<u8>,
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

    if let Some(threshold) = sigma_threshold {
        configuration.condor.sequence_config.bitrate_optimizer.bitrate_sigma_threshold =
            Some(threshold);
    }

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}
