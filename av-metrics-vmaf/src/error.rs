//! Error types for VMAF scoring.

use std::path::PathBuf;

/// Errors that can occur while loading or scoring with libvmaf.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VmafError {
    /// libvmaf could not be found on the system.
    #[error(
        "libvmaf was not found. Install it, or set VMAF_LIB_DIR to the directory containing it. \
         See the crate README for per-distro build instructions."
    )]
    LibraryNotFound,

    /// No candidate loaded; includes attempted paths and original OS diagnostics.
    #[error("libvmaf could not be loaded. Set VMAF_LIB_DIR to its directory. Attempts: {attempts}")]
    LibraryLoadFailed {
        /// Candidate paths and loader errors, including missing dependencies.
        attempts: String,
    },

    /// The loaded library does not match the bounded, header-verified ABI.
    #[error(
        "Incompatible libvmaf runtime `{version}`: {reason}. Reinstall the pinned v3.2.1 \
         release archive/package (verified runtime `3.2.0`, without picture conversion/wrap \
         exports), or point VMAF_LIB_DIR at it. Modified binaries are unsupported."
    )]
    IncompatibleLibrary {
        /// Observed version, or a diagnostic for an invalid native string.
        version: String,
        /// Unverified version or known incompatible API export.
        reason: String,
    },

    /// libvmaf was found but the required symbols could not be resolved.
    #[error("Failed to load symbol `{symbol}` from libvmaf: {message}")]
    SymbolNotFound {
        /// Name of the symbol that could not be resolved.
        symbol: String,
        /// Underlying loader error.
        message: String,
    },

    /// A libvmaf entry point returned a non-zero status.
    #[error("libvmaf call `{function}` failed with status {status}")]
    CallFailed {
        /// Name of the libvmaf function that failed.
        function: &'static str,
        /// Status code returned by libvmaf. Negative values are errno codes.
        status: i32,
    },

    /// A final score query failed after successful native finalization.
    #[error("libvmaf call `vmaf_score_at_index` failed for frame {index} with status {status}")]
    ScoreQueryFailed {
        /// Sequential submitted frame index, never compacted on query failure.
        index: u32,
        /// Original native status, not reclassified as temporary unavailability.
        status: i32,
    },

    /// A configuration value could not be passed to libvmaf.
    #[error("Invalid VMAF configuration: {reason}")]
    InvalidConfiguration {
        /// Description of what was invalid.
        reason: String,
    },

    /// The requested model could not be resolved.
    #[error("Failed to resolve VMAF model `{model}`: {reason}")]
    ModelNotFound {
        /// The model that could not be resolved.
        model: String,
        /// Underlying reason.
        reason: String,
    },

    /// The reference and distorted inputs are not comparable.
    #[error("Input mismatch: {reason}")]
    InputMismatch {
        /// Description of the mismatch.
        reason: String,
    },

    /// A frame did not satisfy libvmaf's requirements.
    #[error("Unsupported frame format: {reason}")]
    UnsupportedFormat {
        /// Description of what was unsupported.
        reason: String,
    },

    /// A frame buffer was too small or otherwise malformed.
    #[error("Malformed frame buffer: {reason}")]
    MalformedFrame {
        /// Description of the problem.
        reason: String,
    },

    /// The frame sources did not produce matching frame counts.
    #[error("Frame count mismatch: reference has {reference}, distorted has {distorted}")]
    FrameCountMismatch {
        /// Frames available from the reference source.
        reference: usize,
        /// Frames available from the distorted source.
        distorted: usize,
    },

    /// A requested feature is unsupported by this wrapper configuration.
    #[error("Unsupported VMAF feature: {0}")]
    UnsupportedFeature(String),

    /// libvmaf did not produce a score for a frame that was submitted.
    ///
    /// Returned instead of skipping the frame, because skipping it would return
    /// a shorter score list and shift every later score into the wrong
    /// slot.
    #[error("libvmaf produced no score for frame {index}")]
    MissingScore {
        /// Index of the submitted frame with no score.
        index: u32,
    },

    /// A model file was expected at a path that does not exist.
    #[error("Model file not found: {}", path.display())]
    ModelFileMissing {
        /// The path that was expected to contain a model.
        path: PathBuf,
    },

    /// Reading from a decoder failed.
    #[error("Failed to read frame: {message}")]
    Decode {
        /// Underlying decoder error.
        message: String,
    },
}

impl From<av_decoders::DecoderError> for VmafError {
    #[inline]
    fn from(error: av_decoders::DecoderError) -> Self {
        Self::Decode {
            message: error.to_string(),
        }
    }
}
