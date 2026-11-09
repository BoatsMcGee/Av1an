use std::sync::{self, Arc, atomic::AtomicBool};

use anyhow::Result;
use thiserror::Error;
use tracing::debug;

use crate::{
    core::{
        Condor,
        sequence::{Sequence, SequenceDetails, SequenceStatus},
    },
    models::sequence::{
        SequenceConfigHandler,
        SequenceDataHandler,
        bitrate_optimizer::BitrateOptimizerConfigHandler,
        target_quality::TargetQualityDataHandler,
    },
};

pub static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Bitrate Optimizer",
    description: "Optimize scenes that exceed normal bitrate after targeting quality.",
    version:     "0.0.1",
};

#[derive(Debug, Default)]
pub struct BitrateOptimizer {}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for BitrateOptimizer
where
    DataHandler: SequenceDataHandler + TargetQualityDataHandler,
    ConfigHandler: SequenceConfigHandler + BitrateOptimizerConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];

        // Only warn when configured
        if configured_with_empty_scenes(
            condor.sequence_config.bitrate_optimizer()?.bitrate_sigma_threshold,
            condor.scenes.is_empty(),
        ) {
            warnings.push(anyhow::Error::new(BitrateOptimizerError::ScenesEmpty));
        }

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];

        if configured_with_empty_scenes(
            condor.sequence_config.bitrate_optimizer()?.bitrate_sigma_threshold,
            condor.scenes.is_empty(),
        ) {
            warnings.push(anyhow::Error::new(BitrateOptimizerError::ScenesEmpty));
        }

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
        _cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];
        let Some(bitrate_sigma_threshold) =
            condor.sequence_config.bitrate_optimizer()?.bitrate_sigma_threshold
        else {
            return Ok(((), warnings));
        };

        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(BitrateOptimizerError::ScenesEmpty));
            return Ok(((), warnings));
        }

        let scene_values = condor
            .scenes
            .iter()
            .filter_map(|scene| scene.sequence_data.get_target_quality().ok())
            .filter_map(|tq| tq.passes.last())
            .map(|pass| (pass.quantizer, pass.bitrate))
            .collect::<Vec<_>>();
        let scene_quantizers = scene_values.iter().map(|(q, _b)| *q).collect::<Vec<_>>();
        let scene_bitrates = scene_values.iter().map(|(_q, b)| *b).collect::<Vec<_>>();

        let quantizer_average =
            scene_quantizers.iter().sum::<f64>() / scene_quantizers.len() as f64;
        debug!("Average Quantizer: {}", quantizer_average);

        let bitrate_average = scene_bitrates.iter().sum::<f64>() / scene_bitrates.len() as f64;
        let bitrate_variance = scene_bitrates
            .iter()
            .map(|bitrate| (bitrate - bitrate_average).powi(2))
            .sum::<f64>()
            / scene_bitrates.len() as f64;
        let bitrate_standard_deviation = bitrate_variance.sqrt();
        let bitrate_sigma_threshold =
            (bitrate_sigma_threshold as f64).mul_add(bitrate_standard_deviation, bitrate_average);

        for (index, scene) in condor.scenes.iter_mut().enumerate() {
            let Some(final_pass) = scene.sequence_data.get_target_quality()?.passes.last() else {
                continue;
            };
            let Some(target_quantizer) = optimize_scene(
                index,
                final_pass.quantizer,
                final_pass.bitrate,
                quantizer_average,
                bitrate_average,
                bitrate_standard_deviation,
                bitrate_sigma_threshold,
            ) else {
                continue;
            };
            scene.encoder.set_quantizer(target_quantizer);

            // Archived: Alternative "efficiency" approach
            // let data = scene
            //     .sequence_data
            //     .get_target_quality()?
            //     .passes
            //     .iter()
            //     .map(|pass| {
            //         (
            //             pass.quantizer,
            //             (pass.scores.iter().sum::<f64>() / pass.scores.len()
            // as f64) / pass.bitrate,         )
            //     })
            //     .collect::<Vec<_>>();

            // let fit = CurveFit::<ChebyshevBasis>::new_auto(&data,
            // DegreeBound::Custom(2), &Aic)?;

            // for critical_point in fit.critical_points()? {
            //     if let CriticalPoint::Maxima(quantizer, _efficiency) =
            // critical_point {         debug!(
            //             "Optimized Scene {}: Bitrate: {}({}), Quantizer: {}",
            //             index,
            //             final_pass.bitrate,
            //             (final_pass.bitrate - bitrate_average) /
            // bitrate_standard_deviation,             quantizer
            //         );
            //         scene.encoder.set_quantizer(quantizer.round());
            //     }
            // }
        }

        condor.save()?;

        Ok(((), warnings))
    }
}

#[derive(Debug, Clone, Error)]
pub enum BitrateOptimizerError {
    #[error("No Scenes found")]
    ScenesEmpty,
    #[error("Previous Pass data not found")]
    PreviousPassDataNotFound,
    #[error("Failed to measure quality")]
    QualityMeasurementFailed,
}

/// Whether an empty scene list is worth reporting.
#[inline]
fn configured_with_empty_scenes(bitrate_sigma_threshold: Option<u8>, scenes_empty: bool) -> bool {
    bitrate_sigma_threshold.is_some() && scenes_empty
}

/// The quantizer scene `index` should be re-encoded at, or `None` to leave it
/// alone.
///
/// A scene is optimized only when its bitrate is an outlier *and* the average
/// quantizer costs it more bits than it already spends. A scene already encoded
/// at or above the average quantizer would gain nothing.
#[inline]
fn optimize_scene(
    index: usize,
    quantizer: f64,
    bitrate: f64,
    quantizer_average: f64,
    bitrate_average: f64,
    bitrate_standard_deviation: f64,
    bitrate_sigma_threshold: f64,
) -> Option<f64> {
    if bitrate <= bitrate_sigma_threshold {
        return None;
    }

    let target_quantizer = quantizer_average.round();
    if quantizer >= target_quantizer {
        debug!(
            "Skipping Scene {}: Quantizer {} is already at or above the average {}",
            index, quantizer, target_quantizer
        );
        return None;
    }

    debug!(
        "Optimized Scene {}: Bitrate: {}({}), Quantizer: {}",
        index,
        bitrate,
        (bitrate - bitrate_average) / bitrate_standard_deviation,
        target_quantizer,
    );
    Some(target_quantizer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bitrate outlier encoded below the average quantizer is raised to it.
    #[test]
    fn an_outlier_below_the_average_quantizer_is_raised() {
        let optimized = optimize_scene(0, 20.0, 10_000.0, 36.0, 2_800.0, 4_000.0, 6_800.0);

        assert_eq!(optimized, Some(36.0));
    }

    /// A scene already at or above the average quantizer is left alone: raising
    /// it further would cost more bits, not fewer.
    #[test]
    fn a_scene_at_or_above_the_average_quantizer_is_untouched() {
        assert_eq!(
            optimize_scene(0, 36.0, 10_000.0, 36.0, 2_800.0, 4_000.0, 6_800.0),
            None
        );
        assert_eq!(
            optimize_scene(0, 40.0, 10_000.0, 36.0, 2_800.0, 4_000.0, 6_800.0),
            None
        );
    }

    /// A scene within the sigma threshold is not an outlier and is untouched.
    #[test]
    fn a_scene_within_the_sigma_threshold_is_untouched() {
        assert_eq!(
            optimize_scene(0, 20.0, 5_000.0, 36.0, 2_800.0, 4_000.0, 6_800.0),
            None
        );
    }

    /// An empty scene list is only worth reporting when the optimizer is
    /// configured; otherwise the sequence is inert and the warning is noise.
    #[test]
    fn an_empty_scene_list_warns_only_when_configured() {
        assert!(!configured_with_empty_scenes(None, true));
        assert!(!configured_with_empty_scenes(Some(1), false));
        assert!(configured_with_empty_scenes(Some(1), true));
    }
}
