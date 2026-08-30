use std::path::{Path, PathBuf};

use andean_condor::models::sequence::scene_concatenator::ConcatMethod;
use anyhow::Result;

use crate::{
    commands::{
        ConcatenationMethod,
        handlers::{configure_temp, load_configuration},
    },
    configuration::Configuration,
};

#[allow(clippy::too_many_arguments)]
pub fn concatenate_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    method: Option<&ConcatenationMethod>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn detect_scenes_default_config() {
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
        let (config, found_config_path) = concatenate_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
            None,
            None,
        )
        .expect("concatenate_handler should succeed");

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
    fn concatenate_custom_config() {
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
        expected_config.condor.sequence_config.scene_concatenator.method = ConcatMethod::FFmpeg;
        expected_config.condor.sequence_config.scene_concatenator.scenes_directory =
            temp_abs.join("scenes");
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
        let (config, found_config_path) = concatenate_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&ConcatenationMethod::FFmpeg),
        )
        .expect("concatanate_handler should succeed");

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
