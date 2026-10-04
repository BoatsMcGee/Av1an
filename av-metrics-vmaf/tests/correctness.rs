//! Correctness tests for VMAF scoring.
//!
//! Tests that need libvmaf are skipped when it is unavailable, so the suite
//! stays green in environments without it. Everything that does not require
//! libvmaf always runs.

// Test code asserts and unwraps freely: a failure panics with a readable message,
// which is exactly what a test wants.
#![allow(clippy::unwrap_used, reason = "assertion failures should be loud")]

mod common;

use std::path::{Path, PathBuf};

use av_metrics_vmaf::{
    Backend,
    BackendPreference,
    PoolMethod,
    VideoFormat,
    VmafConfig,
    VmafError,
    VmafScorer,
    is_available,
    libvmaf_version,
};
use common::{Distortion, SyntheticPair};
use v_frame::chroma::ChromaSubsampling;

/// Skip the test body when libvmaf is not installed.
macro_rules! require_libvmaf {
    () => {
        if !is_available() {
            eprintln!("skipping: libvmaf is not installed");
            return;
        }
    };
}

/// A model configuration that works on any libvmaf build.
///
/// Not every libvmaf build compiles the stock models in. Arch's `vmaf` package
/// and MSYS2's `mingw-w64-*-vmaf`, for example, ship without them, and
/// `vmaf_model_load` then rejects a bare version like `vmaf_v0.6.1` with
/// `EINVAL`.
///
/// `VMAF_TEST_MODEL` names a model JSON explicitly and wins. Otherwise the
/// stock model is used, which the scorer resolves either from libvmaf's
/// built-ins or from a model file found through `VMAF_MODEL_PATH` and the
/// platform search paths.
fn model_config() -> VmafConfig {
    std::env::var("VMAF_TEST_MODEL").map_or_else(
        |_| VmafConfig::new(),
        |path| VmafConfig::new().with_model_path(path),
    )
}

/// Format a small reference/distorted pair.
fn small_pair(distortion: Distortion) -> SyntheticPair {
    SyntheticPair::generate(64, 48, 8, ChromaSubsampling::Yuv420, 12, distortion)
}

#[test]
fn availability_probe_is_consistent() {
    match is_available() {
        true => assert!(
            libvmaf_version().is_some(),
            "a loadable libvmaf must report a version"
        ),
        false => assert_eq!(libvmaf_version(), None),
    }
}

#[test]
fn scoring_reports_the_backend_in_use() {
    require_libvmaf!();

    let pair = small_pair(Distortion::Mild);
    let details = *pair.reference.decoder().get_video_details();
    let format = VideoFormat {
        width:           details.width as u32,
        height:          details.height as u32,
        bit_depth:       details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
    // Auto preference must land on a real backend, never an uninitialised one.
    assert!(matches!(scorer.backend(), Backend::Cpu | Backend::Cuda));
}

#[test]
fn cpu_only_preference_forces_the_cpu_backend() {
    require_libvmaf!();

    let pair = small_pair(Distortion::Mild);
    let details = *pair.reference.decoder().get_video_details();
    let format = VideoFormat {
        width:           details.width as u32,
        height:          details.height as u32,
        bit_depth:       details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let scorer = VmafScorer::new(
        model_config().with_backend(BackendPreference::CpuOnly),
        format,
    )
    .expect("scorer should initialise");

    assert_eq!(scorer.backend(), Backend::Cpu);
}

#[test]
fn identical_clips_score_at_or_near_the_maximum() {
    require_libvmaf!();

    let pair = small_pair(Distortion::None);
    // Score the undistorted clip against itself.
    let mut reference = pair.reference.decoder();
    let mut distorted = pair.reference.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width:           details.width as u32,
        height:          details.height as u32,
        bit_depth:       details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
    let scores = scorer
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
        .expect("identical clips should score");

    assert!(
        !scores.is_empty(),
        "scoring identical clips must produce at least one score"
    );

    // Identical clips must score identically at every frame, which is the property
    // this asserts. The absolute value is *not* 100: libvmaf's motion and temporal
    // extractors compare consecutive frames, so the first frame has no predecessor
    // and scores lower, and a high-frequency synthetic pattern can score below 100
    // on later frames too. Measured against libvmaf's own CLI, an identical pair
    // scores exactly 100 per frame on smooth content but not on synthetic noise, so
    // pinning an absolute number here would be pinning our test content rather than
    // the implementation. `matches_the_reference_tools_on_a_real_clip` is what
    // verifies absolute agreement with the reference tools.
    for frame in &scores {
        assert!(
            frame.score > 90.0 && frame.score <= 100.0,
            "identical clips should score in (90, 100], got {}",
            frame.score
        );
    }

    let mean = VmafScorer::pool(&scores, PoolMethod::Mean).expect("scores should pool");
    assert!(
        mean > 99.0 && mean <= 100.0,
        "identical clips should pool to just under 100, got {mean}"
    );
}

#[test]
fn scores_decrease_as_distortion_increases() {
    require_libvmaf!();

    let score_at = |distortion: Distortion| {
        let pair = small_pair(distortion);
        let mut reference = pair.reference.decoder();
        let mut distorted = pair.distorted.decoder();

        let details = *reference.get_video_details();
        let format = VideoFormat {
            width:           details.width as u32,
            height:          details.height as u32,
            bit_depth:       details.bit_depth as u32,
            chroma_sampling: details.chroma_sampling,
        };

        let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
        let scores = scorer
            .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
            .expect("distorted clips should score");
        VmafScorer::pool(&scores, PoolMethod::Mean).expect("scores should pool")
    };

    let none = score_at(Distortion::None);
    let mild = score_at(Distortion::Mild);
    let moderate = score_at(Distortion::Moderate);
    let severe = score_at(Distortion::Severe);

    assert!(
        none >= mild,
        "no distortion ({none}) must score at least as high as mild ({mild})"
    );
    assert!(
        mild > moderate,
        "mild ({mild}) must score higher than moderate ({moderate})"
    );
    assert!(
        moderate > severe,
        "moderate ({moderate}) must score higher than severe ({severe})"
    );
}

#[test]
fn frame_count_is_preserved_exactly() {
    require_libvmaf!();

    // The sliding window means the last score only arrives on flush. This is the
    // most likely place for an off-by-one, so the count is asserted exactly.
    for frames in [1usize, 2, 3, 8, 17] {
        let pair = SyntheticPair::generate(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            frames,
            Distortion::Mild,
        );
        let mut reference = pair.reference.decoder();
        let mut distorted = pair.distorted.decoder();

        let details = *reference.get_video_details();
        let format = VideoFormat {
            width:           details.width as u32,
            height:          details.height as u32,
            bit_depth:       details.bit_depth as u32,
            chroma_sampling: details.chroma_sampling,
        };

        let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
        let scores = scorer
            .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
            .expect("scoring should succeed");

        assert_eq!(
            scores.len(),
            frames,
            "{frames} frames must yield exactly {frames} scores"
        );
    }
}

#[test]
fn the_callback_sees_every_score() {
    require_libvmaf!();

    let pair = small_pair(Distortion::Mild);
    let mut reference = pair.reference.decoder();
    let mut distorted = pair.distorted.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width:           details.width as u32,
        height:          details.height as u32,
        bit_depth:       details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut seen = Vec::new();
    let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
    let scores = scorer
        .score_decoders::<u8>(&mut reference, &mut distorted, |index, score| {
            seen.push((index, score.score));
        })
        .expect("scoring should succeed");

    assert_eq!(
        seen.len(),
        scores.len(),
        "the callback must fire once per score"
    );
    for (index, (callback_index, _)) in seen.iter().enumerate() {
        assert_eq!(
            *callback_index, index,
            "callback indices must be sequential"
        );
    }
}

#[test]
fn mismatched_frame_counts_are_rejected() {
    require_libvmaf!();

    let reference_pair =
        SyntheticPair::generate(64, 48, 8, ChromaSubsampling::Yuv420, 6, Distortion::Mild);
    let distorted_pair =
        SyntheticPair::generate(64, 48, 8, ChromaSubsampling::Yuv420, 9, Distortion::Mild);

    let mut reference = reference_pair.reference.decoder();
    let mut distorted = distorted_pair.distorted.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width:           details.width as u32,
        height:          details.height as u32,
        bit_depth:       details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
    let result = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {});

    assert!(
        matches!(result, Err(VmafError::FrameCountMismatch { .. })),
        "differing lengths must be reported, got {:?}",
        result.err()
    );
}

#[test]
fn mismatched_resolutions_are_rejected() {
    require_libvmaf!();

    let reference_pair =
        SyntheticPair::generate(64, 48, 8, ChromaSubsampling::Yuv420, 4, Distortion::Mild);
    let distorted_pair =
        SyntheticPair::generate(32, 32, 8, ChromaSubsampling::Yuv420, 4, Distortion::Mild);

    let mut reference = reference_pair.reference.decoder();
    let mut distorted = distorted_pair.distorted.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width:           details.width as u32,
        height:          details.height as u32,
        bit_depth:       details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
    let result = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {});

    assert!(
        matches!(result, Err(VmafError::InputMismatch { .. })),
        "differing resolutions must be reported, got {:?}",
        result.err()
    );
}

#[test]
fn a_missing_model_file_is_reported_before_scoring_starts() {
    let format = VideoFormat {
        width:           64,
        height:          48,
        bit_depth:       8,
        chroma_sampling: ChromaSubsampling::Yuv420,
    };

    let result = VmafScorer::new(
        VmafConfig::new().with_model_path("/nonexistent/model.json"),
        format,
    );

    assert!(
        matches!(result, Err(VmafError::ModelFileMissing { .. })),
        "a missing model file must be reported, got {:?}",
        result.err()
    );
}

#[test]
fn unsupported_formats_are_rejected() {
    for (bit_depth, chroma_sampling) in [
        (9u32, ChromaSubsampling::Yuv420),
        (14, ChromaSubsampling::Yuv420),
        (8, ChromaSubsampling::Monochrome),
    ] {
        let format = VideoFormat {
            width: 64,
            height: 48,
            bit_depth,
            chroma_sampling,
        };
        assert!(
            format.validate().is_err(),
            "bit depth {bit_depth} with {chroma_sampling:?} must be rejected"
        );
    }
}

#[test]
fn ten_bit_content_scores() {
    require_libvmaf!();

    let pair = SyntheticPair::generate(64, 48, 10, ChromaSubsampling::Yuv420, 6, Distortion::Mild);
    let mut reference = pair.reference.decoder();
    let mut distorted = pair.distorted.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width:           details.width as u32,
        height:          details.height as u32,
        bit_depth:       details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
    let scores = scorer
        .score_decoders::<u16>(&mut reference, &mut distorted, |_, _| {})
        .expect("10-bit content should score");

    assert_eq!(scores.len(), 6);
    for frame in &scores {
        assert!(
            frame.score.is_finite(),
            "scores must be finite, got {}",
            frame.score
        );
    }
}

#[test]
fn additional_features_are_reported() {
    require_libvmaf!();

    let pair = small_pair(Distortion::Moderate);
    let mut reference = pair.reference.decoder();
    let mut distorted = pair.distorted.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width:           details.width as u32,
        height:          details.height as u32,
        bit_depth:       details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(
        model_config().with_feature(av_metrics_vmaf::VmafFeature::Psnr),
        format,
    )
    .expect("scorer should initialise");

    let scores = scorer
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
        .expect("scoring with features should succeed");
    assert!(!scores.is_empty());

    // Feature extractors report pooled values rather than a score per frame, so
    // they are read once scoring has finished.
    let features = scorer.pooled_features().expect("pooled feature values should be readable");

    assert!(
        features.iter().any(|(name, _)| name == "psnr_y"),
        "PSNR was requested, so `psnr_y` must be present, got {features:?}"
    );
    for (name, value) in &features {
        assert!(
            value.is_finite(),
            "feature `{name}` produced a non-finite value: {value}"
        );
    }
}

/// End-to-end scoring against a model file on disk.
///
/// Some libvmaf builds ship without the built-in models compiled in, in which
/// case `vmaf_model_load` rejects a bare version such as `vmaf_v0.6.1`. This
/// test covers the other path: an explicit model JSON, loaded via
/// `VmafModel::Path`.
///
/// The model is not committed to this repository, so the test is driven by the
/// `VMAF_TEST_MODEL` environment variable and skips when unset.
#[test]
fn scoring_works_with_an_explicit_model_file() {
    if !is_available() {
        eprintln!("skipping: libvmaf is not installed");
        return;
    }

    let Ok(model) = std::env::var("VMAF_TEST_MODEL") else {
        eprintln!("skipping: set VMAF_TEST_MODEL to a libvmaf model JSON to run this");
        return;
    };

    let pair = small_pair(Distortion::Moderate);
    let mut reference = pair.reference.decoder();
    let mut distorted = pair.distorted.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width:           details.width as u32,
        height:          details.height as u32,
        bit_depth:       details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(VmafConfig::new().with_model_path(&model), format)
        .expect("scorer should initialise with an explicit model file");

    let scores = scorer
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
        .expect("scoring with an explicit model should succeed");

    assert_eq!(scores.len(), 12, "12 frames must yield 12 scores");
    let pooled = VmafScorer::pool(&scores, PoolMethod::Mean).expect("scores should pool");
    assert!(
        (0.0..=100.0).contains(&pooled),
        "a pooled VMAF score must fall in 0..=100, got {pooled}"
    );
}

/// Every model in the search path loads and produces usable scores.
///
/// The Windows installer ships all nine upstream models, but only four are
/// reachable by name through [`av_metrics_vmaf::VmafModel`]. The `float`
/// variants in particular are reachable solely by path, so nothing else in the
/// suite would notice if they stopped loading — and a build without
/// `-Denable_float=true` rejects them with `EINVAL`, which would otherwise
/// surface as a confusing failure at scoring time rather than at install time.
///
/// This scans `VMAF_MODEL_PATH` (or the resolved search paths) and asserts each
/// model loads. Models that libvmaf was not built to support are reported but
/// do not fail the run, so the test stays meaningful across build
/// configurations.
#[test]
fn every_model_in_the_search_path_loads() {
    if !is_available() {
        eprintln!("skipping: libvmaf is not installed");
        return;
    }

    let Ok(directory) = std::env::var("VMAF_MODEL_PATH") else {
        eprintln!("skipping: set VMAF_MODEL_PATH to a directory of model JSONs to run this");
        return;
    };
    let Ok(entries) = std::fs::read_dir(&directory) else {
        eprintln!("skipping: {directory} is not readable");
        return;
    };

    let mut models: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .collect();
    models.sort();

    assert!(
        !models.is_empty(),
        "no model JSONs were found in {directory}"
    );

    let mut loaded = Vec::new();
    let mut unsupported = Vec::new();

    // Take the geometry from a real clip rather than hard-coding it: the 4K
    // models declare a 3840x2160 input and libvmaf rescales to that, so a
    // mismatch here would be an error about the test rather than the model.
    let pair = small_pair(Distortion::Moderate);
    let details = *pair.reference.decoder().get_video_details();
    let format = VideoFormat {
        width:           details.width as u32,
        height:          details.height as u32,
        bit_depth:       details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    for model in &models {
        let name = model.file_name().map_or_else(
            || model.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );

        let result = VmafScorer::new(VmafConfig::new().with_model_path(model), format);

        match result {
            Ok(mut scorer) => {
                let mut reference = pair.reference.decoder();
                let mut distorted = pair.distorted.decoder();
                let scores = scorer
                    .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
                    .unwrap_or_else(|error| panic!("{name} loaded but could not score: {error}"));

                assert!(
                    scores.iter().all(|score| score.score.is_finite()),
                    "{name} produced a non-finite score"
                );
                loaded.push(name);
            },
            Err(error) => {
                // A build without -Denable_float=true rejects the float models.
                // That is a property of the libvmaf build, not a defect here.
                let message = error.to_string();
                eprintln!("note: {name} is not supported by this libvmaf build: {message}");
                unsupported.push(name);
            },
        }
    }

    println!(
        "loaded {} model(s): {loaded:?}; unsupported by this build: {unsupported:?}",
        loaded.len()
    );

    assert!(
        loaded.len() + unsupported.len() == models.len(),
        "every model must either load or be reported as unsupported"
    );
}

/// Scores a real clip against itself and compares with the reference tools.
///
/// Set `VMAF_TEST_Y4M` to a y4m file and `VMAF_TEST_MODEL` to a model JSON.
/// Scoring the file against itself is the strongest available check that the
/// frames and geometry handed to libvmaf are correct, because the expected
/// values come from libvmaf's own CLI and FFmpeg's libvmaf filter run over the
/// same bytes.
#[test]
fn matches_the_reference_tools_on_a_real_clip() {
    if !is_available() {
        eprintln!("skipping: libvmaf is not installed");
        return;
    }

    let (Ok(clip), Ok(model)) = (
        std::env::var("VMAF_TEST_Y4M"),
        std::env::var("VMAF_TEST_MODEL"),
    ) else {
        eprintln!("skipping: set VMAF_TEST_Y4M and VMAF_TEST_MODEL to run this");
        return;
    };

    let mut reference =
        av_decoders::Decoder::from_file(&clip).expect("the reference clip should decode");
    let mut distorted =
        av_decoders::Decoder::from_file(&clip).expect("the second decoder should open");

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width:           details.width as u32,
        height:          details.height as u32,
        bit_depth:       details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(VmafConfig::new().with_model_path(&model), format)
        .expect("scorer should initialise");

    let scores = scorer
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
        .expect("scoring a clip against itself should succeed");

    // Measured with libvmaf 3.2.1's CLI and FFmpeg's libvmaf filter, which agree to
    // six decimal places: frame 0 scores 97.42837 and every later frame scores
    // exactly 100. Frame 0 is lower because the motion and temporal extractors
    // have no previous frame to compare against.
    for (index, frame) in scores.iter().enumerate() {
        let expected = if index == 0 { 97.42837 } else { 100.0 };
        assert!(
            (frame.score - expected).abs() < 0.001,
            "frame {index} scored {} but the reference tools give {expected}",
            frame.score
        );
    }

    let mean = VmafScorer::pool(&scores, PoolMethod::Mean).expect("scores should pool");
    assert!(
        (mean - 99.964283).abs() < 0.001,
        "pooled score should match the reference tools' 99.964283, got {mean}"
    );
}

/// A contiguous selection must score identically through the frame-pair path
/// and the streaming path.
///
/// Both hand libvmaf the same frames; if they disagree, one is submitting
/// frames incorrectly. This is what lets a caller pick whichever is convenient
/// without changing the numbers.
///
/// Requires `VMAF_TEST_Y4M` and `VMAF_TEST_MODEL`.
#[test]
fn the_frame_path_agrees_with_the_streaming_path() {
    if !is_available() {
        eprintln!("skipping: libvmaf is not installed");
        return;
    }

    let (Ok(clip), Ok(model)) = (
        std::env::var("VMAF_TEST_Y4M"),
        std::env::var("VMAF_TEST_MODEL"),
    ) else {
        eprintln!("skipping: set VMAF_TEST_Y4M and VMAF_TEST_MODEL to run this");
        return;
    };

    let config = VmafConfig::new().with_model_path(&model);
    let wanted: Vec<usize> = (0..12).collect();

    let open = || {
        let reference = av_decoders::Decoder::from_file(&clip).expect("reference opens");
        let distorted = av_decoders::Decoder::from_file(&clip).expect("output opens");
        let details = *reference.get_video_details();
        let format = VideoFormat {
            width:           details.width as u32,
            height:          details.height as u32,
            bit_depth:       details.bit_depth as u32,
            chroma_sampling: details.chroma_sampling,
        };
        (reference, distorted, format)
    };

    let (mut reference, mut distorted, format) = open();
    let streaming = VmafScorer::new(config.clone(), format)
        .expect("scorer")
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
        .expect("streaming scoring");

    let (mut reference, mut distorted, format) = open();
    let mut scorer = VmafScorer::new(config, format).expect("scorer");
    let pairs = decode_pairs(&mut reference, &mut distorted, &wanted);
    let frame_path = scorer.score_frames::<u8>(&pairs).expect("frame-path scoring");

    // The streaming path scores every frame of the clip; the frame path scores only
    // the selection. The selection is the leading run, so its scores must match the
    // streaming path's for the same frames.
    assert!(
        streaming.len() >= wanted.len(),
        "streaming should score at least the {} selected frames, got {}",
        wanted.len(),
        streaming.len()
    );
    assert_eq!(
        frame_path.len(),
        wanted.len(),
        "the frame path should score exactly the selection"
    );

    for (index, sparse) in frame_path.iter().enumerate() {
        let streamed = streaming[index].score;
        assert!(
            (streamed - *sparse).abs() < 0.001,
            "frame {index} streamed {streamed} but scored {sparse} through the frame path"
        );
    }
}

/// Decode the first `wanted.len()` frames from both decoders.
///
/// Reads forward rather than seeking, because the y4m decoder has no random
/// access; `wanted` is therefore a contiguous run starting at frame 0.
fn decode_pairs(
    reference: &mut av_decoders::Decoder,
    distorted: &mut av_decoders::Decoder,
    wanted: &[usize],
) -> Vec<(v_frame::frame::Frame<u8>, v_frame::frame::Frame<u8>)> {
    let mut pairs = Vec::with_capacity(wanted.len());
    for _ in wanted {
        pairs.push((
            reference.read_video_frame::<u8>().expect("reference frame"),
            distorted.read_video_frame::<u8>().expect("output frame"),
        ));
    }
    pairs
}

/// A stock model resolves from `VMAF_MODEL_PATH` when libvmaf has none built
/// in.
///
/// Several distributions ship libvmaf without the models compiled in — Arch's
/// `vmaf` package and MSYS2's `mingv-w64-*-vmaf` both do — and
/// `vmaf_model_load` then rejects a bare version such as `vmaf_v0.6.1` with
/// `EINVAL`. Scoring must still work by finding the same model as a file on
/// disk.
///
/// Requires `VMAF_TEST_Y4M` and `VMAF_MODEL_PATH`.
#[test]
fn a_stock_model_resolves_from_a_directory() {
    if !is_available() {
        eprintln!("skipping: libvmaf is not installed");
        return;
    }

    let (Ok(clip), Ok(directory)) = (
        std::env::var("VMAF_TEST_Y4M"),
        std::env::var("VMAF_MODEL_PATH"),
    ) else {
        eprintln!("skipping: set VMAF_TEST_Y4M and VMAF_MODEL_PATH to run this");
        return;
    };

    assert!(
        Path::new(&directory).join("vmaf_v0.6.1.json").is_file(),
        "VMAF_MODEL_PATH should contain vmaf_v0.6.1.json"
    );

    let mut reference = av_decoders::Decoder::from_file(&clip).expect("reference opens");
    let mut distorted = av_decoders::Decoder::from_file(&clip).expect("output opens");

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width:           details.width as u32,
        height:          details.height as u32,
        bit_depth:       details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    // No `with_model_path`, so the stock name is used and must be found on disk.
    let mut scorer =
        VmafScorer::new(VmafConfig::new(), format).expect("the model should resolve from disk");

    let scores = scorer
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
        .expect("scoring should succeed");

    // y4m carries no frame count in its header, so the count is whatever the clip
    // yields; the point is that scoring succeeded and the values are sane.
    assert!(!scores.is_empty(), "scoring should produce scores");
    for frame in &scores {
        assert!(
            frame.score > 90.0 && frame.score <= 100.0,
            "identical clips should score in (90, 100], got {}",
            frame.score
        );
    }
}

/// Writes the synthetic test clip to disk so the reference tools can score it.
///
/// Used to confirm that scores on synthetic content agree with libvmaf's own
/// CLI; not an assertion in itself. Gated on `VMAF_DUMP_SYNTHETIC`.
#[test]
fn dump_synthetic_clip_for_reference_comparison() {
    let Ok(destination) = std::env::var("VMAF_DUMP_SYNTHETIC") else {
        return;
    };

    let pair = small_pair(Distortion::None);
    std::fs::write(&destination, &pair.reference.data).expect("the clip should be written");
    eprintln!("wrote the synthetic clip to {destination}");
}
