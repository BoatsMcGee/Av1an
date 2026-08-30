use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models::sequence::SequenceConfigHandler;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseScalerData {
    pub scaler: f64,
}

pub trait NoiseScalerDataHandler {
    fn get_noise_scaling(&self) -> Result<&Option<NoiseScalerData>>;
    fn get_noise_scaling_mut(&mut self) -> Result<&mut Option<NoiseScalerData>>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NoiseScalerConfig
where
    Self: SequenceConfigHandler,
{
    pub threshold:      f64,
    pub minimum_scaler: f64,
    pub maximum_scaler: f64,
    /// Whether to also scale chroma noise
    pub scale_chroma:   bool,
}

impl SequenceConfigHandler for NoiseScalerConfig {
}

pub trait NoiseScalerConfigHandler
where
    Self: SequenceConfigHandler,
{
    fn noise_scaler(&self) -> Result<&Option<NoiseScalerConfig>>;
    fn noise_scaler_mut(&mut self) -> Result<&mut Option<NoiseScalerConfig>>;
}
