use std::path::{Path, PathBuf};

use andean_condor::models::{
    input::Input as InputModel,
    sequence::noise_detector::NoiseDetectorConfig,
};
use anyhow::{bail, Result};
use tracing::error;

use crate::{
    commands::handlers::{configure_input, load_configuration},
    configuration::Configuration,
    CondorCliError,
};

pub fn detect_noise_handler(
    config_path: Option<&Path>,
    input_path: Option<&Path>,
    vs_args: Option<&[String]>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    if configuration.condor.scenes.is_empty() {
        let err = CondorCliError::NoScenes;
        error!("{}", err);
        bail!(err);
    }

    configure_noise_detector(&mut configuration, input_path, vs_args)?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

pub fn configure_noise_detector(
    configuration: &mut Configuration,
    input_path: Option<&Path>,
    vs_args: Option<&[String]>,
) -> Result<()> {
    if input_path.is_some() || vs_args.is_some() {
        let existing_input = if let Some(Some(input)) = configuration
            .condor
            .sequence_config
            .noise_detector
            .as_ref()
            .map(|nd| nd.input.clone())
        {
            input
        } else {
            configuration.condor.input.clone()
        };
        let input = configure_input(
            configuration,
            &existing_input,
            input_path,
            None,
            vs_args,
            None,
        )?;

        if !matches!(input, InputModel::VapourSynthScript { .. }) {
            let err = CondorCliError::InvalidVapourSynthScript(
                input_path.unwrap_or_else(|| Path::new("")).to_path_buf(),
            );
            error!("{}", err);
            bail!(err);
        }

        if let Some(noise_detector) = configuration.condor.sequence_config.noise_detector.as_mut() {
            noise_detector.input = Some(input);
        } else {
            configuration.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig {
                input: Some(input),
            });
        }
    };

    Ok(())
}
