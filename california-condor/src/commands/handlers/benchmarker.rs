use std::path::{Path, PathBuf};

use andean_condor::vapoursynth::vapoursynth_filters::VapourSynthFilter;
use anyhow::Result;

use crate::{
    commands::{
        handlers::{
            configure_input,
            configure_temp,
            encode::configure_encoder,
            load_configuration,
        },
        DecoderMethod,
        EncoderMethod,
    },
    configuration::Configuration,
};

#[allow(clippy::too_many_arguments)]
pub fn benchmarker_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    encoder: Option<&EncoderMethod>,
    passes: Option<u8>,
    params: Option<String>,
    threshold: Option<u8>,
    max_memory: Option<u32>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_temp(&mut configuration, temp_path)?;
    configure_benchmarker(
        &mut configuration,
        input_path,
        decoder,
        filters,
        vs_args,
        encoder,
        passes,
        params,
        threshold,
        max_memory,
    )?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

#[allow(clippy::too_many_arguments)]
pub fn configure_benchmarker(
    configuration: &mut Configuration,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    encoder: Option<&EncoderMethod>,
    passes: Option<u8>,
    params: Option<String>,
    threshold: Option<u8>,
    max_memory: Option<u32>,
) -> Result<()> {
    if input_path.is_some() || decoder.is_some() || vs_args.is_some() {
        let existing_input = configuration
            .condor
            .sequence_config
            .parallel_encoder
            .input
            .clone()
            .unwrap_or_else(|| configuration.condor.input.clone());
        configuration.condor.input = configure_input(
            configuration,
            &existing_input,
            input_path,
            decoder,
            vs_args,
            None,
        )?;
    };

    configure_encoder(configuration, encoder, passes, params, None, None)?;

    if let Some(filters) = filters {
        configuration.input_filters = filters.to_vec();
    }
    if let Some(threshold) = threshold {
        configuration.condor.sequence_config.benchmarker.threshold = threshold;
    }
    configuration.condor.sequence_config.benchmarker.max_memory = max_memory;

    Ok(())
}
