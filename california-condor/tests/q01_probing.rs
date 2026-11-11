#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };

    use andean_condor::{
        core::{
            Condor,
            input::Input,
            output::Output,
            sequence::{Sequence, quality_check::QualityCheck, target_quality::TargetQuality},
        },
        models::{
            input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
            sequence::target_quality::{
                TargetQualityConfig,
                types::{ProbeStatistic, ProbeStrategy, SubsetProbeLength, SubsetProbePosition},
            },
        },
    };
    use california_condor::{
        commands::{
            QualityProfile,
            config::{ConfigAction, ProbeStrategyKind, StrategyArgs},
            handlers::{quality_check, target_quality},
        },
        configuration::{CliSequenceConfig, CliSequenceData, Configuration},
        test_helpers::{TestVideo, default_config},
    };

    fn fixture(temp: &std::path::Path) -> Configuration {
        let video = TestVideo {
            path: temp.join("must-not-open.mkv"),
            width: 32,
            height: 32,
            frames: 100,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 50), (50, 100)],
        };
        let mut configuration =
            default_config(&video, &temp.join("output.mkv"), &temp.join("scratch"));
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        configuration.condor.input = InputModel::Video {
            path: video.path,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(temp.join("forbidden.ffindex")),
            },
            filters: vec![],
        };
        configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig::default());
        configuration.condor.sequence_config.quality_check = Some(Default::default());
        configuration
    }

    fn runtime(
        configuration: &Configuration,
        saved: Arc<AtomicUsize>,
    ) -> Condor<CliSequenceData, CliSequenceConfig> {
        let seed = InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text("from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=32,height=32,length=2,format=YUV420P8).set_output()\n".into()),
        variables: Default::default(), index: 0, filters: vec![], stream_concurrently: false,
    };
        Condor::new(
            Input::from_data(&seed).expect("blank input"),
            Output::new(&configuration.condor.output).expect("output"),
            configuration.condor.encoder.clone(),
            configuration.condor.scenes.clone(),
            Some(configuration.condor.sequence_config.clone()),
            Box::new(move |_| {
                saved.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }),
        )
    }

    fn sequence(stage: &str) -> Box<dyn Sequence<CliSequenceData, CliSequenceConfig>> {
        if stage == "TQ" {
            Box::new(TargetQuality::new(None, None))
        } else {
            Box::new(QualityCheck::new(None))
        }
    }

    #[test]
    fn q01_real_standard_profiles_select_middle_quarter() {
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = fixture(temp.path());
        target_quality::configure_target_quality(
            &mut configuration,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&QualityProfile::Standard),
        )
        .expect("TQ profile");
        quality_check::configure_quality_check(
            &mut configuration,
            None,
            None,
            None,
            None,
            None,
            Some(&QualityProfile::Standard),
        )
        .expect("QC profile");
        let tq = configuration.condor.sequence_config.target_quality.as_ref().expect("TQ");
        let qc = configuration.condor.sequence_config.quality_check.as_ref().expect("QC");
        for strategy in [&tq.probing.strategy, &qc.strategy] {
            assert_eq!(
                strategy.frame_indices(0, 100).expect("selection"),
                (38..63).collect::<Vec<_>>()
            );
        }
        assert_eq!(tq.probing.statistic, ProbeStatistic::RootMeanSquare);
        assert_eq!(qc.statistic, ProbeStatistic::RootMeanSquare);
    }

    #[test]
    fn q01_failed_strategy_edits_are_offline_and_preserve_saved_bytes() {
        let temp = tempfile::tempdir().expect("fixture");
        let configuration = fixture(temp.path());
        let path = temp.path().join("condor.json");
        configuration.save(&path).expect("save");
        let before = std::fs::read(&path).expect("snapshot");
        let args = StrategyArgs {
            kind: ProbeStrategyKind::Subset,
            skip: None,
            position: None,
            length_frames: None,
            length_percent: None,
        };
        for args in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 101.0]
            .map(|percent| StrategyArgs {
                length_percent: Some(percent),
                ..args
            })
            .into_iter()
            .chain([
                StrategyArgs {
                    length_frames: Some(0),
                    ..args
                },
                StrategyArgs {
                    kind: ProbeStrategyKind::Skip,
                    skip: Some(0),
                    ..args
                },
            ])
        {
            assert!(args.build().is_err(), "accepted {args:?}");
            assert!(
                target_quality::config(
                    Some(&path),
                    &ConfigAction::Set {
                        setting: target_quality::TargetQualitySet::Strategy { args: args.clone() }
                    }
                )
                .is_err()
            );
            assert!(
                quality_check::config(
                    Some(&path),
                    &ConfigAction::Set {
                        setting: quality_check::QualityCheckSet::Strategy { args }
                    }
                )
                .is_err()
            );
            assert_eq!(std::fs::read(&path).expect("unchanged config"), before);
        }
        for percent in [f64::MIN_POSITIVE, 25.0, 100.0] {
            assert!(
                StrategyArgs {
                    length_percent: Some(percent),
                    ..args
                }
                .build()
                .is_ok()
            );
        }
        assert!(!temp.path().join("forbidden.ffindex").exists());
        assert!(!configuration.temp.exists());
    }

    #[test]
    fn q01_stages_reject_loaded_selection_before_work() {
        let temp = tempfile::tempdir().expect("fixture");
        for (invalid_strategy, invalid_end, expected_error) in [
            (
                Some(ProbeStrategy::Subset {
                    position: SubsetProbePosition::Start,
                    length: SubsetProbeLength::Percentage(0.0),
                }),
                None,
                "percent",
            ),
            (
                Some(ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length: SubsetProbeLength::Percentage(-1.0),
                }),
                None,
                "percent",
            ),
            (Some(ProbeStrategy::Skip { skip: 0 }), None, "skip"),
            (
                Some(ProbeStrategy::Subset {
                    position: SubsetProbePosition::End,
                    length: SubsetProbeLength::Frames(0),
                }),
                None,
                "frames",
            ),
            (None, Some(50), "range"),
            (None, Some(49), "range"),
        ] {
            for stage in ["TQ", "QC"] {
                for entry in ["validate", "initialize", "execute", "execute_cancel"] {
                    let mut configuration = fixture(temp.path());
                    if let Some(end) = invalid_end {
                        configuration.condor.scenes[1].end_frame = end;
                    }
                    if let Some(strategy) = &invalid_strategy {
                        configuration
                            .condor
                            .sequence_config
                            .target_quality
                            .as_mut()
                            .expect("TQ")
                            .probing
                            .strategy = strategy.clone();
                        configuration
                            .condor
                            .sequence_config
                            .quality_check
                            .as_mut()
                            .expect("QC")
                            .strategy = strategy.clone();
                    }
                    let bad_input = configuration.condor.input.clone();
                    configuration
                        .condor
                        .sequence_config
                        .target_quality
                        .as_mut()
                        .expect("TQ")
                        .input = Some(bad_input.clone());
                    configuration
                        .condor
                        .sequence_config
                        .quality_check
                        .as_mut()
                        .expect("QC")
                        .input = Some(bad_input);
                    let path = temp.path().join("condor.json");
                    // Simulate malformed hand-edited JSON, bypassing validated saves.
                    std::fs::write(&path, serde_json::to_vec(&configuration).expect("JSON"))
                        .expect("save loaded fixture");
                    let before = std::fs::read(&path).expect("snapshot");
                    let configuration =
                        Configuration::load(&path).expect("load").expect("saved fixture");
                    let saves = Arc::new(AtomicUsize::new(0));
                    let mut condor = runtime(&configuration, Arc::clone(&saves));
                    let mut sequence = sequence(stage);
                    let (tx, rx) = mpsc::channel();
                    let result = match entry {
                        "validate" => sequence.validate(&mut condor),
                        "initialize" => sequence.initialize(&mut condor, tx),
                        "execute" => {
                            sequence.execute(&mut condor, tx, Arc::new(AtomicBool::new(false)))
                        },
                        _ => sequence.execute_cancel(&mut condor, tx, Default::default()),
                    };
                    let error = result.expect_err("invalid selection must fail before work");
                    let message = error.to_string();
                    assert!(
                        message.contains(expected_error),
                        "{stage}/{entry}: {error:#}"
                    );
                    assert_eq!(saves.load(Ordering::Relaxed), 0);
                    assert_eq!(
                        serde_json::to_value(&condor.scenes).expect("scenes"),
                        serde_json::to_value(&configuration.condor.scenes)
                            .expect("original scenes")
                    );
                    assert_eq!(std::fs::read(&path).expect("unchanged metadata"), before);
                    assert!(rx.try_recv().is_err(), "no progress before validation");
                    assert!(!configuration.temp.exists());
                    assert!(!temp.path().join("forbidden.ffindex").exists());
                }
            }
        }
    }

    #[test]
    fn q01_stages_keep_no_work_and_cancellation_precedence() {
        use andean_condor::utils::cancellation::{Cancellation, Cancelled};
        let temp = tempfile::tempdir().expect("fixture");
        for stage in ["TQ", "QC"] {
            for mode in ["empty", "disabled", "graceful", "forced"] {
                let mut configuration = fixture(temp.path());
                configuration
                    .condor
                    .sequence_config
                    .target_quality
                    .as_mut()
                    .expect("TQ")
                    .probing
                    .strategy = ProbeStrategy::Skip { skip: 0 };
                configuration
                    .condor
                    .sequence_config
                    .quality_check
                    .as_mut()
                    .expect("QC")
                    .strategy = ProbeStrategy::Skip { skip: 0 };
                match mode {
                    "empty" => configuration.condor.scenes.clear(),
                    "disabled" => {
                        configuration.condor.sequence_config.target_quality = None;
                        configuration.condor.sequence_config.quality_check = None;
                    },
                    _ => (),
                }
                let saves = Arc::new(AtomicUsize::new(0));
                let mut condor = runtime(&configuration, Arc::clone(&saves));
                let mut sequence = sequence(stage);
                for entry in ["initialize", "execute", "execute_cancel"] {
                    let cancellation = Cancellation::default();
                    if matches!(mode, "graceful" | "forced") {
                        cancellation.request().expect("cancel");
                        if mode == "forced" {
                            cancellation.request().expect("force");
                        }
                    }
                    let (tx, rx) = mpsc::channel();
                    let result = match entry {
                        "initialize" => sequence.initialize_cancel(&mut condor, tx, &cancellation),
                        "execute" => sequence.execute(&mut condor, tx, cancellation.legacy_flag()),
                        _ => sequence.execute_cancel(&mut condor, tx, cancellation),
                    };
                    if matches!(mode, "graceful" | "forced") {
                        assert!(
                            result.expect_err("cancel before selection").is::<Cancelled>(),
                            "{stage}/{mode}/{entry}"
                        );
                    } else {
                        let (_, warnings) = result.expect("no work");
                        assert_eq!(warnings.len(), usize::from(mode == "empty"));
                    }
                    assert!(rx.try_recv().is_err());
                }
                assert_eq!(saves.load(Ordering::Relaxed), 0);
                assert!(!configuration.temp.exists());
                assert!(!temp.path().join("forbidden.ffindex").exists());
            }
        }
    }
}
