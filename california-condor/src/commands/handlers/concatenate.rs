use std::path::{Path, PathBuf};

use andean_condor::models::sequence::scene_concatenator::ConcatMethod;
use anyhow::{bail, Result};
use tracing::error;

use crate::{
    commands::{
        handlers::{configure_temp, load_configuration},
        ConcatenationMethod,
    },
    configuration::Configuration,
    CondorCliError,
};

#[allow(clippy::too_many_arguments)]
pub fn concatenate_handler(
    temp_path: Option<&Path>,
    config_path: Option<&Path>,
    method: Option<&ConcatenationMethod>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    if configuration.condor.scenes.is_empty() {
        let err = CondorCliError::NoScenes;
        error!("{}", err);
        bail!(err);
    }

    configure_temp(&mut configuration, temp_path)?;
    configure_concatenate(&mut configuration, method)?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

pub fn configure_concatenate(
    configuration: &mut Configuration,
    method: Option<&ConcatenationMethod>,
) -> Result<()> {
    if let Some(concat) = method {
        let concat = match concat {
            ConcatenationMethod::MkvMerge => ConcatMethod::MKVMerge,
            ConcatenationMethod::FFmpeg => ConcatMethod::FFmpeg,
            ConcatenationMethod::Ivf => ConcatMethod::Ivf,
        };
        configuration.condor.sequence_config.scene_concatenator.method = concat;
    }

    Ok(())
}
