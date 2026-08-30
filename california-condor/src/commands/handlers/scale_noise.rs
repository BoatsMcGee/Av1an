use std::path::{Path, PathBuf};

use andean_condor::models::sequence::noise_scaler::NoiseScalerConfig;
use anyhow::Result;

use crate::{commands::handlers::load_configuration, configuration::Configuration};

pub fn scale_noise_handler(
    config_path: Option<&Path>,
    threshold: Option<f64>,
    minimum_scaler: Option<f64>,
    maximum_scaler: Option<f64>,
    scale_chroma: bool,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_noise_scaler(
        &mut configuration,
        threshold,
        minimum_scaler,
        maximum_scaler,
        scale_chroma,
    )?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

pub fn configure_noise_scaler(
    configuration: &mut Configuration,
    threshold: Option<f64>,
    minimum_scaler: Option<f64>,
    maximum_scaler: Option<f64>,
    scale_chroma: bool,
) -> Result<()> {
    // Ensure noise_scaler config exists
    if configuration.condor.sequence_config.noise_scaler.is_none() {
        configuration.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig::default());
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

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

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
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.scenes = test_video.mock_scenes(&expected_config.condor.encoder);
        expected_config.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig {
            threshold:      0.002,
            minimum_scaler: 4.0,
            maximum_scaler: 12.0,
            scale_chroma:   false,
        });
        // immutable shadow
        let expected_config = expected_config;

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
        // Mock scenes and save to config file (simulates Scene Detector)
        let (mut config, _) =
            load_configuration(Some(&config_path)).expect("load_config should succeed");
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("save should succeed");
        let (config, found_config_path) = scale_noise_handler(
            Some(&config_path),
            Some(0.002),
            Some(4.0),
            Some(12.0),
            false,
        )
        .expect("scale_scenes_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(!config.condor.scenes.is_empty(), "scenes is not empty");
    }
}
