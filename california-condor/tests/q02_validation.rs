#[path = "common.rs"]
mod common;

#[cfg(test)]
mod tests {
    use super::common;
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
            sequence::{Sequence, target_quality::TargetQuality},
        },
        models::{
            encoder::{Encoder, EncoderBase},
            input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
            sequence::target_quality::TargetQualityConfig,
        },
    };
    use california_condor::{
        commands::{
            config::ConfigAction,
            handlers::target_quality::{self, TargetQualityKey, TargetQualitySet},
        },
        configuration::{CliSequenceConfig, CliSequenceData, Configuration},
        test_helpers::{TestVideo, default_config},
        tui::run_target_quality_tui,
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
        configuration.condor.encoder.set_executable(Some(temp.join("must-not-run")));
        configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: Some(configuration.condor.input.clone()),
            ..Default::default()
        });
        configuration
    }

    fn blank_input() -> InputModel {
        InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text("from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=32,height=32,length=2,format=YUV420P8).set_output()\n".into()),
        variables: Default::default(), index: 0, filters: vec![], stream_concurrently: false,
        }
    }

    fn runtime(
        configuration: &Configuration,
        saved: Arc<AtomicUsize>,
    ) -> Condor<CliSequenceData, CliSequenceConfig> {
        Condor::new(
            Input::from_data(&blank_input()).expect("blank input"),
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

    #[test]
    fn q02_configure_rejects_default_rav1e_maximum_and_nonfinite_targets_atomically() {
        let temp = tempfile::tempdir().expect("fixture");
        for target in [75.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut configuration = fixture(temp.path());
            configuration.condor.encoder = Encoder::default_from_base(&EncoderBase::RAV1E, false);
            configuration.condor.sequence_config.target_quality = None;
            let before = serde_json::to_value(&configuration).expect("snapshot");
            let result = target_quality::configure_target_quality(
                &mut configuration,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(target),
                None,
                Some(if target == 75.0 { 40 } else { 140 }),
                None,
            );
            assert!(
                result.is_err(),
                "accepted target {target} and rav1e default minimum 50 / maximum 40"
            );
            assert_eq!(
                serde_json::to_value(&configuration).expect("unchanged"),
                before
            );
        }
    }

    #[test]
    fn q02_actual_flags_reject_before_save_or_input() {
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = fixture(temp.path());
        configuration.condor.encoder = Encoder::default_from_base(&EncoderBase::RAV1E, false);
        configuration.condor.sequence_config.target_quality = None;
        let path = temp.path().join("condor.json");
        configuration.save(&path).expect("save");
        let before = std::fs::read(&path).expect("snapshot");
        for args in [vec!["--target", "75", "--max-q", "40"], vec!["--target", "NaN"]] {
            let output = common::condor_cmd(&temp)
                .arg("target-quality")
                .args(args)
                .output()
                .expect("CLI");
            assert!(!output.status.success());
            let message = String::from_utf8_lossy(&output.stderr);
            assert!(
                message.contains("quantizer") || message.contains("finite"),
                "{message}"
            );
            assert_eq!(std::fs::read(&path).expect("unchanged bytes"), before);
        }
        assert!(!configuration.temp.exists());
        assert!(!temp.path().join("forbidden.ffindex").exists());
    }

    #[test]
    fn q02_library_and_tui_reject_loaded_config_before_backend_work() {
        let temp = tempfile::tempdir().expect("fixture");
        for invalid in
            ["range", "target", "probes", "scene", "options", "nan", "infinity", "endpoint"]
        {
            for entry in
                ["validate", "initialize", "execute", "execute_cancel", "tui", "instantiate"]
            {
                let mut configuration = fixture(temp.path());
                let quality =
                    configuration.condor.sequence_config.target_quality.as_mut().expect("TQ");
                let expected = match invalid {
                    "range" => {
                        quality.quantizer_range = (40, 20);
                        "quantizer"
                    },
                    "target" => {
                        *quality.metric.target_range_mut() = (90.0, 70.0);
                        "target"
                    },
                    "probes" => {
                        quality.maximum_probes = 0;
                        "probes"
                    },
                    "endpoint" => {
                        quality.quantizer_range = (71, 71);
                        "quantizer"
                    },
                    "nan" => {
                        quality.metric.target_range_mut().0 = f64::NAN;
                        "finite"
                    },
                    "infinity" => {
                        quality.metric.target_range_mut().1 = f64::INFINITY;
                        "finite"
                    },
                    "scene" => {
                        quality.quantizer_range = (50, 70);
                        configuration.condor.scenes[1].encoder =
                            Encoder::default_from_base(&EncoderBase::X264, false);
                        "scene 1"
                    },
                    _ => {
                        quality.probing.encoder_options = Some(
                        andean_condor::models::encoder::cli_parameter::CLIParameter::new_numbers(
                            "--",
                            " ",
                            &[("rc", 1.0)],
                        ),
                    );
                        "rc"
                    },
                };
                let path = temp.path().join("condor.json");
                // Simulate hand-edited JSON, not a validated save.
                let nonfinite = matches!(invalid, "nan" | "infinity");
                if nonfinite {
                    // JSON cannot represent nonfinite scores; public library callers can.
                    std::fs::write(&path, b"nonfinite library sentinel").expect("fixture");
                } else {
                    std::fs::write(&path, serde_json::to_vec(&configuration).expect("JSON"))
                        .expect("fixture");
                }
                let before = std::fs::read(&path).expect("snapshot");
                let configuration = if nonfinite {
                    configuration
                } else {
                    Configuration::load(&path).expect("load").expect("exists")
                };
                let saves = Arc::new(AtomicUsize::new(0));
                let mut condor = runtime(&configuration, Arc::clone(&saves));
                let mut sequence = TargetQuality::new(None, None);
                let (tx, rx) = mpsc::channel();
                let result = match entry {
                    "validate" => sequence.validate(&mut condor).map(|_| ()),
                    "initialize" => sequence.initialize(&mut condor, tx).map(|_| ()),
                    "execute" => sequence
                        .execute(&mut condor, tx, Arc::new(AtomicBool::new(false)))
                        .map(|_| ()),
                    "execute_cancel" => {
                        sequence.execute_cancel(&mut condor, tx, Default::default()).map(|_| ())
                    },
                    "tui" => run_target_quality_tui(&mut condor, Arc::new(AtomicBool::new(false))),
                    _ => configuration.instantiate_condor(Box::new(|_| Ok(()))).map(|_| ()),
                };
                let error = result.expect_err("invalid TQ rejected");
                assert!(
                    format!("{error:#}").contains(expected),
                    "{invalid}/{entry}: {error:#}"
                );
                assert_eq!(saves.load(Ordering::Relaxed), 0);
                assert_eq!(
                    serde_json::to_value(&condor.scenes).expect("scenes"),
                    serde_json::to_value(&configuration.condor.scenes).expect("before")
                );
                assert!(rx.try_recv().is_err());
                assert_eq!(std::fs::read(&path).expect("unchanged bytes"), before);
                assert!(!configuration.temp.exists());
                assert!(!temp.path().join("forbidden.ffindex").exists());
            }
        }
    }

    #[test]
    fn q02_loaded_edits_save_and_repairs_remain_offline() {
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = fixture(temp.path());
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("TQ")
            .quantizer_range = (40, 20);
        let path = temp.path().join("condor.json");
        std::fs::write(&path, serde_json::to_vec(&configuration).expect("JSON")).expect("fixture");
        let before = std::fs::read(&path).expect("snapshot");
        assert!(
            configuration.save(&path).is_err(),
            "invalid loaded section saved"
        );
        assert_eq!(std::fs::read(&path).expect("unchanged"), before);
        assert!(
            target_quality::config(
                Some(&path),
                &ConfigAction::Set {
                    setting: TargetQualitySet::MaximumProbes { value: 8 }
                }
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).expect("unchanged"), before);
        common::condor_cmd(&temp)
            .args(["quality-check", "set", "strategy", "whole"])
            .assert()
            .failure();
        assert_eq!(
            std::fs::read(&path).expect("unrelated CRUD cannot save invalid TQ"),
            before
        );
        target_quality::config(
            Some(&path),
            &ConfigAction::Set {
                setting: TargetQualitySet::QuantizerRange { range: (20, 20) },
            },
        )
        .expect("repair equal bounds");
        let repaired = Configuration::load(&path).expect("load").expect("exists");
        assert_eq!(
            repaired
                .condor
                .sequence_config
                .target_quality
                .as_ref()
                .expect("TQ")
                .quantizer_range,
            (20, 20)
        );
        assert_eq!(
            serde_json::to_value(&repaired.condor.scenes).expect("scenes"),
            serde_json::to_value(&configuration.condor.scenes).expect("original")
        );
        target_quality::config(
            Some(&path),
            &ConfigAction::Reset {
                key: Some(TargetQualityKey::QuantizerRange),
            },
        )
        .expect("reset range");
        std::fs::write(&path, &before).expect("invalid fixture");
        target_quality::target_quality_handler(
            Some(&path),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(20),
            Some(30),
            None,
        )
        .expect("explicit flags repair before validation");
        std::fs::write(&path, &before).expect("invalid fixture");
        target_quality::config(Some(&path), &ConfigAction::Reset { key: None })
            .expect("disable invalid TQ");
        assert!(
            Configuration::load(&path)
                .expect("load")
                .expect("exists")
                .condor
                .sequence_config
                .target_quality
                .is_none()
        );
        assert!(!configuration.temp.exists());
        assert!(!temp.path().join("forbidden.ffindex").exists());
    }

    #[test]
    fn q02_direct_crud_is_atomic_and_all_edits_validate_the_section() {
        use california_condor::commands::{
            QualityMetric,
            config::ConfigCommand,
            handlers::target_quality::{Interpolator, TargetQualityCrud},
        };
        let temp = tempfile::tempdir().expect("fixture");
        for set in [
            TargetQualitySet::MaximumProbes { value: 0 },
            TargetQualitySet::QuantizerRange { range: (71, 71) },
            TargetQualitySet::Target { value: f64::NAN },
            TargetQualitySet::Params {
                params: "--rc 1 --keyint 24".into(),
            },
        ] {
            let mut configuration = fixture(temp.path());
            let before = serde_json::to_value(&configuration).expect("snapshot");
            assert!(TargetQualityCrud::apply(&mut configuration, &set).is_err());
            assert_eq!(
                serde_json::to_value(&configuration).expect("unchanged"),
                before
            );
        }
        let mut configuration = fixture(temp.path());
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("TQ")
            .quantizer_range = (40, 20);
        let before = serde_json::to_value(&configuration).expect("snapshot");
        for set in [
            TargetQualitySet::Metric {
                metric: QualityMetric::XPSNR,
            },
            TargetQualitySet::Interpolators {
                first: Interpolator::Linear,
                second: Interpolator::Pchip,
            },
        ] {
            assert!(TargetQualityCrud::apply(&mut configuration, &set).is_err());
            assert_eq!(
                serde_json::to_value(&configuration).expect("unchanged"),
                before
            );
        }
        assert!(
            TargetQualityCrud::restore(&mut configuration, Some(&TargetQualityKey::MaximumProbes))
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(&configuration).expect("unchanged"),
            before
        );
        TargetQualityCrud::restore(&mut configuration, Some(&TargetQualityKey::QuantizerRange))
            .expect("reset invalid bounds");
        let quality = configuration.condor.sequence_config.target_quality.as_mut().expect("TQ");
        quality.maximum_probes = 0;
        TargetQualityCrud::restore(&mut configuration, Some(&TargetQualityKey::MaximumProbes))
            .expect("reset invalid probes");
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("TQ")
            .metric
            .target_range_mut()
            .0 = 100.0;
        TargetQualityCrud::apply(
            &mut configuration,
            &TargetQualitySet::Metric {
                metric: QualityMetric::XPSNR,
            },
        )
        .expect("metric repairs invalid target");
        assert!(!configuration.temp.exists());
    }

    #[test]
    fn q02_save_checks_each_effective_encoder_and_profile_can_repair_strategy() {
        use andean_condor::models::{
            encoder::cli_parameter::CLIParameter, sequence::target_quality::types::ProbeStrategy,
        };
        use california_condor::commands::QualityProfile;
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = fixture(temp.path());
        let path = temp.path().join("condor.json");
        configuration.save(&path).expect("valid initial save");
        let before = std::fs::read(&path).expect("snapshot");
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("TQ")
            .quantizer_range = (50, 70);
        configuration.condor.scenes[1].encoder =
            Encoder::default_from_base(&EncoderBase::X264, false);
        assert!(
            configuration.save(&path).is_err(),
            "scene endpoint, not just global encoder"
        );
        assert_eq!(std::fs::read(&path).expect("unchanged"), before);
        configuration.condor.scenes[1].encoder = configuration.condor.encoder.clone();
        configuration.condor.scenes[1]
            .encoder
            .parameters_mut()
            .insert("preset".into(), CLIParameter::new_number("--", " ", 99.0));
        assert!(
            configuration.save(&path).is_err(),
            "bad scene options without override"
        );
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("TQ")
            .probing
            .encoder_options = Some(EncoderBase::SVTAV1.default_parameters());
        configuration.save(&path).expect("actual probe override replaces scene options");
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("TQ")
            .probing
            .strategy = ProbeStrategy::Skip { skip: 0 };
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
        .expect("profile repairs strategy before validation");
        configuration.save(&path).expect("repaired save");
        assert!(!configuration.temp.exists());
        assert!(!temp.path().join("forbidden.ffindex").exists());
    }

    #[test]
    fn q02_tui_keeps_cancelled_and_no_work_precedence() {
        use andean_condor::utils::cancellation::Cancelled;
        let temp = tempfile::tempdir().expect("fixture");
        for mode in ["cancelled", "empty", "disabled"] {
            let mut configuration = fixture(temp.path());
            configuration
                .condor
                .sequence_config
                .target_quality
                .as_mut()
                .expect("TQ")
                .maximum_probes = 0;
            match mode {
                "empty" => configuration.condor.scenes.clear(),
                "disabled" => configuration.condor.sequence_config.target_quality = None,
                _ => (),
            }
            let saved = Arc::new(AtomicUsize::new(0));
            let mut condor = runtime(&configuration, Arc::clone(&saved));
            let result =
                run_target_quality_tui(&mut condor, Arc::new(AtomicBool::new(mode == "cancelled")));
            if mode == "cancelled" {
                assert!(result.expect_err("cancel first").is::<Cancelled>());
            } else {
                result.expect("no work");
            }
            assert_eq!(saved.load(Ordering::Relaxed), 0);
            assert!(!configuration.temp.exists());
            assert!(!temp.path().join("forbidden.ffindex").exists());
        }
    }

    #[test]
    fn q02_root_and_start_apply_repairs_before_final_validation() {
        use andean_condor::models::{
            encoder::cli_parameter::CLIParameter, sequence::target_quality::types::ProbeStrategy,
        };
        use california_condor::commands::{CondorCli, handlers::start::start_handler};
        use clap::Parser;

        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut failures = vec![];
        for (case, flags) in [
            (
                "range",
                vec!["--minimum-quantizer", "10", "--maximum-quantizer", "30"],
            ),
            (
                "encoder",
                vec![
                    "--encoder",
                    "svt-av1",
                    "--minimum-quantizer",
                    "20",
                    "--maximum-quantizer",
                    "50",
                ],
            ),
            ("options", vec!["--tq-params", "--preset 6 --rc 0"]),
            ("profile", vec!["--target-profile", "standard"]),
        ] {
            let mut configuration = fixture(temp.path());
            // Start reads SCD metadata even without SCD flags. A generated clip
            // satisfies that existing requirement without opening/indexing the source.
            configuration.condor.sequence_config.scene_detector.input = Some(blank_input());
            let quality = configuration.condor.sequence_config.target_quality.as_mut().expect("TQ");
            match case {
                "range" => quality.quantizer_range = (40, 20),
                "encoder" => {
                    quality.quantizer_range = (50, 140);
                    configuration.condor.encoder =
                        Encoder::default_from_base(&EncoderBase::RAV1E, false);
                    for scene in &mut configuration.condor.scenes {
                        scene.encoder = configuration.condor.encoder.clone();
                    }
                },
                "options" => {
                    quality.probing.encoder_options =
                        Some(CLIParameter::new_numbers("--", " ", &[("rc", 1.0)]));
                },
                _ => quality.probing.strategy = ProbeStrategy::Skip { skip: 0 },
            }
            let before = serde_json::to_vec(&configuration).expect("hand-edited JSON");
            for entry in ["root CLI", "start handler"] {
                std::fs::write(&path, &before).expect("fixture");
                if entry == "root CLI" {
                    let output = common::condor_cmd(&temp).args(&flags).output().expect("root CLI");
                    // The start handler must save the repaired candidate before the runner
                    // reaches the missing-input sentinel; no native encode is needed.
                    assert!(!output.status.success());
                    let message = String::from_utf8_lossy(&output.stderr);
                    if !message.contains("must-not-open.mkv") {
                        failures.push(format!("{case}/{entry}: {message}"));
                    }
                } else {
                    let cli = CondorCli::try_parse_from(
                        std::iter::once("condor").chain(flags.iter().copied()),
                    )
                    .expect("root flags");
                    let result = start_handler(
                        Some(&path),
                        cli.temp.as_deref(),
                        cli.input.as_deref(),
                        cli.scd_input.as_deref(),
                        cli.tq_input.as_deref(),
                        cli.output.as_deref(),
                        cli.decoder.as_ref(),
                        cli.scd_decoder.as_ref(),
                        cli.tq_decoder.as_ref(),
                        cli.filters.as_deref(),
                        cli.scd_filters.as_deref(),
                        cli.tq_filters.as_deref(),
                        cli.vs_args.as_deref(),
                        cli.scd_vs_args.as_deref(),
                        cli.tq_vs_args.as_deref(),
                        cli.concat.as_ref(),
                        cli.discard.as_deref(),
                        cli.workers,
                        cli.encoder.as_ref(),
                        cli.passes,
                        cli.params.as_deref(),
                        cli.tq_params.as_deref(),
                        cli.photon_noise,
                        cli.chroma_noise,
                        cli.ffmpeg_filter.as_deref(),
                        None,
                        cli.target_metric.as_ref(),
                        cli.target,
                        cli.minimum_quantizer,
                        cli.maximum_quantizer,
                        cli.target_profile.as_ref(),
                        cli.max_tries,
                    );
                    if let Err(error) = result {
                        failures.push(format!("{case}/{entry}: {error:#}"));
                    }
                }
                let repaired = Configuration::load(&path).expect("load").expect("exists");
                let quality = repaired.condor.sequence_config.target_quality.as_ref().expect("TQ");
                let expected_range = match case {
                    "range" => (10, 30),
                    "encoder" => (20, 50),
                    _ => (0, 51),
                };
                if quality.quantizer_range != expected_range
                    || quality.validate(&repaired.condor.encoder).is_err()
                {
                    failures.push(format!(
                        "{case}/{entry}: stale candidate saved: {quality:?}"
                    ));
                }
                if case == "encoder" && repaired.condor.encoder.base() != EncoderBase::SVTAV1 {
                    failures.push(format!("{case}/{entry}: actual encoder switch not saved"));
                }
                if case == "options" && quality.validate(&repaired.condor.encoder).is_ok() {
                    assert_eq!(quality.probing.encoder_options.as_ref().expect("explicit probe options")["preset"].to_string_value(), "6");
                }
                if case == "profile" && quality.validate(&repaired.condor.encoder).is_ok() {
                    assert_eq!(
                        quality
                            .probing
                            .strategy
                            .frame_indices(0, 100)
                            .expect("repaired Q01 selection"),
                        (38..63).collect::<Vec<_>>()
                    );
                }
                assert_eq!(
                    serde_json::to_value(&repaired.condor.scenes).expect("scenes"),
                    serde_json::to_value(&configuration.condor.scenes).expect("original scenes")
                );
                assert!(!configuration.temp.exists());
                assert!(!temp.path().join("forbidden.ffindex").exists());
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn q02_root_still_rejects_invalid_final_candidates_and_early_encoder_controls() {
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = fixture(temp.path());
        configuration.condor.sequence_config.scene_detector.input = Some(blank_input());
        let path = temp.path().join("condor.json");
        configuration.save(&path).expect("valid save");
        let before = std::fs::read(&path).expect("snapshot");
        for (flags, expected) in [
            (
                vec!["--minimum-quantizer", "40", "--maximum-quantizer", "20"],
                "quantizer",
            ),
            (vec!["--tq-params", "--rc 1 --keyint 24"], "rc"),
            (vec!["--target", "NaN"], "finite"),
            (vec!["--params", "--preset 99"], "preset"),
            (vec!["--max-tries", "0"], "max-tries"),
        ] {
            let output = common::condor_cmd(&temp).args(flags).output().expect("root CLI");
            assert!(!output.status.success());
            let message = String::from_utf8_lossy(&output.stderr);
            assert!(message.contains(expected), "{message}");
            assert_eq!(std::fs::read(&path).expect("unchanged bytes"), before);
            assert!(!configuration.temp.exists());
            assert!(!temp.path().join("forbidden.ffindex").exists());
        }
    }

    #[test]
    fn q02_empty_scene_public_validation_rejects_global_config_before_detection() {
        use andean_condor::{
            core::{AndeanCondor, DefaultAndeanCondor},
            models::sequence::{
                DefaultSequenceConfig, DefaultSequenceData, target_quality::types::ProbeStrategy,
            },
        };
        let temp = tempfile::tempdir().expect("fixture");
        let mut failures = vec![];
        for invalid in [
            "range", "target", "probes", "strategy", "endpoint", "options", "valid", "disabled",
        ] {
            let mut configuration = fixture(temp.path());
            configuration.condor.scenes.clear();
            configuration.condor.sequence_config.parallel_encoder.validate_frame_count = false;
            let quality = configuration.condor.sequence_config.target_quality.as_mut().expect("TQ");
            let expected = match invalid {
                "range" => {
                    quality.quantizer_range = (40, 20);
                    "quantizer"
                },
                "target" => {
                    quality.metric.target_range_mut().0 = f64::NAN;
                    "finite"
                },
                "probes" => {
                    quality.maximum_probes = 0;
                    "probes"
                },
                "strategy" => {
                    quality.probing.strategy = ProbeStrategy::Skip { skip: 0 };
                    "skip"
                },
                "endpoint" => {
                    quality.quantizer_range = (71, 71);
                    "quantizer"
                },
                "valid" => "",
                "disabled" => {
                    configuration.condor.sequence_config.target_quality = None;
                    ""
                },
                _ => {
                    quality.probing.encoder_options = Some(
                        andean_condor::models::encoder::cli_parameter::CLIParameter::new_numbers(
                            "--",
                            " ",
                            &[("rc", 1.0)],
                        ),
                    );
                    "rc"
                },
            };
            for entry in ["pure admission", "sequence validate", "Condor validate"] {
                let saved = Arc::new(AtomicUsize::new(0));
                let mut condor = runtime(&configuration, Arc::clone(&saved));
                let result = match entry {
                    "pure admission" => TargetQuality::validate_config(&condor),
                    "sequence validate" => {
                        TargetQuality::new(None, None).validate(&mut condor).map(|_| ())
                    },
                    _ => {
                        // The pipeline checks executable existence before its sequences;
                        // the current test binary satisfies that without any native probe.
                        condor
                            .encoder
                            .set_executable(Some(std::env::current_exe().expect("test binary")));
                        let config = DefaultSequenceConfig {
                            target_quality: condor.sequence_config.target_quality,
                            parallel_encoder: condor.sequence_config.parallel_encoder,
                            ..Default::default()
                        };
                        let condor: Condor<DefaultSequenceData, DefaultSequenceConfig> =
                            Condor::new(
                                condor.input,
                                condor.output,
                                condor.encoder,
                                vec![],
                                Some(config),
                                Box::new(|_| panic!("validation must not persist")),
                            );
                        let mut pipeline = DefaultAndeanCondor {
                            condor,
                            sequences: vec![Box::new(TargetQuality::new(None, None))],
                        };
                        let result = pipeline.validate().map(|_| ());
                        assert!(pipeline.condor.scenes.is_empty(), "no scene detection");
                        result
                    },
                };
                match result {
                    Ok(()) if expected.is_empty() => (),
                    Ok(()) => failures.push(format!(
                        "{invalid}/{entry}: accepted invalid global TQ before detection"
                    )),
                    Err(error) if expected.is_empty() => failures.push(format!(
                        "{invalid}/{entry}: rejected valid no-work config: {error:#}"
                    )),
                    Err(error) if !format!("{error:#}").contains(expected) => {
                        failures.push(format!("{invalid}/{entry}: {error:#}"));
                    },
                    Err(_) => (),
                }
                assert_eq!(saved.load(Ordering::Relaxed), 0);
                assert!(!configuration.temp.exists());
                assert!(!temp.path().join("forbidden.ffindex").exists());
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
