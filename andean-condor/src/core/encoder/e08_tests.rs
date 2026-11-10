use super::*;
use std::fs;

fn bytes(value: &StringOrBytes) -> &[u8] {
    match value {
        StringOrBytes::String(s) => s.as_bytes(),
        StringOrBytes::Bytes(b) => b,
    }
}

fn diagnostics(result: &EncoderResult, dropped_after_first: bool) {
    let mut stdout = b"POC: 0\r\n".to_vec();
    if dropped_after_first {
        stdout.extend_from_slice(b"POC: 1\r\n");
    }
    stdout.extend(vec![b'o'; 512 * 1024]);
    stdout.extend_from_slice(b"\r\nbad\xff\rstdout tail");
    let mut stderr = b"1/4 frames\r\n".to_vec();
    if dropped_after_first {
        stderr.extend_from_slice(b"2/4 frames\r\n");
    }
    stderr.extend(vec![b'e'; 512 * 1024]);
    stderr.extend_from_slice(b"\r\nbad\xfe\rstderr tail");
    assert_eq!(bytes(&result.stdout).len(), stdout.len(), "stdout length");
    assert_eq!(bytes(&result.stderr).len(), stderr.len(), "stderr length");
    assert_eq!(bytes(&result.stdout), stdout);
    assert_eq!(bytes(&result.stderr), stderr);
}

#[test]
fn e08_bounded_runtime() {
    bounded_runtime(&[
        "stream",
        "avm",
        "drop-after-first",
        "avm-drop-after-first",
        "input",
        "input-drop-after-first",
        "decode",
        "exit0",
        "exit42",
        "input-exit0",
        "input-exit42",
        "serial0",
        "serialfew",
        "serialmany",
        "serialmultipass",
        "serialdecode",
        "serialencode",
        "watchdog",
    ]);
}

#[test]
fn e26_vvenc_bounded_public_input_and_stream_progress() {
    bounded_runtime(&["vvenc-input", "vvenc-stream"]);
}

fn bounded_runtime(cases: &[&str]) {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    fs::write(root.join("fake.rs"), include_str!("e08_fixture.rs")).expect("fixture source");
    e02_tests::run_bounded(
        Command::new("rustc").arg(root.join("fake.rs")).arg("-o").arg(root.join("fake")),
        &root.join("rustc.log"),
    );
    let mut failed = Vec::new();
    for &case in cases {
        let log_path = root.join(format!("{case}.log"));
        let file = fs::File::create(&log_path).expect("log");
        let status = Command::new(root.join("fake"))
            .arg("--e08-watchdog")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(root)
            .arg(case)
            .stdout(file.try_clone().expect("clone log"))
            .stderr(file)
            .status()
            .expect("bounded child");
        let log = fs::read_to_string(log_path).expect("child log");
        if case == "watchdog" {
            assert_eq!(
                status.code(),
                Some(124),
                "watchdog must bound a stuck process tree"
            );
            assert!(log.contains("reaped 1 descendants; timeout=true"), "{log}");
        } else if !status.success()
            || !log
                .lines()
                .any(|line| line == "E08 watchdog reaped 0 descendants; timeout=false")
        {
            failed.push(format!("{case}: {status}\n{log}"));
        } else if let Some(expected) = match case {
            "serial0" => Some(1),
            "serialfew" => Some(4),
            "serialmany" => Some(9),
            "serialmultipass" => Some(18),
            _ => None,
        } {
            let actual = log.lines().filter(|s| s.starts_with("Progress: ")).count();
            if actual != expected {
                failed.push(format!(
                    "{case}: report count {actual}, expected {expected}"
                ));
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn e08_runtime_child() {
    let Some(root) = std::env::var_os("E08_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let case = std::env::var("E08_CASE").expect("case");
    if case.starts_with("vvenc-") {
        vvenc_case(&root, &case);
        return;
    }
    if case == "watchdog" {
        let mut descendant = Command::new(root.join("fake"))
            .arg("--e08-stall")
            .spawn()
            .expect("stalled descendant");
        fs::write(root.join("watchdog-started"), "descendant started").expect("watchdog handshake");
        descendant.wait().expect("stalled descendant");
        panic!("watchdog did not stop the fixture tree");
    }
    let mut encoder = Encoder::default_from_base(
        &if case.starts_with("avm") {
            EncoderBase::AVM
        } else {
            EncoderBase::X264
        },
        true,
    );
    encoder.set_executable(Some(root.join("fake")));
    encoder.set_append_color_data(false);
    if case.starts_with("serial") {
        serial_case(&root, &case, encoder);
        return;
    }
    let (progress, rx) = mpsc::channel::<EncodeProgress>();
    let dropped_after_first = case.contains("drop-after-first");
    let observer = if dropped_after_first {
        let marker = root.join(format!("{case}.dropped"));
        encoder.parameters_mut().insert(
            "e08-drop-marker".into(),
            CLIParameter::new_string("--", " ", marker.to_str().expect("utf8")),
        );
        Some(thread::spawn(move || {
            assert_eq!(rx.recv().expect("first progress").frame, 1);
            drop(rx);
            fs::write(marker, "observer dropped").expect("drop handshake");
        }))
    } else {
        drop(rx);
        None
    };
    let output = root.join(format!("{case}.264"));
    let early = case.contains("exit");
    if early {
        encoder.parameters_mut().insert(
            "e08-exit".into(),
            CLIParameter::new_string("--", " ", case.rsplit("exit").next().expect("exit code")),
        );
    }
    let result = if case.starts_with("input") || case == "decode" {
        let script = root.join(format!("{case}.vpy"));
        let side = if early { 4096 } else { 64 };
        fs::write(&script, format!("from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width={side},height={side},length=1,format=YUV420P8).set_output()\n")).expect("blank clip");
        let mut input = Input::from_data(&crate::models::input::Input::VapourSynthScript {
            source: crate::models::input::VapourSynthScriptSource::Path(script),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
        .expect("input");
        if case == "decode" {
            *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
            encoder.parameters_mut().insert(
                "e08-marker".into(),
                CLIParameter::new_string("--", " ", root.join("reaped").to_str().expect("utf8")),
            );
        }
        encoder.encode_with_input(
            &mut input,
            &output,
            0,
            if case == "decode" { 2 } else { 1 },
            progress,
        )
    } else {
        let (tx, frames) = crossbeam_channel::unbounded();
        tx.send(Cursor::new(vec![
            0;
            if early { 16 * 1024 * 1024 } else { 64 }
        ]))
        .expect("finite frames");
        drop(tx);
        if dropped_after_first {
            encoder.encode_pass_with_stream(
                (1, 1),
                frames,
                &output,
                progress,
                &tests::clip_info(),
                None,
            )
        } else {
            encoder.encode_with_stream(frames, &output, progress, &tests::clip_info())
        }
    };
    if case == "decode" || early {
        let error = result.expect_err("decode/writer failure must propagate after joining");
        let diagnostic = error
            .chain()
            .find_map(|e| e.downcast_ref::<EncoderResult>())
            .expect("recoverable complete encoder diagnostics");
        diagnostics(diagnostic, dropped_after_first);
        if early {
            assert!(format!("{error:#}").contains("STDIN"), "{error:#}");
            assert_eq!(
                diagnostic.status.code(),
                Some(if case.ends_with('0') { 0 } else { 42 })
            );
        } else {
            assert_eq!(
                fs::read_to_string(root.join("reaped")).expect("child reaped before return"),
                "stats_alive=true"
            );
            assert!(!fs::read_dir(&root).expect("listing").any(|e| {
                e.expect("entry").file_name().to_string_lossy().starts_with(".av1an-stats-")
            }));
        }
    } else {
        let result = result.unwrap_or_else(|error| {
            panic!(
                "optional observer cannot fail an encode: {}",
                error.to_string().lines().next().expect("error")
            )
        });
        assert!(result.status.success());
        diagnostics(&result, dropped_after_first);
    }
    if let Some(observer) = observer {
        observer.join().expect("observer joined");
    }
}

fn vvenc_case(root: &Path, case: &str) {
    let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, true);
    encoder.set_executable(Some(root.join("fake")));
    encoder.set_append_color_data(false);
    encoder
        .parameters_mut()
        .insert("e26-progress".into(), CLIParameter::new_bool("--", true));
    let (tx, rx) = mpsc::channel();
    let output = root.join(format!("{case}.266"));
    let input_path = case == "vvenc-input";
    let result = if input_path {
        let script = root.join("vvenc.vpy");
        fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,length=1,format=YUV420P8).set_output()\n").expect("finite input");
        let mut input = Input::from_data(&crate::models::input::Input::VapourSynthScript {
            source: crate::models::input::VapourSynthScriptSource::Path(script),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        }).expect("input");
        encoder.encode_with_input(&mut input, &output, 0, 1, tx)
    } else {
        let (frames, receiver) = crossbeam_channel::unbounded();
        frames.send(Cursor::new(vec![0; 64])).expect("finite stream");
        drop(frames);
        encoder.encode_with_stream(receiver, &output, tx, &tests::clip_info())
    }.expect("public VVenC encode");
    assert!(result.status.success());
    assert_eq!(
        rx.into_iter().map(|p| (p.pass, p.frame)).collect::<Vec<_>>(),
        if input_path {
            vec![((1, 1), 60), ((1, 1), 90), ((1, 1), 120)]
        } else {
            vec![((1, 1), 0), ((1, 1), 60), ((1, 1), 90), ((1, 1), 120)]
        },
        "only increasing stdout stats, with the existing stream start event"
    );
    let mut stdout = b"noise frame= 700\r\nPOC: 0\nvvenc [info]: stats:  frame= 60 fps= 2.9 avg_fps= 2.9 bitrate= 29173.77 kbps avg_bitrate= 29173.77 kbps elapsed= 0\rvvenc [info]: stats: frame=bad fps= 999\nbad\xff\r\nvvenc [info]: stats: 75.0%% frame= 90/120 fps= 2.9 avg_fps= 2.9 bitrate= 29173.77 kbps avg_bitrate= 29173.77 kbps elapsed= 1\nvvenc [info]: stats: frame=90\rvvenc [info]: stats: frame=30\n".to_vec();
    stdout.extend(vec![b'o'; 512 * 1024]);
    stdout.extend_from_slice(
        b"\nvvenc [info]: stats: frame=120 fps= 2.9\rvvenc [info]: stats: frame=120 fps= 2.9",
    );
    let mut stderr = b"diagnostic\xfe\r\nvvenc [info]: stats: frame=900 fps= 999\n".to_vec();
    stderr.extend(vec![b'e'; 512 * 1024]);
    stderr.extend_from_slice(b"\r\nstderr tail");
    assert_eq!(bytes(&result.stdout), stdout);
    assert_eq!(bytes(&result.stderr), stderr);
}

fn serial_case(root: &Path, case: &str, mut encoder: Encoder) {
    use crate::{
        core::{
            Condor,
            output::Output,
            sequence::{Sequence, serial_encoder::SerialEncoder},
        },
        models::{
            scene::Scene,
            sequence::{DefaultSequenceConfig, DefaultSequenceData},
        },
    };
    let progress_count = if case == "serial0" {
        0
    } else if case == "serialfew" {
        1
    } else {
        8
    };
    encoder.parameters_mut().insert(
        "e08-progress".into(),
        CLIParameter::new_number("--", " ", progress_count as f64),
    );
    let order = root.join(format!("{case}.order"));
    encoder.parameters_mut().insert(
        "e08-order".into(),
        CLIParameter::new_string("--", " ", order.to_str().expect("utf8")),
    );
    if case == "serialmultipass" {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    }
    if case == "serialencode" {
        encoder
            .parameters_mut()
            .insert("e08-exit".into(), CLIParameter::new_number("--", " ", 0.0));
    }
    let script = root.join(format!("{case}.vpy"));
    let side = if case == "serialencode" { 4096 } else { 64 };
    fs::write(&script, format!("from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width={side},height={side},length=4,format=YUV420P8).set_output()\n")).expect("serial input");
    let input = Input::from_data(&crate::models::input::Input::VapourSynthScript {
        source: crate::models::input::VapourSynthScriptSource::Path(script),
        variables: HashMap::new(),
        index: 0,
        filters: vec![],
        stream_concurrently: false,
    })
    .expect("serial input");
    let ranges = if case == "serialfew" {
        vec![(0, 2), (2, 4)]
    } else if case == "serialdecode" {
        vec![(0, 5)]
    } else if case == "serial0" {
        vec![(0, 4)]
    } else {
        vec![(0, 1)]
    };
    let scenes = ranges
        .into_iter()
        .map(|(start_frame, end_frame)| Scene {
            start_frame,
            end_frame,
            sub_scenes: None,
            encoder: encoder.clone(),
            sequence_data: DefaultSequenceData::default(),
        })
        .collect();
    let mut condor = Condor::<DefaultSequenceData, DefaultSequenceConfig>::new(
        input,
        Output {
            path: root.join("unused"),
            tags: HashMap::new(),
            video_tags: HashMap::new(),
        },
        encoder,
        scenes,
        None,
        Box::new(|_| Ok(())),
    );
    let dir = root.join(case);
    fs::create_dir_all(&dir).expect("scenes");
    let mut serial = SerialEncoder::new(&dir);
    // These cases test EOF/join ownership, not decoder-backed bitstream counts.
    serial.validate_frame_count = false;
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let result = serial.execute(
        &mut condor,
        tx,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    if matches!(case, "serialdecode" | "serialencode") {
        assert!(
            result.is_err(),
            "inner encode/decode failure must propagate"
        );
        // Only a fully joined encoder can have finished writing this record.
        assert!(
            order.exists(),
            "error path must join encoder and relay before returning"
        );
        assert_eq!(
            fs::read_dir(&dir).expect("failed scene listing").count(),
            0,
            "joined failed scenes leave neither final files nor private scratch"
        );
    } else {
        assert!(result.is_ok(), "serial encode succeeds");
        let order = fs::read_to_string(order).expect("scene execution records");
        for output in order.lines().filter(|line| *line != "/dev/null") {
            let output = Path::new(output);
            assert!(
                output
                    .parent()
                    .expect("private parent")
                    .file_name()
                    .expect("directory")
                    .to_string_lossy()
                    .starts_with(".av1an-serial-")
            );
            assert!(!output.exists(), "private path is gone after publication");
            assert!(
                dir.join(output.file_name().expect("final basename")).exists(),
                "published final name retained"
            );
        }
        let names: Vec<_> = order
            .lines()
            .map(|line| Path::new(line).file_name().expect("scene").to_str().expect("utf8"))
            .collect();
        assert_eq!(
            names,
            if case == "serialfew" {
                vec!["00000.264", "00001.264"]
            } else if case == "serialmultipass" {
                vec!["null", "00000.264"]
            } else {
                vec!["00000.264"]
            }
        );
    }
}
