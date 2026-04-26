use std::time::SystemTime;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models::{input::Input as InputModel, sequence::SequenceConfigHandler};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseDetectorData {
    pub noise:      f64,
    pub luminance:  f64,
    pub created_on: SystemTime,
}

pub trait NoiseDetectorDataHandler {
    fn get_noise_detection(&self) -> Result<&Option<NoiseDetectorData>>;
    fn get_noise_detection_mut(&mut self) -> Result<&mut Option<NoiseDetectorData>>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NoiseDetectorConfig
where
    Self: SequenceConfigHandler,
{
    pub input: Option<InputModel>,
}

impl SequenceConfigHandler for NoiseDetectorConfig {
}
