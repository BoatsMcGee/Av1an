use std::path::{Path, PathBuf};

use andean_condor::models::{
    input::Input as InputModel,
    sequence::noise_detector::NoiseDetectorConfig,
};
use anyhow::{bail, Result};
use tracing::error;

use crate::{
    commands::{
        handlers::{configure_input, load_configuration},
        CondorCliError,
    },
    configuration::Configuration,
};

pub fn detect_noise_handler(
    config_path: Option<&Path>,
    input_path: Option<&Path>,
    vs_args: Option<&[String]>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

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

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs};

    use andean_condor::models::input::VapourSynthScriptSource;

    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video, vapoursynth_script},
        utils::hash_path::hash_path,
    };

    #[test]
    fn detect_noise_default_config() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();

        let expected_config = default_config(&test_video, &output, &temp_abs);

        init_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))), /* Simulate default directory to
                                                             * avoid changing CWD */
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("init_handler should succeed");
        let (config, found_config_path) = detect_noise_handler(Some(&config_path), None, None)
            .expect("detect_noise_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    #[test]
    fn detect_noise_custom_config() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let script_input = temp.path().join("condor-test-script.vpy");
        let script_input_abs = path_abs::PathAbs::new(&script_input)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let custom_vs_args = vec!["noise_level=2".to_string()];

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        let vpy_script = vapoursynth_script(&test_video, Some(&expected_config.input_filters));
        // Save the VapourSynth script to the temp directory
        fs::write(&script_input, vpy_script).expect("write should succeed");
        let mut vpy_args = HashMap::new();
        vpy_args.insert("noise_level".to_owned(), "2".to_owned());
        expected_config.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig {
            input: Some(InputModel::VapourSynthScript {
                source:    VapourSynthScriptSource::Path(script_input_abs),
                variables: vpy_args,
                index:     0,
            }),
        });
        // immutable shadow
        let expected_config = expected_config;

        init_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("init_handler should succeed");
        // // Mock scenes and save to config file (simulates Scene Detector)
        // let (mut config, _) =
        //     load_configuration(Some(&config_path)).expect("load_config should
        // succeed"); config.condor.scenes =
        // test_video.mock_scenes(&config.condor.encoder); config.save(&
        // config_path).expect("save should succeed");
        let (config, found_config_path) = detect_noise_handler(
            Some(&config_path),
            Some(&script_input),
            Some(&custom_vs_args),
        )
        .expect("detect_noise_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }
}
