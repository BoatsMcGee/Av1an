use std::path::{Path, PathBuf};

use andean_condor::{
    models::encoder::{photon_noise::PhotonNoise, Encoder, EncoderBase, EncoderPasses},
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::{bail, Result};
use tracing::error;

use crate::{
    commands::{
        handlers::{configure_input, configure_temp, load_configuration},
        DecoderMethod,
        EncoderMethod,
    },
    configuration::Configuration,
    utils::parameter_parser::EncoderParamsParser,
    CondorCliError,
};

#[allow(clippy::too_many_arguments)]
pub fn encode_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    workers: Option<u8>,
    encoder: Option<&EncoderMethod>,
    passes: Option<u8>,
    params: Option<String>,
    photon_noise: Option<u32>,
    chroma_noise: Option<u32>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    if configuration.condor.scenes.is_empty() {
        let err = CondorCliError::NoScenes;
        error!("{}", err);
        bail!(err);
    }

    configure_temp(&mut configuration, temp_path)?;
    configure_parallel_encoder(
        &mut configuration,
        input_path,
        decoder,
        filters,
        vs_args,
        workers,
        encoder,
        passes,
        params,
        photon_noise,
        chroma_noise,
    )?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

#[allow(clippy::too_many_arguments)]
pub fn configure_parallel_encoder(
    configuration: &mut Configuration,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    workers: Option<u8>,
    encoder: Option<&EncoderMethod>,
    passes: Option<u8>,
    params: Option<String>,
    photon_noise: Option<u32>,
    chroma_noise: Option<u32>,
) -> Result<()> {
    if input_path.is_some() || decoder.is_some() || vs_args.is_some() {
        let existing_input = configuration
            .condor
            .sequence_config
            .parallel_encoder
            .input
            .clone()
            .unwrap_or_else(|| configuration.condor.input.clone());
        let input = configure_input(
            configuration,
            &existing_input,
            input_path,
            decoder,
            vs_args,
            None,
        )?;
        configuration.condor.sequence_config.parallel_encoder.input = Some(input);
    }

    if let Some(filters) = filters {
        configuration.input_filters = filters.to_vec();
    }

    configure_encoder(
        configuration,
        encoder,
        passes,
        params,
        photon_noise,
        chroma_noise,
    )?;

    if let Some(workers) = workers {
        configuration.condor.sequence_config.parallel_encoder.workers = Some(workers);
    }

    Ok(())
}

pub fn configure_encoder(
    configuration: &mut Configuration,
    encoder: Option<&EncoderMethod>,
    passes: Option<u8>,
    params: Option<String>,
    photon_noise: Option<u32>,
    chroma_noise: Option<u32>,
) -> Result<()> {
    if let Some(encoder) = encoder {
        let encoder = encoder.as_encoder_base();
        let options = encoder.default_parameters();
        let pass = encoder.default_passes();
        configuration.condor.encoder = match encoder {
            EncoderBase::AOM => Encoder::AOM {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::RAV1E => Encoder::RAV1E {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::VPX => Encoder::VPX {
                executable: None,
                pass,
                options,
            },
            EncoderBase::SVTAV1 => Encoder::SVTAV1 {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::AVM => Encoder::AVM {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::X264 => Encoder::X264 {
                executable: None,
                pass,
                options,
            },
            EncoderBase::X265 => Encoder::X265 {
                executable: None,
                pass,
                options,
            },
            EncoderBase::VVenC => Encoder::VVenC {
                executable: None,
                pass,
                options,
            },
            EncoderBase::FFmpeg => Encoder::FFmpeg {
                executable: None,
                options,
            },
        }
    }
    if let Some(passes) = passes
        && let Some(encoder_passes) = configuration.condor.encoder.passes_mut()
    {
        *encoder_passes = EncoderPasses::All(passes);
    }
    if let Some(params) = params {
        let parameters = EncoderParamsParser::parse_string(&params);
        configuration.condor.encoder.parameters_mut().extend(parameters);
    }
    if let Some(iso) = photon_noise {
        // TODO: Support chroma noise only
        configuration.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso,
            chroma_iso: chroma_noise,
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
    }

    Ok(())
}
