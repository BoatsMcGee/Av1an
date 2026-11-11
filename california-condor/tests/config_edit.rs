#[path = "common.rs"]
mod common;
#[path = "config_helpers.rs"]
mod config_helpers;

use andean_condor::models::sequence::{
    benchmarker::BenchmarkerConfig, target_quality::types::ProbeStatistic,
};
use california_condor::test_helpers::*;
use common::condor_cmd;
use config_helpers::{init_config, load};

#[cfg(test)]
mod tests {
    use andean_condor::models::input::Input;
    use serial_test::serial;

    use super::*;

    fn root_edit_fixture(
        directory: &std::path::Path,
    ) -> california_condor::configuration::Configuration {
        use andean_condor::models::input::VapourSynthScriptSource;
        let source = directory.join("a.vpy");
        for (name, color) in [("a.vpy", 16), ("b.vpy", 235)] {
            let marker = serde_json::to_string(&directory.join("source-opened"))
                .expect("Python path literal");
            std::fs::write(directory.join(name), format!("from pathlib import Path\nPath({marker}).write_text('opened')\nimport vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=16,length=12,format=vs.YUV420P8,color=[{color},128,128]).set_output()\n")).expect("equal-length script sources");
        }
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 16,
            frames: 12,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 12)],
        };
        let mut configuration = default_config(
            &video,
            &directory.join("out.mkv"),
            &directory.join(california_condor::utils::hash_path::hash_path(&source)),
        );
        configuration.condor.input = Input::VapourSynthScript {
            source: VapourSynthScriptSource::Path(source),
            variables: [("keep".into(), "yes".into())].into(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        };
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        let data = &mut configuration.condor.scenes[0].sequence_data;
        data.parallel_encoder.started_on = Some(10);
        data.parallel_encoder.completed_on = Some(20);
        data.parallel_encoder.bytes = Some(1234);
        data.quality_check.quality.scores = vec![91.5, 92.5];
        data.target_quality.passes.push(data.quality_check.quality.clone());
        configuration
    }

    fn root_edit_args(
        path: std::path::PathBuf,
    ) -> california_condor::commands::config::InputOverride {
        california_condor::commands::config::InputOverride {
            path: Some(path),
            decoder: None,
            filters: None,
            vs_args: None,
            stream_concurrently: None,
        }
    }

    #[test]
    fn c02_populated_root_direct_edits_refuse_before_mutation() {
        use andean_condor::models::input::ImportMethod;
        use california_condor::commands::{
            config::ConfigCommand,
            handlers::init::{InitCrud, InitSet},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = root_edit_fixture(temp.path());
        configuration.input = temp.path().join("a.mkv");
        configuration.condor.input = Input::Video {
            path: configuration.input.clone(),
            import_method: ImportMethod::FFMS2 {
                index: Some(1),
                cache_path: Some(temp.path().join("a.ffindex")),
            },
            filters: vec![],
        };
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save");
        let before = std::fs::read(&file).expect("snapshot");
        let memory = serde_json::to_value(&configuration).expect("snapshot");
        let chunk = configuration.temp.join("scenes/0.ivf");
        std::fs::create_dir_all(chunk.parent().expect("parent")).expect("workdir");
        std::fs::write(&chunk, b"old chunk").expect("chunk");
        let old_cache = temp.path().join("a.ffindex");
        let new_index = temp.path().join("b.ffindex");
        std::fs::write(&old_cache, b"old cache").expect("cache");
        std::fs::write(&new_index, b"new sentinel").expect("index");
        let replacement = temp.path().join("b.mkv");
        let error = InitCrud::apply(
            &mut configuration,
            &InitSet::Input {
                args: root_edit_args(replacement),
            },
        )
        .expect_err("populated root must refuse");
        let message = format!("{error:#}");
        assert!(
            message.contains("a.mkv")
                && message.contains("b.mkv")
                && message.contains("--config-file"),
            "{message}"
        );
        assert_eq!(serde_json::to_value(&configuration).expect("after"), memory);
        assert_eq!(std::fs::read(&file).expect("bytes"), before);
        assert_eq!(std::fs::read(chunk).expect("chunk"), b"old chunk");
        assert_eq!(std::fs::read(old_cache).expect("cache"), b"old cache");
        assert_eq!(std::fs::read(new_index).expect("index"), b"new sentinel");
        assert!(!temp.path().join("source-opened").exists());
    }

    #[test]
    fn c02_populated_root_cli_refuses_before_save_or_native_work() {
        let temp = tempfile::tempdir().expect("fixture");
        let configuration = root_edit_fixture(temp.path());
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save");
        let before = std::fs::read(&file).expect("snapshot");
        let replacement = temp.path().join("b.vpy");
        let chunk = configuration.temp.join("scenes/0.ivf");
        std::fs::create_dir_all(chunk.parent().expect("parent")).expect("workdir");
        std::fs::write(&chunk, b"CLI chunk").expect("chunk");
        let mut failures = vec![];
        for args in [
            vec!["init", "set", "input", "--path", path_str(&replacement)],
            vec![
                "--input",
                path_str(&replacement),
                "--temp",
                "forbidden-work",
                "--output",
                "forbidden.mkv",
                "--discard",
                "audio",
            ],
        ] {
            let output = condor_cmd(&temp).env("PATH", "").args(args).output().expect("CLI");
            let message = String::from_utf8_lossy(&output.stderr);
            if output.status.success()
                || !message.contains("a.vpy")
                || !message.contains("b.vpy")
                || !message.contains("--config-file")
            {
                failures.push(format!("missing source refusal: {message}"));
            }
            if std::fs::read(&file).expect("unchanged bytes") != before {
                failures.push("config bytes changed".into());
            }
            std::fs::write(&file, &before).expect("restore fixture");
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert!(!temp.path().join("forbidden-work").exists());
        assert!(!temp.path().join("source-opened").exists());
        assert_eq!(std::fs::read(chunk).expect("chunk"), b"CLI chunk");
    }

    #[test]
    fn c02_equivalent_paths_and_stale_duplicate_keep_work_and_cache() {
        use andean_condor::models::input::ImportMethod;
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = root_edit_fixture(temp.path());
        let source = temp.path().join("a.vpy");
        // A real source path is authoritative even when the duplicated metadata is stale.
        configuration.input = temp.path().join("stale.vpy");
        configuration.condor.input = Input::Video {
            path: source.clone(),
            import_method: ImportMethod::FFMS2 {
                index: Some(2),
                cache_path: Some(temp.path().join("keep.ffindex")),
            },
            filters: vec![],
        };
        let file = temp.path().join("condor.json");
        let chunk = configuration.temp.join("scenes/0.ivf");
        std::fs::create_dir_all(chunk.parent().expect("parent")).expect("workdir");
        std::fs::write(&chunk, b"keep chunk").expect("chunk");
        configuration.save(&file).expect("save");
        let mut paths = vec!["a.vpy", "./a.vpy", "sub/../a.vpy"];
        std::fs::create_dir_all(temp.path().join("sub")).expect("subdir");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&source, temp.path().join("alias.vpy")).expect("symlink");
            paths.push("alias.vpy");
        }
        for path in paths {
            condor_cmd(&temp)
                .env("PATH", "")
                .args(["init", "set", "input", "--path", path])
                .assert()
                .success();
            let after = load(&file);
            assert_eq!(after.input, source);
            assert_eq!(
                serde_json::to_value(&after.condor).expect("after"),
                serde_json::to_value(&configuration.condor).expect("before")
            );
            assert_eq!(after.temp, configuration.temp);
            assert_eq!(std::fs::read(&chunk).expect("chunk"), b"keep chunk");
        }
    }

    #[test]
    fn c02_same_source_vs_cache_belongs_to_its_decoder_kind() {
        use andean_condor::models::input::VapourSynthImportMethod;
        use california_condor::commands::{
            DecoderMethod,
            config::ConfigCommand,
            handlers::init::{InitCrud, InitSet},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("offline.mkv");
        let cache = temp.path().join("old.cache");
        let executable = temp.path().join("custom-dgindexnv");
        std::fs::write(&cache, b"old plugin cache").expect("cache");
        let methods = [
            (
                DecoderMethod::VSFFMS2,
                VapourSynthImportMethod::FFMS2 { index: Some(3) },
            ),
            (
                DecoderMethod::LSMASHWorks,
                VapourSynthImportMethod::LSMASHWorks { index: Some(3) },
            ),
            (
                DecoderMethod::BestSource,
                VapourSynthImportMethod::BestSource { index: Some(3) },
            ),
            (
                DecoderMethod::DGDecodeNV,
                VapourSynthImportMethod::DGDecNV {
                    dgindexnv_executable: Some(executable.clone()),
                },
            ),
        ];
        for (old_decoder, old_method) in &methods {
            for (new_decoder, _) in &methods {
                let mut configuration = root_edit_fixture(temp.path());
                configuration.input = source.clone();
                configuration.condor.input = Input::VapourSynth {
                    path: source.clone(),
                    import_method: old_method.clone(),
                    cache_path: Some(cache.clone()),
                    filters: vec![],
                };
                let mut expected = configuration.clone();
                let index = if *old_decoder == DecoderMethod::DGDecodeNV {
                    None
                } else {
                    Some(3)
                };
                let method = match new_decoder {
                    DecoderMethod::VSFFMS2 => VapourSynthImportMethod::FFMS2 { index },
                    DecoderMethod::LSMASHWorks => VapourSynthImportMethod::LSMASHWorks { index },
                    DecoderMethod::BestSource => VapourSynthImportMethod::BestSource { index },
                    DecoderMethod::DGDecodeNV => VapourSynthImportMethod::DGDecNV {
                        dgindexnv_executable: (*old_decoder == DecoderMethod::DGDecodeNV)
                            .then(|| executable.clone()),
                    },
                    DecoderMethod::FFMS2 => unreachable!(),
                };
                expected.condor.input = Input::VapourSynth {
                    path: source.clone(),
                    import_method: method,
                    cache_path: (old_decoder == new_decoder).then(|| cache.clone()),
                    filters: vec![],
                };
                let file = temp.path().join("condor.json");
                configuration.save(&file).expect("save");
                let bytes = std::fs::read(&file).expect("saved snapshot");
                let mut args = root_edit_args(source.clone());
                args.decoder = Some(new_decoder.clone());
                args.vs_args = Some(vec!["keep=yes".into()]);
                InitCrud::apply(&mut configuration, &InitSet::Input { args })
                    .expect("populated same-source edit");
                assert_eq!(
                    std::fs::read(&file).expect("direct edit must not save"),
                    bytes
                );
                condor_cmd(&temp)
                    .env("PATH", "")
                    .args([
                        "init",
                        "set",
                        "input",
                        "--path",
                        path_str(&source),
                        "--decoder",
                        &new_decoder.to_string(),
                        "--vs-args",
                        "keep=yes",
                    ])
                    .assert()
                    .success();
                let expected = serde_json::to_value(&expected).expect("expected");
                assert_eq!(
                    serde_json::to_value(&configuration).expect("direct"),
                    expected,
                    "{old_decoder} -> {new_decoder}"
                );
                assert_eq!(
                    serde_json::to_value(load(&file)).expect("CLI"),
                    expected,
                    "{old_decoder} -> {new_decoder}"
                );
                assert_eq!(
                    std::fs::read(&cache).expect("cache retained"),
                    b"old plugin cache"
                );
                assert!(!configuration.temp.exists());
                assert!(!temp.path().join("source-opened").exists());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn c02_alias_source_replacement_uses_fresh_init_lexical_hash() {
        use andean_condor::models::input::VapourSynthScriptSource;
        use california_condor::{
            commands::{
                config::ConfigCommand,
                handlers::init::{InitCrud, InitSet},
            },
            utils::hash_path::hash_path,
        };
        let temp = tempfile::tempdir().expect("fixture");
        let real = temp.path().join("real");
        std::fs::create_dir_all(&real).expect("real directory");
        std::os::unix::fs::symlink(&real, temp.path().join("alias")).expect("alias directory");
        let old_lexical = temp.path().join("alias/a.vpy");
        let old_canonical = std::fs::canonicalize(&real).expect("real path").join("a.vpy");
        let new_lexical = temp.path().join("alias/b.vpy");
        let expected_temp = temp.path().join(hash_path(&new_lexical));
        let mut failures = vec![];
        for (label, old_hash_path) in [("lexical", &old_lexical), ("canonical", &old_canonical)] {
            let mut configuration = root_edit_fixture(&real);
            configuration.condor.scenes.clear();
            configuration.input = "alias/a.vpy".into();
            if let Input::VapourSynthScript { source, .. } = &mut configuration.condor.input {
                *source = VapourSynthScriptSource::Path("alias/a.vpy".into());
            }
            configuration.temp = temp.path().join(hash_path(old_hash_path));
            configuration.condor.sequence_config.parallel_encoder.scenes_directory =
                configuration.temp.join("scenes");
            configuration.condor.sequence_config.scene_concatenator.scenes_directory =
                configuration.temp.join("scenes");
            let old_scenes = configuration.temp.join("scenes");
            std::fs::create_dir_all(&old_scenes).expect("empty old scenes");
            let mut expected = configuration.clone();
            expected.input = new_lexical.clone();
            expected.temp = expected_temp.clone();
            if let Input::VapourSynthScript { source, .. } = &mut expected.condor.input {
                *source = VapourSynthScriptSource::Path(new_lexical.clone());
            }
            expected.condor.sequence_config.parallel_encoder.scenes_directory =
                expected_temp.join("scenes");
            expected.condor.sequence_config.scene_concatenator.scenes_directory =
                expected_temp.join("scenes");
            let file = temp.path().join("condor.json");
            configuration.save(&file).expect("save");
            let bytes = std::fs::read(&file).expect("saved snapshot");
            let mut direct = configuration.clone();
            // Direct calls use absolute aliases; only the CLI child resolves relative paths.
            if let Input::VapourSynthScript { source, .. } = &mut direct.condor.input {
                *source = VapourSynthScriptSource::Path(old_lexical.clone());
            }
            InitCrud::apply(
                &mut direct,
                &InitSet::Input {
                    args: root_edit_args(new_lexical.clone()),
                },
            )
            .expect("direct alias replacement");
            assert_eq!(
                std::fs::read(&file).expect("direct edit must not save"),
                bytes
            );
            condor_cmd(&temp)
                .env("PATH", "")
                .args(["init", "set", "input", "--path", "alias/b.vpy"])
                .assert()
                .success();
            for (entry, after) in [("direct", direct), ("CLI", load(&file))] {
                if serde_json::to_value(&after).expect("actual")
                    != serde_json::to_value(&expected).expect("expected")
                {
                    failures.push(format!(
                        "{label}/{entry}: input={}, temp={} (wanted input={}, temp={})",
                        after.input.display(),
                        after.temp.display(),
                        new_lexical.display(),
                        expected_temp.display()
                    ));
                }
            }
            assert!(
                old_scenes.is_dir(),
                "do not delete the old empty work directory"
            );
            assert!(
                !expected_temp.exists(),
                "do not open/index/create work just to edit"
            );
            assert!(!real.join("source-opened").exists());
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn c02_empty_offline_replacement_resets_root_cache_and_rebases_only_derived_dirs() {
        use andean_condor::models::input::{
            Ffms2Filter, ImportMethod, VapourSynthImportMethod, VapourSynthScriptSource,
        };
        use california_condor::commands::{
            config::ConfigCommand,
            handlers::init::{InitCrud, InitSet},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("old.mkv");
        let replacement = temp.path().join("offline.mkv");
        std::fs::write(temp.path().join("old.ffindex"), b"unowned cache").expect("cache");
        let empty_work = temp.path().join("empty-work");
        std::fs::create_dir_all(&empty_work).expect("empty directory");
        for custom in [false, true] {
            for kind in ["native", "vs", "lsmash", "bestsource", "dg", "script"] {
                let mut configuration = root_edit_fixture(temp.path());
                configuration.condor.scenes.clear();
                configuration.input = source.clone();
                configuration.temp = temp.path().join(if custom {
                    "custom".into()
                } else {
                    california_condor::utils::hash_path::hash_path(&source)
                });
                configuration.condor.input = match kind {
                    "native" => Input::Video {
                        path: source.clone(),
                        import_method: ImportMethod::FFMS2 {
                            index: Some(3),
                            cache_path: Some(temp.path().join("old.ffindex")),
                        },
                        filters: vec![Ffms2Filter::OutputFormat {
                            bit_depth: Some(10),
                            chroma: None,
                            width: None,
                            height: None,
                        }],
                    },
                    "vs" | "lsmash" | "bestsource" | "dg" => Input::VapourSynth {
                        path: source.clone(),
                        import_method: match kind {
                            "lsmash" => VapourSynthImportMethod::LSMASHWorks { index: Some(3) },
                            "bestsource" => VapourSynthImportMethod::BestSource { index: Some(3) },
                            "dg" => VapourSynthImportMethod::DGDecNV {
                                dgindexnv_executable: Some(temp.path().join("custom-dgindexnv")),
                            },
                            _ => VapourSynthImportMethod::FFMS2 { index: Some(3) },
                        },
                        cache_path: Some(temp.path().join("old.cache")),
                        filters: vec![],
                    },
                    _ => Input::VapourSynthScript {
                        source: VapourSynthScriptSource::Path(source.with_extension("vpy")),
                        variables: [("keep".into(), "yes".into())].into(),
                        index: 3,
                        filters: vec![],
                        stream_concurrently: false,
                    },
                };
                if kind == "script" {
                    configuration.input = source.with_extension("vpy");
                    configuration.temp = temp.path().join(if custom {
                        "custom".into()
                    } else {
                        california_condor::utils::hash_path::hash_path(&configuration.input)
                    });
                }
                let sequence = &mut configuration.condor.sequence_config;
                sequence.parallel_encoder.workers = Some(5);
                sequence.parallel_encoder.max_tries = 7;
                sequence.parallel_encoder.scenes_directory = if kind == "native" {
                    empty_work.clone()
                } else {
                    Default::default()
                };
                sequence.scene_concatenator.scenes_directory = if kind == "native" {
                    temp.path().join("custom-concat")
                } else if kind == "script" {
                    configuration.temp.join("scenes")
                } else {
                    Default::default()
                };
                sequence.scene_concatenator.method =
                    andean_condor::models::sequence::scene_concatenator::ConcatMethod::Ivf;
                sequence.scene_concatenator.output = Some(temp.path().join("separate.ivf"));
                let stage = Input::Video {
                    path: temp.path().join("stage.mkv"),
                    import_method: ImportMethod::FFMS2 {
                        index: Some(1),
                        cache_path: Some(temp.path().join("stage.ffindex")),
                    },
                    filters: vec![],
                };
                sequence.parallel_encoder.input = Some(stage.clone());
                sequence.target_quality = Some(
                    andean_condor::models::sequence::target_quality::TargetQualityConfig {
                        metric_input: Some(stage),
                        ..Default::default()
                    },
                );
                let new_source = if kind == "script" {
                    replacement.with_extension("vpy")
                } else {
                    replacement.clone()
                };
                let original = configuration.clone();
                let file = temp.path().join("condor.json");
                configuration.save(&file).expect("save");
                let bytes = std::fs::read(&file).expect("saved snapshot");
                InitCrud::apply(
                    &mut configuration,
                    &InitSet::Input {
                        args: root_edit_args(new_source.clone()),
                    },
                )
                .expect("offline direct edit");
                assert_eq!(
                    std::fs::read(&file).expect("direct edit must not save"),
                    bytes
                );
                condor_cmd(&temp)
                    .env("PATH", "")
                    .args(["init", "set", "input", "--path", path_str(&new_source)])
                    .assert()
                    .success();
                let after = load(&file);
                assert_eq!(
                    serde_json::to_value(&after).expect("CLI"),
                    serde_json::to_value(&configuration).expect("direct")
                );
                assert_eq!(after.input, new_source);
                let expected_temp = if custom {
                    original.temp.clone()
                } else {
                    temp.path().join(california_condor::utils::hash_path::hash_path(&new_source))
                };
                assert_eq!(after.temp, expected_temp);
                let mut expected = original;
                expected.input = new_source.clone();
                expected.temp = expected_temp.clone();
                expected.condor.sequence_config.parallel_encoder.scenes_directory =
                    if kind == "native" {
                        empty_work.clone()
                    } else {
                        expected_temp.join("scenes")
                    };
                expected.condor.sequence_config.scene_concatenator.scenes_directory =
                    if kind == "native" {
                        temp.path().join("custom-concat")
                    } else {
                        expected_temp.join("scenes")
                    };
                match &mut expected.condor.input {
                    Input::Video {
                        path,
                        import_method: ImportMethod::FFMS2 { index, cache_path },
                        ..
                    } => {
                        *path = new_source;
                        *index = None;
                        *cache_path = None;
                    },
                    Input::VapourSynth {
                        path,
                        import_method,
                        cache_path,
                        ..
                    } => {
                        *path = new_source;
                        match import_method {
                            VapourSynthImportMethod::FFMS2 { index }
                            | VapourSynthImportMethod::LSMASHWorks { index }
                            | VapourSynthImportMethod::BestSource { index } => *index = None,
                            VapourSynthImportMethod::DGDecNV { .. } => {},
                        }
                        *cache_path = None;
                    },
                    Input::VapourSynthScript { source, index, .. } => {
                        *source = VapourSynthScriptSource::Path(new_source);
                        *index = 0;
                    },
                }
                assert_eq!(
                    serde_json::to_value(&after).expect("after"),
                    serde_json::to_value(&expected).expect("expected")
                );
                assert!(
                    !after.temp.exists(),
                    "pure edit must not create work directories"
                );
                assert!(!temp.path().join("source-opened").exists());
                assert!(empty_work.is_dir());
                assert_eq!(
                    std::fs::read(temp.path().join("old.ffindex")).expect("cache retained"),
                    b"unowned cache"
                );
            }
        }
    }

    #[test]
    fn c02_orphan_workdirs_and_errors_refuse_without_unlinking() {
        let temp = tempfile::tempdir().expect("fixture");
        for which in ["parallel", "concat", "not-directory"] {
            let mut configuration = root_edit_fixture(temp.path());
            configuration.condor.scenes.clear();
            let work = temp.path().join(which);
            if which == "not-directory" {
                std::fs::write(&work, b"not a directory").expect("file");
            } else {
                std::fs::create_dir_all(&work).expect("workdir");
                std::fs::write(work.join("orphan"), b"keep orphan").expect("orphan");
            }
            configuration.condor.sequence_config.parallel_encoder.scenes_directory =
                if which == "concat" {
                    Default::default()
                } else {
                    work.clone()
                };
            configuration.condor.sequence_config.scene_concatenator.scenes_directory =
                if which == "concat" {
                    work.clone()
                } else {
                    Default::default()
                };
            let file = temp.path().join("condor.json");
            configuration.save(&file).expect("save");
            let before = std::fs::read(&file).expect("before");
            for args in [
                vec!["init", "set", "input", "--path", "b.vpy"],
                vec!["--input", "b.vpy", "--temp", "new-work"],
            ] {
                let output = condor_cmd(&temp).env("PATH", "").args(args).output().expect("CLI");
                assert!(!output.status.success(), "{which} must refuse");
                let message = String::from_utf8_lossy(&output.stderr);
                assert!(message.contains(which), "context missing: {message}");
                assert_eq!(std::fs::read(&file).expect("after"), before);
            }
            assert!(work.exists());
            if which != "not-directory" {
                assert_eq!(
                    std::fs::read(work.join("orphan")).expect("orphan"),
                    b"keep orphan"
                );
            }
        }
    }

    #[test]
    fn c02_invalid_root_candidate_is_atomic_and_offline_equivalents_keep_inline_text() {
        use andean_condor::models::input::VapourSynthScriptSource;
        use california_condor::commands::{
            DecoderMethod,
            config::{ConfigCommand, Toggle},
            handlers::init::{InitCrud, InitSet},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = root_edit_fixture(temp.path());
        configuration.condor.scenes.clear();
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save");
        let before = serde_json::to_value(&configuration).expect("before");
        let bytes = std::fs::read(&file).expect("before");
        let mut args = root_edit_args(temp.path().join("offline.mkv"));
        args.decoder = Some(DecoderMethod::FFMS2);
        args.filters = Some(vec![
            andean_condor::vapoursynth::vapoursynth_filters::VapourSynthFilter::Resize {
                scaler: None,
                width: None,
                height: None,
                format: Some(andean_condor::ffmpeg::FFPixelFormat::NV12),
            },
        ]);
        args.stream_concurrently = Some(Toggle::On);
        InitCrud::apply(&mut configuration, &InitSet::Input { args })
            .expect_err("invalid native format");
        assert_eq!(serde_json::to_value(&configuration).expect("after"), before);
        assert_eq!(std::fs::read(&file).expect("after"), bytes);
        condor_cmd(&temp)
            .env("PATH", "")
            .args([
                "init",
                "set",
                "input",
                "--path",
                "offline.mkv",
                "--decoder",
                "ffms2",
                "--filters",
                "resize:format=nv12;",
                "--stream-concurrently",
                "on",
            ])
            .assert()
            .failure();
        assert_eq!(std::fs::read(&file).expect("CLI after"), bytes);
        assert!(!temp.path().join("source-opened").exists());
        configuration.input = temp.path().join("missing.vpy");
        if let Input::VapourSynthScript { source, .. } = &mut configuration.condor.input {
            *source = VapourSynthScriptSource::Text("inline source must survive".into());
        }
        configuration.condor.scenes = root_edit_fixture(temp.path()).condor.scenes;
        let before = serde_json::to_value(&configuration).expect("before");
        InitCrud::apply(
            &mut configuration,
            &InitSet::Input {
                args: root_edit_args(temp.path().join("absent/../missing.vpy")),
            },
        )
        .expect("offline lexical equivalent");
        assert_eq!(serde_json::to_value(&configuration).expect("after"), before);
        let mut args = root_edit_args(configuration.input.clone());
        args.vs_args = Some(vec!["keep=updated".into()]);
        args.stream_concurrently = Some(Toggle::On);
        InitCrud::apply(&mut configuration, &InitSet::Input { args })
            .expect("inline settings edit");
        assert!(
            matches!(&configuration.condor.input, Input::VapourSynthScript { source: VapourSynthScriptSource::Text(text), variables, stream_concurrently: true, .. } if text == "inline source must survive" && variables["keep"] == "updated")
        );
    }

    #[cfg(unix)]
    #[test]
    fn c02_normalization_errors_are_not_treated_as_missing_offline_paths() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("fixture");
        let configuration = root_edit_fixture(temp.path());
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save");
        let before = std::fs::read(&file).expect("before");
        std::os::unix::fs::symlink("loop.vpy", temp.path().join("loop.vpy")).expect("symlink loop");
        let output = condor_cmd(&temp)
            .env("PATH", "")
            .args(["init", "set", "input", "--path", "loop.vpy"])
            .output()
            .expect("CLI");
        assert!(!output.status.success());
        let message = String::from_utf8_lossy(&output.stderr);
        assert!(
            message.contains("normaliz") && message.contains("loop.vpy"),
            "{message}"
        );
        assert_eq!(std::fs::read(&file).expect("after"), before);
        assert!(!temp.path().join("source-opened").exists());
        let protected = temp.path().join("protected");
        std::fs::create_dir_all(&protected).expect("directory");
        std::fs::write(protected.join("source.vpy"), b"must not open").expect("source");
        for work_directory in [false, true] {
            let mut candidate = configuration.clone();
            candidate.condor.scenes.clear();
            if work_directory {
                candidate.condor.sequence_config.parallel_encoder.scenes_directory =
                    protected.clone();
            }
            candidate.save(&file).expect("save");
            let before = std::fs::read(&file).expect("before");
            std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o000))
                .expect("deny access");
            let replacement = if work_directory {
                "b.vpy"
            } else {
                "protected/source.vpy"
            };
            let output = condor_cmd(&temp)
                .env("PATH", "")
                .args(["init", "set", "input", "--path", replacement])
                .output();
            std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o700))
                .expect("restore access before assertions/cleanup");
            let output = output.expect("CLI");
            let message = String::from_utf8_lossy(&output.stderr);
            assert!(
                !output.status.success()
                    && message.contains("protected")
                    && message.contains("Permission denied"),
                "{message}"
            );
            assert_eq!(std::fs::read(&file).expect("after"), before);
        }
    }

    #[test]
    fn c02_empty_script_to_video_does_not_reuse_script_output_index() {
        use andean_condor::models::input::ImportMethod;
        use california_condor::commands::{
            DecoderMethod,
            config::ConfigCommand,
            handlers::init::{InitCrud, InitSet},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = root_edit_fixture(temp.path());
        configuration.condor.scenes.clear();
        if let Input::VapourSynthScript { index, .. } = &mut configuration.condor.input {
            *index = 7;
        }
        let mut args = root_edit_args(temp.path().join("offline.mkv"));
        args.decoder = Some(DecoderMethod::FFMS2);
        InitCrud::apply(&mut configuration, &InitSet::Input { args })
            .expect("empty cross-kind edit");
        assert!(
            matches!(
                configuration.condor.input,
                Input::Video {
                    import_method: ImportMethod::FFMS2 {
                        index: None,
                        cache_path: None
                    },
                    ..
                }
            ),
            "new source must not inherit output 7 or default script output 0 as a video track index"
        );
    }

    #[cfg(unix)]
    #[test]
    fn e06_cli_path_name_edits_are_offline_and_not_reported_as_missing_paths() {
        use california_condor::configuration::Configuration;
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("fixture");
        let script = temp.path().join("blank.vpy");
        std::fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=32,height=16,format=YUV420P8).set_output()\n").expect("script");
        let config = Configuration::new(
            &script,
            &temp.path().join("out.mkv"),
            Some(temp.path()),
            None,
            None,
        )
        .expect("config");
        let file = temp.path().join("condor.json");
        config.save(&file).expect("save");
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let executable = bin.join("renamed-encoder");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf unexpected > \"$E06_MARKER\"\n",
        )
        .expect("binary");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("permissions");
        let marker = temp.path().join("must-not-probe");
        let output = condor_cmd(&temp)
            .env("PATH", &bin)
            .env("E06_MARKER", &marker)
            .args(["encode", "set", "executable", "renamed-encoder"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("does not exist"),
            "valid PATH name: {stderr}"
        );
        let output = condor_cmd(&temp)
            .env("PATH", &bin)
            .args(["encode", "info", "executable"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output).expect("info"),
            serde_json::json!("renamed-encoder")
        );
        assert_eq!(
            load(&file).condor.encoder.executable(),
            std::path::Path::new("renamed-encoder")
        );
        assert!(
            !marker.exists(),
            "pure edits must not probe even an available binary"
        );
    }

    #[test]
    fn i06_cli_format_errors_leave_saved_config_unchanged_without_indexing() {
        use andean_condor::models::input::{Ffms2Filter, ImportMethod};
        use california_condor::configuration::Configuration;
        let temp = tempfile::tempdir().expect("CLI format fixture");
        let script = temp.path().join("blank.vpy");
        std::fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=32, height=16, format=YUV420P8).set_output()\n").expect("blank script");
        let mut config = Configuration::new(
            &script,
            &temp.path().join("out.mkv"),
            Some(temp.path()),
            None,
            None,
        )
        .expect("seed config");
        let source = temp.path().join("configuration-only.mkv");
        std::fs::write(&source, b"must not be decoded or indexed").expect("existing source path");
        config.condor.input = Input::Video {
            path: source,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(temp.path().join("must-not-exist.ffindex")),
            },
            filters: vec![Ffms2Filter::OutputFormat {
                bit_depth: Some(10),
                chroma: None,
                width: None,
                height: None,
            }],
        };
        let path = temp.path().join("condor.json");
        config.save(&path).expect("save native config");
        let native = std::fs::read(&path).expect("native snapshot");
        condor_cmd(&temp)
            .args(["init", "set", "input", "--decoder", "vs-ffms2"])
            .assert()
            .success();
        assert!(matches!(
            load(&path).condor.input.vapoursynth_filters(),
            [
                andean_condor::vapoursynth::vapoursynth_filters::VapourSynthFilter::OutputFormat {
                    bit_depth: Some(10),
                    chroma: None,
                    ..
                }
            ]
        ));
        let deferred = std::fs::read(&path).expect("deferred snapshot");
        for format in ["nv12", "gbrp", "yuv440p"] {
            std::fs::write(&path, &native).expect("restore native fixture");
            let filter = format!("resize:format={format};width=16;");
            let output = condor_cmd(&temp)
                .args(["init", "set", "input", "--filters", &filter])
                .assert()
                .failure()
                .get_output()
                .stderr
                .clone();
            assert!(
                String::from_utf8_lossy(&output).contains(format),
                "{output:?}"
            );
            assert_eq!(
                std::fs::read(&path).expect("native config after error"),
                native
            );
            std::fs::write(&path, &deferred).expect("restore VS fixture");
            condor_cmd(&temp)
                .args(["init", "set", "input", "--filters", &filter])
                .assert()
                .success();
            let explicit = std::fs::read(&path).expect("explicit VS format");
            let output = condor_cmd(&temp)
                .args(["init", "set", "input", "--decoder", "ffms2"])
                .assert()
                .failure()
                .get_output()
                .stderr
                .clone();
            assert!(
                String::from_utf8_lossy(&output).contains(format),
                "{output:?}"
            );
            assert_eq!(
                std::fs::read(&path).expect("VS config after error"),
                explicit
            );
        }
        assert!(!temp.path().join("must-not-exist.ffindex").exists());
        let mut entries = std::fs::read_dir(temp.path())
            .expect("source directory")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(
            entries,
            ["blank.vpy", "condor.json", "configuration-only.mkv", "logs"],
            "CLI logs are expected, source indexes are not"
        );
    }

    /// The noise scaler defaults stay consistent when nothing is configured
    /// yet, so `info` never fails on a fresh configuration.
    #[serial]
    #[test]
    fn info_works_on_a_fresh_configuration() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        init_config(&temp);

        for args in [
            vec!["detect-scenes", "info"],
            vec!["scale-noise", "info"],
            vec!["benchmark", "info"],
            vec!["optimize-bitrate", "info"],
            vec!["detect-noise", "info"],
            vec!["target-quality", "info"],
            vec!["scale-speed", "info"],
            vec!["concatenate", "info"],
            vec!["quality-check", "info"],
        ] {
            let output =
                condor_cmd(&temp).args(&args).assert().success().get_output().stdout.clone();
            serde_json::from_slice::<serde_json::Value>(&output)
                .unwrap_or_else(|err| panic!("{args:?} prints valid JSON: {err}"));
        }

        // Optional sections report themselves as null until they are set.
        let scaler = load(&temp.path().join("condor.json")).condor.sequence_config.noise_scaler;
        assert!(scaler.is_none(), "noise scaler is unset");
    }

    /// The whole section goes back to null, as the CLI contract promises for
    /// every optional section.
    #[serial]
    #[test]
    fn reset_section_nulls_optional_sections() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        // Nothing configured yet: resetting the section is a no-op, not an
        // error, and never materialises it.
        for command in ["detect-noise", "target-quality", "quality-check", "scale-noise"] {
            condor_cmd(&temp).args([command, "reset"]).assert().success();
        }

        condor_cmd(&temp)
            .args(["detect-noise", "set", "reference-filters", "resize:width=960;"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["target-quality", "set", "target", "80"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["quality-check", "set", "statistic", "mean"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["scale-noise", "set", "threshold", "0.002"])
            .assert()
            .success();

        let config = load(&config_path);
        assert!(
            config.condor.sequence_config.noise_detector.is_some(),
            "noise detector is materialized"
        );
        assert!(
            config.condor.sequence_config.target_quality.is_some(),
            "target quality is materialized"
        );
        assert!(
            config.condor.sequence_config.quality_check.is_some(),
            "quality check is materialized"
        );
        assert!(
            config.condor.sequence_config.noise_scaler.is_some(),
            "noise scaler is materialized"
        );

        for command in ["detect-noise", "target-quality", "quality-check", "scale-noise"] {
            condor_cmd(&temp).args([command, "reset"]).assert().success();
        }

        let config = load(&config_path);
        assert!(
            config.condor.sequence_config.noise_detector.is_none(),
            "noise detector is null again"
        );
        assert!(
            config.condor.sequence_config.target_quality.is_none(),
            "target quality is null again"
        );
        assert!(
            config.condor.sequence_config.quality_check.is_none(),
            "quality check is null again"
        );
        assert!(
            config.condor.sequence_config.noise_scaler.is_none(),
            "noise scaler is null again"
        );
    }

    /// A write reports the property it changed, which is enough to explain the
    /// run; the value it left behind is printed only under `--verbose`.
    #[serial]
    #[test]
    fn set_and_reset_report_what_changed() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        init_config(&temp);

        let plain = condor_cmd(&temp)
            .args(["benchmark", "set", "threshold", "8"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let plain = String::from_utf8(plain).expect("stdout is UTF-8");
        assert!(plain.contains("threshold updated"), "output is {plain}");
        assert!(
            !plain.contains('8'),
            "the value waits for --verbose: {plain}"
        );

        let verbose = condor_cmd(&temp)
            .args(["--verbose", "benchmark", "set", "threshold", "8"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let verbose = String::from_utf8(verbose).expect("stdout is UTF-8");
        assert!(
            verbose.contains("threshold updated") && verbose.contains('8'),
            "--verbose adds the value: {verbose}"
        );

        let plain = condor_cmd(&temp)
            .args(["benchmark", "reset", "threshold"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let plain = String::from_utf8(plain).expect("stdout is UTF-8");
        assert!(plain.contains("reset to its default"), "output is {plain}");
        assert!(
            !plain.contains(&BenchmarkerConfig::default().threshold.to_string()),
            "the restored value waits for --verbose: {plain}"
        );

        let verbose = condor_cmd(&temp)
            .args(["--verbose", "benchmark", "reset", "threshold"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let verbose = String::from_utf8(verbose).expect("stdout is UTF-8");
        assert!(
            verbose.contains(&BenchmarkerConfig::default().threshold.to_string()),
            "--verbose adds the restored default: {verbose}"
        );
    }

    /// `statistic` accepts positional values for `percentile` and
    /// `standard-deviation-distance`, and rejects giving both positional and
    /// flag forms at once.
    #[serial]
    #[test]
    fn statistic_accepts_positional_values() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args(["target-quality", "set", "statistic", "percentile", "90"])
            .assert()
            .success();
        let quality = load(&config_path)
            .condor
            .sequence_config
            .target_quality
            .expect("target quality exists");
        assert_eq!(
            quality.probing.statistic,
            ProbeStatistic::Percentile(90.0),
            "positional percentile is set"
        );

        condor_cmd(&temp)
            .args(["quality-check", "set", "statistic", "standard-deviation-distance", "-1.5"])
            .assert()
            .success();
        let check = load(&config_path)
            .condor
            .sequence_config
            .quality_check
            .expect("quality check exists");
        assert_eq!(
            check.statistic,
            ProbeStatistic::StandardDeviationDistance { sigma: -1.5 },
            "positional sigma is set"
        );

        // Passing both positional and flag is rejected.
        condor_cmd(&temp)
            .args(["target-quality", "set", "statistic", "percentile", "90", "--percentile", "10"])
            .assert()
            .failure();
    }

    /// `--stream-concurrently`, `encode set executable`, and missing-path
    /// warnings work as configured.
    #[serial]
    #[test]
    fn stream_concurrently_executable_and_missing_path_warnings() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);
        let script = temp.path().join("script.vpy");
        std::fs::write(
            &script,
            "import vapoursynth as vs\ncore = vs.core\nclip = core.std.BlankClip(width=16, \
             height=16, format=vs.YUV420P8)\nclip.set_output(0)\n",
        )
        .expect("script writes");

        condor_cmd(&temp)
            .args([
                "init",
                "set",
                "input",
                "--path",
                path_str(&script),
                "--stream-concurrently",
                "off",
            ])
            .assert()
            .success();
        let input = load(&config_path).condor.input;
        assert!(
            matches!(
                input,
                Input::VapourSynthScript {
                    stream_concurrently: false,
                    ..
                }
            ),
            "stream_concurrently is false: {input:?}"
        );

        // Setting executable on encode warns when the binary path does not
        // exist, and still saves the path; reset clears it.
        let missing_bin = temp.path().join("no-such-encoder");
        let stderr = condor_cmd(&temp)
            .args(["encode", "set", "executable", path_str(&missing_bin)])
            .assert()
            .success()
            .get_output()
            .stderr
            .clone();
        let stderr = String::from_utf8(stderr).expect("stderr is UTF-8");
        assert!(
            stderr.contains("does not exist"),
            "missing executable path warns: {stderr}"
        );
        assert_eq!(
            load(&config_path).condor.encoder.custom_executable(),
            Some(&missing_bin),
            "custom executable is saved"
        );

        condor_cmd(&temp).args(["encode", "reset", "executable"]).assert().success();
        assert_eq!(
            load(&config_path).condor.encoder.custom_executable(),
            None,
            "custom executable is cleared"
        );
    }
}
