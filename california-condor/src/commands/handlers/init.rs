use std::path::Path;

use andean_condor::vapoursynth::vapoursynth_filters::VapourSynthFilter;
use anyhow::{Result, bail};
use tracing::{error, info};

use crate::{
    DEFAULT_CONFIG_PATH,
    commands::{
        ConcatenationMethod,
        CondorCliError,
        DecoderMethod,
        EncoderMethod,
        QualityMetric,
        handlers::{
            concatenate::configure_concatenate,
            configure_temp,
            encode::{configure_encoder, configure_parallel_encoder},
            target_quality::configure_target_quality,
        },
    },
    configuration::Configuration,
};

#[allow(clippy::too_many_arguments)]
pub fn init_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: &Path,
    output_path: &Path,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    concatenator: Option<&ConcatenationMethod>,
    workers: Option<u8>,
    encoder: Option<&EncoderMethod>,
    params: Option<&str>,
    photon_noise: Option<u32>,
    target_metric: Option<&QualityMetric>,
    target: Option<f64>,
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

    let mut configuration = Configuration::new(&input, &output, temp_path, vs_args, decoder)?;

    configure_temp(&mut configuration, temp_path)?;
    if let Some(filters) = filters {
        configuration.input_filters = filters.to_vec();
    }
    configure_encoder(
        &mut configuration,
        encoder,
        None,
        params,
        photon_noise,
        None,
    )?;
    configure_parallel_encoder(&mut configuration, None, None, None, None, workers)?;
    configure_target_quality(
        &mut configuration,
        None,
        None,
        None,
        None,
        None,
        target_metric,
        target,
        None,
        None,
        None,
    )?;
    configure_concatenate(&mut configuration, concatenator)?;

    configuration.save(&config_path)?;

    info!(
        "Initialized Condor configuration at: {}",
        config_path.display()
    );
    info!("Run \"condor\" to start or \"condor config\" to further modify the configuration.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use andean_condor::{
        core::sequence::target_quality::TargetQuality,
        models::{
            encoder::{
                Encoder,
                EncoderBase,
                EncoderPasses,
                cli_parameter::CLIParameter,
                photon_noise::PhotonNoise,
            },
            input::{Input, VapourSynthImportMethod},
            sequence::{
                scene_concatenator::ConcatMethod,
                target_quality::{TargetQualityConfig, types::QualityMetric},
            },
        },
    };
    use serial_test::serial;

    use super::*;
    use crate::{
        commands::{
            ConcatenationMethod,
            DecoderMethod,
            EncoderMethod,
            QualityMetric as QualityMetricBase,
            handlers::load_configuration,
        },
        test_helpers::{check_basic_config, default_config, get_test_video, set_cwd},
        utils::hash_path::hash_path,
    };

    #[serial(needs_cwd)]
    #[test]
    fn init_creates_default_config() {
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

        // Must run inside the temp directory so the default config path resolves there
        set_cwd(temp.path());

        let expected_config = default_config(&test_video, &output, &temp_abs);

        init_handler(
            None, // defaults to ./condor.json
            None, // defaults to directory 7-character hash of input file
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

        assert!(config_path.exists(), "config file exists");
        let (config, found_config_path) =
            load_configuration(Some(&config_path)).expect("config loads");
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
    fn init_creates_custom_config() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join("custom-temp"))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("custom-condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let custom_filters = vec![VapourSynthFilter::Crop {
            top:    Some(2),
            bottom: Some(2),
            left:   None,
            right:  None,
        }];
        let custom_vs_args = vec!["key=value".to_string()];
        let mut custom_encoder_parameters = EncoderBase::AOM.default_parameters();
        custom_encoder_parameters.insert(
            "cpu-used".to_owned(),
            CLIParameter::new_number("--", "=", 7.0),
        );

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.input_filters = custom_filters.clone();
        expected_config.condor.input = Input::VapourSynth {
            path:          input_abs,
            import_method: VapourSynthImportMethod::FFMS2 {
                index: None
            },
            cache_path:    None,
        };
        expected_config.condor.encoder = Encoder::AOM {
            executable:   None,
            pass:         EncoderPasses::All(2),
            options:      custom_encoder_parameters,
            photon_noise: Some(PhotonNoise {
                iso:        4800,
                chroma_iso: None,
                width:      None,
                height:     None,
                c_y:        None,
                ccb:        None,
                ccr:        None,
            }),
        };
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(2);
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: None,
            metric: QualityMetric::SSIMULACRA2 {
                target_range: (74.0, 76.0),
                resolution:   None,
                threads:      None,
            },
            quantizer_range: TargetQuality::default_quantizer_range(&EncoderBase::AOM),
            ..Default::default()
        });
        expected_config.condor.sequence_config.scene_concatenator.method = ConcatMethod::FFmpeg;
        // immutable shadow
        let expected_config = expected_config;

        init_handler(
            Some(&config_path),
            Some(&temp.path().join("custom-temp")),
            &test_video.path,
            &output,
            Some(&DecoderMethod::VSFFMS2),
            Some(&custom_filters),
            Some(&custom_vs_args),
            Some(&ConcatenationMethod::FFmpeg),
            Some(2),
            Some(&EncoderMethod::AOM),
            Some("--cpu-used=7"),
            Some(4800),
            Some(&QualityMetricBase::SSIMULACRA2),
            Some(75.0),
        )
        .expect("init_handler should succeed");

        assert!(config_path.exists(), "config file exists");
        let (config, found_config_path) =
            load_configuration(Some(&config_path)).expect("config loads");
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
    fn init_input_not_found() {
        let test_video = std::env::temp_dir().join("not_found.mkv");
        let input_abs = path_abs::PathAbs::new(&test_video)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");

        let result = init_handler(
            // Simulate default directory to avoid changing CWD and conflicting with other parallel
            // tests
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            &test_video,
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
        );
        assert_matches!(result, Err(_), "init_handler should fail");
        assert!(!config_path.exists(), "config file does not exist");
    }

    // TODO: Improve parameters parser
    // #[test]
    // fn init_invalid_params() {
    //     let test_video = get_test_video();
    //     let input_abs = path_abs::PathAbs::new(&test_video.path)
    //         .expect("path_abs should succeed")
    //         .as_path()
    //         .to_path_buf();
    //     let temp = tempfile::tempdir().expect("temp directory");
    //     let output = temp.path().join("out.mkv");
    //     let config_path = temp.path().join("condor.json");

    //     let result = init_handler(
    //         Some(&config_path),
    //         Some(&temp.path().join(hash_path(&input_abs))), /* Simulate
    // default directory to
    //                                                          * avoid changing
    //                                                            CWD in other
    //                                                          * parallel tests
    //                                                            */
    //         &test_video.path,
    //         &output,
    //         None,
    //         None,
    //         None,
    //         None,
    //         None,
    //         None,
    //         Some("lorem ipsum dolor sit -amet"),
    //         None,
    //         None,
    //         None,
    //     );

    //     assert_matches!(result, Err(_), "init_handler should fail");
    //     // .downcast::<>()
    //     // .expect("scale_speed_handler error should be SpeedScalerError");
    //     // assert_matches!(error, SpeedScalerError::MismatchedPairs {
    //     //     quantizers: 3,
    //     //     speeds:     2,
    //     // });

    //     assert!(!config_path.exists(), "config file does not exist");
    // }
}
