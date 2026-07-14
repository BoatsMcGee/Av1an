use std::path::PathBuf;

use andean_condor::{
    models::{
        encoder::EncoderBase,
        sequence::scene_detector::{
            SceneDetectionMethod as CoreSCDMethod,
            ScenecutMethod,
            DEFAULT_MAX_SCENE_LENGTH_SECONDS,
            DEFAULT_MIN_SCENE_LENGTH_FRAMES,
        },
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use clap::{value_parser, Parser as ClapParser, Subcommand};
use serde::{Deserialize, Serialize};
use strum::{Display as DisplayMacro, EnumString, IntoStaticStr};
use thiserror::Error;

use crate::commands::help_text::*;

pub mod handlers;
pub mod help_text;

#[derive(Debug, ClapParser)]
#[command(
    name = "condor",
    about = "A simple, extensible Commandline tool for the Condor chunked encoding framework.",
    version = "0.0.1"
)]
pub struct CondorCli {
    #[command(subcommand)]
    pub command:           Option<Commands>,
    #[arg(long, global = true, value_name = "Config File", help = HELP_CONFIG_SHORT, long_help = HELP_CONFIG)]
    pub config_file:       Option<PathBuf>,
    #[arg(long, global = true, value_name = "Temporary Directory", help = HELP_TEMP_SHORT, long_help = HELP_TEMP)]
    pub temp:              Option<PathBuf>,
    #[arg(long, global = true, value_name = "Log File", help = HELP_LOGS_SHORT, long_help = HELP_LOGS)]
    pub logs:              Option<PathBuf>,
    /// Enable verbose output and logging.
    #[arg(long, global = true, default_value_t = false)]
    pub verbose:           bool,
    // Main command arguments
    #[arg(long, short('i'), value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
    pub input:             Option<PathBuf>,
    #[arg(long, short('o'), value_name = "Output", help = HELP_OUTPUT_SHORT, long_help = HELP_OUTPUT)]
    pub output:            Option<PathBuf>,
    #[arg(long, value_name = "Scene Detector Input", help = HELP_SCD_INPUT_SHORT, long_help = HELP_SCD_INPUT)]
    pub scd_input:         Option<PathBuf>,
    #[arg(long, value_name = "Target Quality Input", help = HELP_TQ_INPUT_SHORT, long_help = HELP_TQ_INPUT)]
    pub tq_input:          Option<PathBuf>,
    #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
    pub decoder:           Option<DecoderMethod>,
    #[arg(long, value_name = "Decoder", help = HELP_SCD_DECODER_SHORT, long_help = HELP_SCD_DECODER)]
    pub scd_decoder:       Option<DecoderMethod>,
    #[arg(long, value_name = "Decoder", help = HELP_TQ_DECODER_SHORT, long_help = HELP_TQ_DECODER)]
    pub tq_decoder:        Option<DecoderMethod>,
    #[arg(long, value_name = "VapourSynth Filters", help = HELP_FILTERS_SHORT, long_help = HELP_FILTERS)]
    pub filters:           Option<Vec<VapourSynthFilter>>,
    #[arg(long, value_name = "VapourSynth Filters", help = HELP_SCD_FILTERS_SHORT, long_help = HELP_SCD_FILTERS)]
    pub scd_filters:       Option<Vec<VapourSynthFilter>>,
    #[arg(long, value_name = "VapourSynth Filters", help = HELP_TQ_FILTERS_SHORT, long_help = HELP_TQ_FILTERS)]
    pub tq_filters:        Option<Vec<VapourSynthFilter>>,
    #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
    pub vs_args:           Option<Vec<String>>,
    #[arg(long, value_name = "VapourSynth Arguments", help = HELP_SCD_VS_ARGS_SHORT, long_help = HELP_SCD_VS_ARGS)]
    pub scd_vs_args:       Option<Vec<String>>,
    #[arg(long, value_name = "VapourSynth Arguments", help = HELP_TQ_VS_ARGS_SHORT, long_help = HELP_TQ_VS_ARGS)]
    pub tq_vs_args:        Option<Vec<String>>,
    #[arg(long, value_name = "Concatenation Method", help = HELP_CONCAT_SHORT, long_help = HELP_CONCAT)]
    pub concat:            Option<ConcatenationMethod>,
    /// The amount of encoder processes to use at once
    #[arg(long, short('w'), value_name = "Workers", help = HELP_WORKERS_SHORT, long_help = HELP_WORKERS)]
    pub workers:           Option<u8>,
    #[arg(long, short('e'), value_name = "Encoder", help = HELP_ENCODER_SHORT, long_help = HELP_ENCODER)]
    pub encoder:           Option<EncoderMethod>,
    #[arg(long, value_name = "Passes", help = HELP_PASSES_SHORT, long_help = HELP_PASSES)]
    pub passes:            Option<u8>,
    #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_PARAMS_SHORT, long_help = HELP_PARAMS)]
    pub params:            Option<String>,
    #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_TQ_PARAMS_SHORT, long_help = HELP_TQ_PARAMS)]
    pub tq_params:         Option<String>,
    #[arg(long, value_name = "ISO", help = HELP_PHOTON_NOISE_SHORT, long_help = HELP_PHOTON_NOISE)]
    pub photon_noise:      Option<u32>,
    #[arg(long, value_name = "ISO", help = HELP_CHROMA_NOISE_SHORT, long_help = HELP_CHROMA_NOISE)]
    pub chroma_noise:      Option<u32>,
    #[arg(long, value_name = "Metric", help = HELP_TARGET_METRIC_SHORT, long_help = HELP_TARGET_METRIC)]
    pub target_metric:     Option<TargetQualityMetric>,
    #[arg(long, value_name = "Score", help = HELP_TARGET_SHORT, long_help = HELP_TARGET)]
    pub target:            Option<f64>,
    #[arg(long, value_name = "Quantizer", help = HELP_MINIMUM_QUANTIZER_SHORT, long_help = HELP_MINIMUM_QUANTIZER)]
    pub minimum_quantizer: Option<u8>,
    #[arg(long, value_name = "Quantizer", help = HELP_MAXIMUM_QUANTIZER_SHORT, long_help = HELP_MAXIMUM_QUANTIZER)]
    pub maximum_quantizer: Option<u8>,
    #[arg(long, value_name = "Profile", help = HELP_TARGET_QUALITY_PROFILE_SHORT, long_help = HELP_TARGET_QUALITY_PROFILE)]
    pub target_profile:    Option<TargetQualityProfile>,
    /// Skip Scene Detection. Useful when encoding a subset of scenes.
    #[arg(long, default_value_t = false)]
    pub skip_scd:          bool,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Initialize a new configuration.
    Init {
        #[arg(value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
        input:         PathBuf,
        #[arg(value_name = "Output", help = HELP_OUTPUT_SHORT, long_help = HELP_OUTPUT)]
        output:        PathBuf,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder:       Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters", help = HELP_FILTERS_SHORT, long_help = HELP_FILTERS)]
        filters:       Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args:       Option<Vec<String>>,
        #[arg(long, value_name = "Concatenation Method", help = HELP_CONCAT_SHORT, long_help = HELP_CONCAT)]
        concat:        Option<ConcatenationMethod>,
        /// The amount of encoder processes to use at once
        #[arg(long, short('w'), value_name = "Workers", help = HELP_WORKERS_SHORT, long_help = HELP_WORKERS)]
        workers:       Option<u8>,
        #[arg(long, short('e'), value_name = "Encoder", help = HELP_ENCODER_SHORT, long_help = HELP_ENCODER)]
        encoder:       Option<EncoderMethod>,
        #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_PARAMS_SHORT, long_help = HELP_PARAMS)]
        params:        Option<String>,
        #[arg(long, value_name = "ISO", help = HELP_PHOTON_NOISE_SHORT, long_help = HELP_PHOTON_NOISE)]
        photon_noise:  Option<u32>,
        #[arg(long, value_name = "Metric", help = HELP_TARGET_METRIC_SHORT, long_help = HELP_TARGET_METRIC)]
        target_metric: Option<TargetQualityMetric>,
        #[arg(long, value_name = "Score", help = HELP_TARGET_SHORT, long_help = HELP_TARGET)]
        target:        Option<f64>,
    },
    /// Detect scenes (Triggers TUI).
    DetectScenes {
        #[arg(long, short('i'), value_name = "Input", help = HELP_SCD_INPUT_SHORT, long_help = HELP_SCD_INPUT)]
        input:             Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder:           Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters", help = HELP_SCD_FILTERS_SHORT, long_help = HELP_SCD_FILTERS)]
        filters:           Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_SCD_VS_ARGS_SHORT, long_help = HELP_SCD_VS_ARGS)]
        vs_args:           Option<Vec<String>>,
        #[arg(long, value_name = "Scene Detection Method", help = HELP_SCD_METHOD_SHORT, long_help = HELP_SCD_METHOD)]
        method:            Option<SceneDetectionMethod>,
        #[arg(long, value_name = "Scene Duration", help = HELP_MIN_SCENE_SECONDS_SHORT, long_help = HELP_MIN_SCENE_SECONDS)]
        min_scene_seconds: Option<usize>,
        #[arg(long, value_name = "Scene Duration", help = HELP_MAX_SCENE_SECONDS_SHORT, long_help = HELP_MAX_SCENE_SECONDS)]
        max_scene_seconds: Option<usize>,
    },
    /// Detect noise per scene (Triggers TUI).
    DetectNoise {
        /// Path to the input VapourSynth script for noise detection.
        ///
        /// Must be a VapourSynth script (`.vpy`) that outputs 2 videos, the
        /// original (or lightly denoised) video and a denoised (heavily) video.
        #[arg(long, value_name = "VapourSynth Script Input")]
        input:   Option<PathBuf>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args: Option<Vec<String>>,
    },
    /// Scale Photon Noise ISO per scene based on Noise Detector results.
    ///
    /// Scenes must have Photon Noise (*--photon-noise*/*--chroma-noise*)
    /// configured. Scenes below the threshold will not be scaled.
    ScaleNoise {
        /// Minimum noise value for a scene to scale Photon Noise ISO.
        ///
        /// Recommended: 0.002
        #[arg(long, value_name = "Threshold")]
        threshold:      Option<f64>,
        /// Minimum scale factor for Photon Noise ISO scaling.
        #[arg(long, value_name = "Scaler")]
        minimum_scaler: Option<f64>,
        /// Maximum scale factor for Photon Noise ISO scaling.
        #[arg(long, value_name = "Scaler")]
        maximum_scaler: Option<f64>,
        /// Scale Chroma ISO.
        #[arg(long)]
        scale_chroma:   bool,
    },
    /// Benchmark the optimum amount of workers (Triggers TUI).
    Benchmark {
        /// The minimum speed increase (in percent) required to add an
        /// additional worker.
        ///
        /// Defaults to `5`.
        #[arg(long, value_name = "Percent", value_parser = value_parser!(u8).range(0..=100))]
        threshold:  Option<u8>,
        /// The maximum amount of RAM (in megabytes) allowed across all workers
        /// (unimplemented)
        #[arg(long, value_name = "Megabytes", hide = true)]
        max_memory: Option<u32>,
    },
    /// Calculate the optimum quantizer per scene for a given metric target
    /// (Triggers TUI).
    TargetQuality {
        #[arg(long, short('i'), value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
        input:             Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder:           Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters")]
        filters:           Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args:           Option<Vec<String>>,
        #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_PARAMS_SHORT, long_help = HELP_PARAMS)]
        params:            Option<String>,
        #[arg(long, value_name = "Metric", help = HELP_TARGET_METRIC_SHORT, long_help = HELP_TARGET_METRIC)]
        metric:            Option<TargetQualityMetric>,
        #[arg(long, value_name = "Score", help = HELP_TARGET_SHORT, long_help = HELP_TARGET)]
        target:            Option<f64>,
        #[arg(long("min-q"), value_name = "Quantizer", help = HELP_MINIMUM_QUANTIZER_SHORT, long_help = HELP_MINIMUM_QUANTIZER)]
        minimum_quantizer: Option<u8>,
        #[arg(long("max-q"), value_name = "Quantizer", help = HELP_MAXIMUM_QUANTIZER_SHORT, long_help = HELP_MAXIMUM_QUANTIZER)]
        maximum_quantizer: Option<u8>,
        #[arg(long, value_name = "Profile", help = HELP_TARGET_QUALITY_PROFILE_SHORT, long_help = HELP_TARGET_QUALITY_PROFILE)]
        profile:           Option<TargetQualityProfile>,
    },
    /// Optimize bitrate for scenes that exceed normal bitrate after Target
    /// Quality.
    ///
    /// Scenes with a bitrate above the normal threshold will be optimized by
    /// clamping the quantizer to the average quantizer value.
    OptimizeBitrate {
        /// Minimum bitrate sigma (σ) threshold for a scene to be optimized.
        #[arg(long, value_name = "Sigma", value_parser = value_parser!(u8).range(1..=10))]
        sigma_threshold: Option<u8>,
    },
    /// Apply speed based on quantizer per scene using Convex Hull
    /// interpolation.
    ///
    /// Speeds for scenes with quantizers between points will be interpolated
    /// between the nearest points.
    ScaleSpeed {
        /// Quantizer values for speed-quantizer pairs. (must match number of
        /// speeds)
        ///
        /// Examples:
        ///
        /// - `--encoder svt-av1 --quantizers 20 --speeds 5 --quantizers 30
        ///   --speeds 4 --quantizers 55 --speeds 2`
        ///
        /// - `--encoder aom --quantizers 10 --speeds 6 --quantizers 25 --speeds
        ///   4 --quantizers 40 --speeds 3`
        #[arg(long, value_name = "Quantizer", num_args = 1.., requires = "speeds")]
        quantizers: Option<Vec<i8>>,
        /// Speed values for speed-quantizer pairs (must match number of
        /// quantizers)
        ///
        /// Examples:
        ///
        /// - `--encoder svt-av1 --quantizers 20 --speeds 5 --quantizers 30
        ///   --speeds 4 --quantizers 55 --speeds 2`
        ///
        /// - `--encoder aom --quantizers 10 --speeds 6 --quantizers 25 --speeds
        ///   4 --quantizers 40 --speeds 3`
        #[arg(long, value_name = "Speed", num_args = 1.., requires = "quantizers")]
        speeds:     Option<Vec<i8>>,
    },
    /// Encode scenes in parallel (Triggers TUI).
    Encode {
        #[arg(long, short('i'), value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
        input:        Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder:      Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters", help = HELP_FILTERS_SHORT, long_help = HELP_FILTERS)]
        filters:      Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args:      Option<Vec<String>>,
        /// The amount of encoder processes to use at once
        #[arg(long, short('w'), value_name = "Workers", help = HELP_WORKERS_SHORT, long_help = HELP_WORKERS)]
        workers:      Option<u8>,
        #[arg(long, short('e'), value_name = "Encoder", help = HELP_ENCODER_SHORT, long_help = HELP_ENCODER)]
        encoder:      Option<EncoderMethod>,
        #[arg(long, value_name = "Passes", help = HELP_PASSES_SHORT, long_help = HELP_PASSES)]
        passes:       Option<u8>,
        #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_PARAMS_SHORT, long_help = HELP_PARAMS)]
        params:       Option<String>,
        #[arg(long, value_name = "ISO", help = HELP_PHOTON_NOISE_SHORT, long_help = HELP_PHOTON_NOISE)]
        photon_noise: Option<u32>,
        #[arg(long, value_name = "ISO", help = HELP_CHROMA_NOISE_SHORT, long_help = HELP_CHROMA_NOISE)]
        chroma_noise: Option<u32>,
    },
    /// Concatenate encoded scenes into output video.
    Concatenate {
        #[arg(long, value_name = "Concatenation Method", help = HELP_CONCAT_SHORT, long_help = HELP_CONCAT)]
        method: Option<ConcatenationMethod>,
    },
    /// Clean temporary files.
    Clean {
        #[arg(long, value_name = "All")]
        all: bool,
    },
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    DisplayMacro,
    clap::ValueEnum,
)]
pub enum SceneDetectionMethod {
    /// No scene detection, chunks scenes by maximum length
    #[strum(serialize = "none")]
    #[value(name = "none")]
    None,
    /// Fast scene detection, uses av-scenechange with the fast algorithm
    #[strum(serialize = "fast")]
    #[value(name = "fast")]
    Fast,
    /// Standard scene detection, uses av-scenechange with the standard
    /// algorithm
    #[strum(serialize = "standard")]
    #[value(name = "standard")]
    Standard,
}

impl SceneDetectionMethod {
    pub fn as_core_method(
        &self,
        minimum_length: Option<usize>,
        maximum_length: Option<usize>,
    ) -> CoreSCDMethod {
        let min_length = minimum_length.unwrap_or(DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize);
        let max_length = maximum_length.unwrap_or(
            DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize * DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize,
        );
        match self {
            SceneDetectionMethod::None => CoreSCDMethod::None {
                minimum_length: min_length,
                maximum_length: max_length,
            },
            SceneDetectionMethod::Fast => CoreSCDMethod::AVSceneChange {
                method:         ScenecutMethod::Fast,
                minimum_length: min_length,
                maximum_length: max_length,
            },
            SceneDetectionMethod::Standard => CoreSCDMethod::AVSceneChange {
                method:         ScenecutMethod::Standard,
                minimum_length: min_length,
                maximum_length: max_length,
            },
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    DisplayMacro,
    clap::ValueEnum,
)]
pub enum DecoderMethod {
    /// BestSource VapourSynth plugin
    #[strum(serialize = "bestsource")]
    #[value(name = "bestsource")]
    BestSource,
    /// FFmpegSource VapourSynth plugin
    #[strum(serialize = "vs-ffms2")]
    #[value(name = "vs-ffms2")]
    VSFFMS2,
    /// LSMASH VapourSynth plugin
    #[strum(serialize = "lsmash")]
    #[value(name = "lsmash")]
    LSMASHWorks,
    /// DGDecodeNV VapourSynth plugin
    #[strum(serialize = "dgdecnv")]
    #[value(name = "dgdecnv")]
    DGDecodeNV,
    /// FFmpegSource (integrated)
    #[strum(serialize = "ffms2")]
    #[value(name = "ffms2")]
    FFMS2,
}

#[allow(clippy::upper_case_acronyms)]
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    DisplayMacro,
    clap::ValueEnum,
)]
pub enum EncoderMethod {
    /// Alliance for Open Media AV1 encoder
    #[strum(serialize = "aom")]
    #[value(name = "aom")]
    AOM,
    /// Rust AV1 encoder
    #[strum(serialize = "rav1e")]
    #[value(name = "rav1e")]
    RAV1E,
    /// Alliance for Open Media VP8/VP9
    #[strum(serialize = "vpx")]
    #[value(name = "vpx")]
    VPX,
    /// Scalable Video Technology for AV1
    #[strum(serialize = "svt-av1")]
    #[value(name = "svt-av1")]
    SVTAV1,
    /// Alliance for Open Media Video Model
    #[strum(serialize = "avm")]
    #[value(name = "avm")]
    AVM,
    /// VideoLAN x264
    #[strum(serialize = "x264")]
    #[value(name = "x264")]
    X264,
    /// MulticoreWare x265
    #[strum(serialize = "x265")]
    #[value(name = "x265")]
    X265,
    /// Fraunhofer Versatile Video Encoder
    #[strum(serialize = "vvenc")]
    #[value(name = "vvenc")]
    VVenC,
    /// FFmpeg
    #[strum(serialize = "ffmpeg")]
    #[value(name = "ffmpeg")]
    FFmpeg,
}

impl EncoderMethod {
    pub fn as_encoder_base(&self) -> EncoderBase {
        match self {
            EncoderMethod::AOM => EncoderBase::AOM,
            EncoderMethod::RAV1E => EncoderBase::RAV1E,
            EncoderMethod::VPX => EncoderBase::VPX,
            EncoderMethod::SVTAV1 => EncoderBase::SVTAV1,
            EncoderMethod::AVM => EncoderBase::AVM,
            EncoderMethod::X264 => EncoderBase::X264,
            EncoderMethod::X265 => EncoderBase::X265,
            EncoderMethod::VVenC => EncoderBase::VVenC,
            EncoderMethod::FFmpeg => EncoderBase::FFmpeg,
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    DisplayMacro,
    clap::ValueEnum,
)]
pub enum ConcatenationMethod {
    /// MKVToolNix mkvmerge -- Merge multimedia streams into a Matroska™ file
    ///
    /// Generally the best concatenation method, but can only produce Matroska™
    /// (.mkv) files. Requires mkvmerge to be installed.
    #[strum(serialize = "mkvmerge")]
    #[value(name = "mkvmerge")]
    MkvMerge,
    /// FFmpeg command line tool
    ///
    /// Can mux into formats besides Matroska (.mkv) and Indeo Video Format
    /// (.ivf). Unfortunately, sometimes produces file with partially broken
    /// audio seeking, so mkvmerge should generally be preferred if available.
    /// Also produces broken files with the `--enable-keyframe filtering=2`
    /// option in aomenc. Requires FFmpeg to be installed.
    #[strum(serialize = "ffmpeg")]
    #[value(name = "ffmpeg")]
    FFmpeg,
    /// Indeo Video Format (.ivf)
    ///
    /// Only supports concatenation of IVF files and will not include other
    /// streams or metadata (audio, subtitle, chapters, etc.) from the input.
    #[strum(serialize = "ivf")]
    #[value(name = "ivf")]
    Ivf,
}

#[allow(clippy::upper_case_acronyms)]
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    DisplayMacro,
    clap::ValueEnum,
)]
pub enum TargetQualityMetric {
    /// Video Multi-Method Assessment Fusion
    ///
    /// (unimplemented)
    #[strum(serialize = "vmaf")]
    #[value(name = "vmaf")]
    VMAF,
    /// Structural SIMilarity Unveiling Local And Compression Related Artifacts
    ///
    /// Requires VapourSynth plugin Vapoursynth-HIP for Hardware-accelerated
    /// processing (recommended) or Vapoursynth-Zig Image Process for CPU
    /// processing.
    #[strum(serialize = "ssimulacra2")]
    #[value(name = "ssimulacra2")]
    SSIMULACRA2,
    /// butteraugli Infinite-Norm
    ///
    /// Requires VapourSynth plugin Vapoursynth-HIP for Hardware-accelerated
    /// processing (recommended) or vapoursynth-julek-plugin for CPU processing.
    #[strum(serialize = "butteraugli")]
    #[value(name = "butteraugli")]
    BUTTERAUGLI,
    /// butteraugli 3-Norm
    ///
    /// Requires VapourSynth plugin Vapoursynth-HIP for Hardware-accelerated
    /// processing (recommended) or vapoursynth-julek-plugin for CPU processing.
    #[strum(serialize = "butteraugli-3")]
    #[value(name = "butteraugli-3")]
    BUTTERAUGLI3Norm,
    /// Extended Perceptually Weighted Peak Signal-to-Noise Ratio
    ///
    /// Uses the minimum of the `Y`, `U`, and `V` scores.
    ///
    /// Requires VapourSynth plugin Vapoursynth-Zig Image Process for CPU
    /// processing.
    #[strum(serialize = "xpsnr")]
    #[value(name = "xpsnr")]
    XPSNR,
    /// ColorVideoVDP
    ///
    /// Requires VapourSynth plugin Vapoursynth-HIP for Hardware-accelerated
    /// processing.
    #[strum(serialize = "cvvdp")]
    CVVDP,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    DisplayMacro,
    clap::ValueEnum,
)]
pub enum TargetQualityProfile {
    /// Fast
    ///
    /// Measures the average of the middle 11 frames.
    #[strum(serialize = "fast")]
    #[value(name = "fast")]
    Fast,
    /// Standard
    ///
    /// Measures the root-mean-square of the middle 25% of frames.
    #[strum(serialize = "standard")]
    #[value(name = "standard")]
    Standard,
    /// Slow
    ///
    /// Measures the 10th percentile of all frames.
    #[strum(serialize = "slow")]
    #[value(name = "slow")]
    Slow,
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

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use andean_condor::models::{
        encoder::EncoderBase,
        sequence::scene_detector::{SceneDetectionMethod as CoreSCDMethod, ScenecutMethod},
    };
    use clap::Parser;

    use super::{
        Commands,
        ConcatenationMethod,
        CondorCli,
        DecoderMethod,
        EncoderMethod,
        SceneDetectionMethod,
        TargetQualityMetric,
        TargetQualityProfile,
    };
    use crate::test_helpers::get_test_video;

    mod cli_parser {
        use super::*;

        mod init {
            use super::*;

            #[test]
            fn default() {
                let test_video = get_test_video();
                let temp = tempfile::tempdir().expect("temp directory");
                let output = temp.path().join("out.mkv");

                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    test_video.path.to_str().expect("test_video path is valid"),
                    output.to_str().expect("output path is valid"),
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Init { .. }),
                        ..
                    }),
                    "\"condor init INPUT OUTPUT\" parses"
                );
            }

            #[test]
            fn with_relative_paths() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    "./relative-input.mov",
                    "./relative-output.ivf",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Init { .. }),
                        ..
                    }),
                    "\"condor init ./relative-input.mov ./relative-output.ivf\" parses"
                );
            }

            #[test]
            fn with_optional_flags() {
                let test_video = get_test_video();
                let temp = tempfile::tempdir().expect("temp directory");
                let output = temp.path().join("out.mkv");
                let config_path = temp.path().join("condor.json");
                let logs_path = temp.path().join("condor.log");

                let result = CondorCli::try_parse_from([
                    "condor",
                    "--config-file",
                    config_path.to_str().expect("config path is valid"),
                    "--temp",
                    temp.path().to_str().expect("temp path is valid"),
                    "--logs",
                    logs_path.to_str().expect("log path is valid"),
                    "init",
                    test_video.path.to_str().expect("test_video path is valid"),
                    output.to_str().expect("output path is valid"),
                    "--encoder",
                    "x264",
                    "--workers",
                    "4",
                    "--photon-noise",
                    "2400",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Init { .. }),
                        ..
                    }),
                    "\"condor init INPUT OUTPUT --encoder x264 --workers 4 --photon-noise 2400\" \
                     parses"
                );
            }

            #[test]
            fn without_output() {
                let test_video = get_test_video();

                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    test_video.path.to_str().expect("test_video path is valid"),
                ]);
                assert_matches!(result, Err(_), "\"condor init input\" without output fails");
            }

            #[test]
            fn with_invalid_optional_flags() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    "./relative-input.mov",
                    "./relative-output.ivf",
                    "--encoder",
                    "x263",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor init INPUT OUTPUT --encoder x263\" does not parse"
                );
                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    "C:\\absolute-input.mov",
                    "C:\\output\\absolute-output.mkv",
                    "--workers",
                    "-2",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor init INPUT OUTPUT --workers -2\" does not parse"
                );
                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    "../relative-input.mov",
                    "../relative-output.ivf",
                    "--photon-noise",
                    "-24.8",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor init INPUT OUTPUT --photon-noise -24.8\" does not parse"
                );
            }
        }

        mod start {
            use super::*;

            #[test]
            fn default() {
                let test_video = get_test_video();

                let result = CondorCli::try_parse_from([
                    "condor",
                    "--input",
                    test_video.path.to_str().expect("test_video path is valid"),
                    "--output",
                    "./out.mkv",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: None,
                        ..
                    }),
                    "\"condor --input INPUT --output OUTPUT\" parses"
                );
            }

            #[test]
            fn without_input() {
                let temp = tempfile::tempdir().expect("temp directory");
                let output = temp.path().join("out.mkv");

                let result = CondorCli::try_parse_from([
                    "condor",
                    "--output",
                    output.to_str().expect("output path is valid"),
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: None,
                        ..
                    }),
                    "\"condor --output OUTPUT\" without --input parses"
                );
            }

            #[test]
            fn without_output() {
                let test_video = get_test_video();

                let result = CondorCli::try_parse_from([
                    "condor",
                    "--input",
                    test_video.path.to_str().expect("test_video path exists"),
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: None,
                        ..
                    }),
                    "\"condor init --input INPUT\" without --output parses"
                );
            }

            #[test]
            fn without_parameters() {
                let result = CondorCli::try_parse_from(["condor"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: None,
                        ..
                    }),
                    "\"condor\" parses"
                );
            }

            #[test]
            fn with_invalid_optional_flags() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "--input",
                    "./relative-input.mp4",
                    "--output",
                    "/absolute/path/to/output.mkv",
                    "--encoder",
                    "x263",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor --input INPUT --output OUTPUT --encoder x263\" does not parse"
                );
                let result = CondorCli::try_parse_from([
                    "condor",
                    "--input",
                    "../relative-input.mp4",
                    "--output",
                    "/absolute/path/to/output.mkv",
                    "--workers",
                    "-2",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor --input INPUT --output OUTPUT --workers -2\" does not parse"
                );
                let result = CondorCli::try_parse_from([
                    "condor",
                    "--input",
                    "./relative-input.mp4",
                    "--output",
                    "/absolute/path/to/output.mkv",
                    "--photon-noise",
                    "-24.8",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor --input INPUT --output OUTPUT --photon-noise -24.8\" does not parse"
                );
            }
        }

        mod detect_scenes {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "detect-scenes"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::DetectScenes { .. }),
                        ..
                    }),
                    "\"condor detect-scenes\" parses"
                );
            }

            #[test]
            fn with_input() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "--input",
                    "./input.mp4",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::DetectScenes {
                            input: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor detect-scenes --input INPUT\" parses"
                );
            }

            #[test]
            fn with_options() {
                let temp = tempfile::tempdir().expect("temp directory");
                let config_path = temp.path().join("condor.json");
                let logs_path = temp.path().join("condor.log");

                let result = CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "--method",
                    "fast",
                    "--min-scene-seconds",
                    "2",
                    "--max-scene-seconds",
                    "5",
                    "--config-file",
                    config_path.to_str().expect("config path exists"),
                    "--temp",
                    temp.path().to_str().expect("temp path exists"),
                    "--logs",
                    logs_path.to_str().expect("log path exists"),
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::DetectScenes {
                            method: Some(SceneDetectionMethod::Fast),
                            min_scene_seconds: Some(2),
                            max_scene_seconds: Some(5),
                            ..
                        }),
                        ..
                    }),
                    "\"condor detect-scenes --method fast --min-scene-seconds 2 \
                     --max-scene-seconds 5\" parses"
                );
            }

            #[test]
            fn with_invalid_optional_flags() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "--min-scene-seconds",
                    "pi",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor detect-scenes --min-scene-seconds pi\" does not parse"
                );
                let result = CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "--max-scene-seconds",
                    "-12",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor detect-scenes --max-scene-seconds -12\" does not parse"
                );
                let result = CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "--method",
                    "thefastestoneyoucangivemeplease",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor --input INPUT --output OUTPUT --method \
                     thefastestoneyoucangivemeplease\" does not parse"
                );
            }
        }

        mod detect_noise {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "detect-noise"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::DetectNoise { .. }),
                        ..
                    }),
                    "\"condor detect-noise\" parses"
                );
            }

            #[test]
            fn with_input() {
                let result =
                    CondorCli::try_parse_from(["condor", "detect-noise", "--input", "./input.mp4"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::DetectNoise {
                            input: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor detect-noise --input INPUT\" parses"
                );
            }

            #[test]
            fn with_vs_args() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "detect-noise",
                    "--input",
                    "./script.vpy",
                    "--vs-args",
                    "preset=slow",
                    "--vs-args",
                    "threads=4",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command:
                            Some(Commands::DetectNoise {
                                input: Some(_),
                                vs_args: Some(ref args),
                                ..
                            }),
                        ..
                    }) if args.len() == 2,
                    "\"condor detect-noise --input ./script.vpy --vs_args \"preset=slow\" --vs-args \"threads=4\" parses"
                );
            }
        }

        mod scale_noise {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "scale-noise"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleNoise { .. }),
                        ..
                    }),
                    "\"condor scale-noise\" parses"
                );
            }

            #[test]
            fn with_threshold() {
                let result =
                    CondorCli::try_parse_from(["condor", "scale-noise", "--threshold", "0.002"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleNoise {
                            threshold: Some(0.002),
                            ..
                        }),
                        ..
                    }),
                    "\"condor scale-noise --threshold 0.002\" parses"
                );
            }

            #[test]
            fn with_min_max_scaler() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "scale-noise",
                    "--minimum-scaler",
                    "0.5",
                    "--maximum-scaler",
                    "2.0",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleNoise {
                            minimum_scaler: Some(0.5),
                            maximum_scaler: Some(2.0),
                            ..
                        }),
                        ..
                    }),
                    "\"condor scale-noise --minimum-scaler 0.5 --maximum-scaler 2.0\" parses"
                );
            }

            #[test]
            fn with_scale_chroma() {
                let result = CondorCli::try_parse_from(["condor", "scale-noise", "--scale-chroma"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleNoise {
                            scale_chroma: true,
                            ..
                        }),
                        ..
                    }),
                    "\"condor scale-noise --scale-chroma\" parses"
                );
            }

            #[test]
            fn with_all_options() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "scale-noise",
                    "--threshold",
                    "0.005",
                    "--minimum-scaler",
                    "0.3",
                    "--maximum-scaler",
                    "1.5",
                    "--scale-chroma",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleNoise {
                            threshold: Some(0.005),
                            minimum_scaler: Some(0.3),
                            maximum_scaler: Some(1.5),
                            scale_chroma: true,
                            ..
                        }),
                        ..
                    }),
                    "\"condor scale-noise\" with all options parses"
                );
            }
        }

        mod benchmark {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "benchmark"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Benchmark { .. }),
                        ..
                    }),
                    "\"condor benchmark\" parses"
                );
            }

            #[test]
            fn with_threshold() {
                let result =
                    CondorCli::try_parse_from(["condor", "benchmark", "--threshold", "10"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Benchmark {
                            threshold: Some(10),
                            ..
                        }),
                        ..
                    }),
                    "\"condor benchmark --threshold 10\" parses"
                );
            }

            #[test]
            fn with_max_memory() {
                let result =
                    CondorCli::try_parse_from(["condor", "benchmark", "--max-memory", "4096"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Benchmark {
                            max_memory: Some(4096),
                            ..
                        }),
                        ..
                    }),
                    "\"condor benchmark --max-memory 4096\" parses"
                );
            }

            #[test]
            fn with_invalid_threshold_range() {
                let result =
                    CondorCli::try_parse_from(["condor", "benchmark", "--threshold", "101"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor benchmark --threshold 101\" does not parse (range 0..=100)"
                );
            }

            #[test]
            fn with_invalid_threshold_negative() {
                let result =
                    CondorCli::try_parse_from(["condor", "benchmark", "--threshold", "-5"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor benchmark --threshold -5\" does not parse"
                );
            }
        }

        mod target_quality {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "target-quality"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality { .. }),
                        ..
                    }),
                    "\"condor target-quality\" parses"
                );
            }

            #[test]
            fn with_input() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "target-quality",
                    "--input",
                    "./input.mp4",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality {
                            input: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor target-quality --input INPUT\" parses"
                );
            }

            #[test]
            fn with_metric_and_target() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "target-quality",
                    "--metric",
                    "ssimulacra2",
                    "--target",
                    "90",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality {
                            metric: Some(TargetQualityMetric::SSIMULACRA2),
                            target: Some(90.0),
                            ..
                        }),
                        ..
                    }),
                    "\"condor target-quality --metric ssimulacra2 --target 90\" parses"
                );
            }

            #[test]
            fn with_profile() {
                let result =
                    CondorCli::try_parse_from(["condor", "target-quality", "--profile", "slow"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality {
                            profile: Some(TargetQualityProfile::Slow),
                            ..
                        }),
                        ..
                    }),
                    "\"condor target-quality --profile slow\" parses"
                );
            }

            #[test]
            fn with_all_metric_variants() {
                for (name, expected) in [
                    ("vmaf", TargetQualityMetric::VMAF),
                    ("ssimulacra2", TargetQualityMetric::SSIMULACRA2),
                    ("butteraugli", TargetQualityMetric::BUTTERAUGLI),
                    ("butteraugli-3", TargetQualityMetric::BUTTERAUGLI3Norm),
                    ("xpsnr", TargetQualityMetric::XPSNR),
                    ("cvvdp", TargetQualityMetric::CVVDP),
                ] {
                    let result = CondorCli::try_parse_from([
                        "condor",
                        "target-quality",
                        "--metric",
                        name,
                        "--target",
                        "80",
                    ]);
                    assert_matches!(
                        result,
                        Ok(CondorCli {
                            command: Some(Commands::TargetQuality {
                                metric: Some(_),
                                ..
                            }),
                            ..
                        }),
                        "\"condor target-quality --metric {name}\" parses"
                    );
                    if let Ok(CondorCli {
                        command:
                            Some(Commands::TargetQuality {
                                metric: Some(ref m),
                                ..
                            }),
                        ..
                    }) = result
                    {
                        assert_eq!(m, &expected, "metric variant matches for {name}");
                    }
                }
            }

            #[test]
            fn with_all_profile_variants() {
                for (name, expected) in [
                    ("fast", TargetQualityProfile::Fast),
                    ("standard", TargetQualityProfile::Standard),
                    ("slow", TargetQualityProfile::Slow),
                ] {
                    let result =
                        CondorCli::try_parse_from(["condor", "target-quality", "--profile", name]);
                    assert_matches!(
                        result,
                        Ok(CondorCli {
                            command: Some(Commands::TargetQuality {
                                profile: Some(_),
                                ..
                            }),
                            ..
                        }),
                        "\"condor target-quality --profile {name}\" parses"
                    );
                    if let Ok(CondorCli {
                        command:
                            Some(Commands::TargetQuality {
                                profile: Some(ref p),
                                ..
                            }),
                        ..
                    }) = result
                    {
                        assert_eq!(p, &expected, "profile variant matches for {name}");
                    }
                }
            }

            #[test]
            fn with_min_max_quantizer() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "target-quality",
                    "--min-q",
                    "10",
                    "--max-q",
                    "50",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality {
                            minimum_quantizer: Some(10),
                            maximum_quantizer: Some(50),
                            ..
                        }),
                        ..
                    }),
                    "\"condor target-quality --min-q 10 --max-q 50\" parses"
                );
            }

            #[test]
            fn with_params() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "target-quality",
                    "--params",
                    "--preset 2 --crf 24",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality {
                            params: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor target-quality --params ...\" parses"
                );
            }

            #[test]
            fn with_invalid_metric() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "target-quality",
                    "--metric",
                    "nonexistent",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor target-quality --metric nonexistent\" does not parse"
                );
            }

            #[test]
            fn with_invalid_profile() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "target-quality",
                    "--profile",
                    "nonexistent",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor target-quality --profile nonexistent\" does not parse"
                );
            }
        }

        mod optimize_bitrate {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "optimize-bitrate"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::OptimizeBitrate { .. }),
                        ..
                    }),
                    "\"condor optimize-bitrate\" parses"
                );
            }

            #[test]
            fn with_sigma_threshold() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "optimize-bitrate",
                    "--sigma-threshold",
                    "3",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::OptimizeBitrate {
                            sigma_threshold: Some(3),
                            ..
                        }),
                        ..
                    }),
                    "\"condor optimize-bitrate --sigma-threshold 3\" parses"
                );
            }

            #[test]
            fn with_invalid_sigma_below_range() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "optimize-bitrate",
                    "--sigma-threshold",
                    "0",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor optimize-bitrate --sigma-threshold 0\" does not parse (range 1..=10)"
                );
            }

            #[test]
            fn with_invalid_sigma_above_range() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "optimize-bitrate",
                    "--sigma-threshold",
                    "11",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor optimize-bitrate --sigma-threshold 11\" does not parse (range \
                     1..=10)"
                );
            }
        }

        mod scale_speed {
            use super::*;

            #[test]
            fn requires_both_quantizers_and_speeds() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "scale-speed",
                    "--quantizers",
                    "20",
                    "--speeds",
                    "5",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleSpeed { .. }),
                        ..
                    }),
                    "\"condor scale-speed --quantizers 20 --speeds 5\" parses"
                );
            }

            #[test]
            fn with_multiple_pairs() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "scale-speed",
                    "--quantizers",
                    "20",
                    "--speeds",
                    "5",
                    "--quantizers",
                    "30",
                    "--speeds",
                    "4",
                    "--quantizers",
                    "55",
                    "--speeds",
                    "2",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleSpeed {
                            quantizers: Some(_),
                            speeds: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor scale-speed\" with three pairs parses"
                );
                if let Ok(CondorCli {
                    command:
                        Some(Commands::ScaleSpeed {
                            quantizers: Some(ref q),
                            speeds: Some(ref s),
                            ..
                        }),
                    ..
                }) = result
                {
                    assert_eq!(q.len(), 3, "three quantizer values");
                    assert_eq!(s.len(), 3, "three speed values");
                    assert_eq!(q[0], 20);
                    assert_eq!(s[0], 5);
                    assert_eq!(q[1], 30);
                    assert_eq!(s[1], 4);
                    assert_eq!(q[2], 55);
                    assert_eq!(s[2], 2);
                }
            }

            #[test]
            fn quantizers_without_speeds() {
                let result =
                    CondorCli::try_parse_from(["condor", "scale-speed", "--quantizers", "20"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor scale-speed --quantizers 20\" without --speeds fails"
                );
            }

            #[test]
            fn speeds_without_quantizers() {
                let result = CondorCli::try_parse_from(["condor", "scale-speed", "--speeds", "5"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor scale-speed --speeds 5\" without --quantizers fails"
                );
            }

            #[test]
            fn with_negative_quantizers() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "scale-speed",
                    "--quantizers",
                    "-10",
                    "--speeds",
                    "7",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor scale-speed --quantizers -10 ...\" fails (no allow_hyphen_values)"
                );
            }
        }

        mod encode {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "encode"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode { .. }),
                        ..
                    }),
                    "\"condor encode\" parses"
                );
            }

            #[test]
            fn with_input() {
                let result =
                    CondorCli::try_parse_from(["condor", "encode", "--input", "./input.mp4"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            input: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --input INPUT\" parses"
                );
            }

            #[test]
            fn with_encoder_and_workers() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "encode",
                    "--encoder",
                    "x264",
                    "--workers",
                    "4",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            encoder: Some(EncoderMethod::X264),
                            workers: Some(4),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --encoder x264 --workers 4\" parses"
                );
            }

            #[test]
            fn with_passes() {
                let result = CondorCli::try_parse_from(["condor", "encode", "--passes", "2"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            passes: Some(2),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --passes 2\" parses"
                );
            }

            #[test]
            fn with_params() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "encode",
                    "--params",
                    "--preset 8 --crf 30",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            params: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --params ...\" parses"
                );
            }

            #[test]
            fn with_photon_noise() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "encode",
                    "--photon-noise",
                    "2400",
                    "--chroma-noise",
                    "1200",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            photon_noise: Some(2400),
                            chroma_noise: Some(1200),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --photon-noise 2400 --chroma-noise 1200\" parses"
                );
            }

            #[test]
            fn with_decoder_and_filters() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "encode",
                    "--decoder",
                    "bestsource",
                    "--filters",
                    "resize:scaler=bilinear;width=1280;height=720;format=yuv420p;",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            decoder: Some(DecoderMethod::BestSource),
                            filters: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --decoder bestsource --filters ...\" parses"
                );
            }

            #[test]
            fn with_vs_args() {
                let result =
                    CondorCli::try_parse_from(["condor", "encode", "--vs-args", "threads=8"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            vs_args: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --vs-args threads=8\" parses"
                );
            }

            #[test]
            fn with_invalid_encoder() {
                let result = CondorCli::try_parse_from(["condor", "encode", "--encoder", "x263"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor encode --encoder x263\" does not parse"
                );
            }

            #[test]
            fn with_invalid_workers_negative() {
                let result = CondorCli::try_parse_from(["condor", "encode", "--workers", "-1"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor encode --workers -1\" does not parse"
                );
            }
        }

        mod concatenate {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "concatenate"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Concatenate { .. }),
                        ..
                    }),
                    "\"condor concatenate\" parses"
                );
            }

            #[test]
            fn with_mkvmerge() {
                let result =
                    CondorCli::try_parse_from(["condor", "concatenate", "--method", "mkvmerge"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Concatenate {
                            method: Some(ConcatenationMethod::MkvMerge),
                            ..
                        }),
                        ..
                    }),
                    "\"condor concatenate --method mkvmerge\" parses"
                );
            }

            #[test]
            fn with_ffmpeg() {
                let result =
                    CondorCli::try_parse_from(["condor", "concatenate", "--method", "ffmpeg"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Concatenate {
                            method: Some(ConcatenationMethod::FFmpeg),
                            ..
                        }),
                        ..
                    }),
                    "\"condor concatenate --method ffmpeg\" parses"
                );
            }

            #[test]
            fn with_ivf() {
                let result =
                    CondorCli::try_parse_from(["condor", "concatenate", "--method", "ivf"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Concatenate {
                            method: Some(ConcatenationMethod::Ivf),
                            ..
                        }),
                        ..
                    }),
                    "\"condor concatenate --method ivf\" parses"
                );
            }

            #[test]
            fn with_invalid_method() {
                let result =
                    CondorCli::try_parse_from(["condor", "concatenate", "--method", "invalid"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor concatenate --method invalid\" does not parse"
                );
            }
        }

        mod clean {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "clean"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Clean { .. }),
                        ..
                    }),
                    "\"condor clean\" parses"
                );
            }

            #[test]
            fn with_all() {
                let result = CondorCli::try_parse_from(["condor", "clean", "--all"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Clean {
                            all: true,
                            ..
                        }),
                        ..
                    }),
                    "\"condor clean --all\" parses"
                );
            }

            #[test]
            fn all_defaults_to_false() {
                let result = CondorCli::try_parse_from(["condor", "clean"]);
                if let Ok(CondorCli {
                    command:
                        Some(Commands::Clean {
                            all, ..
                        }),
                    ..
                }) = result
                {
                    assert!(!all, "--all defaults to false");
                }
            }
        }
    }

    mod enum_mappings {
        use super::*;

        #[test]
        fn scene_detection_method_mapping() {
            // None -> CoreSCDMethod::None
            let result = SceneDetectionMethod::None.as_core_method(Some(120), Some(240));
            assert_matches!(result, CoreSCDMethod::None { .. });
            if let CoreSCDMethod::None {
                minimum_length,
                maximum_length,
            } = result
            {
                assert_eq!(minimum_length, 120);
                assert_eq!(maximum_length, 240);
            }

            // Fast -> CoreSCDMethod::AVSceneChange with Fast
            let result = SceneDetectionMethod::Fast.as_core_method(Some(60), Some(300));
            assert_matches!(result, CoreSCDMethod::AVSceneChange {
                method: ScenecutMethod::Fast,
                ..
            });
            if let CoreSCDMethod::AVSceneChange {
                minimum_length,
                maximum_length,
                ..
            } = result
            {
                assert_eq!(minimum_length, 60);
                assert_eq!(maximum_length, 300);
            }

            // Standard -> CoreSCDMethod::AVSceneChange with Standard
            let result = SceneDetectionMethod::Standard.as_core_method(Some(120), Some(600));
            assert_matches!(result, CoreSCDMethod::AVSceneChange {
                method: ScenecutMethod::Standard,
                ..
            });
            if let CoreSCDMethod::AVSceneChange {
                minimum_length,
                maximum_length,
                ..
            } = result
            {
                assert_eq!(minimum_length, 120);
                assert_eq!(maximum_length, 600);
            }
        }

        #[test]
        fn scene_detection_method_default_lengths() {
            // None with None lengths uses defaults
            let result = SceneDetectionMethod::None.as_core_method(None, None);
            assert_matches!(result, CoreSCDMethod::None { .. });
            if let CoreSCDMethod::None {
                minimum_length,
                maximum_length,
            } = result
            {
                assert_eq!(
                    minimum_length,
                    andean_condor::models::sequence::scene_detector::DEFAULT_MIN_SCENE_LENGTH_FRAMES
                        as usize
                );
                assert_eq!(
                    maximum_length,
                    andean_condor::models::sequence::scene_detector::
                        DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize
                        * andean_condor::models::sequence::scene_detector::
                            DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize
                );
            }
        }

        #[test]
        fn encoder_method_mapping() {
            assert_matches!(EncoderMethod::AOM.as_encoder_base(), EncoderBase::AOM);
            assert_matches!(EncoderMethod::RAV1E.as_encoder_base(), EncoderBase::RAV1E);
            assert_matches!(EncoderMethod::VPX.as_encoder_base(), EncoderBase::VPX);
            assert_matches!(EncoderMethod::SVTAV1.as_encoder_base(), EncoderBase::SVTAV1);
            assert_matches!(EncoderMethod::AVM.as_encoder_base(), EncoderBase::AVM);
            assert_matches!(EncoderMethod::X264.as_encoder_base(), EncoderBase::X264);
            assert_matches!(EncoderMethod::X265.as_encoder_base(), EncoderBase::X265);
            assert_matches!(EncoderMethod::VVenC.as_encoder_base(), EncoderBase::VVenC);
            assert_matches!(EncoderMethod::FFmpeg.as_encoder_base(), EncoderBase::FFmpeg);
        }

        #[test]
        fn decoder_method_strum_serialization() {
            use super::DecoderMethod;
            assert_eq!(<&str>::from(&DecoderMethod::BestSource), "bestsource");
            assert_eq!(<&str>::from(&DecoderMethod::VSFFMS2), "vs-ffms2");
            assert_eq!(<&str>::from(&DecoderMethod::LSMASHWorks), "lsmash");
            assert_eq!(<&str>::from(&DecoderMethod::DGDecodeNV), "dgdecnv");
            assert_eq!(<&str>::from(&DecoderMethod::FFMS2), "ffms2");
        }

        #[test]
        fn decoder_method_from_str() {
            use std::str::FromStr;
            assert_matches!(
                DecoderMethod::from_str("bestsource"),
                Ok(DecoderMethod::BestSource)
            );
            assert_matches!(
                DecoderMethod::from_str("vs-ffms2"),
                Ok(DecoderMethod::VSFFMS2)
            );
            assert_matches!(
                DecoderMethod::from_str("lsmash"),
                Ok(DecoderMethod::LSMASHWorks)
            );
            assert_matches!(
                DecoderMethod::from_str("dgdecnv"),
                Ok(DecoderMethod::DGDecodeNV)
            );
            assert_matches!(DecoderMethod::from_str("ffms2"), Ok(DecoderMethod::FFMS2));
            assert_matches!(DecoderMethod::from_str("nonexistent"), Err(_));
        }

        #[test]
        fn concatenation_method_strum_serialization() {
            assert_eq!(<&str>::from(&ConcatenationMethod::MkvMerge), "mkvmerge");
            assert_eq!(<&str>::from(&ConcatenationMethod::FFmpeg), "ffmpeg");
            assert_eq!(<&str>::from(&ConcatenationMethod::Ivf), "ivf");
        }

        #[test]
        fn encoder_method_strum_serialization() {
            assert_eq!(<&str>::from(&EncoderMethod::AOM), "aom");
            assert_eq!(<&str>::from(&EncoderMethod::RAV1E), "rav1e");
            assert_eq!(<&str>::from(&EncoderMethod::VPX), "vpx");
            assert_eq!(<&str>::from(&EncoderMethod::SVTAV1), "svt-av1");
            assert_eq!(<&str>::from(&EncoderMethod::AVM), "avm");
            assert_eq!(<&str>::from(&EncoderMethod::X264), "x264");
            assert_eq!(<&str>::from(&EncoderMethod::X265), "x265");
            assert_eq!(<&str>::from(&EncoderMethod::VVenC), "vvenc");
            assert_eq!(<&str>::from(&EncoderMethod::FFmpeg), "ffmpeg");
        }

        #[test]
        fn target_quality_metric_strum_serialization() {
            assert_eq!(<&str>::from(&TargetQualityMetric::VMAF), "vmaf");
            assert_eq!(
                <&str>::from(&TargetQualityMetric::SSIMULACRA2),
                "ssimulacra2"
            );
            assert_eq!(
                <&str>::from(&TargetQualityMetric::BUTTERAUGLI),
                "butteraugli"
            );
            assert_eq!(
                <&str>::from(&TargetQualityMetric::BUTTERAUGLI3Norm),
                "butteraugli-3"
            );
            assert_eq!(<&str>::from(&TargetQualityMetric::XPSNR), "xpsnr");
            assert_eq!(<&str>::from(&TargetQualityMetric::CVVDP), "cvvdp");
        }

        #[test]
        fn target_quality_profile_strum_serialization() {
            assert_eq!(<&str>::from(&TargetQualityProfile::Fast), "fast");
            assert_eq!(<&str>::from(&TargetQualityProfile::Standard), "standard");
            assert_eq!(<&str>::from(&TargetQualityProfile::Slow), "slow");
        }

        #[test]
        fn scene_detection_method_strum_serialization() {
            assert_eq!(<&str>::from(&SceneDetectionMethod::None), "none");
            assert_eq!(<&str>::from(&SceneDetectionMethod::Fast), "fast");
            assert_eq!(<&str>::from(&SceneDetectionMethod::Standard), "standard");
        }
    }
}
