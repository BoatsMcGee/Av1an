//! Real QC/TQ scoring boundaries and decoders; only the native ABI is doubled.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "fixture assertions fail loudly")]

use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use super::{
    SequenceCompletion, SequenceStatus, Status, quality_check::QualityCheck,
    target_quality::TargetQuality,
};
use crate::{
    metrics::{self, Engine, OutputIndexing},
    models::sequence::target_quality::types::QualityMetric,
};

fn metric(disabled: Option<bool>) -> QualityMetric {
    QualityMetric::CVVDP {
        target_range: (0.0, 1.0),
        resolution: None,
        display_model: None,
        resize_to_display: Some(false),
        disable_temporal: disabled,
        gpu_id: Some(0),
    }
}

fn decoder(samples: &[u8]) -> av_decoders::Decoder {
    let clips = samples.iter().map(|sample| format!("c.std.BlankClip(width=32,height=32,length=1,format=vs.YUV420P8,color=[{sample},128,128])")).collect::<Vec<_>>().join(",");
    av_decoders::Decoder::from_script(&format!("import vapoursynth as vs\nc=vs.core\nc.std.Splice([{clips}]).std.SetFrameProps(_Matrix=1,_Transfer=13,_Primaries=1,_Range=1).set_output()"), Default::default()).unwrap()
}

fn calls() -> String {
    std::fs::read_to_string(std::env::var_os("M15_LOG").unwrap()).unwrap_or_default()
}
fn clear() {
    std::fs::write(std::env::var_os("M15_LOG").unwrap(), "").unwrap();
}

#[test]
fn m27a_native_scene_consumers() {
    if std::env::var_os("M27A_CHILD").is_some() {
        let engine = if std::env::var("M15_SCOPE").unwrap().contains("-f-") {
            Engine::Fmetrics
        } else {
            Engine::Vship
        };
        let failure = std::env::var("M15_FAIL").unwrap();
        if failure.is_empty() {
            native_consumers(engine);
            sequential_decoders(engine);
        } else {
            native_failure(engine, &failure);
        }
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libfmetrics.dylib"
    } else {
        "libfmetrics.so"
    });
    let output = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-shared", "-fPIC", "-o"])
        .arg(&library)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/m15_native.c"
        ))
        .output()
        .expect("native boundary requires cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::copy(
        &library,
        dir.path().join(if cfg!(target_os = "macos") {
            "libvship.dylib"
        } else {
            "libvship.so"
        }),
    )
    .unwrap();
    let mut errors = Vec::new();
    for (scope, failure) in [
        ("m27a-f-qc", ""),
        ("m27a-v-qc", ""),
        ("m27a-f-tq", ""),
        ("m27a-v-tq", ""),
        ("m27a-f-qc", "fp"),
        ("m27a-v-qc", "vp"),
        ("m27a-f-qc", "fp2"),
        ("m27a-v-qc", "vp2"),
    ] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "core::sequence::m27a_tests::m27a_native_scene_consumers",
                "--nocapture",
            ])
            .env("M27A_CHILD", "1")
            .env("M15_SCOPE", scope)
            .env("M15_FAIL", failure)
            .env("M16_FPS_AUDIT", "")
            .env("M15_LOG", dir.path().join("calls"))
            .env("FMETRICS_LIB_PATH", dir.path())
            .env("VSHIP_PLUGIN_PATH", dir.path())
            .output()
            .unwrap();
        if !output.status.success() {
            errors.push(format!(
                "{scope}/{failure}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

fn native_failure(engine: Engine, failure: &str) {
    clear();
    let prefix = if engine == Engine::Fmetrics { "f" } else { "v" };
    let mut a = decoder(&[1, 2, 3, 4, 5, 6]);
    let mut b = decoder(&[10, 20, 50, 60]);
    let mut positions = Vec::new();
    let error = metrics::score_scene_frames(
        engine,
        &mut a,
        &mut b,
        &metric(None),
        &[&[0, 1], &[4, 5]],
        OutputIndexing::Compacted,
        None,
        |position, _| positions.push(position),
    )
    .unwrap_err();
    assert!(!error.is::<metrics::probe::Cancelled>());
    assert_eq!(
        error.is::<metrics::SceneScoringStarted>(),
        failure.ends_with('2')
    );
    assert_eq!(
        positions,
        if failure.ends_with('2') {
            vec![0]
        } else {
            vec![]
        }
    );
    assert_eq!(
        calls(),
        if failure.ends_with('2') {
            format!("{prefix}c+\n{prefix}p\n{prefix}p\n{prefix}c-\n")
        } else {
            format!("{prefix}c+\n{prefix}p\n{prefix}c-\n")
        }
    );
}

fn sequential_decoders(engine: Engine) {
    let fixture = |samples: &[u8]| {
        let file = tempfile::Builder::new().suffix(".y4m").tempfile().unwrap();
        let mut data = b"YUV4MPEG2 W32 H32 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for sample in samples {
            data.extend_from_slice(b"FRAME\n");
            data.extend_from_slice(&[*sample; 1024]);
            data.extend_from_slice(&[128; 512]);
        }
        std::fs::write(file.path(), data).unwrap();
        file
    };
    let reference = fixture(&[32, 48, 64, 80, 96, 112]);
    let aligned = fixture(&[40, 56, 72, 88, 104, 120]);
    let compacted = fixture(&[40, 56, 104, 120]);
    let second = fixture(&[104, 120]);
    for indexing in [OutputIndexing::Aligned, OutputIndexing::Compacted] {
        let output = if indexing == OutputIndexing::Aligned {
            aligned.path()
        } else {
            compacted.path()
        };
        let mut a = av_decoders::Decoder::from_file(reference.path()).unwrap();
        let mut b = av_decoders::Decoder::from_file(output).unwrap();
        assert!(
            a.seek_to_frame(0).is_err(),
            "fixture must exercise a sequential reader"
        );
        clear();
        let values = metrics::score_scene_frames(
            engine,
            &mut a,
            &mut b,
            &metric(None),
            &[&[0, 1], &[4, 5]],
            indexing,
            None,
            |_, _| {},
        )
        .expect("reuse sequential readers across fresh scorers");
        let prefix = if engine == Engine::Fmetrics { "f" } else { "v" };
        assert_eq!(
            calls().lines().filter(|s| *s == format!("{prefix}c+")).count(),
            2
        );
        let mut a = av_decoders::Decoder::from_file(reference.path()).unwrap();
        let mut b = av_decoders::Decoder::from_file(second.path()).unwrap();
        let standalone = if engine == Engine::Fmetrics {
            metrics::fmetrics::score_probed_frames(
                &mut a,
                &mut b,
                &metric(None),
                &[4, 5],
                OutputIndexing::Compacted,
                None,
                |_, _| {},
            )
        } else {
            metrics::vship::score_probed_frames(
                &mut a,
                &mut b,
                &metric(None),
                &[4, 5],
                OutputIndexing::Compacted,
                None,
                |_, _| {},
            )
        }
        .unwrap();
        assert_eq!(&values[2..], standalone);
    }
}

fn native_consumers(engine: Engine) {
    let prefix = if engine == Engine::Fmetrics { "f" } else { "v" };
    for second in [[2, 3], [4, 5]] {
        let scenes: &[&[usize]] = &[&[0, 1], &second];
        let selected = [0, 1, second[0], second[1]];
        for qc in [true, false] {
            if qc != std::env::var("M15_SCOPE").unwrap().ends_with("-qc") {
                continue;
            }
            let distorted = if qc {
                vec![10, 20, 30, 40, 50, 60]
            } else {
                vec![10, 20, second[0] as u8 * 10 + 10, second[1] as u8 * 10 + 10]
            };
            for disabled in [None, Some(false)] {
                let mut a = decoder(&[1, 2, 3, 4, 5, 6]);
                let mut b = decoder(&distorted);
                let cancel = AtomicBool::new(false);
                let (tx, rx) = mpsc::channel();
                clear();
                let values = if qc {
                    QualityCheck::score_native_scenes(
                        engine,
                        &mut a,
                        &mut b,
                        &metric(disabled),
                        scenes,
                        &tx,
                        &cancel,
                    )
                } else {
                    TargetQuality::score_native_scenes(
                        engine,
                        &mut a,
                        &mut b,
                        &metric(disabled),
                        scenes,
                        &tx,
                        &cancel,
                    )
                }
                .unwrap();
                let expected = if second == [2, 3] {
                    [31.0, 31372.0, 93.0, 94054.0]
                } else {
                    [31.0, 31372.0, 155.0, 156736.0]
                };
                assert_eq!(
                    values, expected,
                    "each scene is an independent prefix vector; QC={qc} {second:?}"
                );
                let log = calls();
                assert_eq!(
                    log.lines().filter(|s| *s == format!("{prefix}c+")).count(),
                    2,
                    "{log}"
                );
                assert_eq!(
                    log.lines().filter(|s| *s == format!("{prefix}c-")).count(),
                    2,
                    "{log}"
                );
                // Independently submitted scene sessions have identical prefixes.
                let mut standalone = Vec::new();
                for scene in scenes {
                    let mut reference = decoder(&[1, 2, 3, 4, 5, 6]);
                    let samples: Vec<_> = scene.iter().map(|i| (*i as u8 + 1) * 10).collect();
                    let mut output = decoder(&samples);
                    let values = if engine == Engine::Fmetrics {
                        metrics::fmetrics::score_probed_frames(
                            &mut reference,
                            &mut output,
                            &metric(disabled),
                            scene,
                            OutputIndexing::Compacted,
                            None,
                            |_, _| {},
                        )
                    } else {
                        metrics::vship::score_probed_frames(
                            &mut reference,
                            &mut output,
                            &metric(disabled),
                            scene,
                            OutputIndexing::Compacted,
                            None,
                            |_, _| {},
                        )
                    }
                    .unwrap();
                    standalone.extend(values);
                }
                assert_eq!(values, standalone);
                let progress: Vec<_> = rx.try_iter().collect();
                if qc {
                    let events: Vec<_> = progress
                        .into_iter()
                        .map(|event| match event {
                            SequenceStatus::Subprocess {
                                parent:
                                    Status::Processing {
                                        completion:
                                            SequenceCompletion::Custom {
                                                completed, total, ..
                                            },
                                        ..
                                    },
                                child:
                                    Status::Processing {
                                        completion: SequenceCompletion::FrameScore { frame, score },
                                        ..
                                    },
                            } => (completed, total, frame, score),
                            other => panic!("unexpected QC progress {other:?}"),
                        })
                        .collect();
                    assert_eq!(
                        events,
                        (0..4)
                            .map(|i| ((i + 1) as f64, 4.0, selected[i] as u64, expected[i]))
                            .collect::<Vec<_>>()
                    );
                } else {
                    let counts: Vec<_> = progress
                        .into_iter()
                        .map(|event| match event {
                            SequenceStatus::Whole(Status::Processing {
                                completion: SequenceCompletion::Frames { completed, total },
                                ..
                            }) => (completed, total),
                            other => panic!("unexpected TQ progress {other:?}"),
                        })
                        .collect();
                    assert_eq!(counts, vec![(0, 4), (1, 4), (2, 4), (3, 4), (4, 4)]);
                }
                // A direct clip-wide API intentionally still has one accumulating session.
                if second == [2, 3] {
                    clear();
                    let mut a = decoder(&[1, 2, 3, 4, 5, 6]);
                    let mut b = decoder(&distorted);
                    let direct = if engine == Engine::Fmetrics {
                        metrics::fmetrics::score_probed_frames(
                            &mut a,
                            &mut b,
                            &metric(disabled),
                            &selected,
                            OutputIndexing::Aligned,
                            None,
                            |_, _| {},
                        )
                    } else {
                        metrics::vship::score_probed_frames(
                            &mut a,
                            &mut b,
                            &metric(disabled),
                            &selected,
                            OutputIndexing::Aligned,
                            None,
                            |_, _| {},
                        )
                    }
                    .unwrap();
                    assert_ne!(direct[2], values[2]);
                    assert_eq!(
                        calls().lines().filter(|s| *s == format!("{prefix}c+")).count(),
                        1
                    );
                }
            }
        }
    }
    // A callback cancellation must prevent both later frames and the next context.
    for cancel_at in [0, 1] {
        clear();
        let mut a = decoder(&[1, 2, 3, 4, 5, 6]);
        let mut b = decoder(&[10, 20, 30, 40]);
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut positions = Vec::new();
        let result = metrics::score_scene_frames(
            engine,
            &mut a,
            &mut b,
            &metric(None),
            &[&[0, 1], &[4, 5]],
            OutputIndexing::Compacted,
            Some(&cancelled),
            |position, _| {
                positions.push(position);
                if position == cancel_at {
                    cancelled.store(true, Ordering::Relaxed);
                }
            },
        );
        let error = result.unwrap_err();
        assert!(error.is::<metrics::probe::Cancelled>());
        assert_eq!(positions, if cancel_at == 0 { vec![0] } else { vec![0, 1] });
        assert_eq!(
            calls(),
            if cancel_at == 0 {
                format!("{prefix}c+\n{prefix}p\n{prefix}c-\n")
            } else {
                format!("{prefix}c+\n{prefix}p\n{prefix}p\n{prefix}c-\n")
            }
        );
    }
    for scenes in [&[&[1, 0][..]][..], &[&[0, 1][..], &[1, 2][..]][..], &[&[usize::MAX][..]][..]] {
        clear();
        let mut a = decoder(&[1]);
        let mut b = decoder(&[10]);
        assert!(
            metrics::score_scene_frames(
                engine,
                &mut a,
                &mut b,
                &metric(None),
                scenes,
                OutputIndexing::Compacted,
                None,
                |_, _| panic!("invalid scene scored")
            )
            .is_err()
        );
        assert_eq!(calls(), "");
    }
    clear();
    let mut a = decoder(&[1]);
    let mut b = decoder(&[10]);
    assert!(
        metrics::score_scene_frames(
            engine,
            &mut a,
            &mut b,
            &metric(None),
            &[&[], &[]],
            OutputIndexing::Compacted,
            None,
            |_, _| panic!("empty scene scored")
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(calls(), "");
    let cancelled = AtomicBool::new(true);
    assert!(
        metrics::score_scene_frames(
            engine,
            &mut a,
            &mut b,
            &metric(None),
            &[&[0]],
            OutputIndexing::Aligned,
            Some(&cancelled),
            |_, _| panic!("cancelled scene scored")
        )
        .unwrap_err()
        .is::<metrics::probe::Cancelled>()
    );
    assert_eq!(calls(), "");
    // Independent-frame CVVDP keeps a single session and resets after each pair.
    let mut a = decoder(&[1, 2, 3, 4, 5, 6]);
    let mut b = decoder(&[10, 20, 50, 60]);
    let values = metrics::score_scene_frames(
        engine,
        &mut a,
        &mut b,
        &metric(Some(true)),
        &[&[0, 1], &[4, 5]],
        OutputIndexing::Compacted,
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(values, [31.0, 62.0, 155.0, 186.0]);
    assert_eq!(
        calls().lines().filter(|s| *s == format!("{prefix}c+")).count(),
        1
    );
    // Within-scene gaps still use the M15 capability rules.
    clear();
    let mut a = decoder(&[1, 2, 3, 4, 5, 6]);
    let mut b = decoder(&[10, 30, 50, 60]);
    let values = metrics::score_scene_frames(
        engine,
        &mut a,
        &mut b,
        &metric(None),
        &[&[0, 2], &[4, 5]],
        OutputIndexing::Compacted,
        None,
        |_, _| {},
    );
    if engine == Engine::Fmetrics {
        assert!(values.unwrap_err().to_string().contains("history-only"));
        assert_eq!(calls(), "");
    } else {
        assert_eq!(values.unwrap(), [31.0, 403.0, 155.0, 156736.0]);
        assert_eq!(calls(), "vc+\nvp\nvr\nvp\nvc-\nvc+\nvp\nvp\nvc-\n");
    }
}
