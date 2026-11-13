use super::{e02_tests::run_bounded, *};
use std::fs;

#[test]
fn e23_aom_default_keyframe_flag_respects_false_removal_and_excluded_defaults() {
    for mode in ["default", "false", "removed", "excluded"] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, mode == "excluded");
        match mode {
            "false" => {
                encoder
                    .parameters_mut()
                    .insert("disable-kf".into(), CLIParameter::new_bool("--", false));
            },
            "removed" => {
                encoder.parameters_mut().remove("disable-kf");
            },
            _ => {},
        }
        for pass in [(1, 1), (1, 2), (2, 2)] {
            *encoder.passes_mut().expect("passes") = EncoderPasses::All(pass.1);
            let before = encoder.clone();
            for argv in [
                encoder.parameters_with_output(pass, Path::new("out.ivf"), &tests::clip_info()),
                encoder.parameters_with_stats(
                    pass,
                    Path::new("out.ivf"),
                    &tests::clip_info(),
                    Path::new("stats"),
                ),
            ] {
                let argv = argv.expect("production argv");
                assert_eq!(
                    argv.iter().filter(|arg| arg.starts_with("--disable-kf")).count(),
                    usize::from(mode == "default"),
                    "{mode} {pass:?}: {argv:?}"
                );
                assert_eq!(
                    argv.contains(&"--kf-max-dist=9999".to_owned()),
                    mode != "excluded",
                    "{mode} {pass:?}: {argv:?}"
                );
            }
            assert_eq!(encoder, before, "argv must not override caller options");
        }
    }
}

#[test]
fn e23_installed_aom_scene_default_has_only_initial_decoded_keyframe() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    let aom = which::which("aomenc").expect("install aomenc to run the native regression");
    let ffprobe = which::which("ffprobe").expect("install ffprobe to verify decoded keyframes");
    let help = run_bounded(Command::new(&aom).arg("--help"), &root.join("help.log"));
    assert!(help.contains("--disable-kf"));
    eprintln!(
        "E23 installed {}: {}",
        aom.display(),
        help.lines()
            .find(|line| line.contains("AOMedia Project AV1 Encoder"))
            .expect("version")
    );
    let mut clip = tests::clip_info();
    clip.num_frames = 32;
    clip.resolution = (64, 64);
    clip.format_info = crate::core::input::pixel_format::PixelFormat::VapourSynth { bit_depth: 8 };
    let mut y4m = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
    // A hard cut from flat luma to a checkerboard must not split an already cut scene.
    for frame in 0..32 {
        y4m.extend_from_slice(b"FRAME\n");
        for y in 0..64 {
            for x in 0..64 {
                y4m.push(if frame < 16 {
                    32
                } else if (x / 8 + y / 8) % 2 == 0 {
                    64
                } else {
                    224
                });
            }
        }
        y4m.extend(vec![128; 64 * 64 / 2]);
    }
    let input = root.join("internal-cut.y4m");
    fs::write(&input, y4m).expect("finite Y4M");
    for user_enabled in [true, false] {
        let name = if user_enabled {
            "user-enabled"
        } else {
            "default"
        };
        let output = root.join(format!("{name}.ivf"));
        let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
        encoder.set_append_color_data(false);
        encoder
            .parameters_mut()
            .insert("ivf".into(), CLIParameter::new_bool("--", true));
        if user_enabled {
            encoder
                .parameters_mut()
                .insert("disable-kf".into(), CLIParameter::new_bool("--", false));
            encoder.parameters_mut().insert(
                "kf-max-dist".into(),
                CLIParameter::new_number("--", "=", 4.0),
            );
        }
        let argv = encoder.parameters_with_output((1, 1), &output, &clip).expect("production argv");
        eprintln!("E23 {name} production argv: {argv:?}");
        run_bounded(
            Command::new(&aom).args(&argv).stdin(fs::File::open(&input).expect("input")),
            &root.join(format!("{name}-encode.log")),
        );
        let probe = run_bounded(
            Command::new(&ffprobe)
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_frames",
                    "-show_entries",
                    "frame=key_frame",
                    "-of",
                    "csv=p=0",
                ])
                .arg(&output)
                .stdin(Stdio::null()),
            &root.join(format!("{name}-probe.log")),
        );
        let frames: Vec<u8> = probe
            .lines()
            .map(|line| line.parse().expect("decoded key_frame flag"))
            .collect();
        assert_eq!(frames.len(), 32, "{name}: {probe}");
        assert!(frames.iter().all(|flag| *flag <= 1), "{probe}");
        let keyframes: Vec<_> = frames
            .iter()
            .enumerate()
            .filter_map(|(index, flag)| (*flag == 1).then_some(index))
            .collect();
        eprintln!(
            "E23 {name}: {} decoded frames, keyframe indices {keyframes:?}",
            frames.len()
        );
        if user_enabled {
            assert_eq!(keyframes.first(), Some(&0));
            assert!(
                keyframes.len() > 1,
                "positive oracle must see additional keyframes"
            );
        } else {
            assert_eq!(
                keyframes,
                [0],
                "already cut scenes must not gain automatic keyframes"
            );
        }
    }
}

#[test]
fn e22_vpx_color_argv_respects_codec_metadata_and_native_overrides() {
    use av_decoders::ColorSpace;

    for codec in [Some("vp8"), Some("vp9"), None] {
        for append in [true, false] {
            for matrix in [Some(ColorSpace::BT709), None, Some(ColorSpace::Unspecified)] {
                for native_override in [false, true] {
                    let mut encoder = Encoder::default_from_base(&EncoderBase::VPX, false);
                    *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
                    encoder.set_append_color_data(append);
                    encoder.parameters_mut().remove("codec");
                    if let Some(codec) = codec {
                        encoder
                            .parameters_mut()
                            .insert("codec".into(), CLIParameter::new_string("--", "=", codec));
                    }
                    if native_override {
                        encoder.parameters_mut().insert(
                            "color-space".into(),
                            CLIParameter::new_string("--", "=", "smpte240"),
                        );
                    }
                    let before = encoder.clone();
                    let mut clip = tests::clip_info();
                    clip.color_space = matrix;
                    let expected = if native_override {
                        vec!["--color-space=smpte240"]
                    } else if append && codec != Some("vp8") && matrix == Some(ColorSpace::BT709) {
                        vec!["--color-space=bt709"]
                    } else {
                        vec![]
                    };
                    for argv in [
                        encoder.parameters_with_output((1, 1), Path::new("out.ivf"), &clip),
                        encoder.parameters_with_stats(
                            (1, 1),
                            Path::new("out.ivf"),
                            &clip,
                            Path::new("stats"),
                        ),
                    ] {
                        let argv = argv.expect("production argv");
                        let color: Vec<_> = argv
                            .iter()
                            .filter(|arg| arg.starts_with("--color-space"))
                            .map(String::as_str)
                            .collect();
                        assert_eq!(
                            color, expected,
                            "codec={codec:?} append={append} matrix={matrix:?} native={native_override}: {argv:?}"
                        );
                    }
                    assert_eq!(encoder, before, "argv must not sanitize caller options");
                }
            }
        }
    }
}

#[test]
fn e22_installed_vp8_and_vp9_pipe_outputs_decode_all_frames() {
    use av_decoders::ColorSpace;

    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    let vpx = which::which("vpxenc").expect("install vpxenc to run the native regression");
    let ffprobe = which::which("ffprobe").expect("install ffprobe to verify decoded frames");
    let help = run_bounded(Command::new(&vpx).arg("--help"), &root.join("help.log"));
    let (vp8_help, vp9_help) = help.split_once("VP9 Specific Options:").expect("VP9 options");
    assert!(!vp8_help.contains("--color-space"));
    assert!(vp9_help.contains("--color-space"));
    for version in help.lines().filter(|line| line.contains("WebM Project")) {
        eprintln!("E22 installed {}: {version}", vpx.display());
    }

    for codec in ["vp8", "vp9"] {
        let mut clip = tests::clip_info();
        clip.num_frames = 3;
        clip.resolution = (64, 64);
        clip.format_info =
            crate::core::input::pixel_format::PixelFormat::VapourSynth { bit_depth: 8 };
        clip.color_space = Some(ColorSpace::BT709);
        let mut y4m = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for frame in 0..3 {
            y4m.extend_from_slice(b"FRAME\n");
            y4m.extend(vec![32 + frame * 64; 64 * 64]);
            y4m.extend(vec![128; 64 * 64 / 2]);
        }
        let output = root.join(format!("{codec}.ivf"));
        // VPX defaults target VP9; select native-valid options, not a VP8 sanitizer.
        let mut encoder = Encoder::default_from_base(&EncoderBase::VPX, true);
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
        encoder.set_executable(Some(vpx.clone()));
        encoder
            .parameters_mut()
            .insert("codec".into(), CLIParameter::new_string("--", "=", codec));
        encoder
            .parameters_mut()
            .insert("ivf".into(), CLIParameter::new_bool("--", true));
        encoder
            .parameters_mut()
            .insert("cpu-used".into(), CLIParameter::new_number("--", "=", 6.0));
        let argv = encoder.parameters_with_output((1, 1), &output, &clip).expect("production argv");
        eprintln!("E22 {codec} production argv: {argv:?}");
        let (reader, mut writer) = std::io::pipe().expect("nonseekable stdin");
        let producer = thread::spawn(move || writer.write_all(&y4m));
        let log = run_bounded(
            Command::new(encoder.executable()).args(&argv).stdin(reader),
            &root.join(format!("{codec}-encode.log")),
        );
        producer.join().expect("producer thread").expect("finite pipe write");
        let bytes = fs::metadata(&output).expect("encoded output").len();
        assert!(bytes > 0);
        eprintln!("E22 {codec} native output: {bytes} bytes\n{log}");
        let probe = run_bounded(
            Command::new(&ffprobe)
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-count_frames",
                    "-show_entries",
                    "stream=codec_name,nb_read_frames,color_space",
                    "-of",
                    "default=noprint_wrappers=1",
                ])
                .arg(&output)
                .stdin(Stdio::null()),
            &root.join(format!("{codec}-probe.log")),
        );
        assert_eq!(
            probe.trim(),
            format!(
                "codec_name={codec}\ncolor_space={}\nnb_read_frames=3",
                if codec == "vp9" { "bt709" } else { "unknown" }
            )
        );
        eprintln!("E22 independently decoded {codec} output:\n{probe}");
    }
}

fn e19_noise() -> PhotonNoise {
    PhotonNoise {
        iso: 800,
        chroma_iso: Some(800),
        width: None,
        height: None,
        c_y: None,
        ccb: None,
        ccr: None,
    }
}

#[test]
fn e19_avm_generation_rejects_instead_of_silently_omitting_noise() {
    use crate::models::encoder::EncoderSettingsError;
    let mut encoder = Encoder::default_from_base(&EncoderBase::AVM, false);
    encoder.set_photon_noise(Some(e19_noise()));
    let before = encoder.clone();
    let error = encoder
        .generate_photon_noise_table(64, 64, TransferFunction::BT1886, None)
        .expect_err("AVM cannot use generated filmgrn1 tables");
    assert_eq!(
        error.downcast_ref::<EncoderSettingsError>(),
        Some(&EncoderSettingsError::PhotonNoiseNotSupported(
            EncoderBase::AVM
        ))
    );
    assert_eq!(encoder, before);
}

#[test]
fn e19_avm_explicit_pass_and_argv_reject_noise() {
    use crate::models::encoder::EncoderSettingsError;
    let mut encoder = Encoder::default_from_base(&EncoderBase::AVM, false);
    encoder.set_photon_noise(Some(e19_noise()));
    let before = encoder.clone();
    assert_eq!(
        encoder.validate_pass_settings((1, 1)),
        Err(EncoderSettingsError::PhotonNoiseNotSupported(
            EncoderBase::AVM
        ))
    );
    for result in [
        encoder.parameters_with_output((1, 1), Path::new("out.ivf"), &tests::clip_info()),
        encoder.parameters_with_stats(
            (1, 1),
            Path::new("out.ivf"),
            &tests::clip_info(),
            Path::new("stats"),
        ),
    ] {
        assert_eq!(
            result.expect_err("unsupported argv").downcast_ref::<EncoderSettingsError>(),
            Some(&EncoderSettingsError::PhotonNoiseNotSupported(
                EncoderBase::AVM
            ))
        );
    }
    assert_eq!(encoder, before);
}

#[test]
fn e19_avm_raw_native_table_remains_available_without_application_noise() {
    let mut encoder = Encoder::default_from_base(&EncoderBase::AVM, true);
    encoder.set_append_color_data(false);
    encoder.parameters_mut().insert(
        "film-grain-table".into(),
        CLIParameter::new_string("--", "=", "caller filmgrn2.tbl"),
    );
    assert_eq!(encoder.validate_settings(), Ok(()));
    let mut argv = encoder
        .parameters_with_output((1, 1), Path::new("out.ivf"), &tests::clip_info())
        .expect("raw native options are caller-owned");
    argv.sort();
    assert_eq!(
        argv,
        ["-", "--film-grain-table=caller filmgrn2.tbl", "-o", "out.ivf"]
    );
    encoder.set_photon_noise(Some(e19_noise()));
    let before = encoder.clone();
    let error = encoder
        .apply_photon_noise_parameters(Path::new("generated filmgrn1.tbl"))
        .expect_err("application table is unsupported");
    assert!(matches!(
        error.downcast_ref::<EncoderError>(),
        Some(EncoderError::PhotonNoiseNotSupported(EncoderBase::AVM))
    ));
    assert_eq!(
        encoder, before,
        "application must not replace a raw native option"
    );
    encoder.set_photon_noise(None);
    assert_eq!(encoder.validate_settings(), Ok(()));
}

#[test]
fn e19_avm_encode_preflight_precedes_lookup_receive_progress_and_output() {
    use crate::models::encoder::EncoderSettingsError;
    let temp = tempfile::tempdir().expect("preflight fixture");
    let output = temp.path().join("out.ivf");
    fs::write(&output, b"existing output").expect("sentinel");
    let mut encoder = Encoder::default_from_base(&EncoderBase::AVM, false);
    encoder.set_executable(Some(temp.path().join("missing-avmenc")));
    encoder.set_photon_noise(Some(e19_noise()));
    for direct_pass in [false, true] {
        let (frames_tx, frames_rx) = crossbeam_channel::unbounded();
        let queued = frames_rx.clone();
        frames_tx.send(Cursor::new(b"queued frame".to_vec())).expect("queue");
        let (progress_tx, progress_rx) = mpsc::channel();
        let result = if direct_pass {
            encoder.encode_pass_with_stream(
                (1, 1),
                frames_rx,
                &output,
                progress_tx,
                &tests::clip_info(),
                None,
            )
        } else {
            encoder.encode_with_stream(frames_rx, &output, progress_tx, &tests::clip_info())
        };
        let error = result.expect_err("unsupported settings must win over missing executable");
        assert_eq!(
            error.downcast_ref::<EncoderSettingsError>(),
            Some(&EncoderSettingsError::PhotonNoiseNotSupported(
                EncoderBase::AVM
            )),
            "{error:#}"
        );
        assert_eq!(
            queued.try_recv().expect("unconsumed input").into_inner(),
            b"queued frame"
        );
        assert!(progress_rx.try_iter().next().is_none());
        assert_eq!(
            fs::read(&output).expect("untouched output"),
            b"existing output"
        );
        assert_eq!(
            fs::read_dir(temp.path()).expect("no sidecars/statistics").count(),
            1
        );
    }
}

#[test]
fn e19_supported_backends_keep_generated_filmgrn1_tables_and_native_flags() {
    let temp = tempfile::tempdir().expect("table fixture");
    for (base, flag, delimiter) in [
        (EncoderBase::AOM, "film-grain-table", "="),
        (EncoderBase::RAV1E, "photon-noise-table", " "),
        (EncoderBase::SVTAV1, "fgs-table", " "),
    ] {
        assert!(base.supports_photon_noise());
        let mut encoder = Encoder::default_from_base(&base, false);
        encoder.set_photon_noise(Some(e19_noise()));
        assert_eq!(encoder.photon_noise(), Some(&e19_noise()));
        assert_eq!(encoder.validate_settings(), Ok(()));
        let (_, segment) = encoder
            .generate_photon_noise_table(64, 64, TransferFunction::BT1886, None)
            .expect("supported settings")
            .expect("generated table");
        let path = temp.path().join(format!("{base}.tbl"));
        av1_grain::write_grain_table(&path, &[segment]).expect("real table writer");
        let text = fs::read_to_string(&path).expect("generated table");
        assert_eq!(text.lines().next(), Some("filmgrn1"));
        let params = text.lines().find(|line| line.trim_start().starts_with("p ")).expect("p line");
        assert_eq!(
            params.split_whitespace().count(),
            13,
            "p plus 12 AV1 values"
        );
        encoder.apply_photon_noise_parameters(&path).expect("supported application");
        assert_eq!(
            encoder.parameters().get(flag),
            Some(&CLIParameter::new_string(
                "--",
                delimiter,
                path.to_str().expect("UTF-8")
            ))
        );
        let argv = encoder
            .parameters_with_output((1, 1), &temp.path().join("out.ivf"), &tests::clip_info())
            .expect("supported argv");
        if delimiter == "=" {
            assert!(argv.contains(&format!("--{flag}={}", path.display())));
        } else {
            assert!(
                argv.windows(2)
                    .any(|pair| pair == [format!("--{flag}"), path.display().to_string()])
            );
        }
    }
    assert!(
        !EncoderBase::AVM.supports_photon_noise(),
        "not a filmgrn1 backend"
    );
}

#[test]
fn e04_aom_exact_pass_parameters_and_argv() {
    // Omitting --passes=1 lets aomenc attempt to reread exhausted piped stdin.
    let output = Path::new("job space 映像/scene0.ivf");
    let base = std::env::current_dir().expect("cwd").join("job space 映像/scene0");
    let stats = format!("{}.log", base.display());
    let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, true);
    encoder.set_append_color_data(false);
    for pass in [(1, 2), (2, 2), (1, 1)] {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(pass.1);
        let mut expected = HashMap::from([(
            "passes".into(),
            CLIParameter::new_number("--", "=", f64::from(pass.1)),
        )]);
        if pass.1 == 2 {
            expected.extend([
                (
                    "pass".into(),
                    CLIParameter::new_number("--", "=", f64::from(pass.0)),
                ),
                ("fpf".into(), CLIParameter::new_string("--", "=", &stats)),
            ]);
        }
        assert_eq!(
            EncoderBase::AOM
                .pass_parameters(pass, base.to_str().expect("utf8"))
                .expect("params"),
            expected,
            "{pass:?}"
        );
        let argv = encoder.parameters_with_output(pass, output, &tests::clip_info()).expect("argv");
        assert_eq!(argv[0], "-");
        let mut actual = argv[1..].to_vec();
        let mut expected = vec![
            "-o".to_owned(),
            if pass.0 == pass.1 {
                output.to_str().expect("utf8")
            } else {
                NULL_OUTPUT
            }
            .to_owned(),
            format!("--passes={}", pass.1),
        ];
        if pass.1 == 2 {
            expected.extend([format!("--pass={}", pass.0), format!("--fpf={stats}")]);
        }
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected, "{pass:?}");
    }
}

#[test]
fn e04_avm_one_pass_control_has_no_pass_or_statistics_options() {
    // The pinned AVM CLI already defaults to one pass; do not copy the AOM fix.
    let output = Path::new("job space 映像/scene0.ivf");
    assert_eq!(
        EncoderBase::AVM.pass_parameters((1, 1), "unused").expect("params"),
        HashMap::new()
    );
    let mut encoder = Encoder::default_from_base(&EncoderBase::AVM, true);
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
    encoder.set_append_color_data(false);
    assert_eq!(
        encoder
            .parameters_with_output((1, 1), output, &tests::clip_info())
            .expect("argv"),
        ["-", "-o", output.to_str().expect("utf8")]
    );
}

#[test]
fn e04_installed_aom_one_pass_pipe_output_decodes_all_frames() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    let aom = which::which("aomenc").expect("install aomenc to run the native regression");
    let ffprobe = which::which("ffprobe").expect("install ffprobe to verify decoded frames");
    let help = run_bounded(Command::new(&aom).arg("--help"), &root.join("help.log"));
    assert!(help.contains("--passes=<arg>"), "{help}");
    eprintln!(
        "E04 installed {}: {}",
        aom.display(),
        help.lines()
            .find(|line| line.contains("AOMedia Project AV1 Encoder"))
            .expect("version")
    );

    let job = root.join("job space 映像");
    let cwd = root.join("cwd");
    fs::create_dir_all(&job).expect("job");
    fs::create_dir_all(&cwd).expect("cwd");
    fs::write(cwd.join("scene0.log"), "unrelated CWD sentinel").expect("sentinel");
    let mut y4m = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
    for frame in 0..3 {
        y4m.extend_from_slice(b"FRAME\n");
        y4m.extend(vec![32 + frame * 64; 64 * 64]);
        y4m.extend(vec![128; 64 * 64 / 2]);
    }
    let output = job.join("scene0.ivf");
    let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, true);
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
    encoder.set_append_color_data(false);
    encoder.parameters_mut().extend(CLIParameter::new_numbers(
        "--",
        "=",
        &[("cpu-used", 6.0), ("threads", 1.0)],
    ));
    let argv = encoder
        .parameters_with_output((1, 1), &output, &tests::clip_info())
        .expect("production argv");
    eprintln!("E04 one-pass argv: {argv:?}");
    // A real OS pipe cannot rewind. EOF follows this finite producer; the reused
    // watchdog kills/reaps a stalled encoder, releasing any blocked write.
    let (reader, mut writer) = std::io::pipe().expect("nonseekable stdin");
    let producer = thread::spawn(move || writer.write_all(&y4m));
    let log = run_bounded(
        Command::new(&aom).args(&argv).stdin(reader).current_dir(&cwd),
        &root.join("encode.log"),
    );
    producer.join().expect("producer thread").expect("finite pipe write");
    eprintln!("E04 nonseekable PIPE native log:\n{log}");
    let probe = run_bounded(
        Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-count_frames",
                "-show_entries",
                "stream=codec_name,nb_read_frames",
                "-of",
                "default=noprint_wrappers=1",
            ])
            .arg(&output)
            .stdin(Stdio::null()),
        &root.join("probe.log"),
    );
    assert_eq!(probe.trim(), "codec_name=av1\nnb_read_frames=3");
    eprintln!("E04 decoded final PIPE output: {probe}");
    assert_eq!(fs::read_dir(&job).expect("job listing").count(), 1);
    assert_eq!(fs::read_dir(&cwd).expect("cwd listing").count(), 1);
    assert_eq!(
        fs::read_to_string(cwd.join("scene0.log")).expect("sentinel"),
        "unrelated CWD sentinel"
    );
}
