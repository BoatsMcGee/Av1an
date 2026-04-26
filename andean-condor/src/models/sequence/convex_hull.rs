use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models::sequence::SequenceConfigHandler;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConvexHullConfig
where
    Self: SequenceConfigHandler,
{
    pub speed_quantizers: Vec<(i8, f64)>,
}

impl SequenceConfigHandler for ConvexHullConfig {
}

pub trait ConvexHullConfigHandler
where
    Self: SequenceConfigHandler,
{
    fn convex_hull(&self) -> Result<&ConvexHullConfig>;
    fn convex_hull_mut(&mut self) -> Result<&mut ConvexHullConfig>;
}
