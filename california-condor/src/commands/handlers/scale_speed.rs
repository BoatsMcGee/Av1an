use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use thiserror::Error;
use tracing::error;

use crate::{commands::handlers::load_configuration, configuration::Configuration, CondorCliError};

pub fn scale_speed_handler(
    config_path: Option<&Path>,
    quantizers: Option<&[i8]>,
    speeds: Option<&[i8]>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    if configuration.condor.scenes.is_empty() {
        let err = CondorCliError::NoScenes;
        error!("{}", err);
        bail!(err);
    }

    configure_scale_speed(&mut configuration, quantizers, speeds)?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

pub fn configure_scale_speed(
    configuration: &mut Configuration,
    quantizers: Option<&[i8]>,
    speeds: Option<&[i8]>,
) -> Result<()> {
    let (existing_quantizers, existing_speeds): (Vec<i8>, Vec<i8>) = configuration
        .condor
        .sequence_config
        .speed_scaler
        .speed_quantizers
        .iter()
        .map(|(s, q)| (*q as i8, *s))
        .unzip();
    let quantizers = quantizers.unwrap_or(existing_quantizers.as_slice());
    let speeds = speeds.unwrap_or(existing_speeds.as_slice());
    if quantizers.len() != speeds.len() {
        let err = SpeedScalerError::MismatchedPairs {
            quantizers: quantizers.len(),
            speeds:     speeds.len(),
        };
        error!("{}", err);
        bail!(err);
    }

    if quantizers.len() < 2 {
        let err = SpeedScalerError::MinimumPairsRequired;
        error!("{}", err);
        bail!(err);
    }

    let speed_quantizers: Vec<(i8, f64)> =
        speeds.iter().zip(quantizers).map(|(s, q)| (*s, *q as f64)).collect();

    configuration.condor.sequence_config.speed_scaler.speed_quantizers = speed_quantizers;

    Ok(())
}

#[derive(Debug, Error)]
pub enum SpeedScalerError {
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
