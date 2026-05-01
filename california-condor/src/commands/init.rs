use std::path::Path;

use andean_condor::models::{
    encoder::{photon_noise::PhotonNoise, Encoder, EncoderBase, EncoderPasses},
    input::{ImportMethod, Input as InputModel, VapourSynthImportMethod},
    sequence::scene_concatenator::ConcatMethod,
};
use anyhow::{bail, Result};
use tracing::{error, info};

use crate::{
    commands::DecoderMethod,
    configuration::Configuration,
    utils::parameter_parser::EncoderParamsParser,
    CondorCliError,
    DEFAULT_CONFIG_PATH,
};

#[allow(clippy::too_many_arguments)]
pub fn init_handler(
    config_path: Option<&Path>,
    input_path: &Path,
    output_path: &Path,
    temp_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    vs_args: Option<&[String]>,
    encoder: Option<&EncoderBase>,
    passes: Option<u8>,
    params: Option<String>,
    photon_noise: Option<u32>,
    chroma_noise: Option<u32>,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let input = path_abs::PathAbs::new(input_path)?.as_path().to_path_buf();
    let output = path_abs::PathAbs::new(output_path)?.as_path().to_path_buf();
    let config_path = path_abs::PathAbs::new(
        config_path.map_or_else(|| cwd.join(DEFAULT_CONFIG_PATH), |p| p.to_path_buf()),
    )?
    .as_path()
    .to_path_buf();

    if config_path.exists() {
        let err = CondorCliError::ConfigFileAlreadyExists(config_path);
        error!("{}", err);
        bail!(err);
    }

    let mut configuration = Configuration::new(&input, &output, temp_path, vs_args)?;

    if let Some(decoder) = &decoder {
        match decoder {
            DecoderMethod::FFMS2 => {
                configuration.condor.input = InputModel::Video {
                    path:          input_path.to_path_buf(),
                    import_method: ImportMethod::FFMS2 {
                        index: None
                    },
                };
            },
            vs_decoders => {
                configuration.condor.input = InputModel::VapourSynth {
                    path:          input_path.to_path_buf(),
                    import_method: match vs_decoders {
                        DecoderMethod::BestSource => VapourSynthImportMethod::BestSource {
                            index: None,
                        },
                        DecoderMethod::VSFFMS2 => VapourSynthImportMethod::FFMS2 {
                            index: None
                        },
                        DecoderMethod::LSMASHWorks => VapourSynthImportMethod::LSMASHWorks {
                            index: None,
                        },
                        DecoderMethod::DGDecodeNV => VapourSynthImportMethod::DGDecNV {
                            dgindexnv_executable: None,
                        },
                        DecoderMethod::FFMS2 => unreachable!(),
                    },
                    cache_path:    None,
                };
            },
        };
    } else {
        configuration.condor.input = InputModel::VapourSynth {
            path:          input,
            import_method: VapourSynthImportMethod::BestSource {
                index: None
            },
            cache_path:    None,
        };
    }
    configuration.condor.sequence_config.scene_concatenator.method = ConcatMethod::MKVMerge;
    if let Some(encoder) = encoder {
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

    configuration.save(&config_path)?;

    info!(
        "Initialized Condor configuration at: {}",
        config_path.display()
    );
    info!(
        "Run \"condor start\" to start encoding or \"condor config\" to further modify the \
         configuration."
    );

    Ok(())
}
