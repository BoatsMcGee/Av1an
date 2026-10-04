//! Error types for vship scoring.

/// Errors that can occur while loading or scoring with libvship.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VshipError {
    /// libvship could not be found or opened on this system.
    ///
    /// libvship ships as a VapourSynth plugin rather than as a standalone
    /// install, so the usual cause is that neither `VSHIP_PLUGIN_PATH` nor
    /// `VSSCRIPT_PATH` points at an installation that has it.
    #[error(
        "libvship is not available: {reason}. Install it as the VapourSynth plugin `libvship` \
         beside `vsscript.dll`, or set VSHIP_PLUGIN_PATH to the directory containing it. See the \
         crate README for install instructions."
    )]
    LibraryNotFound {
        /// Why the load failed.
        reason: String,
    },

    /// A libvship entry point returned a non-zero status.
    #[error("libvship call `{function}` failed with status {status}: {message}")]
    CallFailed {
        /// Name of the libvship function that failed.
        function: &'static str,
        /// The `Vship_Exception` value, as its integer.
        status:   std::ffi::c_int,
        /// The message libvship associates with that status.
        message:  String,
    },

    /// `Vship_GetDeviceCount` reported a negative count.
    #[error("libvship reported an implausible device count: {count}")]
    DeviceCount {
        /// The value libvship wrote.
        count: std::ffi::c_int,
    },

    /// A configuration value could not be passed to libvship.
    #[error("Invalid vship configuration: {reason}")]
    InvalidConfiguration {
        /// Description of what was invalid.
        reason: String,
    },

    /// The requested metric could not be initialised on this machine.
    #[error("Metric `{metric}` is unavailable: {reason}")]
    MetricUnavailable {
        /// Name of the metric that could not be initialised.
        metric: String,
        /// Underlying reason.
        reason: String,
    },

    /// The reference and distorted inputs are not comparable.
    #[error("Input mismatch: {reason}")]
    InputMismatch {
        /// Description of the mismatch.
        reason: String,
    },

    /// A frame did not satisfy libvship's requirements.
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

impl From<av_decoders::DecoderError> for VshipError {
    #[inline]
    fn from(error: av_decoders::DecoderError) -> Self {
        Self::Decode {
            message: error.to_string(),
        }
    }
}
