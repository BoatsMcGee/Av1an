use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::{
    models::encoder::cli_parameter::CLIParameter,
    vapoursynth::plugins::vship::cvvdp::DisplayModel,
};

pub static DEFAULT_MAXIMUM_PROBES: u8 = 4;
pub static DEFAULT_VMAF_TARGET_RANGE: (f64, f64) = (94.0, 96.0);
pub static DEFAULT_SSIMULACRA2_TARGET_RANGE: (f64, f64) = (74.0, 76.0);
pub static DEFAULT_BUTTERAUGLI_TARGET_RANGE: (f64, f64) = (0.8, 1.2);
pub static DEFAULT_XPSNR_TARGET_RANGE: (f64, f64) = (44.0, 46.0);
pub static DEFAULT_CVVDP_TARGET_RANGE: (f64, f64) = (9.4, 9.6);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString, IntoStaticStr,
)]
pub enum InterpolationMethod {
    #[strum(serialize = "linear")]
    Linear,
    #[strum(serialize = "quadratic")]
    Quadratic,
    #[strum(serialize = "natural")]
    Natural,
    #[strum(serialize = "pchip")]
    Pchip,
    #[strum(serialize = "catmull")]
    Catmull,
    #[strum(serialize = "akima")]
    Akima,
    #[strum(serialize = "cubicpolynomial")]
    CubicPolynomial,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, EnumString, IntoStaticStr, Display)]
pub enum VmafFeature {
    #[strum(serialize = "default")]
    Default,
    #[strum(serialize = "weighted")]
    Weighted,
    #[strum(serialize = "neg")]
    Neg,
    #[strum(serialize = "motionless")]
    Motionless,
    #[strum(serialize = "uhd")]
    Uhd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QualityMetric {
    VMAF {
        target_range: (f64, f64),
        resolution:   Option<(u32, u32)>,
        scaler:       String,
        filter:       Option<String>,
        threads:      usize,
        model:        Option<PathBuf>,
        features:     Vec<VmafFeature>,
    },
    SSIMULACRA2 {
        target_range: (f64, f64),
        resolution:   Option<(u32, u32)>,
        threads:      Option<u8>,
    },
    BUTTERAUGLI {
        target_range:         (f64, f64),
        resolution:           Option<(u32, u32)>,
        threads:              Option<u8>,
        intensity_multiplier: Option<f64>,
        norm:                 Option<u8>,
    },
    XPSNR {
        target_range: (f64, f64),
        resolution:   Option<(u32, u32)>,
    },
    CVVDP {
        target_range:      (f64, f64),
        resolution:        Option<(u32, u32)>,
        display_model:     Option<DisplayModel>,
        resize_to_display: Option<bool>,
        disable_temporal:  Option<bool>,
    },
}

impl Default for QualityMetric {
    #[inline]
    fn default() -> Self {
        Self::SSIMULACRA2 {
            target_range: DEFAULT_SSIMULACRA2_TARGET_RANGE,
            resolution:   None,
            threads:      None,
        }
    }
}

impl QualityMetric {
    #[inline]
    pub fn score_within_target(&self, score: f64) -> bool {
        match self {
            QualityMetric::VMAF {
                target_range, ..
            } => score >= target_range.0 && score <= target_range.1,
            QualityMetric::SSIMULACRA2 {
                target_range, ..
            } => score >= target_range.0 && score <= target_range.1,
            QualityMetric::BUTTERAUGLI {
                target_range, ..
            } => score >= target_range.0 && score <= target_range.1,
            QualityMetric::XPSNR {
                target_range, ..
            } => score >= target_range.0 && score <= target_range.1,
            QualityMetric::CVVDP {
                target_range, ..
            } => score >= target_range.0 && score <= target_range.1,
        }
    }

    #[inline]
    pub fn target_range(&self) -> (f64, f64) {
        match self {
            QualityMetric::VMAF {
                target_range, ..
            } => *target_range,
            QualityMetric::SSIMULACRA2 {
                target_range, ..
            } => *target_range,
            QualityMetric::BUTTERAUGLI {
                target_range, ..
            } => *target_range,
            QualityMetric::XPSNR {
                target_range, ..
            } => *target_range,
            QualityMetric::CVVDP {
                target_range, ..
            } => *target_range,
        }
    }

    #[inline]
    pub fn target_range_mut(&mut self) -> &mut (f64, f64) {
        match self {
            QualityMetric::VMAF {
                target_range, ..
            } => target_range,
            QualityMetric::SSIMULACRA2 {
                target_range, ..
            } => target_range,
            QualityMetric::BUTTERAUGLI {
                target_range, ..
            } => target_range,
            QualityMetric::XPSNR {
                target_range, ..
            } => target_range,
            QualityMetric::CVVDP {
                target_range, ..
            } => target_range,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TargetQualityProbing {
    pub encoder_options: Option<HashMap<String, CLIParameter>>,
    pub strategy:        ProbeStategy,
    pub statistic:       ProbeStatistic,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum ProbeStategy {
    #[default]
    Whole,
    Skip {
        skip: u32,
    },
    Subset {
        position: SubsetProbePosition,
        length:   SubsetProbeLength,
    },
}

impl ProbeStategy {
    #[inline]
    pub fn frame_indices(&self, start: usize, end: usize) -> Vec<usize> {
        match self {
            ProbeStategy::Whole => (start..end).collect::<Vec<_>>(),
            ProbeStategy::Skip {
                skip,
            } => (start..end)
                .filter(|index| {
                    (index - start) == 0 || (index - start).is_multiple_of(*skip as usize)
                })
                .collect::<Vec<_>>(),
            ProbeStategy::Subset {
                position,
                length,
            } => {
                let max_length = end - start;
                let length = match length {
                    SubsetProbeLength::Percentage(percentage) => {
                        (percentage * max_length as f64).round() as usize
                    },
                    SubsetProbeLength::Frames(frames) => (*frames as usize).min(max_length),
                };
                match position {
                    SubsetProbePosition::Start => {
                        (start..(start + length).min(end)).collect::<Vec<_>>()
                    },
                    SubsetProbePosition::Middle => {
                        let middle = start + (max_length / 2);
                        let middle_start = (middle - (length / 2)).max(start);
                        let middle_end = (middle + (length as f64 / 2.0).ceil() as usize).min(end);
                        (middle_start..middle_end).collect::<Vec<_>>()
                    },
                    SubsetProbePosition::End => {
                        ((end - length).max(start)..end).collect::<Vec<_>>()
                    },
                }
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub enum SubsetProbePosition {
    Start,
    #[default]
    Middle,
    End,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SubsetProbeLength {
    Percentage(f64),
    Frames(u32),
}

impl Default for SubsetProbeLength {
    #[inline]
    fn default() -> Self {
        SubsetProbeLength::Frames(11)
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub enum ProbeStatistic {
    #[default]
    Mean,
    Median,
    Harmonic,
    Percentile(f64),
    StandardDeviationDistance {
        sigma: f64,
    },
    Mode,
    Minimum,
    Maximum,
    RootMeanSquare,
}

impl ProbeStatistic {
    #[inline]
    pub fn calculate(&self, scores: &[f64]) -> f64 {
        let mut scores = scores.to_vec();
        scores.sort_by(f64::total_cmp);
        match self {
            ProbeStatistic::Mean => {
                scores.iter().fold(0.0, |acc, score| acc + score) / scores.len() as f64
            },
            ProbeStatistic::Median => {
                let mid = scores.len() / 2;
                if scores.len().is_multiple_of(2) {
                    f64::midpoint(scores[mid - 1], scores[mid])
                } else {
                    scores[mid]
                }
            },
            ProbeStatistic::Harmonic => {
                let sum_reciprocals: f64 = scores.iter().map(|&x| 1.0 / x).sum();
                scores.len() as f64 / sum_reciprocals
            },
            ProbeStatistic::Percentile(index) => *scores
                .get((*index / 100.0 * scores.len() as f64) as usize)
                .unwrap_or(&scores[0]),
            ProbeStatistic::StandardDeviationDistance {
                sigma,
            } => {
                let average = ProbeStatistic::Mean.calculate(&scores);
                let minimum = ProbeStatistic::Minimum.calculate(&scores);
                let maximum = ProbeStatistic::Maximum.calculate(&scores);
                let variance = scores
                    .iter()
                    .map(|x| {
                        let diff = x - average;
                        diff * diff
                    })
                    .sum::<f64>()
                    / scores.len() as f64;
                (average + (sigma * variance.sqrt())).clamp(minimum, maximum)
            },
            ProbeStatistic::Mode => {
                let mut counts = HashMap::new();
                for score in &scores {
                    // Round to nearest integer for fewer unique buckets
                    let rounded_score = score.round() as i32;
                    *counts.entry(rounded_score).or_insert(0) += 1;
                }
                let max_count = counts.values().copied().max().unwrap_or(0);
                *scores
                    .iter()
                    .find(|score| counts[&(score.round() as i32)] == max_count)
                    .unwrap_or(&0.0)
            },
            ProbeStatistic::Minimum => *scores
                .iter()
                .min_by(|a, b| a.partial_cmp(b).expect("should not have NaN in scores set"))
                .unwrap_or(&0.0),
            ProbeStatistic::Maximum => *scores
                .iter()
                .max_by(|a, b| a.partial_cmp(b).expect("should not have NaN in scores set"))
                .unwrap_or(&0.0),
            ProbeStatistic::RootMeanSquare => {
                let sum_of_squares: f64 = scores.iter().map(|&x| x * x).sum();
                (sum_of_squares / scores.len() as f64).sqrt()
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityPass {
    pub quantizer:    f64,
    pub scores:       Vec<f64>,
    pub bitrate:      f64,
    /// Must be in milliseconds since UNIX Epoch
    pub started_on:   u128,
    /// Must be in milliseconds since UNIX Epoch
    pub completed_on: u128,
}

impl Default for QualityPass {
    #[inline]
    fn default() -> Self {
        Self {
            quantizer:    0.0,
            scores:       Vec::new(),
            bitrate:      0.0,
            started_on:   0,
            completed_on: 0,
        }
    }
}
