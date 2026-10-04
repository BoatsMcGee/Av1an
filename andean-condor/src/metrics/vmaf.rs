//! VMAF scoring on top of the shared decode/probe driver.
//!
//! Av1an's `QualityMetric::VMAF` predates the `av-metrics-vmaf` crate and
//! describes the metric in terms of the old VapourSynth plugin, where
//! `features` doubled as a model selector (`neg`, `uhd`, `weighted`). libvmaf
//! instead selects the model directly and registers extra feature extractors by
//! name, so the two vocabularies are reconciled here rather than in the metric
//! crate.
//!
//! Everything that is not specific to VMAF -- reading the two clips, pairing
//! the selected frames, handing each pair to a callback -- belongs to
//! [`super::probe`]. This module supplies the scorer, the per-pair submission
//! libvmaf needs, and the mapping from scorer errors to advice.

use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use av_metrics_vmaf::{
    BackendPreference,
    PlaneSet as MetricPlaneSet,
    PlaneSource as MetricPlaneSource,
    VideoFormat,
    VmafConfig as ScorerConfig,
    VmafError,
    VmafModel,
    VmafScorer,
};

use crate::{
    metrics::probe::{self, OutputIndexing, PlaneSet},
    models::sequence::target_quality::types::{QualityMetric, VmafFeature},
};

/// Build a `VmafConfig` from Av1an's metric configuration.
///
/// The `resolution` pre-scale is not applied here; VMAF scores at the
/// resolution it is given, so callers that need one must rescale the decoded
/// frames first. A `threads` value of 0 expands to one worker per available
/// core rather than being passed to libvmaf raw (see `vmaf_thread_count`).
///
/// # Errors
///
/// Returns an error when a feature has no libvmaf equivalent, or when an
/// explicit model path is missing. Validating early keeps the failure next to
/// the configuration that caused it.
#[inline]
pub fn scorer_config(metric: &QualityMetric) -> Result<ScorerConfig> {
    let QualityMetric::VMAF {
        threads,
        model,
        features,
        ..
    } = metric
    else {
        bail!("expected a VMAF metric configuration");
    };

    if features.contains(&VmafFeature::Motionless) {
        bail!("the `motionless` VMAF feature has no libvmaf equivalent");
    }

    // An explicit path wins; otherwise the feature list selects a stock model.
    let model = model.as_ref().map_or_else(
        || stock_model(features),
        |path| VmafModel::Path(path.clone()),
    );

    Ok(ScorerConfig::new()
        .with_model(model)
        .with_threads(vmaf_thread_count(*threads))
        // Try CUDA when libvmaf was built with it, and fall back to CPU
        // otherwise.
        .with_backend(BackendPreference::Auto))
}

/// libvmaf worker threads for a configured `threads` value.
///
/// Zero means "one worker per available core". Passing 0 through to libvmaf
/// leaves it with no worker pool rather than one per core, so the request is
/// expanded to the machine's parallelism here instead. A positive value is
/// honoured exactly, which matters for capping the thread pool on a shared
/// machine.
#[inline]
fn vmaf_thread_count(configured: usize) -> u32 {
    if configured == 0 {
        std::thread::available_parallelism()
            .map_or(1, |parallelism| parallelism.get())
            .try_into()
            .unwrap_or(u32::MAX)
    } else {
        u32::try_from(configured).unwrap_or(u32::MAX)
    }
}

/// Pick a stock libvmaf model from the feature selectors.
///
/// The features are a set, so a combination can be requested that names its own
/// stock model: `uhd` together with `neg` selects `vmaf_4k_v0.6.1neg`, the
/// 4K model trained for negative content. Checking `uhd` first would silently
/// drop `neg`, so the combined case is matched before either alone.
#[inline]
fn stock_model(features: &[VmafFeature]) -> VmafModel {
    let has = |feature: VmafFeature| features.contains(&feature);

    match (has(VmafFeature::Uhd), has(VmafFeature::Neg)) {
        (true, true) => VmafModel::UhdNeg,
        (true, false) => VmafModel::Uhd,
        (false, true) => VmafModel::Neg,
        (false, false) if has(VmafFeature::Weighted) => VmafModel::Weighted,
        (false, false) => VmafModel::Default,
    }
}

/// Score only the frames named by `selected`, returning one score per
/// selection.
///
/// The decode loop itself belongs to [`probe::probe_selected_frames`]; this
/// wraps it with the scorer and the per-pair submission libvmaf needs.
///
/// # Index spaces
///
/// The two decoders may use different index spaces, which is why `indexing` is
/// passed explicitly rather than inferred. See `probe::output_index`.
///
/// # Cost
///
/// Only the selected frames are decoded when a side can seek.
///
/// A caveat specific to VMAF: its motion and temporal extractors compare
/// consecutive frames, so a score is only meaningful within a contiguous run.
/// When the selection skips frames — `ProbeStrategy::Skip` or `Subset` — each
/// wanted frame is scored against whichever selected frame precedes it rather
/// than its true predecessor. Absolute numbers therefore differ slightly from a
/// continuous pass, though they stay directly comparable across encodes of the
/// same frames, which is what target-quality probing needs.
///
/// `selected` need not be sorted; frames are scored in the order given.
///
/// # Progress
///
/// `on_score` is called with `(position in the selection, score)` for each
/// scored frame, in the order the frames were visited.
///
/// libvmaf extracts on its own threads, so a score becomes readable only once
/// its features exist, which lags the submission by a few frames. Scores are
/// reported as they land rather than all at the end, so a caller sees them
/// progressively; the final few are reported after [`VmafScorer::finish`]
/// flushes.
///
/// # Errors
///
/// Returns an error when the two clips disagree on resolution or bit depth,
/// when libvmaf is unavailable, when seeking or decoding fails, or when a
/// selected frame lies past the end of the encoded output.
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

    // A scorer is created before any frame is read, so a bad configuration is
    // reported before spending time decoding.
    let opened = Instant::now();
    let details = *reference.get_video_details();
    let format = VideoFormat {
        width:           details.width as u32,
        height:          details.height as u32,
        bit_depth:       details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };
    let mut scorer = VmafScorer::new(scorer_config(config)?, format).map_err(describe_error)?;
    let setup = opened.elapsed();

    // Decoding is hard to separate from libvmaf's extraction from the outside,
    // so the split is recorded where the calls happen: `exchanged` is the whole
    // probe pass, `submitted` only the libvmaf call, so the fetch cost is
    // roughly their difference.
    let mut submitted = Duration::ZERO;
    // The lowest index whose score has not been reported yet. libvmaf scores
    // indices in order as its extractors complete, so advancing this by however
    // many scores each drain returned reports each one exactly once.
    let mut reported = 0_usize;

    // libvmaf's temporal extractors depend on the *index sequence* given to
    // `vmaf_read_pictures`, not on frames arriving in one batch, so
    // interleaving decoding with submission produces identical scores.
    //
    // `submit_pair_raw` already returns the index it scored, so the driver's
    // per-pair results are collected but discarded: the scores that matter come
    // from `finish`, one per submitted pair, in submission order.
    let pass = Instant::now();
    probe::probe_selected_frames::<(), _>(
        reference,
        distorted,
        selected,
        indexing,
        cancelled,
        probe::check_geometry,
        |_position, reference_planes, distorted_planes| {
            let submitted_at = Instant::now();
            let result = scorer
                .submit_pair_raw(
                    metric_planes(reference_planes),
                    metric_planes(distorted_planes),
                )
                .map(|_| ())
                .map_err(describe_error);
            submitted += submitted_at.elapsed();
            // Scores are drained only once a pair has been submitted, so a frame
            // is never reported before libvmaf has seen its reference.
            result?;

            for score in scorer.drain_scores(reported).map_err(describe_error)? {
                on_score(reported, score);
                reported += 1;
            }
            Ok(())
        },
    )?;
    let exchanged = pass.elapsed();

    let finished = Instant::now();
    let scores = scorer.finish().map_err(describe_error)?;
    let finished = finished.elapsed();

    // Whatever the sliding window had not finished is only available now.
    for score in scores.iter().skip(reported) {
        on_score(reported, *score);
        reported += 1;
    }

    tracing::debug!(
        frames = selected.len(),
        ?setup,
        fetch = ?exchanged.saturating_sub(submitted),
        submit = ?submitted,
        finish = ?finished,
        "vmaf pass phases"
    );

    if scores.len() != selected.len() {
        bail!(
            "VMAF scored {} frames but {} were selected",
            scores.len(),
            selected.len()
        );
    }

    Ok(scores)
}

/// Adapt the driver's plane set to the one `av-metrics-vmaf` submits.
///
/// The two descriptors carry the same fields, so this is a field-by-field copy
/// rather than a reinterpretation: the pointer still aliases the frame the
/// driver is holding, and libvmaf copies out of it before the frame is dropped.
#[inline]
fn metric_planes(planes: PlaneSet) -> MetricPlaneSet {
    planes.map(|plane| {
        // SAFETY: every field is copied unchanged from a `PlaneSource` the
        // driver produced from a live frame. The driver consumes the set inside
        // the `submit` call that receives it, so the frame outlives this
        // reference, and libvmaf only reads the planes before it is dropped.
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
fn describe_error(error: VmafError) -> anyhow::Error {
    match error {
        VmafError::LibraryNotFound => anyhow::anyhow!(
            "libvmaf is not installed. Install it, or set VMAF_LIB_DIR to the directory \
             containing it. See the av-metrics-vmaf README for per-distro instructions."
        ),
        VmafError::ModelNotFound {
            model, ..
        } => anyhow::anyhow!(
            "libvmaf could not load the VMAF model `{model}`. Some libvmaf builds ship without \
             the built-in models compiled in; pass an explicit model path instead."
        ),
        other => anyhow::anyhow!(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A VMAF metric configuration.
    ///
    /// Every field is spelled out rather than derived from another value,
    /// because `QualityMetric` is an enum and so cannot use struct-update
    /// syntax.
    fn metric(features: Vec<VmafFeature>) -> QualityMetric {
        QualityMetric::VMAF {
            target_range: (94.0, 96.0),
            resolution: None,
            scaler: String::new(),
            threads: 1,
            model: None,
            features,
        }
    }

    #[test]
    fn the_default_metric_selects_the_default_model() {
        let config = scorer_config(&metric(Vec::new())).expect("config should build");
        assert_eq!(config.model, VmafModel::Default);
    }

    /// The test helper configures a single thread, which must be honoured
    /// verbatim so a capped pool stays capped.
    #[test]
    fn an_explicit_thread_count_is_passed_through() {
        let config = scorer_config(&metric(Vec::new())).expect("config should build");
        assert_eq!(config.n_threads, 1);
    }

    #[test]
    fn zero_threads_expands_to_one_worker_per_available_core() {
        let configured = QualityMetric::VMAF {
            target_range: (94.0, 96.0),
            resolution:   None,
            scaler:       String::new(),
            threads:      0,
            model:        None,
            features:     vec![],
        };
        let config = scorer_config(&configured).expect("config should build");
        let available = std::thread::available_parallelism().map_or(1, |p| p.get());
        assert_eq!(
            config.n_threads,
            u32::try_from(available).unwrap_or(u32::MAX),
            "threads: 0 must expand to one worker per available core"
        );
        assert_ne!(
            config.n_threads, 0,
            "passing 0 through would give libvmaf no thread pool"
        );
    }

    #[test]
    fn feature_selectors_pick_the_matching_stock_model() {
        for (feature, expected) in [
            (VmafFeature::Neg, VmafModel::Neg),
            (VmafFeature::Uhd, VmafModel::Uhd),
            (VmafFeature::Weighted, VmafModel::Weighted),
        ] {
            let config = scorer_config(&metric(vec![feature])).expect("config should build");
            assert_eq!(config.model, expected, "{feature} should select {expected}");
        }
    }

    /// `uhd` and `neg` each name a different model, and together they name a
    /// third: the 4K model trained for negative content. Reporting the plain
    /// 4K model would silently drop the `neg` half of the request.
    #[test]
    fn the_uhd_and_neg_features_select_the_negative_4k_model() {
        let config = scorer_config(&metric(vec![VmafFeature::Uhd, VmafFeature::Neg]))
            .expect("config should build");
        assert_eq!(
            config.model,
            VmafModel::UhdNeg,
            "uhd + neg must select vmaf_4k_v0.6.1neg"
        );
        assert_eq!(
            config.model.as_libvmaf_model().expect("a stock model name"),
            "vmaf_4k_v0.6.1neg"
        );

        // The order the features are listed in must not matter.
        let reversed = scorer_config(&metric(vec![VmafFeature::Neg, VmafFeature::Uhd]))
            .expect("config should build");
        assert_eq!(reversed.model, VmafModel::UhdNeg);
    }

    #[test]
    fn an_explicit_model_path_wins_over_the_feature_selectors() {
        let configured = QualityMetric::VMAF {
            target_range: (94.0, 96.0),
            resolution:   None,
            scaler:       String::new(),
            threads:      1,
            model:        Some(std::path::PathBuf::from("/nonexistent/model.json")),
            features:     vec![VmafFeature::Neg],
        };
        let config = scorer_config(&configured).expect("config should build");
        assert!(
            matches!(config.model, VmafModel::Path(_)),
            "an explicit path must take precedence over the feature selectors"
        );
    }

    #[test]
    fn motionless_is_rejected_rather_than_silently_ignored() {
        let error = scorer_config(&metric(vec![VmafFeature::Motionless]))
            .expect_err("motionless has no libvmaf equivalent");
        assert!(
            error.to_string().contains("motionless"),
            "the error should name the offending feature, got: {error}"
        );
    }

    #[test]
    fn a_non_vmaf_metric_is_rejected() {
        let other = QualityMetric::SSIMULACRA2 {
            target_range: (74.0, 76.0),
            resolution:   None,
            threads:      None,
        };
        assert!(scorer_config(&other).is_err());
    }
}
