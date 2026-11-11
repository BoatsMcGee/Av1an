#[path = "common.rs"]
mod common;

#[cfg(test)]
mod tests {
    use andean_condor::{
        core::input::{clip_info::ClipInfo, pixel_format::PixelFormat},
        models::{
            encoder::{Encoder, EncoderBase},
            input::{ImportMethod, Input},
        },
    };
    use california_condor::{
        commands::{EncoderMethod, handlers::encode::configure_encoder},
        configuration::Configuration,
    };

    use super::common::condor_cmd;

    fn clip_info() -> ClipInfo {
        ClipInfo {
            num_frames: 1,
            format_info: PixelFormat::VapourSynth { bit_depth: 8 },
            frame_rate: av_format::rational::Rational64::new(24, 1),
            resolution: (64, 64),
            color_range: None,
            color_primaries: None,
            color_transfer_characteristic: None,
            color_space: None,
            chroma_location: None,
            mastering_display: None,
            content_light_level: None,
        }
    }

    #[test]
    fn c01_consecutive_flags_override_default_crf_in_encoder_argv() {
        let temp = tempfile::tempdir().expect("argv paths");
        let output = temp.path().join("out.mkv");
        let stats = temp.path().join("stats");
        let mut encoder = Encoder::default_from_base(&EncoderBase::X265, false);
        encoder.set_append_color_data(false);
        let before = encoder
            .parameters_with_stats((1, 1), &output, &clip_info(), &stats)
            .expect("default argv");
        configure_encoder(
            &mut encoder,
            None,
            None,
            Some("--no-sao --crf 20"),
            None,
            None,
            None,
            None,
        )
        .expect("valid parameters");
        let argv = encoder
            .parameters_with_stats((1, 1), &output, &clip_info(), &stats)
            .expect("configured argv");
        assert_eq!(argv.len(), before.len() + 1, "{argv:?}");
        assert_eq!(argv.iter().filter(|arg| *arg == "--no-sao").count(), 1);
        assert_eq!(argv.iter().filter(|arg| *arg == "--crf").count(), 1);
        assert!(
            argv.windows(2).any(|pair| pair == ["--crf", "20"]),
            "{argv:?}"
        );
    }

    #[test]
    fn c01_configured_parameters_reach_encoder_argv_without_resplitting() {
        let temp = tempfile::tempdir().expect("argv paths");
        let output = temp.path().join("out.mkv");
        let stats = temp.path().join("stats");
        for (input, expected) in [
            (
                r#"--key 'C:\my clips\source.y4m'"#,
                vec![vec!["--key", r"C:\my clips\source.y4m"]],
            ),
            (
                "--no-sao --crf 20",
                vec![vec!["--no-sao"], vec!["--crf", "20"]],
            ),
            (
                "--sharpness -2 --crf 20",
                vec![vec!["--sharpness", "-2"], vec!["--crf", "20"]],
            ),
            ("-an -sn", vec![vec!["-an"], vec!["-sn"]]),
            ("--key=value", vec![vec!["--key=value"]]),
            ("--key=a=b", vec![vec!["--key=a=b"]]),
            (
                "--key\tvalue\n--other\tword",
                vec![vec!["--key", "value"], vec!["--other", "word"]],
            ),
            (r#"--key "a b""#, vec![vec!["--key", "a b"]]),
            (r#"--key="a b""#, vec![vec!["--key=a b"]]),
            (r#"--key "say \"hi\"""#, vec![vec!["--key", "say \"hi\""]]),
            (r#"--key """#, vec![vec!["--key", ""]]),
            ("--key=", vec![vec!["--key="]]),
            ("--key -0.5", vec![vec!["--key", "-0.5"]]),
            ("--key -1e-3", vec![vec!["--key", "-0.001"]]),
            ("--key=-literal", vec![vec!["--key=-literal"]]),
            ("crf=16 tune=0", vec![vec!["crf=16"], vec!["tune=0"]]),
            ("crf 16", vec![vec!["crf", "16"]]),
            ("-metadata:s:v 'a b'", vec![vec!["-metadata:s:v", "a b"]]),
            (
                r#"--key '$HOME;$(echo literal)*'"#,
                vec![vec!["--key", "$HOME;$(echo literal)*"]],
            ),
            (
                "--key value # comment\n--other=word",
                vec![vec!["--key", "value"], vec!["--other=word"]],
            ),
            ("", vec![]),
            (" \t\n ", vec![]),
            ("\r\n \t\u{b}", vec![]),
        ] {
            let mut encoder = Encoder::default_from_base(&EncoderBase::X265, false);
            encoder.parameters_mut().clear();
            configure_encoder(
                &mut encoder,
                None,
                None,
                Some(input),
                None,
                None,
                None,
                Some(false),
            )
            .expect("valid encoder parameters");
            let argv = encoder
                .parameters_with_stats((1, 1), &output, &clip_info(), &stats)
                .expect("actual emitted encoder argv");
            assert_eq!(
                argv.len(),
                3 + expected.iter().map(Vec::len).sum::<usize>(),
                "{input:?}: {argv:?}"
            );
            assert_eq!(argv[0], "-");
            assert!(
                argv.windows(2).any(|pair| pair == ["-o", output.to_str().expect("UTF-8")]),
                "{argv:?}"
            );
            for tokens in expected {
                assert!(
                    argv.windows(tokens.len()).any(|window| window == tokens),
                    "{input:?}: missing {tokens:?} in {argv:?}"
                );
            }
        }
    }

    #[test]
    fn c01_malformed_parameters_leave_configure_encoder_unchanged() {
        for input in [
            "--key \"unfinished",
            "--key value\\",
            "--crf 20 --crf 21",
            "--ok=1 --",
            "--ok=1 trailing",
        ] {
            let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
            let before = encoder.clone();
            let error = configure_encoder(
                &mut encoder,
                Some(&EncoderMethod::X265),
                Some(1),
                Some(input),
                None,
                None,
                Some("hflip"),
                Some(false),
            )
            .expect_err("malformed parameters must not commit any edits");
            assert!(
                error.to_string().contains("Invalid parameter"),
                "{input:?}: {error:#}"
            );
            assert_eq!(encoder, before, "{input:?}");
        }
    }

    #[test]
    fn c01_cli_rejection_preserves_disk_and_valid_edits_preserve_argv() {
        let temp = tempfile::tempdir().expect("CLI fixture");
        let script = temp.path().join("blank.vpy");
        std::fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,format=YUV420P8).set_output()\n").expect("script");
        let output = temp.path().join("out.mkv");
        let mut config =
            Configuration::new(&script, &output, Some(temp.path()), None, None).expect("config");
        let source = temp.path().join("must-not-open.mkv");
        std::fs::write(&source, b"not a video").expect("inert input");
        config.condor.input = Input::Video {
            path: source,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(temp.path().join("forbidden.ffindex")),
            },
            filters: vec![],
        };
        config.condor.encoder = Encoder::default_from_base(&EncoderBase::X265, false);
        config.condor.encoder.set_append_color_data(false);
        let path = temp.path().join("condor.json");
        config.save(&path).expect("save");
        let before = std::fs::read(&path).expect("snapshot");
        for input in [
            "--key \"unfinished",
            "--key value\\",
            "--crf 20 --crf 21",
            "--ok=1 --",
            "--ok=1 trailing",
        ] {
            for mut args in [
                vec!["encode", "--params"],
                vec!["encode", "set", "params"],
                vec!["target-quality", "set", "params"],
            ] {
                args.push(input);
                let stderr = condor_cmd(&temp)
                    .args(&args)
                    .timeout(std::time::Duration::from_secs(10))
                    .assert()
                    .failure()
                    .get_output()
                    .stderr
                    .clone();
                assert!(
                    String::from_utf8_lossy(&stderr).contains("Invalid parameter"),
                    "{args:?}: {}",
                    String::from_utf8_lossy(&stderr)
                );
                assert_eq!(
                    std::fs::read(&path).expect("unchanged config"),
                    before,
                    "{args:?}"
                );
                assert!(!temp.path().join("forbidden.ffindex").exists());
                assert!(!output.exists());
            }
        }
        condor_cmd(&temp)
            .args(["encode", "set", "params", r#"--no-sao --crf 20 --analysis-save 'a b.dat'"#])
            .assert()
            .success();
        let loaded = Configuration::load(&path).expect("load").expect("persisted config");
        let argv = loaded
            .condor
            .encoder
            .parameters_with_stats((1, 1), &output, &clip_info(), &temp.path().join("stats"))
            .expect("persisted emitted argv");
        assert!(argv.iter().any(|arg| arg == "--no-sao"), "{argv:?}");
        assert!(
            argv.windows(2).any(|pair| pair == ["--crf", "20"]),
            "{argv:?}"
        );
        assert!(
            argv.windows(2).any(|pair| pair == ["--analysis-save", "a b.dat"]),
            "{argv:?}"
        );
        assert_eq!(
            loaded.condor.encoder.parameters().len(),
            config.condor.encoder.parameters().len() + 2
        );
        assert_eq!(
            argv.len(),
            config
                .condor
                .encoder
                .parameters_with_stats((1, 1), &output, &clip_info(), &temp.path().join("stats"))
                .expect("default argv")
                .len()
                + 3
        );
        assert!(!temp.path().join("forbidden.ffindex").exists());
        assert!(!output.exists());
    }
}
