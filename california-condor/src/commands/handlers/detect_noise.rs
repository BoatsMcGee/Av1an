use std::path::{Path, PathBuf};

use andean_condor::{
    models::sequence::noise_detector::NoiseDetectorConfig,
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::Result;

use crate::{
    commands::{
        handlers::{configure_input, load_configuration},
        DecoderMethod,
    },
    configuration::Configuration,
};

pub fn detect_noise_handler(
    config_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    vs_args: Option<&[String]>,
    reference_filters: Option<&[VapourSynthFilter]>,
    denoised_filters: Option<&[VapourSynthFilter]>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    if configuration.condor.sequence_config.noise_detector.is_none() {
        configuration.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig::default());
    }

    configure_noise_detector(
        &mut configuration,
        input_path,
        decoder,
        vs_args,
        reference_filters,
        denoised_filters,
    )?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

pub fn configure_noise_detector(
    configuration: &mut Configuration,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    vs_args: Option<&[String]>,
    reference_filters: Option<&[VapourSynthFilter]>,
    denoised_filters: Option<&[VapourSynthFilter]>,
) -> Result<()> {
    // Initialize noise_detector if it doesn't exist yet
    if configuration.condor.sequence_config.noise_detector.is_none()
        && (input_path.is_some()
            || decoder.is_some()
            || vs_args.is_some()
            || reference_filters.is_some()
            || denoised_filters.is_some())
    {
        configuration.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig::default());
    }

    if input_path.is_some() || decoder.is_some() || vs_args.is_some() {
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

        if let Some(noise_detector) = configuration.condor.sequence_config.noise_detector.as_mut() {
            noise_detector.input = Some(input);
        } else {
            configuration.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig {
                input: Some(input),
                ..Default::default()
            });
        }
    };

    if let (Some(filters), Some(noise_detector)) = (
        reference_filters,
        &mut configuration.condor.sequence_config.noise_detector,
    ) {
        noise_detector.reference_filters = filters.to_vec();
    }

    if let (Some(filters), Some(noise_detector)) = (
        denoised_filters,
        &mut configuration.condor.sequence_config.noise_detector,
    ) {
        noise_detector.denoised_filters = filters.to_vec();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use andean_condor::models::input::{Input as InputModel, VapourSynthImportMethod};

    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
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
        let (config, found_config_path) =
            detect_noise_handler(Some(&config_path), None, None, None, None, None)
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
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let custom_vs_args = vec!["noise_level=2".to_string()];
        let custom_reference_filters = vec![VapourSynthFilter::WNNM {
            sigma:                Some(vec![2.0, 0.0, 0.0]),
            block_size:           None,
            block_step:           None,
            group_size:           None,
            bm_range:             None,
            radius:               None,
            ps_num:               None,
            ps_range:             None,
            residual:             None,
            adaptive_aggregation: None,
        }];
        let custom_denoised_filters = vec![VapourSynthFilter::WNNM {
            sigma:                Some(vec![4.0, 0.0, 0.0]),
            block_size:           None,
            block_step:           None,
            group_size:           None,
            bm_range:             None,
            radius:               None,
            ps_num:               None,
            ps_range:             None,
            residual:             None,
            adaptive_aggregation: None,
        }];

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig {
            input:             Some(InputModel::VapourSynth {
                path:          input_abs.clone(),
                import_method: VapourSynthImportMethod::FFMS2 {
                    index: None
                },
                cache_path:    None,
            }),
            reference_filters: custom_reference_filters.clone(),
            denoised_filters:  custom_denoised_filters.clone(),
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
            Some(&test_video.path),
            Some(&DecoderMethod::VSFFMS2),
            Some(&custom_vs_args),
            Some(&custom_reference_filters),
            Some(&custom_denoised_filters),
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
