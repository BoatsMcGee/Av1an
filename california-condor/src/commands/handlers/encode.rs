use std::path::{Path, PathBuf};

use andean_condor::{
    models::encoder::{photon_noise::PhotonNoise, Encoder, EncoderBase, EncoderPasses},
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::Result;

use crate::{
    commands::{
        handlers::{configure_input, configure_temp, load_configuration},
        DecoderMethod,
        EncoderMethod,
    },
    configuration::Configuration,
    utils::parameter_parser::EncoderParamsParser,
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
    params: Option<&str>,
    photon_noise: Option<u32>,
    chroma_noise: Option<u32>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

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
    params: Option<&str>,
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
    params: Option<&str>,
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
        let parameters = EncoderParamsParser::parse_string(params)?;
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

#[cfg(test)]
mod tests {
    use andean_condor::models::{
        encoder::cli_parameter::CLIParameter,
        input::{Input, VapourSynthImportMethod},
        sequence::parallel_encoder::ParallelEncoderConfig,
    };

    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn encode_default_config() {
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
        let (config, found_config_path) = encode_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
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
            None,
        )
        .expect("encode_handler should succeed");

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
    fn encode_custom_config() {
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
        let custom_filters = vec![VapourSynthFilter::Crop {
            top:    Some(140),
            bottom: Some(140),
            left:   None,
            right:  None,
        }];
        let custom_vs_args = vec!["method=target quality".to_string()];

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.input_filters = custom_filters.clone();
        expected_config.condor.encoder = Encoder::default_from_base(&EncoderBase::RAV1E, false);
        expected_config.condor.encoder.parameters_mut().insert(
            "speed".to_owned(),
            CLIParameter::Number {
                prefix:    "--".to_owned(),
                delimiter: " ".to_owned(),
                value:     10.0,
            },
        );
        if let Some(encoder_passes) = expected_config.condor.encoder.passes_mut() {
            *encoder_passes = EncoderPasses::All(2);
        }
        expected_config.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso:        1600,
            chroma_iso: Some(400),
            width:      None,
            height:     None,
            c_y:        None,
            ccb:        None,
            ccr:        None,
        }));
        expected_config.condor.sequence_config.parallel_encoder = ParallelEncoderConfig {
            input: Some(Input::VapourSynth {
                path:          input_abs.clone(),
                import_method: VapourSynthImportMethod::DGDecNV {
                    dgindexnv_executable: None,
                },
                cache_path:    None,
            }),
            workers: Some(2),
            scenes_directory: temp_abs.join("scenes"),
            ..Default::default()
        };
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
        let (config, found_config_path) = encode_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
            None,
            Some(&test_video.path),
            Some(&DecoderMethod::DGDecodeNV),
            Some(&custom_filters),
            Some(&custom_vs_args),
            Some(2),
            Some(&EncoderMethod::RAV1E),
            Some(2),
            Some("--speed 10"),
            Some(1600),
            Some(400),
        )
        .expect("encode_handler should succeed");

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
