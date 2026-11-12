use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::models::{
    encoder::{Encoder, photon_noise::PhotonNoise},
    input::Input,
    scene::Scene,
    sequence::{
        SequenceConfigHandler,
        noise_detector::{NoiseDetectorConfig, NoiseDetectorData},
    },
};

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoiseScalerData {
    pub scaler: f64,
    /// Only NoiseScaler's successful publication supplies reversible ISO proof.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<NoiseScalerBaseline>,
}

/// Exact PhotonNoise before/after scaling, plus its declared measurement inputs.
/// No inverse of rounded ISO values can recover this original reliably.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoiseScalerBaseline {
    pub original: PhotonNoise,
    pub scaled: PhotonNoise,
    pub context: serde_json::Value,
}

impl NoiseScalerData {
    /// Withdraw only a proven scaling effect; preserve every other encoder option.
    #[inline]
    pub fn restore(&self, encoder: &mut Encoder, index: usize) -> Result<()> {
        let baseline = self.baseline.as_ref().ok_or_else(|| anyhow::anyhow!(
            "scene {index} noise scaling has no original PhotonNoise baseline; preserve this job and use a new job with distinct config, empty temp directory and output; resetting a legacy scaling record cannot safely undo rounded ISO values"))?;
        anyhow::ensure!(
            encoder
                .photon_noise()
                .is_some_and(|photon| photon == &baseline.scaled || photon == &baseline.original),
            "scene {index} PhotonNoise changed after noise scaling; cannot safely restore its original values. Reset noise-scaling before editing PhotonNoise, or preserve this job and use a new job with distinct config, empty temp directory and output"
        );
        encoder.set_photon_noise(Some(baseline.original.clone()));
        Ok(())
    }
}

/// Scoped dependency snapshot for scaling, not a source-file/binary attestation.
#[inline]
pub fn scaling_context<Data>(
    input: &Input,
    detector: Option<&NoiseDetectorConfig>,
    config: Option<&NoiseScalerConfig>,
    scene: &Scene<Data>,
    noise: Option<&NoiseDetectorData>,
    maximum_noise: Option<f64>,
) -> serde_json::Value {
    let eligible = match &scene.encoder {
        Encoder::AOM { .. } | Encoder::RAV1E { .. } => true,
        Encoder::SVTAV1 { options, .. } => !options.contains_key("film-grain"),
        _ => false,
    };
    serde_json::json!({"input": input, "detector": detector, "config": config,
        "start": scene.start_frame, "end": scene.end_frame, "sub_scenes": scene.sub_scenes,
        "noise": noise, "maximum_noise": maximum_noise, "eligible": eligible})
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
