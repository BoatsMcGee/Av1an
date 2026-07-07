use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models::sequence::SequenceConfigHandler;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpeedScalerConfig
where
    Self: SequenceConfigHandler,
{
    pub speed_quantizers: Vec<(i8, f64)>,
}

impl SequenceConfigHandler for SpeedScalerConfig {
}

pub trait SpeedScalerConfigHandler
where
    Self: SequenceConfigHandler,
{
    fn speed_scaler(&self) -> Result<&SpeedScalerConfig>;
    fn speed_scaler_mut(&mut self) -> Result<&mut SpeedScalerConfig>;
}
