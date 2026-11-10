use super::{e02_tests::run_bounded, *};
use std::fs;

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
