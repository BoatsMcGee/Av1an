//! Error types for fmetrics scoring.

/// Errors that can occur while loading or scoring with fmetrics.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FmetricsError {
    /// fmetrics could not be found or opened on this system.
    ///
    /// fmetrics ships as a source library rather than as a prebuilt install, so
    /// the usual cause is that neither `FMETRICS_LIB_PATH` nor the executable's
    /// own directory holds a built copy.
    #[error(
        "fmetrics is not available: {reason}. Build it with `zig build -Dshared=true` and place \
         the library beside the executable, or set FMETRICS_LIB_PATH to the directory containing \
         it. See the crate README for build instructions."
    )]
    LibraryNotFound {
        /// Why the load failed.
        reason: String,
    },

    /// An fmetrics entry point returned a non-`FMETRICS_OK` code.
    #[error("fmetrics call `{function}` failed with code {code}: {message}")]
    CallFailed {
        /// Name of the fmetrics function that failed.
        function: &'static str,
        /// The `FmetricsErr` value, as its integer.
        code:     std::ffi::c_int,
        /// The message fmetrics associates with that code.
        message:  String,
    },

    /// A configuration value could not be passed to fmetrics.
    #[error("Invalid fmetrics configuration: {reason}")]
    InvalidConfiguration {
        /// Description of what was invalid.
        reason: String,
    },

    /// The requested metric is not exposed through this crate.
    #[error("Metric `{metric}` is unavailable: {reason}")]
    MetricUnavailable {
        /// Name of the metric that could not be used.
        metric: String,
        /// Underlying reason.
        reason: String,
    },

    /// Every workspace was already in use, so this submission could not start.
    ///
    /// fmetrics' scratch is per-workspace and sharing one corrupts it silently,
    /// so a pool cannot hand the same workspace to two threads. Running out
    /// means the caller is submitting from more threads at once than the pool
    /// has workspaces, which is a configuration error rather than a transient
    /// condition to wait out.
    #[error("All {workspaces} workspaces are in use: {reason}")]
    PoolExhausted {
        /// How many workspaces the pool holds.
        workspaces: usize,
        /// Underlying reason.
        reason:     String,
    },

    /// The reference and distorted inputs are not comparable.
    #[error("Input mismatch: {reason}")]
    InputMismatch {
        /// Description of the mismatch.
        reason: String,
    },

    /// A frame did not satisfy fmetrics' requirements.
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

    /// Reading from a decoder failed.
    #[error("Failed to read frame: {message}")]
    Decode {
        /// Underlying decoder error.
        message: String,
    },
}

impl From<av_decoders::DecoderError> for FmetricsError {
    #[inline]
    fn from(error: av_decoders::DecoderError) -> Self {
        Self::Decode {
            message: error.to_string(),
        }
    }
}
