use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::models::sequence::SequenceConfigHandler;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NoiseScalerData {
    pub scaler: f64,
}

pub trait NoiseScalerDataHandler {
    fn get_noise_scaling(&self) -> Result<&Option<NoiseScalerData>>;
    fn get_noise_scaling_mut(&mut self) -> Result<&mut Option<NoiseScalerData>>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct NoiseScalerConfig
where
    Self: SequenceConfigHandler,
{
    pub threshold: f64,
    pub minimum_scaler: f64,
    pub maximum_scaler: f64,
    /// Whether to also scale chroma noise
    pub scale_chroma: bool,
}

impl SequenceConfigHandler for NoiseScalerConfig {}

impl NoiseScalerConfig {
    /// Validate physical calibration bounds without imposing an arbitrary upper limit.
    #[inline]
    pub fn validate(&self) -> Result<(), NoiseScalerConfigError> {
        if !self.threshold.is_finite() || self.threshold < 0.0 {
            return Err(NoiseScalerConfigError::InvalidThreshold);
        }
        if !self.minimum_scaler.is_finite() || self.minimum_scaler < 0.0 {
            return Err(NoiseScalerConfigError::InvalidMinimumScaler);
        }
        if !self.maximum_scaler.is_finite() || self.maximum_scaler < 0.0 {
            return Err(NoiseScalerConfigError::InvalidMaximumScaler);
        }
        if self.minimum_scaler > self.maximum_scaler {
            return Err(NoiseScalerConfigError::ScalerRangeInverted);
        }
        Ok(())
    }
}

/// Unusable Noise Scaler calibration bounds, shared by core and configuration edits.
#[derive(Debug, Error)]
pub enum NoiseScalerConfigError {
    #[error("threshold must be a finite, non-negative noise level")]
    InvalidThreshold,
    #[error("minimum scaler must be a finite, non-negative factor")]
    InvalidMinimumScaler,
    #[error("maximum scaler must be a finite, non-negative factor")]
    InvalidMaximumScaler,
    #[error("minimum scaler must not exceed maximum scaler")]
    ScalerRangeInverted,
}

pub trait NoiseScalerConfigHandler
where
    Self: SequenceConfigHandler,
{
    fn noise_scaler(&self) -> Result<&Option<NoiseScalerConfig>>;
    fn noise_scaler_mut(&mut self) -> Result<&mut Option<NoiseScalerConfig>>;
}
