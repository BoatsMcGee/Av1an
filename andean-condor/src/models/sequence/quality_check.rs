use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models::{
    input::Input as InputModel,
    sequence::{
        SequenceConfigHandler,
        target_quality::types::{ProbeStatistic, ProbeStrategy, QualityMetric, QualityPass},
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityCheckConfig
where
    Self: SequenceConfigHandler,
{
    pub metric:    QualityMetric,
    pub strategy:  ProbeStrategy,
    pub statistic: ProbeStatistic,
    pub input:     Option<InputModel>,
}

impl Default for QualityCheckConfig {
    #[inline]
    fn default() -> Self {
        Self {
            metric:    QualityMetric::default(),
            strategy:  ProbeStrategy::Whole,
            statistic: ProbeStatistic::Mean,
            input:     None,
        }
    }
}

impl SequenceConfigHandler for QualityCheckConfig {
}

pub trait QualityCheckConfigHandler {
    fn quality_check(&self) -> Result<&Option<QualityCheckConfig>>;
    fn quality_check_mut(&mut self) -> Result<&mut Option<QualityCheckConfig>>;
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QualityCheckData {
    pub quality: QualityPass,
}

pub trait QualityCheckDataHandler {
    fn get_quality_check(&self) -> Result<&QualityCheckData>;
    fn get_quality_check_mut(&mut self) -> Result<&mut QualityCheckData>;
}
