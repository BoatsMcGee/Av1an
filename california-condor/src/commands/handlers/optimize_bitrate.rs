use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use tracing::error;

use crate::{commands::handlers::load_configuration, configuration::Configuration, CondorCliError};

pub fn optimize_bitrate_handler(
    config_path: Option<&Path>,
    sigma_threshold: Option<u8>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    if configuration.condor.scenes.is_empty() {
        let err = CondorCliError::NoScenes;
        error!("{}", err);
        bail!(err);
    }

    configure_bitrate_optimizer(&mut configuration, sigma_threshold)?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

pub fn configure_bitrate_optimizer(
    configuration: &mut Configuration,
    sigma_threshold: Option<u8>,
) -> Result<()> {
    if let Some(threshold) = sigma_threshold {
        configuration.condor.sequence_config.bitrate_optimizer.bitrate_sigma_threshold =
            Some(threshold);
    }

    Ok(())
}
