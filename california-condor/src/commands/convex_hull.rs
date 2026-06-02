use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use thiserror::Error;
use tracing::{debug, error};

use crate::{
    configuration::{ConfigError, Configuration},
    CondorCliError,
    DEFAULT_CONFIG_PATH,
};

pub fn convex_hull_handler(
    config_path: Option<&Path>,
    quantizers: Vec<i8>,
    speeds: Vec<i8>,
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

    if quantizers.len() != speeds.len() {
        let err = ConvexHullError::MismatchedPairs {
            quantizers: quantizers.len(),
            speeds:     speeds.len(),
        };
        error!("{}", err);
        bail!(err);
    }

    if quantizers.len() < 2 {
        let err = ConvexHullError::MinimumPairsRequired;
        error!("{}", err);
        bail!(err);
    }

    let speed_quantizers: Vec<(i8, f64)> =
        speeds.into_iter().zip(quantizers).map(|(s, q)| (s, q as f64)).collect();

    configuration.condor.sequence_config.convex_hull.speed_quantizers = speed_quantizers;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

#[derive(Debug, Error)]
pub enum ConvexHullError {
    #[error(
        "Mismatched speed-quantizer pairs: got {quantizers} quantizer(s) and {speeds} speed(s). \
         Each --quantizer must have a matching --speed value."
    )]
    MismatchedPairs {
        quantizers: usize,
        speeds:     usize,
    },
    #[error("At least 2 speed-quantizer pairs are required.")]
    MinimumPairsRequired,
}
