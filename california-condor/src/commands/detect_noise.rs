use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use andean_condor::models::input::{Input as InputModel, VapourSynthScriptSource};
use anyhow::{bail, Result};
use tracing::{debug, error};

use crate::{
    configuration::{ConfigError, Configuration},
    CondorCliError,
    DEFAULT_CONFIG_PATH,
};

pub fn detect_noise_handler(
    config_path: Option<&Path>,
    input_path: Option<&PathBuf>,
    vs_args: Option<&[String]>,
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

    if let Some(input) = input_path {
        let is_script = input
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e, "vpy" | "py"));
        if !is_script {
            let err = CondorCliError::InvalidVapourSynthScript(input.clone());
            error!("{}", err);
            bail!(err);
        }
        let input = path_abs::PathAbs::new(input)?.as_path().to_path_buf();
        let variables = vs_args.map_or_else(HashMap::new, |args| {
            args.iter()
                .map(|arg| {
                    let (key, value) = arg.split_once('=').unwrap_or((arg, ""));
                    (key.to_string(), value.to_string())
                })
                .collect()
        });
        let input_model = InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(input),
            variables,
            index: 0,
        };
        configuration.condor.sequence_config.noise_detector = Some(
            andean_condor::models::sequence::noise_detector::NoiseDetectorConfig {
                input: Some(input_model),
            },
        );
    }

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}
