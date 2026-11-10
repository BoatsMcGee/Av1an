use super::*;
use crate::{
    core::{
        Condor,
        encoder::{EncoderResult, StringOrBytes},
        output::Output,
        sequence::{Sequence, parallel_encoder::ParallelEncoder, serial_encoder::SerialEncoder},
    },
    models::{
        input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
        scene::Scene,
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    },
};
use std::{collections::HashMap, os::unix::fs::PermissionsExt, process::Command};

// A clean nonzero exit after complete input must reach scene finalization, not
// bypass its stop flag and permits. Raw bytes also distinguish it from an I/O error.
const FAKE: &str = r#"#!/bin/sh
set -eu
argv=$(printf '%s\n' "$@")
pass=1 total=1 stats= records= scene= partial= out=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --pass) pass=$2; total=2; shift ;;
        --stats) stats=$2; shift ;;
        --e27-records) records=$2; shift ;;
        --e27-scene) scene=$2; shift ;;
        --e27-partial) partial=$2; shift ;;
        -o) out=$2; shift ;;
    esac
    shift
done
printf '%s:%s/%s\n' "$scene" "$pass" "$total" >> "$records/order"
printf '%s\n' "$argv" > "$records/argv"
if [ "$E08_CASE" = zone-worker ]; then
    exec 0<&-
else
    cat > "$records/input.y4m"
fi
case "$E08_CASE" in
    parallel-*)
        # Per-scene attempts own private paths; never fabricate a legacy caller temp.
        if [ "$out" = /dev/null ]; then partial="$stats.partial"; else partial=$out; fi ;;
esac
printf '%s' "$partial" > "$records/partial-path"
printf partial > "$partial"
if [ -n "$stats" ]; then
    [ -d "${stats%/*}" ]
    printf owned > "$stats"
fi
printf 'E27 sequence stdout\377\r\n'
printf '1/4 frames\r\nE27 sequence stderr\376' >&2
printf completed > "$records/complete"
exit 42
"#;

#[test]
fn e27_bounded_sequence_failed_status_cleanup() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    fs::write(
        root.join("watchdog.rs"),
        include_str!("../../encoder/e08_fixture.rs"),
    )
    .expect("watchdog source");
    let compiled = Command::new("rustc")
        .arg(root.join("watchdog.rs"))
        .arg("-o")
        .arg(root.join("watchdog"))
        .output()
        .expect("rustc");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let mut failures = Vec::new();
    for case in [
        "parallel-vs1",
        "parallel-vs2",
        "parallel-native1",
        "parallel-native2",
        "zone",
        "zone-spawn",
        "zone-worker",
        "serial",
    ] {
        let job = root.join(case);
        let cwd = job.join("cwd");
        fs::create_dir_all(&cwd).expect("isolated job");
        fs::write(cwd.join("sentinel.stat"), "CWD sentinel").expect("sentinel");
        let file = fs::File::create(job.join("child.log")).expect("log");
        let status = Command::new(root.join("watchdog"))
            .arg("--e08-watchdog")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(&job)
            .arg(case)
            .arg("core::sequence::zone_encoder::e27_tests::e27_sequence_runtime_child")
            .current_dir(&cwd)
            .stdout(file.try_clone().expect("clone log"))
            .stderr(file)
            .status()
            .expect("watchdog");
        let log = fs::read_to_string(job.join("child.log")).expect("log");
        if !status.success()
            || !job.join("executed").exists()
            || !job.join("verified").exists()
            || !log.contains("test result: ok. 1 passed;")
            || !log
                .lines()
                .any(|line| line == "E08 watchdog reaped 0 descendants; timeout=false")
        {
            failures.push(format!("{case}: {status}\n{log}"));
        }
        assert_eq!(fs::read_dir(&cwd).expect("cwd listing").count(), 1);
        assert_eq!(
            fs::read_to_string(cwd.join("sentinel.stat")).expect("sentinel"),
            "CWD sentinel"
        );
        eprint!("{log}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn input(root: &Path, native: bool, side: usize) -> Input {
    if native {
        let path = root.join("four frames.y4m");
        let mut bytes = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for _ in 0..4 {
            bytes.extend(frame());
        }
        fs::write(&path, bytes).expect("native source");
        Input::from_video(&InputModel::Video {
            path,
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: Some(root.join("native.ffindex")),
            },
            filters: vec![],
        })
        .expect("native FFMS2 input")
    } else {
        let script = root.join("four frames.vpy");
        fs::write(&script, format!("from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width={side},height={side},length=4,format=YUV420P8,fpsnum=24,fpsden=1,color=[48,128,128]).set_output()\n")).expect("finite VS script");
        Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(script),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
        .expect("nonstream VS input")
    }
}

fn frame() -> Vec<u8> {
    let mut bytes = b"FRAME\n".to_vec();
    bytes.extend(vec![48; 64 * 64]);
    bytes.extend(vec![128; 64 * 64 / 2]);
    bytes
}

fn encoder(root: &Path, scene: usize, total: u8, partial: &Path) -> Encoder {
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
    encoder.set_executable(Some(root.join("fake encoder")));
    encoder.set_append_color_data(false);
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(total);
    for (key, value) in [
        ("e27-records", root.to_str().expect("utf8").to_owned()),
        ("e27-scene", scene.to_string()),
        ("e27-partial", partial.to_str().expect("utf8").to_owned()),
    ] {
        encoder
            .parameters_mut()
            .insert(key.into(), CLIParameter::new_string("--", " ", &value));
    }
    encoder
}

fn diagnostic(root: &Path, result: &EncoderResult) {
    assert_eq!(result.encoder, EncoderBase::X264);
    assert_eq!(result.status.code(), Some(42));
    let argv: Vec<_> = fs::read_to_string(root.join("argv"))
        .expect("actual argv")
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(result.parameters, argv);
    let StringOrBytes::Bytes(stdout) = &result.stdout else {
        panic!("raw stdout lost");
    };
    let StringOrBytes::Bytes(stderr) = &result.stderr else {
        panic!("raw stderr lost");
    };
    assert_eq!(stdout, b"E27 sequence stdout\xff\r\n");
    assert_eq!(stderr, b"1/4 frames\r\nE27 sequence stderr\xfe");
    assert_eq!(
        fs::read_to_string(root.join("complete")).expect("child finished before return"),
        "completed"
    );
}

#[test]
fn e27_sequence_runtime_child() {
    let Some(root) = std::env::var_os("E08_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let case = std::env::var("E08_CASE").expect("case");
    fs::write(root.join("executed"), &case).expect("execution handshake");
    fs::write(root.join("fake encoder"), FAKE).expect("fake");
    fs::set_permissions(root.join("fake encoder"), fs::Permissions::from_mode(0o755))
        .expect("executable");
    let native = case.contains("native");
    let mut input = input(&root, native, if case == "zone-worker" { 4096 } else { 64 });
    assert_eq!(
        input.streams_concurrently(),
        native,
        "actual selected parallel path"
    );
    assert_eq!(input.clip_info().expect("clip info").num_frames, 4);
    let scenes_dir = root.join("scenes");
    fs::create_dir_all(&scenes_dir).expect("scenes");
    let sentinel = scenes_dir.join("unrelated.stat");
    fs::write(&sentinel, "unrelated stats").expect("sentinel");
    let cancelled = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let serial = case == "serial";
    let zone = case.starts_with("zone");
    let total = if case.ends_with('2') || serial { 2 } else { 1 };
    let first = usize::from(!serial);
    let count = if zone { 3 } else { 1 };
    let temp_output = scenes_dir.join(if zone {
        "zone-pass.temp.264".to_owned()
    } else if serial {
        "00000.264".to_owned()
    } else {
        "00001.temp.264".to_owned()
    });
    let mut first_encoder = encoder(&root, first, total, &temp_output);
    let error = if zone {
        if case == "zone-spawn" {
            let unspawnable = root.join("unspawnable encoder");
            fs::write(
                &unspawnable,
                format!("#!{}\n", root.join("missing-interpreter").display()),
            )
            .expect("invalid interpreter fixture");
            fs::set_permissions(&unspawnable, fs::Permissions::from_mode(0o755))
                .expect("executable fixture");
            first_encoder.set_executable(Some(unspawnable));
        }
        let tasks = (1..4)
            .map(|index| Task {
                original_index: index,
                index: index - 1,
                frame_indices: vec![index],
                sub_scenes: None,
                encoder: first_encoder.clone(),
                output: scenes_dir.join(format!("{index:05}.264")),
            })
            .collect();
        let plan = ZonePlan {
            zones: vec![],
            zone_option: "0,2,crf=23".to_owned(),
            qpfile_content: "1 I\n2 I\n".to_owned(),
            config_content: String::new(),
        };
        ZoneEncoder::encode_tasks(&mut input, tasks, plan, tx, cancelled).expect_err("zone failure")
    } else {
        let scenes = (0..4)
            .map(|index| Scene {
                start_frame: index,
                end_frame: index + 1,
                sub_scenes: None,
                encoder: encoder(
                    &root,
                    index,
                    total,
                    &scenes_dir.join(format!(
                        "{index:05}{}.264",
                        if serial { "" } else { ".temp" }
                    )),
                ),
                sequence_data: DefaultSequenceData::default(),
            })
            .collect();
        let saved = Arc::new(Mutex::new(Vec::new()));
        let saved_clone = Arc::clone(&saved);
        let mut config = DefaultSequenceConfig::default();
        config.parallel_encoder.workers = Some(1);
        config.parallel_encoder.max_tries = 1; // One-attempt failed-status/pass-order control.
        config.parallel_encoder.scenes_directory = scenes_dir.clone();
        config.parallel_encoder.validate_frame_count = false; // Scheduler markers, not native output.
        let mut condor = Condor::<DefaultSequenceData, DefaultSequenceConfig>::new(
            input,
            Output {
                path: root.join("unused"),
                tags: HashMap::new(),
                video_tags: HashMap::new(),
            },
            first_encoder,
            scenes,
            Some(config),
            Box::new(move |data| {
                saved_clone.lock().expect("saved snapshots").push(
                    data.scenes.iter().skip(usize::from(!serial)).any(|s| {
                        s.sequence_data.parallel_encoder.bytes.is_some()
                            || s.sequence_data.parallel_encoder.completed_on.is_some()
                    }),
                );
                Ok(())
            }),
        );
        if !serial {
            fs::write(scenes_dir.join("00000.264"), "preexisting scene")
                .expect("preexisting final");
            let data = &mut condor.scenes[0].sequence_data.parallel_encoder;
            data.bytes = Some(17);
            data.started_on = Some(10);
            data.completed_on = Some(20);
        }
        let error = if serial {
            SerialEncoder::new(&scenes_dir).execute(&mut condor, tx, cancelled)
        } else {
            ParallelEncoder::default().execute(&mut condor, tx, cancelled)
        }
        .expect_err("scene failure, not panic/hang");
        assert!(
            !saved.lock().expect("saved snapshots").iter().any(|&success| success),
            "failed result must not be persisted as successful"
        );
        assert!(
            condor.scenes.iter().skip(usize::from(!serial)).all(|s| s
                .sequence_data
                .parallel_encoder
                .bytes
                .is_none()
                && s.sequence_data.parallel_encoder.completed_on.is_none())
        );
        if !serial {
            let data = &condor.scenes[0].sequence_data.parallel_encoder;
            assert_eq!(
                (data.bytes, data.started_on, data.completed_on),
                (Some(17), Some(10), Some(20)),
                "eligible historical peer is not discarded with current failure"
            );
            assert_eq!(
                fs::read_to_string(scenes_dir.join("00000.264")).expect("preexisting final"),
                "preexisting scene"
            );
        }
        error
    };
    if case == "zone-spawn" {
        assert!(error.downcast_ref::<ParallelEncoderError>().is_none());
        assert!(error.downcast_ref::<EncoderResult>().is_none());
        assert!(
            format!("{error:#}").contains("Could not spawn"),
            "{error:#}"
        );
        assert!(
            !root.join("order").exists(),
            "generic spawn failure is not an exit status"
        );
    } else {
        let worker = case == "zone-worker";
        let failed = if serial || worker {
            if worker {
                assert!(
                    error.downcast_ref::<ParallelEncoderError>().is_none(),
                    "worker error must not be downgraded to a clean status"
                );
                let process = error
                    .downcast_ref::<crate::core::encoder::EncoderProcessError>()
                    .expect("worker error type retained");
                assert!(process.failures.iter().any(|e| format!("{e:#}").contains("STDIN")));
            }
            error
                .chain()
                .find_map(|e| e.downcast_ref::<EncoderResult>())
                .expect("serial retains typed result through joins")
        } else {
            let ParallelEncoderError::EncoderFailed { scene, result } = error
                .downcast_ref::<ParallelEncoderError>()
                .expect("scene-numbered status error")
            else {
                panic!("wrong scene error");
            };
            assert_eq!(*scene, first);
            assert!(error.to_string().contains("Scene 1"));
            result
        };
        diagnostic(&root, failed);
        assert_eq!(
            fs::read_to_string(root.join("order")).expect("process records"),
            format!("{first}:1/{total}\n"),
            "no later pass or scene"
        );
        let mut expected =
            format!("YUV4MPEG2 C420 W64 H64 F24:1 Ip A0:0 XLENGTH={count}\n").into_bytes();
        for _ in 0..count {
            expected.extend(frame());
        }
        if !worker {
            assert_eq!(
                fs::read(root.join("input.y4m")).expect("all consumed input"),
                expected
            );
        }
        if serial {
            assert_eq!(
                fs::read_to_string(&temp_output).expect("serial partial policy unchanged"),
                "partial"
            );
        }
    }
    if !serial {
        assert!(
            !temp_output.exists(),
            "owned failed temporary output removed"
        );
        if case != "zone-spawn" {
            let path = fs::read_to_string(root.join("partial-path"))
                .expect("actual codec partial path recorded");
            assert!(
                !Path::new(&path).exists(),
                "actual owned codec partial removed after joins"
            );
        }
    }
    assert!(
        !scenes_dir.join("zone.qpfile").exists(),
        "owned zone file removed"
    );
    assert!(!fs::read_dir(&scenes_dir).expect("listing").any(|entry| {
        let name = entry.expect("entry").file_name();
        let name = name.to_string_lossy();
        name.starts_with(".av1an-stats-") || name.starts_with(".av1an-attempt-")
    }));
    assert_eq!(
        fs::read_to_string(&sentinel).expect("stats sentinel"),
        "unrelated stats"
    );
    let events: Vec<_> = rx.try_iter().collect();
    assert!(
        matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)),
        "all sequence progress senders joined/dropped before return"
    );
    let sizes: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SequenceStatus::Whole(Status::Processing {
                id,
                completion:
                    SequenceCompletion::Custom {
                        name, completed, ..
                    },
            }) if name == "size" => Some((id.as_str(), *completed)),
            _ => None,
        })
        .collect();
    assert_eq!(
        sizes,
        if !zone && !serial {
            vec![("0", 17.0)]
        } else {
            vec![]
        },
        "only eligible historical scene reports size; failed scenes must not report successful size"
    );
    fs::write(root.join("verified"), "all assertions ran").expect("verification handshake");
    eprintln!(
        "E27 sequence {case}: returned Err without panic/hang; case-specific diagnostics, ordering, persistence, cleanup and sentinels verified"
    );
}
