use thiserror::Error;

use crate::core::encoder::EncoderResult;

#[derive(Debug, Clone, Error)]
pub enum ParallelEncoderError {
    #[error("Must have at least one worker")]
    NoWorkers,
    #[error("No Scenes found")]
    ScenesEmpty,
    #[error("FFmpeg is required for the configured FFmpeg filter but was not found in PATH")]
    FfmpegNotFound,
    #[error("Failed to encode Scene {scene}: {result}")]
    EncoderFailed {
        scene:  usize,
        result: EncoderResult,
    },
}
