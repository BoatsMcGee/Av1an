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

use crate::commands::{config::ConfigSubcommand, help_text::*};

pub mod config;
pub mod handlers;
pub mod help_text;

#[derive(ClapParser)]
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
#[derive(Subcommand)]
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
    /// View and change configuration values.
    Config {
        #[command(subcommand)]
        subcommand: ConfigSubcommand,
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
        #[arg(long, short('i'), value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
        input:      Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder:    Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters", help = HELP_FILTERS_SHORT, long_help = HELP_FILTERS)]
        filters:    Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args:    Option<Vec<String>>,
        #[arg(long, short('e'), value_name = "Encoder", help = HELP_ENCODER_SHORT, long_help = HELP_ENCODER)]
        encoder:    Option<EncoderMethod>,
        #[arg(long, value_name = "Passes", help = HELP_PASSES_SHORT, long_help = HELP_PASSES)]
        passes:     Option<u8>,
        #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_PARAMS_SHORT, long_help = HELP_PARAMS)]
        params:     Option<String>,
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
    Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, DisplayMacro, clap::ValueEnum,
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
    Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, DisplayMacro, clap::ValueEnum,
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
    Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, DisplayMacro, clap::ValueEnum,
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
    Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, DisplayMacro, clap::ValueEnum,
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
    Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, DisplayMacro, clap::ValueEnum,
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
    Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, DisplayMacro, clap::ValueEnum,
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
