use anyhow::{Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::{encoder::Encoder, sequence::SequenceConfigHandler};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SpeedScalerConfig
where
    Self: SequenceConfigHandler,
{
    pub speed_quantizers: Vec<(i8, f64)>,
}

impl SequenceConfigHandler for SpeedScalerConfig {}

impl SpeedScalerConfig {
    /// Checks knots and the encoder's prospective settings without running native tools.
    /// Empty knots disable scaling; native-only speed restrictions remain the encoder's job.
    #[inline]
    pub fn validate(&self, encoder: &Encoder) -> Result<()> {
        ensure!(
            self.speed_quantizers.is_empty() || self.speed_quantizers.len() >= 2,
            "At least 2 speed-quantizer pairs are required"
        );
        let mut quantizers: Vec<_> = self.speed_quantizers.iter().map(|(_, q)| *q).collect();
        ensure!(
            quantizers.iter().all(|q| q.is_finite()),
            "speed-scaler quantizers must be finite"
        );
        quantizers.sort_by(f64::total_cmp);
        ensure!(
            quantizers.windows(2).all(|pair| pair[0] != pair[1]),
            "each quantizer may appear only once"
        );
        let mut candidate = encoder.clone();
        for (speed, _) in &self.speed_quantizers {
            candidate.set_speed(*speed);
            candidate.validate_settings()?;
        }
        Ok(())
    }
}

pub trait SpeedScalerConfigHandler
where
    Self: SequenceConfigHandler,
{
    fn speed_scaler(&self) -> Result<&SpeedScalerConfig>;
    fn speed_scaler_mut(&mut self) -> Result<&mut SpeedScalerConfig>;
}
