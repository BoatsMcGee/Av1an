use std::path::{Path, PathBuf};

use andean_condor::models::input::{
    ImportMethod,
    Input as InputModel,
    VapourSynthImportMethod,
    VapourSynthScriptSource,
};
use anyhow::{Result, bail};
use tracing::{debug, error};

use crate::{
    DEFAULT_CONFIG_PATH,
    commands::{CondorCliError, DecoderMethod},
    configuration::{ConfigError, Configuration},
};

pub mod benchmarker;
pub mod concatenate;
// pub mod config;
pub mod detect_noise;
pub mod detect_scenes;
pub mod encode;
pub mod init;
pub mod optimize_bitrate;
pub mod quality_check;
pub mod scale_noise;
pub mod scale_speed;
pub mod start;
pub mod target_quality;

pub fn load_configuration(config_path: Option<&Path>) -> Result<(Configuration, PathBuf)> {
    if let Some(config_path) = config_path
        && !config_path.exists()
    {
        let err = CondorCliError::ConfigFileNotFound(config_path.to_path_buf());
        error!("{}", err);
        bail!(err);
    }
    let config_path =
        path_abs::PathAbs::new(config_path.unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH)))?
            .as_path()
            .to_path_buf();
    if !config_path.exists() {
        let err = CondorCliError::NoConfig;
        error!("{}", err);
        bail!(err);
    }

    let configuration = {
        debug!("Loading existing configuration");
        match Configuration::load(&config_path) {
            Ok(config) => config.expect("Config should exist"),
            Err(err) => match err {
                ConfigError::Load(path) => {
                    let err = CondorCliError::ConfigLoadError(path);
                    error!("{}", err);
                    bail!(err);
                },
                ConfigError::Parse(err) => {
                    let err = CondorCliError::ConfigParseError(err);
                    error!("{}", err);
                    bail!(err);
                },
                ConfigError::Serialize(err) => {
                    let err = CondorCliError::ConfigSerializeError(err);
                    error!("{}", err);
                    bail!(err);
                },
                ConfigError::Save(err) => {
                    let err = CondorCliError::ConfigSaveError(err);
                    error!("{}", err);
                    bail!(err);
                },
            },
        }
    };

    Ok((configuration, config_path))
}

pub fn configure_temp(configuration: &mut Configuration, temp_path: Option<&Path>) -> Result<()> {
    if let Some(temp_path) = temp_path {
        configuration.temp = path_abs::PathAbs::new(temp_path)?.as_path().to_path_buf();
    }

    Ok(())
}

pub fn configure_input(
    configuration: &Configuration,
    existing_input: &InputModel,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    vs_args: Option<&[String]>,
    index: Option<u8>,
    // cache_path: Option<&Path>,
) -> Result<InputModel> {
    let (existing_input_path, existing_decoder, existing_vs_args, existing_index) =
        match existing_input {
            InputModel::Video {
                path,
                import_method,
            } => match import_method {
                ImportMethod::FFMS2 {
                    index,
                } => (path, Some(DecoderMethod::FFMS2), None, *index),
            },
            InputModel::VapourSynth {
                path,
                import_method,
                ..
            } => match import_method {
                VapourSynthImportMethod::LSMASHWorks {
                    index,
                } => (path, Some(DecoderMethod::LSMASHWorks), None, *index),
                VapourSynthImportMethod::DGDecNV {
                    ..
                } => (path, Some(DecoderMethod::DGDecodeNV), None, None),
                VapourSynthImportMethod::FFMS2 {
                    index,
                } => (path, Some(DecoderMethod::VSFFMS2), None, *index),
                VapourSynthImportMethod::BestSource {
                    index,
                } => (path, Some(DecoderMethod::BestSource), None, *index),
            },
            InputModel::VapourSynthScript {
                source,
                variables,
                index,
            } => match source {
                VapourSynthScriptSource::Path(path) => {
                    (path, None, Some(variables.clone()), Some(*index))
                },
                VapourSynthScriptSource::Text(_) => (
                    &configuration.input,
                    None,
                    Some(variables.clone()),
                    Some(*index),
                ),
            },
        };

    let existing_vs_args: Option<Vec<String>> = existing_vs_args
        .map(|args| args.iter().map(|(key, value)| format!("{}={}", key, value)).collect());

    Configuration::new_input_model(
        path_abs::PathAbs::new(input_path.unwrap_or(existing_input_path))?.as_path(),
        decoder.or(existing_decoder.as_ref()),
        vs_args.or(existing_vs_args.as_deref()),
        index.or(existing_index),
        // cache_path: None, // TODO: Support Cache Path
    )
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    #[test]
    fn invalid_json_reports_parse_error() {
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("condor.json");
        std::fs::write(&config_path, "{ invalid JSON").expect("config file writes to disk");

        let error = load_configuration(Some(&config_path))
            .expect_err("invalid JSON should fail to load")
            .downcast::<CondorCliError>()
            .expect("load_configuration error should be CondorCliError");
        let error_string = format!("{error:#}");

        assert_matches!(
            error,
            CondorCliError::ConfigParseError(_),
            "invalid JSON should be ConfigParseError"
        );
        assert!(
            error_string.contains("line"),
            "error should include the JSON parse detail, got: {error_string}"
        );
    }
}
