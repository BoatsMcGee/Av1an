use std::path::{Path, PathBuf};

use andean_condor::{core::input::Input, vapoursynth::vapoursynth_filters::VapourSynthFilter};
use anyhow::Result;

use crate::{
    commands::{
        handlers::{configure_input, configure_temp, load_configuration},
        DecoderMethod,
        SceneDetectionMethod,
    },
    configuration::Configuration,
};

#[allow(clippy::too_many_arguments)]
pub fn detect_scenes_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    method: Option<&SceneDetectionMethod>,
    min_scene_seconds: Option<usize>,
    max_scene_seconds: Option<usize>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_temp(&mut configuration, temp_path)?;

    configure_scene_detector(
        &mut configuration,
        input_path,
        decoder,
        filters,
        vs_args,
        method,
        min_scene_seconds,
        max_scene_seconds,
    )?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

#[allow(clippy::too_many_arguments)]
pub fn configure_scene_detector(
    configuration: &mut Configuration,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    method: Option<&SceneDetectionMethod>,
    min_scene_seconds: Option<usize>,
    max_scene_seconds: Option<usize>,
) -> Result<()> {
    if input_path.is_some() || decoder.is_some() || vs_args.is_some() {
        let existing_input = configuration
            .condor
            .sequence_config
            .scene_detector
            .input
            .clone()
            .unwrap_or_else(|| configuration.condor.input.clone());
        let scd_input = configure_input(
            configuration,
            &existing_input,
            input_path,
            decoder,
            vs_args,
            None,
        )?;
        configuration.condor.sequence_config.scene_detector.input = Some(scd_input);
    };
    let mut input = Input::from_data(
        configuration
            .condor
            .sequence_config
            .scene_detector
            .input
            .as_ref()
            .unwrap_or(&configuration.condor.input),
    )?;
    let clip_info = input.clip_info()?;
    let fps = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;

    let previous_method = configuration.condor.sequence_config.scene_detector.method;
    let min_scene_frames = min_scene_seconds.map_or_else(
        || previous_method.minimum_length(),
        |seconds| (fps * seconds as f64).round() as usize,
    );
    let max_scene_frames = max_scene_seconds.map_or_else(
        || previous_method.maximum_length(),
        |seconds| (fps * seconds as f64).round() as usize,
    );
    let new_method =
        method.map(|method| method.as_core_method(Some(min_scene_frames), Some(max_scene_frames)));
    if let Some(new_method) = new_method {
        configuration.condor.sequence_config.scene_detector.method = new_method;
    }
    configuration
        .condor
        .sequence_config
        .scene_detector
        .method
        .set_minimum_length(min_scene_frames)?;
    configuration
        .condor
        .sequence_config
        .scene_detector
        .method
        .set_maximum_length(max_scene_frames)?;
    if let Some(filters) = filters {
        configuration.scd_input_filters = filters.to_vec();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use andean_condor::{
        models::{
            input::{Input, VapourSynthImportMethod},
            sequence::scene_detector::{
                SceneDetectionMethod as SceneDetectionMethodModel,
                ScenecutMethod,
            },
        },
        vapoursynth::plugins::resize::Scaler,
    };

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
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))), /* Simulate default directory to
                                                             * avoid changing CWD in other
                                                             * parallel tests */
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
        let (config, found_config_path) = detect_scenes_handler(
            Some(&config_path),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("detect_scenes_handler should succeed");

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
    fn detect_scenes_custom_config() {
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
        let custom_filters = vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Point),
            width:  Some(960),
            height: Some(540),
            format: None,
        }];
        let min_scene_seconds = 2usize;
        let max_scene_seconds = 12usize;
        let expected_min = (test_video.fps() * min_scene_seconds as f64).round() as usize;
        let expected_max = (test_video.fps() * max_scene_seconds as f64).round() as usize;

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.scd_input_filters = custom_filters.clone();
        expected_config.condor.sequence_config.scene_detector.input = Some(Input::VapourSynth {
            path:          input_abs.clone(),
            import_method: VapourSynthImportMethod::FFMS2 {
                index: None
            },
            cache_path:    None,
        });
        expected_config.condor.sequence_config.scene_detector.method =
            SceneDetectionMethodModel::AVSceneChange {
                minimum_length: expected_min,
                maximum_length: expected_max,
                method:         ScenecutMethod::Fast,
            };
        // immutable shadow
        let expected_config = expected_config;

        init_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))), /* Simulate default directory to
                                                             * avoid changing CWD in other
                                                             * parallel tests */
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
        let (config, found_config_path) = detect_scenes_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&test_video.path),
            Some(&DecoderMethod::VSFFMS2),
            Some(&custom_filters),
            Some(&["method=scd".to_owned()]),
            Some(&SceneDetectionMethod::Fast),
            Some(min_scene_seconds),
            Some(max_scene_seconds),
        )
        .expect("detect_scenes_handler should succeed");

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
