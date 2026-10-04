//! Native libvship scoring for SSIMULACRA2, Butteraugli and CVVDP.
//!
//! libvship implements the same three perceptual metrics Av1an exposes as
//! VapourSynth plugins, but behind a C API rather than a filter graph. Binding
//! it directly lets the pass decode only the frames it selected and hand each
//! pair straight to the device, with no temporary files and no distortion maps.
//!
//! Everything that is not metric-specific lives in [`super::probe`]: opening
//! the two clips, walking the selection, and keeping each frame's planes alive
//! across the submission. This module supplies only the mapping from Av1an's
//! [`QualityMetric`] to a [`VshipConfig`], the plane adaptor, and the driver
//! loop that submits each pair.
//!
//! # Availability and fallback
//!
//! libvship ships as a VapourSynth plugin and is opened with `dlopen` at
//! runtime, so it may be absent. [`is_available`] reports whether the library
//! loaded and a device passed libvship's full check. When it has, this path is
//! preferred over the plugin branches; when it has not, or if scoring fails at
//! any point, the caller falls back to those branches, whose behaviour is
//! unchanged.
//!
//! # Temporal metrics
//!
//! CVVDP accumulates across the frames a handler has seen, so
//! [`VshipScorer`] builds it exactly one handler and sees every frame in order.
//! The scorer resets that history on an index discontinuity -- which is what a
//! scene break is, from the temporal filter's point of view -- which this
//! module drives through the probe driver's `reset_temporal`.

use std::{sync::atomic::AtomicBool, time::Instant};

use anyhow::{Result, bail};
use av_metrics_vship::{
    PlaneSet as MetricPlaneSet,
    PlaneSource as MetricPlaneSource,
    VideoFormat,
    VshipConfig,
    VshipMetric,
    VshipScorer,
};

use crate::{
    metrics::probe::{self, OutputIndexing, PlaneSet},
    models::sequence::target_quality::types::QualityMetric,
};

/// The libvship metric a configured [`QualityMetric`] maps to, or `None` when
/// the metric has no libvship equivalent.
///
/// VMAF and XPSNR are absent: VMAF is served by [`super::vmaf`]'s libvmaf
/// binding and XPSNR by the `vszip` plugin, neither of which libvship provides.
#[inline]
#[must_use]
pub const fn vship_metric_for(metric: &QualityMetric) -> Option<VshipMetric> {
    match metric {
        QualityMetric::SSIMULACRA2 {
            ..
        } => Some(VshipMetric::Ssimulacra2),
        QualityMetric::BUTTERAUGLI {
            ..
        } => Some(VshipMetric::Butteraugli),
        QualityMetric::CVVDP {
            ..
        } => Some(VshipMetric::Cvvdp),
        QualityMetric::VMAF {
            ..
        }
        | QualityMetric::XPSNR {
            ..
        } => None,
    }
}

/// Whether the native path should be attempted for `metric`.
///
/// True only for a metric libvship implements *and* when the library and a
/// device are usable. When this is false the caller keeps its VapourSynth
/// branch, so behaviour on a machine without libvship is exactly what it was
/// before this module existed.
#[inline]
#[must_use]
pub fn native_supported(metric: &QualityMetric) -> bool {
    vship_metric_for(metric).is_some() && av_metrics_vship::is_available()
}

/// Build a `VshipConfig` from Av1an's metric configuration.
///
/// `threads` becomes libvship's handler count. The VapourSynth plugin path
/// treats an absent `threads` as 4 streams, so an absent value here likewise
/// becomes [`av_metrics_vship::DEFAULT_HANDLER_THREADS`] rather than the
/// library's own thread handling; a present value is honoured verbatim so a
/// capped pool stays capped.
///
/// `resolution` is not applied here. It becomes libvship's `target_width` and
/// `target_height`, and libvship scales each input internally, so no plane is
/// resized before submission.
///
/// CVVDP's display model, resize-to-display and temporal options pass through
/// unchanged, as do Butteraugli's `norm` and `intensity_multiplier`.
///
/// `frame_rate` supplies CVVDP's `fps`. A rate of zero is not neutral to that
/// metric — its temporal filter is parameterised by the rate — so it is
/// applied whenever the source reports one.
///
/// # Errors
///
/// Returns an error for a metric libvship does not implement, or when the
/// display model carries an interior NUL, which cannot reach the C API.
/// Validating early keeps the failure beside the configuration that caused it.
#[inline]
pub fn scorer_config(metric: &QualityMetric, frame_rate: f32) -> Result<VshipConfig> {
    match metric {
        QualityMetric::SSIMULACRA2 {
            threads, ..
        } => Ok(VshipConfig::new()
            .with_metric(VshipMetric::Ssimulacra2)
            .with_handler_threads(handler_threads(*threads))),
        QualityMetric::BUTTERAUGLI {
            threads,
            intensity_multiplier,
            norm,
            ..
        } => {
            let mut config = VshipConfig::new()
                .with_metric(VshipMetric::Butteraugli)
                .with_handler_threads(handler_threads(*threads));
            if let Some(norm) = *norm {
                config = config.with_q_norm(i32::from(norm));
            }
            if let Some(intensity_multiplier) = *intensity_multiplier {
                // `intensity_multiplier` is an `f64` in Av1an's config and an
                // `f32` here; a value too large for `f32` is not a real
                // brightness target, and the library rejects it.
                config = config.with_intensity_multiplier(intensity_multiplier as f32);
            }
            Ok(config)
        },
        QualityMetric::CVVDP {
            display_model,
            resize_to_display,
            disable_temporal,
            ..
        } => {
            // CVVDP is temporal, so its score cannot be split across handlers;
            // `VshipConfig::handler_count` pins it to one regardless.
            let mut config = VshipConfig::new()
                .with_metric(VshipMetric::Cvvdp)
                .with_handler_threads(av_metrics_vship::DEFAULT_HANDLER_THREADS);
            if frame_rate > 0.0 {
                config = config.with_fps(frame_rate);
            }
            if let Some(display_model) = display_model {
                config = config
                    .with_display_model(display_model.to_string())
                    .map_err(|error| anyhow::anyhow!(error))?;
            }
            if let Some(resize_to_display) = *resize_to_display {
                config = config.with_resize_to_display(resize_to_display);
            }
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
            bail!("{} has no libvship equivalent", metric.friendly_name());
        },
    }
}

/// The resolution every variant of [`QualityMetric`] scores at.
///
/// It is an `Option` on all five variants alike, so it can be read without
/// matching, and it reaches libvship as the colorspace's target size rather
/// than as a resize of the decoded planes.
#[inline]
#[must_use]
const fn resolution(metric: &QualityMetric) -> Option<(u32, u32)> {
    match metric {
        QualityMetric::VMAF {
            resolution, ..
        }
        | QualityMetric::SSIMULACRA2 {
            resolution, ..
        }
        | QualityMetric::BUTTERAUGLI {
            resolution, ..
        }
        | QualityMetric::XPSNR {
            resolution, ..
        }
        | QualityMetric::CVVDP {
            resolution, ..
        } => *resolution,
    }
}

/// libvship handler count for a configured `threads` value.
///
/// `None` takes the plugin path's default of four handlers, so the two paths
/// agree when `threads` is unset. `Some(0)` would ask for a pool with no
/// handlers, which the crate clamps to one.
#[inline]
fn handler_threads(threads: Option<u8>) -> u32 {
    threads.map_or(av_metrics_vship::DEFAULT_HANDLER_THREADS, u32::from)
}

/// Score only the frames named by `selected`, returning one score per
/// selection.
///
/// The decode loop belongs to [`probe::probe_selected_frames`]; this wraps it
/// with the libvship scorer and the per-pair submission the library needs.
///
/// # Index spaces
///
/// The two decoders may use different index spaces, which is why `indexing` is
/// passed explicitly rather than inferred. See `probe::output_index`.
///
/// # Temporal handling
///
/// For CVVDP the scorer keeps a single handler and sees frames in submission
/// order. The probe driver visits `selected` in the order given, so a temporal
/// metric's history is reset whenever the driver signals a discontinuity -- a
/// scene break, or the start of a non-contiguous selection -- rather than
/// carrying history across frames it never saw.
///
/// # Errors
///
/// Returns an error when the two clips disagree on resolution or bit depth,
/// when libvship is unavailable or rejects the configuration, when seeking or
/// decoding fails, or when a selected frame cannot be scored.
#[inline]
pub fn score_probed_frames(
    reference: &mut av_decoders::Decoder,
    distorted: &mut av_decoders::Decoder,
    config: &QualityMetric,
    selected: &[usize],
    indexing: OutputIndexing,
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<f64>> {
    if selected.is_empty() {
        return Ok(Vec::new());
    }

    let reference_details = *reference.get_video_details();
    let distorted_details = *distorted.get_video_details();

    // The scorer is created before any frame is read, so a bad configuration or
    // an unusable device is reported before spending time decoding.
    let opened = Instant::now();
    let frame_rate = {
        let rate = reference_details.frame_rate;
        (*rate.numer() as f32) / (*rate.denom() as f32)
    };
    let vship_config = scorer_config(config, frame_rate)?;
    let metric = vship_config.metric();

    // Which Butteraugli norm is the headline depends on the configuration, not on
    // the metric. The VapourSynth plugin reads `BUTTERAUGLI_INFNorm` unless a
    // norm is requested and `BUTTERAUGLI_QNorm` when one is, so the native path
    // has to follow the same rule or the two report different quantities for the
    // same frames -- and `target_range` is calibrated against whichever the plugin
    // reported.
    let butteraugli_uses_q_norm = !matches!(config, QualityMetric::BUTTERAUGLI {
        norm: Some(_),
        ..
    });

    let reference_format = VideoFormat::from_details(&reference_details);
    let distorted_format = VideoFormat::from_details(&distorted_details);

    // `resolution` becomes libvship's `target_width`/`target_height` rather than
    // a rescale: libvship scales each input to it internally.
    let target_resolution = resolution(config);

    let mut scorer = VshipScorer::new(
        vship_config,
        reference_format,
        distorted_format,
        target_resolution,
    )
    .map_err(describe_error)?;
    let setup = opened.elapsed();

    let mut scores = Vec::with_capacity(selected.len());
    // `previous` tracks the source index the temporal handler last saw, so the
    // next submission can be classified as contiguous or a discontinuity.
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

            // A temporal handler must see every frame in order. When the
            // selection skips frames -- `ProbeStrategy::Skip` or `Subset`, or a
            // scene boundary -- the handler would otherwise fold a discontinuity
            // into its motion history, so its history is cleared first.
            if metric.is_temporal()
                && previous.is_some_and(|previous| previous.checked_add(1) != Some(source_index))
            {
                scorer.reset_temporal().map_err(describe_error)?;
            }

            let frame_score = scorer
                .submit_pair_raw(
                    metric_planes(reference_planes),
                    metric_planes(distorted_planes),
                )
                .map_err(describe_error)?;

            let value = match metric {
                VshipMetric::Butteraugli => {
                    frame_score.butteraugli_value(butteraugli_uses_q_norm).ok_or_else(|| {
                        anyhow::anyhow!("libvship returned no Butteraugli norm for this pair")
                    })?
                },
                _ => frame_score.value(metric).ok_or_else(|| {
                    anyhow::anyhow!("libvship returned no score for {}", metric.as_str())
                })?,
            };

            previous = Some(source_index);
            scores.push(value);
            Ok(())
        },
    )?;
    let exchanged = pass.elapsed();

    tracing::debug!(
        frames = selected.len(),
        ?setup,
        pass = ?exchanged,
        "vship pass"
    );

    if scores.len() != selected.len() {
        bail!(
            "libvship scored {} frames but {} were selected",
            scores.len(),
            selected.len()
        );
    }

    Ok(scores)
}

/// Adapt the driver's plane set to the one `av-metrics-vship` submits.
///
/// The two descriptors carry the same fields, so this is a field-by-field copy
/// rather than a reinterpretation: the pointer still aliases the frame the
/// driver is holding, and libvship reads the planes before the frame is
/// dropped.
#[inline]
fn metric_planes(planes: PlaneSet) -> MetricPlaneSet {
    planes.map(|plane| {
        // SAFETY: every field is copied unchanged from a `PlaneSource` the
        // driver produced from a live frame. The driver consumes the set inside
        // the `submit` call that receives it, so the frame outlives this
        // reference, and libvship only reads the planes before it is dropped.
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
    })
}

/// Turn a scorer error into a message that says what to do about it.
#[inline]
fn describe_error(error: av_metrics_vship::VshipError) -> anyhow::Error {
    anyhow::anyhow!(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vapoursynth::plugins::vship::cvvdp::DisplayModel;

    /// A SSIMULACRA2 metric configuration.
    fn ssimulacra2(threads: Option<u8>) -> QualityMetric {
        QualityMetric::SSIMULACRA2 {
            target_range: (74.0, 76.0),
            resolution: None,
            threads,
        }
    }

    /// A minimal 4x4 4:2:0 y4m holding `frames` flat-luma frames.
    ///
    /// y4m is a text header followed by `FRAME` and three planar planes per
    /// frame, so a flat picture needs no real content. Chroma is neutral, which
    /// leaves luma as the only thing a metric can see a difference in.
    fn y4m(frames: &[u8]) -> Vec<u8> {
        let mut bytes = b"YUV4MPEG2 W4 H4 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for &luma in frames {
            bytes.extend_from_slice(b"FRAME\n");
            bytes.extend_from_slice(&[luma; 16]);
            bytes.extend_from_slice(&[128; 4]);
            bytes.extend_from_slice(&[128; 4]);
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

    #[test]
    fn the_supported_metrics_map_to_their_libvship_counterparts() {
        let butteraugli = QualityMetric::BUTTERAUGLI {
            target_range:         (1.0, 3.0),
            resolution:           None,
            threads:              None,
            intensity_multiplier: None,
            norm:                 None,
        };
        let cvvdp = QualityMetric::CVVDP {
            target_range:      (0.0, 1.0),
            resolution:        None,
            display_model:     None,
            resize_to_display: None,
            disable_temporal:  None,
        };

        assert_eq!(
            vship_metric_for(&ssimulacra2(None)),
            Some(VshipMetric::Ssimulacra2)
        );
        assert_eq!(
            vship_metric_for(&butteraugli),
            Some(VshipMetric::Butteraugli)
        );
        assert_eq!(vship_metric_for(&cvvdp), Some(VshipMetric::Cvvdp));
    }

    #[test]
    fn metrics_libvship_does_not_implement_are_rejected() {
        let vmaf = QualityMetric::VMAF {
            target_range: (94.0, 96.0),
            resolution:   None,
            scaler:       String::new(),
            threads:      1,
            model:        None,
            features:     vec![],
        };
        let xpsnr = QualityMetric::XPSNR {
            target_range: (40.0, 45.0),
            resolution:   None,
        };

        assert_eq!(vship_metric_for(&vmaf), None);
        assert_eq!(vship_metric_for(&xpsnr), None);
        assert!(scorer_config(&vmaf, 24.0).is_err());
        assert!(scorer_config(&xpsnr, 24.0).is_err());
    }

    #[test]
    fn an_absent_thread_count_takes_the_plugin_default_of_four() {
        let config = scorer_config(&ssimulacra2(None), 24.0).expect("config should build");
        assert_eq!(
            config.handler_threads(),
            av_metrics_vship::DEFAULT_HANDLER_THREADS
        );
        assert_eq!(config.handler_threads(), 4);
    }

    #[test]
    fn an_explicit_thread_count_is_passed_through() {
        let config = scorer_config(&ssimulacra2(Some(2)), 24.0).expect("config should build");
        assert_eq!(config.handler_threads(), 2);
    }

    #[test]
    fn butteraugli_norm_and_intensity_pass_through() {
        let metric = QualityMetric::BUTTERAUGLI {
            target_range:         (1.0, 3.0),
            resolution:           None,
            threads:              Some(3),
            intensity_multiplier: Some(250.0),
            norm:                 Some(3),
        };
        let config = scorer_config(&metric, 24.0).expect("config should build");

        assert_eq!(config.metric(), VshipMetric::Butteraugli);
        assert_eq!(config.handler_threads(), 3);
        assert_eq!(config.butteraugli().q_norm, 3);
        assert!((config.butteraugli().intensity_multiplier - 250.0).abs() < f32::EPSILON);
    }

    #[test]
    fn cvvdp_display_model_and_temporal_options_pass_through() {
        let metric = QualityMetric::CVVDP {
            target_range:      (0.0, 1.0),
            resolution:        None,
            display_model:     Some(DisplayModel::StandardFHD),
            resize_to_display: Some(true),
            disable_temporal:  Some(true),
        };
        let config = scorer_config(&metric, 23.976).expect("config should build");

        assert_eq!(config.metric(), VshipMetric::Cvvdp);
        assert_eq!(config.cvvdp().display_model, "standard_fhd");
        assert!(config.cvvdp().resize_to_display);

        // The plugin's `disableTemporal` is the inverse of clearing history at
        // a cut, so `true` there must leave the reset off here.
        assert!(
            !config.cvvdp().reset_on_discontinuity,
            "disableTemporal = true must not be read as keeping history"
        );
    }

    /// CVVDP's temporal filter is parameterised by the frame rate, so an unset
    /// rate is not neutral — the score drifts further from the correct value
    /// the longer the clip runs.
    #[test]
    fn cvvdp_receives_the_source_frame_rate() {
        let metric = QualityMetric::CVVDP {
            target_range:      (0.0, 1.0),
            resolution:        None,
            display_model:     None,
            resize_to_display: None,
            disable_temporal:  None,
        };

        let config = scorer_config(&metric, 23.976).expect("config should build");
        assert!(
            (config.cvvdp().fps - 23.976).abs() < f32::EPSILON,
            "the source rate must reach the temporal model"
        );

        // A source that reports no rate leaves the default rather than a
        // negative or infinite value.
        let unknown = scorer_config(&metric, 0.0).expect("config should build");
        assert_eq!(unknown.cvvdp().fps, 0.0);
    }

    /// The native path is only chosen for a libvship metric, and even then only
    /// when the library is actually usable. A machine without it keeps the
    /// plugin branches, which is what makes the no-vship behaviour unchanged.
    #[test]
    fn native_support_requires_both_a_supported_metric_and_the_library() {
        let vmaf = QualityMetric::VMAF {
            target_range: (94.0, 96.0),
            resolution:   None,
            scaler:       String::new(),
            threads:      1,
            model:        None,
            features:     vec![],
        };

        // A metric libvship cannot compute is never routed natively, regardless
        // of whether the library happens to be present.
        assert!(!native_supported(&vmaf));
        // For a supported metric, the decision tracks the library exactly.
        assert_eq!(
            native_supported(&ssimulacra2(None)),
            av_metrics_vship::is_available(),
            "a supported metric is native exactly when libvship is available"
        );
    }

    /// A metric libvship can compute scores identically to itself, so the
    /// distortion must not change the result.
    ///
    /// This drives the whole native path -- the config mapping, the probe
    /// driver, the plane adaptor and the submission -- against real y4m clips,
    /// so a wiring mistake anywhere in it would show up as a failure or a
    /// missing score rather than passing silently.
    ///
    /// It is skipped when libvship is absent, because the native path is not
    /// supposed to run then; `native_support_requires_both_a_supported_metric_
    /// and_the_library` covers that side of the decision.
    #[test]
    fn the_native_path_scores_a_real_frame_pair() {
        if !av_metrics_vship::is_available() {
            return;
        }

        let reference = y4m_fixture(&y4m(&[10, 20, 30]));
        let distorted = y4m_fixture(&y4m(&[10, 20, 30]));

        let mut reference = av_decoders::Decoder::from_file(&reference).expect("open reference");
        let mut distorted = av_decoders::Decoder::from_file(&distorted).expect("open distorted");

        let selected = [0usize, 1, 2];
        let scores = score_probed_frames(
            &mut reference,
            &mut distorted,
            &ssimulacra2(None),
            &selected,
            OutputIndexing::Aligned,
            None,
        )
        .expect("libvship should score the pair");

        assert_eq!(scores.len(), selected.len(), "one score per selected frame");
        for score in &scores {
            assert!(score.is_finite(), "a score must be finite, got {score}");
        }
    }

    /// Butteraugli is minimised, so a larger luma difference must score lower
    /// than a smaller one. CVVDP is pooled over time, so a discontinuity in the
    /// selection must be handled by `reset_temporal` rather than carried over.
    ///
    /// Both properties are only observable with a working library, so the test
    /// returns early when it is absent rather than failing on a machine that
    /// has no GPU backend for libvship.
    #[test]
    fn distortion_lowers_a_per_pair_metric_and_cvvdp_survives_a_discontinuity() {
        if !av_metrics_vship::is_available() {
            return;
        }

        let reference = y4m_fixture(&y4m(&[10, 10, 10, 10]));

        let butteraugli = QualityMetric::BUTTERAUGLI {
            target_range:         (1.0, 3.0),
            resolution:           None,
            threads:              None,
            intensity_multiplier: None,
            norm:                 None,
        };
        let cvvdp = QualityMetric::CVVDP {
            target_range:      (0.0, 1.0),
            resolution:        None,
            display_model:     None,
            resize_to_display: None,
            disable_temporal:  None,
        };

        // A near-identical encode and a badly wrong one, both against the same
        // reference.
        let mut cases = Vec::new();
        for luma in [11u8, 128] {
            let distorted = y4m_fixture(&y4m(&[luma; 4]));

            let mut reference_decoder =
                av_decoders::Decoder::from_file(&reference).expect("open reference");
            let mut distorted_decoder =
                av_decoders::Decoder::from_file(&distorted).expect("open distorted");

            let selected = [0usize, 1, 2, 3];
            cases.push(
                score_probed_frames(
                    &mut reference_decoder,
                    &mut distorted_decoder,
                    &butteraugli,
                    &selected,
                    OutputIndexing::Aligned,
                    None,
                )
                .expect("libvship should score butteraugli"),
            );
        }

        let near = cases[0].iter().sum::<f64>() / f64::from(cases[0].len() as u32);
        let far = cases[1].iter().sum::<f64>() / f64::from(cases[1].len() as u32);
        assert!(
            far > near,
            "a larger distortion must score worse: near={near} far={far}"
        );

        // A selection that skips a frame must still score: the temporal handler
        // is reset at the discontinuity rather than fed a gap.
        let distorted = y4m_fixture(&y4m(&[10, 10, 10, 10]));
        let mut reference_decoder =
            av_decoders::Decoder::from_file(&reference).expect("open reference");
        let mut distorted_decoder =
            av_decoders::Decoder::from_file(&distorted).expect("open distorted");
        let scores = score_probed_frames(
            &mut reference_decoder,
            &mut distorted_decoder,
            &cvvdp,
            &[0usize, 1, 3],
            OutputIndexing::Aligned,
            None,
        )
        .expect("libvship should score cvvdp across a discontinuity");
        assert_eq!(scores.len(), 3, "one score per selected frame");
    }
}
