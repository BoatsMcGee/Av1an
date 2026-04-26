use std::{
    panic,
    path::{Path, PathBuf},
    process,
};

use andean_condor::core::Condor;
use anyhow::Result;
use clap::Parser;
use thiserror::Error;
use tracing::{debug, info, level_filters::LevelFilter};

use crate::{
    commands::{
        benchmarker::benchmarker_handler,
        config::config_sub_handler,
        detect_scenes::detect_scenes_handler,
        init::init_handler,
        start::start_handler,
        target_quality::target_quality_handler,
        Commands,
        CondorCli,
    },
    configuration::Configuration,
    logging::init_logging,
    tui::{
        run_benchmarker_tui,
        run_bitrate_optimizer_tui,
        run_convex_hull_tui,
        run_noise_detector_tui,
        run_noise_scaler_tui,
        run_parallel_encoder_tui,
        run_scene_concatenator_tui,
        run_scene_detector_tui,
        run_tq_tui,
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
    let cli = CondorCli::parse();
    let cwd = std::env::current_dir()?;
    let config_path = cli.config_file;
    let logs = cli.logs.unwrap_or_else(|| cwd.join(DEFAULT_LOG_PATH));
    init_logging(
        LevelFilter::INFO,
        &logs,
        if cli.verbose {
            LevelFilter::TRACE
        } else {
            LevelFilter::DEBUG
        },
    )?;

    match cli.command {
        Commands::Init {
            input,
            output,
            temp,
            decoder,
            vs_args,
            encoder,
            passes,
            params,
            photon_noise,
            chroma_noise,
        } => {
            init_handler(
                config_path.as_deref(),
                input.as_path(),
                output.as_path(),
                temp.as_deref(),
                decoder.as_ref(),
                vs_args.as_deref(),
                encoder.as_ref(),
                passes,
                params,
                photon_noise,
                chroma_noise,
            )?;
        },
        Commands::Config {
            subcommand,
        } => {
            config_sub_handler(config_path, subcommand)?;
        },
        Commands::DetectScenes {
            temp,
            input,
            decoder,
            filters,
            vs_args,
            method,
            min_scene_seconds,
            max_scene_seconds,
        } => {
            let (configuration, save_file) = detect_scenes_handler(
                temp.as_deref(),
                config_path.as_deref(),
                input.as_deref(),
                decoder.as_ref(),
                filters.as_deref(),
                vs_args.as_deref(),
                method.as_ref(),
                min_scene_seconds,
                max_scene_seconds,
            )?;

            run_scene_detection_tui(&configuration, &save_file)?;
        },
        Commands::Benchmark {
            temp,
            input,
            decoder,
            filters,
            vs_args,
            encoder,
            passes,
            params,
            threshold,
            max_memory,
        } => {
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

            run_benchmark_tui(&configuration, &save_file)?;
        },
        Commands::TargetQuality {
            temp,
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
        } => {
            let (configuration, save_file) = target_quality_handler(
                temp.as_deref(),
                config_path.as_deref(),
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

            run_target_quality_tui(&configuration, &save_file)?;
        },
        Commands::Start {
            temp,
            input,
            scd_input,
            tq_input,
            decoder,
            filters,
            scd_filters,
            tq_filters,
            vs_args,
            scd_vs_args,
            tq_vs_args,
            output,
            concat,
            workers,
            encoder,
            passes,
            params,
            tq_params,
            photon_noise,
            chroma_noise,
            target_metric,
            target,
            minimum_quantizer,
            maximum_quantizer,
            target_profile,
            skip_scd,
        } => {
            let (configuration, save_file) = start_handler(
                config_path.as_deref(),
                temp.as_deref(),
                input.as_deref(),
                scd_input.as_deref(),
                tq_input.as_deref(),
                output.as_deref(),
                decoder.as_ref(),
                filters.as_deref(),
                scd_filters.as_deref(),
                tq_filters.as_deref(),
                vs_args.as_deref(),
                scd_vs_args.as_deref(),
                tq_vs_args.as_deref(),
                concat.as_ref(),
                workers,
                encoder.as_ref(),
                passes,
                params,
                tq_params,
                photon_noise,
                chroma_noise,
                target_metric,
                target,
                minimum_quantizer,
                maximum_quantizer,
                target_profile,
            )?;

            // let config_copy = configuration.clone();
            run_condor_tui(&configuration, &save_file, skip_scd)?;
        },
        Commands::Clean {
            all,
        } => {
            todo!();
        },
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_condor_tui(
    configuration: &Configuration,
    save_file: &Path,
    skip_scd: bool,
) -> Result<()> {
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
        run_tq_tui(
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

    if configuration.condor.sequence_config.convex_hull.speed_quantizers.len() >= 2 {
        run_convex_hull_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
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
pub fn run_scene_detection_tui(configuration: &Configuration, save_file: &Path) -> Result<()> {
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
pub fn run_benchmark_tui(configuration: &Configuration, save_file: &Path) -> Result<()> {
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
pub fn run_target_quality_tui(configuration: &Configuration, save_file: &Path) -> Result<()> {
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

    run_tq_tui(
        &mut condor,
        &configuration.tq_input_filters,
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
}
