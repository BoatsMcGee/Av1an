use std::time::SystemTime;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{
    models::{input::Input as InputModel, sequence::SequenceConfigHandler},
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseDetectorConfig
where
    Self: SequenceConfigHandler,
{
    pub input:             Option<InputModel>,
    pub reference_filters: Vec<VapourSynthFilter>,
    pub denoised_filters:  Vec<VapourSynthFilter>,
}

impl SequenceConfigHandler for NoiseDetectorConfig {
}

impl Default for NoiseDetectorConfig {
    #[inline]
    fn default() -> Self {
        Self {
            input:             None,
            reference_filters: vec![VapourSynthFilter::WNNM {
                sigma:                Some(vec![3.0, 0.0, 0.0]),
                block_size:           None,
                block_step:           None,
                group_size:           None,
                bm_range:             None,
                radius:               None,
                ps_num:               None,
                ps_range:             None,
                residual:             None,
                adaptive_aggregation: None,
            }],
            denoised_filters:  vec![VapourSynthFilter::WNNM {
                sigma:                Some(vec![6.0, 0.0, 0.0]),
                block_size:           None,
                block_step:           None,
                group_size:           None,
                bm_range:             None,
                radius:               None,
                ps_num:               None,
                ps_range:             None,
                residual:             None,
                adaptive_aggregation: None,
            }],
        }
    }
}

pub trait NoiseDetectorConfigHandler
where
    Self: SequenceConfigHandler,
{
    fn noise_detector(&self) -> Result<&Option<NoiseDetectorConfig>>;
    fn noise_detector_mut(&mut self) -> Result<&mut Option<NoiseDetectorConfig>>;
}
