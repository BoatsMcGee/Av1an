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
