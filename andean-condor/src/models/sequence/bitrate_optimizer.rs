use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models::sequence::SequenceConfigHandler;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BitrateOptimizerConfig
where
    Self: SequenceConfigHandler,
{
    /// Standard Deviation distance from average scene bitrate to be considered
    /// an excessively large scene
    pub bitrate_sigma_threshold: Option<u8>,
}

impl SequenceConfigHandler for BitrateOptimizerConfig {
}

pub trait BitrateOptimizerConfigHandler {
    fn bitrate_optimizer(&self) -> Result<&BitrateOptimizerConfig>;
    fn bitrate_optimizer_mut(&mut self) -> Result<&mut BitrateOptimizerConfig>;
}
