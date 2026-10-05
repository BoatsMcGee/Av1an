//! Native CPU scoring for SSIMULACRA2, Butteraugli and CVVDP via fmetrics.
//!
//! fmetrics is the CPU implementation of the three metrics libvship runs on a
//! GPU, so it is the fallback for a machine with no usable device. It does not
//! agree with libvship to the last digit, so a score records which engine
//! produced it -- see [`super::Engine`].
//!
//! Decoding and frame lifetime belong to [`super::probe`]; this module supplies
//! the metric mapping, the plane adaptor, and the submit loop. Where this
//! engine sits relative to the plugin branches is decided once in
//! [`super::engine`].

use std::{sync::atomic::AtomicBool, time::Instant};

use anyhow::{Result, bail};
use av_metrics_fmetrics::{
    ColorInfo,
    FmetricsConfig,
    FmetricsCvvdpDisplayModel,
    FmetricsMetric,
    FmetricsScorer,
    PlaneSet as MetricPlaneSet,
    PlaneSource as MetricPlaneSource,
};

use crate::{
    metrics::probe::{self, OutputIndexing, PlaneSet},
    models::sequence::target_quality::types::QualityMetric,
    vapoursynth::plugins::vship::cvvdp::DisplayModel,
};

/// The fmetrics metric a configured [`QualityMetric`] maps to, or `None` when
/// the metric has no fmetrics equivalent.
///
/// VMAF and XPSNR have no fmetrics implementation.
#[inline]
#[must_use]
pub const fn fmetrics_metric_for(metric: &QualityMetric) -> Option<FmetricsMetric> {
    match metric {
        QualityMetric::SSIMULACRA2 {
            ..
        } => Some(FmetricsMetric::Ssimulacra2),
        QualityMetric::BUTTERAUGLI {
            ..
        } => Some(FmetricsMetric::Butteraugli),
        QualityMetric::CVVDP {
            ..
        } => Some(FmetricsMetric::Cvvdp),
        QualityMetric::VMAF {
            ..
        }
        | QualityMetric::XPSNR {
            ..
        } => None,
    }
}

/// Whether the fmetrics path should be attempted for `metric`.
///
/// An absent library leaves the caller's existing branch in place.
#[inline]
#[must_use]
pub fn native_supported(metric: &QualityMetric) -> bool {
    fmetrics_metric_for(metric).is_some() && av_metrics_fmetrics::is_available()
}

/// Build an [`FmetricsConfig`] from Av1an's metric configuration.
///
/// `threads` becomes the number of workspaces, so it bounds how many
/// submissions can be inside the library at once.
///
/// `frame_rate` supplies CVVDP's `fps`. Zero is not neutral to a temporal
/// filter -- it accumulates error silently -- so it is passed only when the
/// source reports one and the scorer refuses a config that ends up without it.
///
/// `resolution` is **not** applied: fmetrics scores at the source resolution
/// and cannot scale, so a configured target resolution is honoured here only by
/// coincidence.
///
/// # Errors
///
/// Returns an error for a metric fmetrics does not implement.
#[inline]
pub fn scorer_config(metric: &QualityMetric, frame_rate: f32) -> Result<FmetricsConfig> {
    match metric {
        QualityMetric::SSIMULACRA2 {
            threads, ..
        } => {
            Ok(FmetricsConfig::new(FmetricsMetric::Ssimulacra2)
                .with_threads(worker_threads(*threads)))
        },
        QualityMetric::BUTTERAUGLI {
            threads,
            intensity_multiplier,
            norm,
            ..
        } => {
            let mut config = FmetricsConfig::new(FmetricsMetric::Butteraugli)
                .with_threads(worker_threads(*threads));
            if let Some(norm) = *norm {
                config = config.with_pnorm(i32::from(norm));
            }
            if let Some(intensity_multiplier) = *intensity_multiplier {
                config = config.with_intensity_target(intensity_multiplier as f32);
            }
            Ok(config)
        },
        QualityMetric::CVVDP {
            display_model,
            disable_temporal,
            ..
        } => {
            let mut config = FmetricsConfig::new(FmetricsMetric::Cvvdp)
                .with_threads(av_metrics_fmetrics::DEFAULT_THREADS);
            if frame_rate > 0.0 {
                config = config.with_frame_rate(frame_rate);
            }
            if let Some(display_model) = display_model {
                config = config.with_display_model(display_model_for(*display_model));
            }
            // The C API has no `disableTemporal` argument, only a reset, so this
            // maps onto the inverse: disabling the temporal model keeps history.
            if let Some(disable_temporal) = *disable_temporal {
                config = config.with_disable_temporal(disable_temporal);
            }
            Ok(config)
        },
        QualityMetric::VMAF {
            ..
        }
        | QualityMetric::XPSNR {
            ..
        } => {
            bail!("{} has no fmetrics equivalent", metric.friendly_name());
        },
    }
}

/// fmetrics worker count for a configured `threads` value.
#[inline]
fn worker_threads(threads: Option<u8>) -> u32 {
    threads.map_or(av_metrics_fmetrics::DEFAULT_THREADS, u32::from)
}

/// Map Av1an's display model onto fmetrics'.
///
/// One-to-one: both enums carry the same five HDR variants.
#[inline]
#[must_use]
const fn display_model_for(model: DisplayModel) -> FmetricsCvvdpDisplayModel {
    match model {
        DisplayModel::Standard4K => FmetricsCvvdpDisplayModel::Standard4K,
        DisplayModel::StandardFHD => FmetricsCvvdpDisplayModel::StandardFhd,
        DisplayModel::StandardHDRPQ => FmetricsCvvdpDisplayModel::StandardHdrPq,
        DisplayModel::StandardHDRHLG => FmetricsCvvdpDisplayModel::StandardHdrHlg,
        DisplayModel::StandardHDRLinear => FmetricsCvvdpDisplayModel::StandardHdrLinear,
        DisplayModel::StandardHDRDark => FmetricsCvvdpDisplayModel::StandardHdrDark,
        DisplayModel::StandardHDRLinearZoom => FmetricsCvvdpDisplayModel::StandardHdrLinearZoom,
    }
}

/// Score only the frames named by `selected`, returning one score per
/// selection.
///
/// `indexing` states each side's index space; see `probe::output_index`.
///
/// A temporal metric's history is cleared at a discontinuity in `selected`, so
/// it never carries across frames it never saw. `on_score` is called per pair
/// as the pairs are visited.
///
/// # Errors
///
/// Returns an error when the clips disagree, when fmetrics is unavailable or
/// rejects the configuration, or when a frame cannot be decoded or scored.
#[inline]
pub fn score_probed_frames(
    reference: &mut av_decoders::Decoder,
    distorted: &mut av_decoders::Decoder,
    config: &QualityMetric,
    selected: &[usize],
    indexing: OutputIndexing,
    cancelled: Option<&AtomicBool>,
    mut on_score: impl FnMut(usize, f64),
) -> Result<Vec<f64>> {
    if selected.is_empty() {
        return Ok(Vec::new());
    }

    // The driver checks that both sides agree; only the reference's details are
    // read here.
    let reference_details = *reference.get_video_details();

    // Built before any frame is read, so a bad configuration costs nothing.
    let opened = Instant::now();
    let frame_rate = {
        let rate = reference_details.frame_rate;
        (*rate.numer() as f32) / (*rate.denom() as f32)
    };
    let fmetrics_config = scorer_config(config, frame_rate)?;
    let metric = fmetrics_config.metric();

    // Which Butteraugli norm is the headline depends on the configuration, not the
    // metric, and `target_range` is calibrated against the plugin's choice.
    let butteraugli_uses_q_norm = matches!(config, QualityMetric::BUTTERAUGLI {
        norm: Some(_),
        ..
    });

    let color = ColorInfo::from_details(&reference_details).map_err(describe_error)?;

    let reset_on_discontinuity = fmetrics_config.reset_on_discontinuity;

    let mut scorer = FmetricsScorer::new(fmetrics_config, color).map_err(describe_error)?;
    let setup = opened.elapsed();

    let total = selected.len();
    let mut scores = Vec::with_capacity(total);
    // The source index the temporal context last saw.
    let mut previous: Option<usize> = None;

    let pass = Instant::now();
    probe::probe_selected_frames::<(), _>(
        reference,
        distorted,
        selected,
        indexing,
        cancelled,
        probe::check_geometry,
        |position, reference_planes, distorted_planes| {
            let source_index = selected[position];

            // The reset is gated on the configured value: left ungated it would
            // override the very option meant to honour it.
            if reset_on_discontinuity
                && previous.is_some_and(|previous| previous.checked_add(1) != Some(source_index))
            {
                scorer.reset_temporal().map_err(describe_error)?;
            }

            let reference_planes = metric_planes(reference_planes);
            let distorted_planes = metric_planes(distorted_planes);

            // `submit_pair` refuses a context-backed scorer, so CVVDP must go
            // through the entry point that owns its accumulating context.
            let frame_score = if metric.is_temporal() {
                scorer.submit_pair_temporal(&reference_planes, &distorted_planes)
            } else {
                scorer.submit_pair(&reference_planes, &distorted_planes)
            }
            .map_err(describe_error)?;

            let value = frame_score.value(metric).ok_or_else(|| {
                anyhow::anyhow!("fmetrics returned no score for {}", metric.as_str())
            })?;

            previous = Some(source_index);
            on_score(position, value);
            scores.push(value);
            Ok(())
        },
    )?;
    let exchanged = pass.elapsed();

    tracing::debug!(
        frames = selected.len(),
        ?setup,
        pass = ?exchanged,
        ?butteraugli_uses_q_norm,
        "fmetrics pass"
    );

    if scores.len() != selected.len() {
        bail!(
            "fmetrics scored {} frames but {} were selected",
            scores.len(),
            selected.len()
        );
    }

    Ok(scores)
}

/// Adapt the driver's plane set to the one `av-metrics-fmetrics` submits.
///
/// A field-by-field copy of the same descriptor. The pointer still aliases the
/// frame the driver holds, and the conversion reads it before the frame drops.
///
/// # Safety
///
/// Each field comes from a `PlaneSource` describing a live frame, which the
/// driver consumes inside the `submit` call receiving this.
#[inline]
fn metric_planes(planes: PlaneSet) -> MetricPlaneSet {
    MetricPlaneSet {
        planes: planes.map(|plane| {
            // SAFETY: as documented above.
            unsafe {
                MetricPlaneSource::new(
                    plane.width,
                    plane.height,
                    plane.stride,
                    plane.data_origin,
                    plane.bytes_per_sample,
                    plane.data,
                )
            }
        }),
    }
}

/// Turn a scorer error into a message that says what to do about it.
#[inline]
fn describe_error(error: av_metrics_fmetrics::FmetricsError) -> anyhow::Error {
    anyhow::anyhow!(error)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "a failing assertion should panic loudly, which is what a unit test wants"
)]
mod tests {
    use super::*;

    /// A SSIMULACRA2 metric configuration.
    fn ssimulacra2(threads: Option<u8>) -> QualityMetric {
        QualityMetric::SSIMULACRA2 {
            target_range: (74.0, 76.0),
            resolution: None,
            threads,
            gpu_id: None,
        }
    }

    /// Fixture width and height.
    ///
    /// Below ~32 pixels SSIMULACRA2's scale pyramid has nothing to compare and
    /// returns 100.0 for any input. Measured on a 30-level shift: 8x8 gives
    /// 89.2, 16x16 gives 86.0, 32x32 gives 57.1.
    const FIXTURE_SIZE: usize = 32;

    /// A y4m with a diagonal luma ramp, optionally shifted.
    ///
    /// Not flat: SSIMULACRA2 is scale invariant, so a uniform offset on a
    /// constant picture is legitimately scored as no distortion.
    fn y4m_gradient(shift: u8) -> Vec<u8> {
        let size = FIXTURE_SIZE;
        let mut bytes = format!("YUV4MPEG2 W{size} H{size} F24:1 Ip A1:1 C420mpeg2\n").into_bytes();

        for frame in 0..4usize {
            bytes.extend_from_slice(b"FRAME\n");
            for row in 0..size {
                let ramp: Vec<u8> = (0..size)
                    .map(|column| {
                        let base = (row * 4 + column * 3 + frame * 5) as u16;
                        (base + u16::from(shift)).clamp(16, 235) as u8
                    })
                    .collect();
                bytes.extend_from_slice(&ramp);
            }
            // Neutral chroma, one plane per 2x2 cell.
            let chroma_len = (size / 2) * (size / 2);
            bytes.extend(std::iter::repeat_n(128u8, chroma_len));
            bytes.extend(std::iter::repeat_n(128u8, chroma_len));
        }
        bytes
    }

    /// Write `bytes` to a y4m fixture that is removed when it goes out of
    /// scope.
    fn y4m_fixture(bytes: &[u8]) -> tempfile::TempPath {
        let file = tempfile::Builder::new()
            .suffix(".y4m")
            .tempfile()
            .expect("create a y4m fixture");
        let path = file.into_temp_path();
        std::fs::write(&path, bytes).expect("write the y4m fixture");
        path
    }

    /// Score a gradient clip pair differing by `shift` levels of luma.
    ///
    /// Drives the real submit loop, so a metric routed to the wrong entry point
    /// fails here rather than passing every mapping test.
    fn score_pair(metric: &QualityMetric, shift: u8) -> Result<Vec<f64>> {
        let reference = y4m_fixture(&y4m_gradient(0));
        let distorted = y4m_fixture(&y4m_gradient(shift));
        let selected: Vec<usize> = (0..4).collect();

        let mut reference =
            av_decoders::Decoder::from_file(&reference).expect("the reference fixture decodes");
        let mut distorted =
            av_decoders::Decoder::from_file(&distorted).expect("the distorted fixture decodes");

        score_probed_frames(
            &mut reference,
            &mut distorted,
            metric,
            &selected,
            OutputIndexing::Aligned,
            None,
            |_, _| {},
        )
    }

    #[test]
    fn every_supported_metric_scores_a_pair_through_the_driver() {
        if !av_metrics_fmetrics::is_available() {
            return;
        }

        let butteraugli = QualityMetric::BUTTERAUGLI {
            target_range:         (1.0, 3.0),
            resolution:           None,
            threads:              None,
            intensity_multiplier: None,
            norm:                 None,
            gpu_id:               None,
        };

        // Each metric must produce one score per selected frame. CVVDP is the case
        // that distinguishes the two submit entry points.
        for metric in [ssimulacra2(None), butteraugli, cvvdp()] {
            let scores = score_pair(&metric, 30).unwrap_or_else(|error| {
                panic!("{} failed to score: {error:#}", metric.friendly_name())
            });

            assert_eq!(
                scores.len(),
                4,
                "{} should score every selected frame",
                metric.friendly_name()
            );
            assert!(
                scores.iter().all(|score| score.is_finite()),
                "{} produced a non-finite score",
                metric.friendly_name()
            );
        }
    }

    /// A temporal filter accumulates, so its per-frame scores cannot all be
    /// equal on content that differs frame to frame. Equal values would mean
    /// each frame was scored independently, which is what the per-pair path
    /// does.
    #[test]
    fn cvvdp_accumulates_rather_than_scoring_each_frame_alone() {
        if !av_metrics_fmetrics::is_available() {
            return;
        }

        let scores = score_pair(&cvvdp(), 30).expect("CVVDP scores");

        assert!(
            scores.windows(2).any(|pair| pair[0] != pair[1]),
            "CVVDP scores should change as the temporal filter accumulates, got {scores:?}"
        );
    }

    /// Confirms the fixture pair really differs, in both metric conventions:
    /// SSIMULACRA2 is a score below its ceiling, Butteraugli a distance above
    /// zero. Without this every other assertion here also holds for an
    /// identical pair.
    #[test]
    fn a_distorted_gradient_scores_below_the_ceiling() {
        if !av_metrics_fmetrics::is_available() {
            return;
        }

        let scores = score_pair(&ssimulacra2(None), 30).expect("SSIMULACRA2 scores");
        assert!(
            scores.iter().all(|score| *score < 100.0),
            "a shifted gradient should not score at the ceiling, got {scores:?}"
        );

        let butteraugli = QualityMetric::BUTTERAUGLI {
            target_range:         (1.0, 3.0),
            resolution:           None,
            threads:              None,
            intensity_multiplier: None,
            norm:                 None,
            gpu_id:               None,
        };
        let distances = score_pair(&butteraugli, 30).expect("Butteraugli scores");
        assert!(
            distances.iter().all(|distance| *distance > 0.0),
            "a shifted gradient should register a non-zero distance, got {distances:?}"
        );
    }

    /// A CVVDP metric configuration with no display model.
    fn cvvdp() -> QualityMetric {
        cvvdp_with(None)
    }

    /// A CVVDP configuration with the given `disable_temporal`.
    fn cvvdp_with(disable_temporal: Option<bool>) -> QualityMetric {
        QualityMetric::CVVDP {
            target_range: (40.0, 50.0),
            resolution: None,
            display_model: None,
            resize_to_display: None,
            disable_temporal,
            gpu_id: None,
        }
    }

    #[test]
    fn the_three_shared_metrics_map_and_the_others_do_not() {
        assert_eq!(
            fmetrics_metric_for(&ssimulacra2(None)),
            Some(FmetricsMetric::Ssimulacra2)
        );
        assert_eq!(
            fmetrics_metric_for(&QualityMetric::BUTTERAUGLI {
                target_range:         (1.0, 3.0),
                resolution:           None,
                threads:              None,
                intensity_multiplier: None,
                norm:                 None,
                gpu_id:               None,
            }),
            Some(FmetricsMetric::Butteraugli)
        );
        assert_eq!(fmetrics_metric_for(&cvvdp()), Some(FmetricsMetric::Cvvdp));

        // Neither VMAF nor XPSNR has an fmetrics implementation, so neither may
        // claim one.
        assert!(
            fmetrics_metric_for(&QualityMetric::XPSNR {
                target_range: (45.0, 55.0),
                resolution:   None,
            })
            .is_none()
        );
        assert!(
            fmetrics_metric_for(&QualityMetric::VMAF {
                target_range: (85.0, 95.0),
                resolution:   None,
                scaler:       "area".to_owned(),
                threads:      4,
                model:        None,
                features:     Vec::new(),
            })
            .is_none()
        );
    }

    #[test]
    fn availability_follows_the_metric_and_the_library() {
        // Availability follows the library, so the assertion mirrors it rather than
        // assuming either answer.
        assert_eq!(
            native_supported(&ssimulacra2(None)),
            av_metrics_fmetrics::is_available(),
            "SSIMULACRA2 is supported exactly when the library loaded"
        );
        assert!(
            !native_supported(&QualityMetric::XPSNR {
                target_range: (45.0, 55.0),
                resolution:   None,
            }),
            "a metric fmetrics cannot score is never supported, whatever the library state"
        );
    }

    #[test]
    fn the_thread_count_reaches_the_config() {
        assert_eq!(
            scorer_config(&ssimulacra2(Some(3)), 0.0).unwrap().threads(),
            3
        );
        // An absent value takes the crate default, not zero.
        assert_eq!(
            scorer_config(&ssimulacra2(None), 0.0).unwrap().threads(),
            av_metrics_fmetrics::DEFAULT_THREADS
        );
    }

    #[test]
    fn a_frame_rate_reaches_the_config_and_zero_is_not_invented() {
        let config = scorer_config(&cvvdp(), 23.976).unwrap();
        assert!((config.frame_rate() - 23.976).abs() < 1e-3);

        // A source reporting no rate leaves zero, which the scorer refuses rather than
        // scoring with it.
        assert_eq!(scorer_config(&cvvdp(), 0.0).unwrap().frame_rate(), 0.0);
    }

    #[test]
    fn every_display_model_maps_to_a_distinct_fmetrics_model() {
        // A silent fall-through would score HDR content against an SDR model, so the
        // mapping must be total and injective.
        let models = [
            DisplayModel::Standard4K,
            DisplayModel::StandardFHD,
            DisplayModel::StandardHDRPQ,
            DisplayModel::StandardHDRHLG,
            DisplayModel::StandardHDRLinear,
            DisplayModel::StandardHDRDark,
            DisplayModel::StandardHDRLinearZoom,
        ];
        let mapped: std::collections::HashSet<_> =
            models.iter().copied().map(display_model_for).collect();
        assert_eq!(mapped.len(), models.len(), "two models collided");

        // Spot-check one HDR model, which has no default.
        assert_eq!(
            display_model_for(DisplayModel::StandardHDRPQ),
            FmetricsCvvdpDisplayModel::StandardHdrPq
        );
    }

    #[test]
    fn a_display_model_reaches_the_config() {
        let config = scorer_config(
            &QualityMetric::CVVDP {
                target_range:      (40.0, 50.0),
                resolution:        None,
                display_model:     Some(DisplayModel::StandardHDRPQ),
                resize_to_display: None,
                disable_temporal:  None,
                gpu_id:            None,
            },
            24.0,
        )
        .unwrap();
        assert_eq!(
            config.display_model(),
            FmetricsCvvdpDisplayModel::StandardHdrPq
        );
    }

    #[test]
    fn disable_temporal_inverts_the_discontinuity_reset() {
        // Upstream `disableTemporal` clears the context, so the two are opposites.
        // Reading it the wrong way round would silently give the opposite
        // behaviour.
        let disabled = cvvdp_with(Some(true));
        assert!(
            !scorer_config(&disabled, 24.0).unwrap().reset_on_discontinuity,
            "disabling the temporal model must not be read as keeping history"
        );

        let kept = cvvdp_with(Some(false));
        assert!(
            scorer_config(&kept, 24.0).unwrap().reset_on_discontinuity,
            "leaving the temporal model enabled keeps the reset"
        );

        // Unset takes the crate default, not `false`.
        assert!(
            scorer_config(&cvvdp_with(None), 24.0).unwrap().reset_on_discontinuity,
            "an unset flag must not silently disable the reset"
        );
    }

    #[test]
    fn cvvdp_is_temporal_so_the_submit_loop_uses_the_context_entry_point() {
        // The submit loop branches on this, and the branch is otherwise
        // unreachable from a unit test.
        assert!(FmetricsMetric::Cvvdp.is_temporal());
        for metric in [
            FmetricsMetric::Ssimulacra2,
            FmetricsMetric::Butteraugli,
            FmetricsMetric::Iwssim,
            FmetricsMetric::Msssim,
        ] {
            assert!(!metric.is_temporal(), "{metric:?} is per-pair");
        }

        let config = scorer_config(&cvvdp(), 24.0).unwrap();
        assert!(config.is_temporal(), "CVVDP must build a temporal config");
    }

    #[test]
    fn unsupported_metrics_are_rejected_rather_than_silently_substituted() {
        // Scoring XPSNR here would produce a number of the wrong kind.
        let error = scorer_config(
            &QualityMetric::XPSNR {
                target_range: (45.0, 55.0),
                resolution:   None,
            },
            24.0,
        )
        .unwrap_err();
        assert!(error.to_string().contains("XPSNR"), "{error}");
    }
}
