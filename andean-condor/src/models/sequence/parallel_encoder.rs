use std::path::{Path, PathBuf};

use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::{
    input::Input as InputModel,
    sequence::{SequenceConfigHandler, SequenceDataHandler},
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ParallelEncoderConfig
where
    Self: SequenceConfigHandler,
{
    pub workers: Option<u8>,
    /// Total scene attempts including the initial attempt (not additional retries).
    #[serde(default = "max_tries_default")]
    #[schemars(range(min = 1))]
    pub max_tries: u32,
    pub scenes_directory: PathBuf,
    pub input: Option<InputModel>,
    /// Decode-count each completed chunk with ffprobe before publishing/resuming.
    /// Disable only for intentional frame-changing filters; status and size remain mandatory.
    #[serde(default = "validate_frame_count_default")]
    pub validate_frame_count: bool,
}

fn validate_frame_count_default() -> bool {
    true
}

fn max_tries_default() -> u32 {
    3
}

impl Default for ParallelEncoderConfig {
    #[inline]
    fn default() -> Self {
        Self {
            workers: None,
            max_tries: max_tries_default(),
            scenes_directory: PathBuf::new(),
            input: None,
            validate_frame_count: true,
        }
    }
}

impl ParallelEncoderConfig {
    /// Offline settings validation; never opens a source or resolves an executable.
    #[inline]
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.max_tries >= 1,
            "max_tries must be at least 1 total attempt"
        );
        Ok(())
    }

    #[inline]
    pub fn new(scenes_directory: &Path) -> Self {
        Self {
            scenes_directory: scenes_directory.to_path_buf(),
            ..Default::default()
        }
    }
}

impl SequenceConfigHandler for ParallelEncoderConfig {}

pub trait ParallelEncoderConfigHandler
where
    Self: SequenceConfigHandler,
{
    fn parallel_encoder(&self) -> Result<&ParallelEncoderConfig>;
    fn parallel_encoder_mut(&mut self) -> Result<&mut ParallelEncoderConfig>;
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ParallelEncoderData {
    #[serde(default)]
    pub provenance: Option<super::provenance::ResultProvenance>,
    /// Must be milliseconds since UNIX Epoch
    pub started_on: Option<u128>,
    /// Must be milliseconds since UNIX Epoch
    pub completed_on: Option<u128>,
    pub bytes: Option<u64>,
}

pub trait ParallelEncoderDataHandler
where
    Self: SequenceDataHandler,
{
    fn get_parallel_encoder(&self) -> Result<&ParallelEncoderData>;
    fn get_parallel_encoder_mut(&mut self) -> Result<&mut ParallelEncoderData>;
}
