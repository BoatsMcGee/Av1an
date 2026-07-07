use std::{
    panic,
    path::{Path, PathBuf},
    process,
};

use andean_condor::core::Condor;
use anyhow::Result;
use clap::{CommandFactory, FromArgMatches};
use thiserror::Error;
use tracing::{debug, info, level_filters::LevelFilter};

use crate::{
    commands::{
        config::config_sub_handler,
        handlers::{
            benchmarker::benchmarker_handler,
            concatenate::concatenate_handler,
            detect_noise::detect_noise_handler,
            detect_scenes::detect_scenes_handler,
            encode::encode_handler,
            init::init_handler,
            optimize_bitrate::optimize_bitrate_handler,
            scale_noise::scale_noise_handler,
            scale_speed::scale_speed_handler,
            start::start_handler,
            target_quality::target_quality_handler,
        },
        help_text::process_command_tree,
        Commands,
        CondorCli,
    },
    configuration::Configuration,
    logging::init_logging,
    tui::{
        run_benchmarker_tui,
        run_bitrate_optimizer_tui,
        run_noise_detector_tui,
        run_noise_scaler_tui,
        run_parallel_encoder_tui,
        run_scene_concatenator_tui,
        run_scene_detector_tui,
        run_speed_scaler_tui,
        run_target_quality_tui,
    },
};

mod apps;
mod commands;
mod components;
mod configuration;
mod logging;
mod tui;
mod utils;

pub const DEFAULT_CONFIG_PATH: &str = "./condor.json";
pub const DEFAULT_LOG_PATH: &str = "./logs/condor.log";

fn main() -> anyhow::Result<()> {
    let orig_hook = panic::take_hook();
    // Catch panics in child threads
    panic::set_hook(Box::new(move |panic_info| {
        orig_hook(panic_info);
        process::exit(1);
    }));
    run()
}

fn run() -> anyhow::Result<()> {
    let mut cmd = CondorCli::command();
    cmd = process_command_tree(cmd);
    let matches = cmd.get_matches();
    let cli = CondorCli::from_arg_matches(&matches).map_err(|err| err.exit())?;

    let cwd = std::env::current_dir()?;
    let config_path = cli.config_file;
    let logs = cli.logs.unwrap_or_else(|| cwd.join(DEFAULT_LOG_PATH));
    let set_logs = |log_path: &Path| {
        init_logging(
            LevelFilter::INFO,
            log_path,
            if cli.verbose {
                LevelFilter::TRACE
            } else {
                LevelFilter::DEBUG
            },
        )
    };
    set_logs(&logs)?;
    let temp = cli.temp;

    match cli.command {
        Some(Commands::Init {
            input,
            output,
            decoder,
            filters,
            vs_args,
            concat,
            workers,
            encoder,
            params,
            photon_noise,
            target_metric,
            target,
        }) => {
            init_handler(
                config_path.as_deref(),
                temp.as_deref(),
                input.as_path(),
                output.as_path(),
                decoder.as_ref(),
                filters.as_deref(),
                vs_args.as_deref(),
                concat.as_ref(),
                workers,
                encoder.as_ref(),
                params,
                photon_noise,
                target_metric,
                target,
            )?;
        },
        Some(Commands::DetectScenes {
            input,
            decoder,
            filters,
            vs_args,
            method,
            min_scene_seconds,
            max_scene_seconds,
        }) => {
            let (configuration, save_file) = detect_scenes_handler(
                config_path.as_deref(),
                temp.as_deref(),
                input.as_deref(),
                decoder.as_ref(),
                filters.as_deref(),
                vs_args.as_deref(),
                method.as_ref(),
                min_scene_seconds,
                max_scene_seconds,
            )?;

            run_scene_detector(&configuration, &save_file)?;
        },
        Some(Commands::Benchmark {
            input,
            decoder,
            filters,
            vs_args,
            encoder,
            passes,
            params,
            threshold,
            max_memory,
        }) => {
            let (configuration, save_file) = benchmarker_handler(
                config_path.as_deref(),
                temp.as_deref(),
                input.as_deref(),
                decoder.as_ref(),
                filters.as_deref(),
                vs_args.as_deref(),
                encoder.as_ref(),
                passes,
                params,
                threshold,
                max_memory,
            )?;

            run_benchmarker(&configuration, &save_file)?;
        },
        Some(Commands::DetectNoise {
            input,
            vs_args,
        }) => {
            let (configuration, save_file) =
                detect_noise_handler(config_path.as_deref(), input.as_deref(), vs_args.as_deref())?;

            run_noise_detector(&configuration, &save_file)?;
        },
        Some(Commands::ScaleNoise {
            threshold,
            minimum_scaler,
            maximum_scaler,
            scale_chroma,
        }) => {
            let (configuration, save_file) = scale_noise_handler(
                config_path.as_deref(),
                threshold,
                minimum_scaler,
                maximum_scaler,
                scale_chroma,
            )?;

            run_noise_scaler(&configuration, &save_file)?;
        },
        Some(Commands::TargetQuality {
            input,
            decoder,
            filters,
            vs_args,
            params,
            metric,
            target,
            minimum_quantizer,
            maximum_quantizer,
            profile,
        }) => {
            let (configuration, save_file) = target_quality_handler(
                config_path.as_deref(),
                temp.as_deref(),
                input.as_deref(),
                decoder.as_ref(),
                filters.as_deref(),
                vs_args.as_deref(),
                params,
                metric,
                target,
                minimum_quantizer,
                maximum_quantizer,
                profile,
            )?;

            run_target_quality(&configuration, &save_file)?;
        },
        Some(Commands::OptimizeBitrate {
            sigma_threshold,
        }) => {
            let (configuration, save_file) =
                optimize_bitrate_handler(config_path.as_deref(), sigma_threshold)?;

            run_bitrate_optimizer(&configuration, &save_file)?;
        },
        Some(Commands::ScaleSpeed {
            quantizers,
            speeds,
        }) => {
            let (configuration, save_file) = scale_speed_handler(
                config_path.as_deref(),
                quantizers.as_deref(),
                speeds.as_deref(),
            )?;

            run_speed_scaler(&configuration, &save_file)?;
        },
        Some(Commands::Encode {
            input,
            decoder,
            filters,
            vs_args,
            workers,
            encoder,
            passes,
            params,
            photon_noise,
            chroma_noise,
        }) => {
            let (configuration, save_file) = encode_handler(
                config_path.as_deref(),
                temp.as_deref(),
                input.as_deref(),
                decoder.as_ref(),
                filters.as_deref(),
                vs_args.as_deref(),
                workers,
                encoder.as_ref(),
                passes,
                params,
                photon_noise,
                chroma_noise,
            )?;

            run_encoder(&configuration, &save_file)?;
        },
        Some(Commands::Concatenate {
            method,
        }) => {
            let (configuration, save_file) =
                concatenate_handler(config_path.as_deref(), temp.as_deref(), method.as_ref())?;

            run_concatenator(&configuration, &save_file)?;
        },
        Some(Commands::Config {
            subcommand,
        }) => {
            config_sub_handler(config_path, subcommand)?;
        },
        Some(Commands::Clean {
            all,
        }) => {
            todo!();
        },
        None => {
            let (configuration, save_file) = start_handler(
                config_path.as_deref(),
                temp.as_deref(),
                cli.input.as_deref(),
                cli.scd_input.as_deref(),
                cli.tq_input.as_deref(),
                cli.output.as_deref(),
                cli.decoder.as_ref(),
                cli.scd_decoder.as_ref(),
                cli.tq_decoder.as_ref(),
                cli.filters.as_deref(),
                cli.scd_filters.as_deref(),
                cli.tq_filters.as_deref(),
                cli.vs_args.as_deref(),
                cli.scd_vs_args.as_deref(),
                cli.tq_vs_args.as_deref(),
                cli.concat.as_ref(),
                cli.workers,
                cli.encoder.as_ref(),
                cli.passes,
                cli.params,
                cli.tq_params,
                cli.photon_noise,
                cli.chroma_noise,
                cli.target_metric,
                cli.target,
                cli.minimum_quantizer,
                cli.maximum_quantizer,
                cli.target_profile,
            )?;

            run_condor(&configuration, &save_file, cli.skip_scd)?;
        },
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_condor(configuration: &Configuration, save_file: &Path, skip_scd: bool) -> Result<()> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    debug!("Instantiating Condor with {:?}", {
        // Remove scenes to reduce log spam
        let mut config = configuration.clone();
        config.condor.scenes = Vec::new();
        config
    });
    let mut condor: Condor<configuration::CliSequenceData, configuration::CliSequenceConfig> =
        configuration.instantiate_condor(Box::new(move |data| {
            let mut config = config_copy.clone();
            config.condor = data;
            Configuration::save(&config, &save_file_copy)?;
            Ok(())
        }))?;

    let cancellation_token = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let cancelled = || {
        if cancellation_token.load(std::sync::atomic::Ordering::Relaxed) {
            info!("Condor cancelled. Exiting...");
            return true;
        }
        false
    };

    if !skip_scd {
        run_scene_detector_tui(
            &mut condor,
            &configuration.input_filters,
            &configuration.scd_input_filters,
            std::sync::Arc::clone(&cancellation_token),
        )?;
        if cancelled() {
            return Ok(());
        }
    }

    if configuration.condor.sequence_config.noise_detector.is_some() {
        run_noise_detector_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
        if cancelled() {
            return Ok(());
        }

        run_noise_scaler_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
        if cancelled() {
            return Ok(());
        }
    }

    if configuration.condor.sequence_config.parallel_encoder.workers.is_none() {
        run_benchmarker_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
        if cancelled() {
            return Ok(());
        }
    }

    if configuration.condor.sequence_config.target_quality.is_some() {
        run_target_quality_tui(
            &mut condor,
            &configuration.tq_input_filters,
            std::sync::Arc::clone(&cancellation_token),
        )?;
        if cancelled() {
            return Ok(());
        }

        if configuration
            .condor
            .sequence_config
            .bitrate_optimizer
            .bitrate_sigma_threshold
            .is_some()
        {
            run_bitrate_optimizer_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
            if cancelled() {
                return Ok(());
            }
        }
    }

    if configuration.condor.sequence_config.speed_scaler.speed_quantizers.len() >= 2 {
        run_speed_scaler_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
        if cancelled() {
            return Ok(());
        }
    }

    run_parallel_encoder_tui(
        &mut condor,
        &configuration.input_filters,
        std::sync::Arc::clone(&cancellation_token),
    )?;
    if cancelled() {
        return Ok(());
    }

    // run_quality_normalizer_tui(
    //     &mut condor,
    //     scenes_directory,
    //     std::sync::Arc::clone(&cancellation_token),
    // )?;
    // if cancelled() {
    //     return Ok(());
    // }

    run_scene_concatenator_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
    if cancelled() {
        return Ok(());
    }

    // run_quality_analyzer_tui(
    //     &mut condor,
    //     scenes_directory,
    //     std::sync::Arc::clone(&cancellation_token),
    // )?;
    // if cancelled() {
    //     return Ok(());
    // }

    info!(
        "Condor has landed. Output: {}",
        condor.output.path.display()
    );
    info!("Have a nice day!");

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_scene_detector(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    debug!("Instantiating Condor with {:?}", {
        // Remove scenes to reduce log spam
        let mut config = configuration.clone();
        config.condor.scenes = Vec::new();
        config
    });
    let mut condor: Condor<configuration::CliSequenceData, configuration::CliSequenceConfig> =
        configuration.instantiate_condor(Box::new(move |data| {
            let mut config = config_copy.clone();
            config.condor = data;
            Configuration::save(&config, &save_file_copy)?;
            Ok(())
        }))?;

    run_scene_detector_tui(
        &mut condor,
        &configuration.input_filters,
        &configuration.scd_input_filters,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_benchmarker(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    debug!("Instantiating Condor with {:?}", {
        // Remove scenes to reduce log spam
        let mut config = configuration.clone();
        config.condor.scenes = Vec::new();
        config
    });
    let mut condor: Condor<configuration::CliSequenceData, configuration::CliSequenceConfig> =
        configuration.instantiate_condor(Box::new(move |data| {
            let mut config = config_copy.clone();
            config.condor = data;
            Configuration::save(&config, &save_file_copy)?;
            Ok(())
        }))?;

    run_benchmarker_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_noise_detector(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    debug!("Instantiating Condor with {:?}", {
        let mut config = configuration.clone();
        config.condor.scenes = Vec::new();
        config
    });
    let mut condor: Condor<configuration::CliSequenceData, configuration::CliSequenceConfig> =
        configuration.instantiate_condor(Box::new(move |data| {
            let mut config = config_copy.clone();
            config.condor = data;
            Configuration::save(&config, &save_file_copy)?;
            Ok(())
        }))?;

    run_noise_detector_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_noise_scaler(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    debug!("Instantiating Condor with {:?}", {
        let mut config = configuration.clone();
        config.condor.scenes = Vec::new();
        config
    });
    let mut condor: Condor<configuration::CliSequenceData, configuration::CliSequenceConfig> =
        configuration.instantiate_condor(Box::new(move |data| {
            let mut config = config_copy.clone();
            config.condor = data;
            Configuration::save(&config, &save_file_copy)?;
            Ok(())
        }))?;

    run_noise_scaler_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_target_quality(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    debug!("Instantiating Condor with {:?}", {
        // Remove scenes to reduce log spam
        let mut config = configuration.clone();
        config.condor.scenes = Vec::new();
        config
    });
    let mut condor: Condor<configuration::CliSequenceData, configuration::CliSequenceConfig> =
        configuration.instantiate_condor(Box::new(move |data| {
            let mut config = config_copy.clone();
            config.condor = data;
            Configuration::save(&config, &save_file_copy)?;
            Ok(())
        }))?;

    run_target_quality_tui(
        &mut condor,
        &configuration.tq_input_filters,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_bitrate_optimizer(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    debug!("Instantiating Condor with {:?}", {
        let mut config = configuration.clone();
        config.condor.scenes = Vec::new();
        config
    });
    let mut condor: Condor<configuration::CliSequenceData, configuration::CliSequenceConfig> =
        configuration.instantiate_condor(Box::new(move |data| {
            let mut config = config_copy.clone();
            config.condor = data;
            Configuration::save(&config, &save_file_copy)?;
            Ok(())
        }))?;

    run_bitrate_optimizer_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_speed_scaler(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    debug!("Instantiating Condor with {:?}", {
        let mut config = configuration.clone();
        config.condor.scenes = Vec::new();
        config
    });
    let mut condor: Condor<configuration::CliSequenceData, configuration::CliSequenceConfig> =
        configuration.instantiate_condor(Box::new(move |data| {
            let mut config = config_copy.clone();
            config.condor = data;
            Configuration::save(&config, &save_file_copy)?;
            Ok(())
        }))?;

    run_speed_scaler_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_encoder(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    debug!("Instantiating Condor with {:?}", {
        let mut config = configuration.clone();
        config.condor.scenes = Vec::new();
        config
    });

    let mut condor: Condor<configuration::CliSequenceData, configuration::CliSequenceConfig> =
        configuration.instantiate_condor(Box::new(move |data| {
            let mut config = config_copy.clone();
            config.condor = data;
            Configuration::save(&config, &save_file_copy)?;
            Ok(())
        }))?;

    run_parallel_encoder_tui(
        &mut condor,
        &configuration.input_filters,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_concatenator(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    debug!("Instantiating Condor with {:?}", {
        let mut config = configuration.clone();
        config.condor.scenes = Vec::new();
        config
    });

    let mut condor: Condor<configuration::CliSequenceData, configuration::CliSequenceConfig> =
        configuration.instantiate_condor(Box::new(move |data| {
            let mut config = config_copy.clone();
            config.condor = data;
            Configuration::save(&config, &save_file_copy)?;
            Ok(())
        }))?;

    run_scene_concatenator_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[derive(Debug, Error)]
pub enum CondorCliError {
    #[error("Cannot initialize over an existing config file: {0}")]
    ConfigFileAlreadyExists(PathBuf),
    #[error("No config file found at: {0}")]
    ConfigFileNotFound(PathBuf),
    #[error("Failed to load config file: {0}")]
    ConfigLoadError(PathBuf),
    #[error("Cannot start without a config file or without input path")]
    NoConfigOrInput,
    #[error("Cannot start without a config file or without input and output paths")]
    NoConfigOrInputOrOutput,
    #[error("Cannot set Decoder without a valid Input path")]
    DecoderWithoutInput,
    #[error("No config file found. Run 'condor init' to create a configuration.")]
    NoConfig,
    #[error("No scenes found in the config. Run 'condor detect-scenes' to populate scenes")]
    NoScenes,
    #[error("Input {0} must be a VapourSynth script (.vpy or .py)")]
    InvalidVapourSynthScript(PathBuf),
}
