//! Runtime shutdown checks: the supervisor kills/reaps a deadlocked baseline.
use super::*;
use crate::{
    core::encoder::{EncoderProcessError, EncoderResult},
    models::{
        encoder::{Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter},
        input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
    },
    utils::semaphore::Semaphore,
};
use std::{
    collections::HashMap, fs, os::unix::fs::PermissionsExt, path::Path, process::Command,
    sync::atomic::AtomicBool,
};

// Private subprocess-local seams: header/producer/size and panic faults are
// injected. Missing/spawn/filter/rename/results/save failures use real APIs.
pub(super) fn fault(point: &str, scene: usize) -> anyhow::Result<()> {
    let Ok(case) = std::env::var("E08_CASE") else {
        return Ok(());
    };
    let root = std::path::PathBuf::from(std::env::var_os("E08_ROOT").expect("fixture root"));
    if case.starts_with("vs-")
        && point == "header"
        && scene == 8
        && !matches!(
            case.as_str(),
            "vs-success1"
                | "vs-success2"
                | "vs-multipass"
                | "vs-save-active"
                | "vs-cancelled"
                | "vs-zero"
        )
    {
        // Make scene 7 the failing owner, rather than assuming FIFO wakeups.
        while !root.join("started-7").exists() && !root.join("stop-observed").exists() {
            std::thread::yield_now();
        }
    }
    if case == "vs-save-active" && point == "header" && scene == 9 {
        // Fill both active slots before publishing lookahead. Semaphore
        // fairness is not part of the contract, so do not rely on thread order.
        while !root.join("started-8").exists() {
            std::thread::yield_now();
        }
    }
    if point == "decode" {
        fs::write(root.join(format!("decoded-{scene}")), "admitted")?;
    }
    if scene != 7 {
        return Ok(());
    }
    let kind = case.split_once('-').expect("path-kind").1;
    if point == "open" && kind == "open" {
        fs::remove_file(root.join("finite.y4m"))?;
    } else if kind == point {
        anyhow::bail!("e11 {point} fault");
    } else if kind == format!("{point}-panic") {
        panic!("e11 {point} panic");
    }
    Ok(())
}

pub(super) fn stopped() {
    if let Some(root) = std::env::var_os("E08_ROOT") {
        fs::write(Path::new(&root).join("stop-observed"), "admission closed")
            .expect("stop handshake");
    }
}

pub(super) fn verify_pools(stop: &lifecycle::Stop, workers: usize) {
    for (pool, expected) in [(&stop.workers, workers), (&stop.decoders, workers + 1)] {
        let mut returned = 0;
        while pool.try_acquire().is_some() {
            returned += 1;
        }
        assert_eq!(
            returned, expected,
            "every transferred permit returned exactly once after joins"
        );
    }
}

const FAKE: &str = r#"#!/bin/sh
set -eu
out= stats= root= scene=
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) out=$2; shift ;;
        --stats) stats=$2; shift ;;
        --e11-root) root=$2; shift ;;
        --e11-scene) scene=$2; shift ;;
    esac
    shift
done
printf started > "$root/started-$scene"
if [ "$E08_CASE" = native-early ] || [ "$E08_CASE" = vs-early ]; then
    printf 'E11 early diagnostic' >&2
    exit 42
fi
if [ "$E08_CASE" = native-save-active ] || [ "$E08_CASE" = vs-save-active ]; then
    if [ "$scene" = 7 ]; then
        while [ ! -f "$root/started-8" ]; do :; done
    else
        while [ ! -f "$root/stop-observed" ]; do :; done
    fi
fi
cat > "$root/input-$scene"
printf owned > "$out"
if [ -n "$stats" ]; then printf owned > "$stats"; fi
printf complete > "$root/complete-$scene"
if [ "$E08_CASE" = native-status ] || [ "$E08_CASE" = vs-status ]; then
    printf 'E11 status diagnostic' >&2
    exit 42
fi
"#;

#[test]
fn e11_bounded_parallel_shutdown() {
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
    for path in ["vs", "native"] {
        for kind in [
            "missing",
            "spawn",
            "filter",
            "early",
            "status",
            "rename",
            "size",
            "results",
            "save",
            "save-panic",
            "save-active",
            "relay-error",
            "worker-panic",
            "relay-panic",
            "success1",
            "success2",
            "multipass",
            "cancelled",
            "zero",
            "decode",
        ] {
            run_child(root, &format!("{path}-{kind}"), &mut failures);
        }
    }
    for case in [
        "vs-header",
        "vs-gate",
        "vs-gate-panic",
        "vs-schedule-panic",
        "native-open",
        "native-produce",
        "vs-metric",
        "vs-metric-error",
    ] {
        run_child(root, case, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn run_child(root: &Path, case: &str, failures: &mut Vec<String>) {
    let job = root.join(case);
    fs::create_dir_all(&job).expect("job");
    let log = fs::File::create(job.join("child.log")).expect("log");
    let status = Command::new(root.join("watchdog"))
        .arg("--e08-watchdog")
        .arg(std::env::current_exe().expect("test binary"))
        .arg(&job)
        .arg(case)
        .arg("core::sequence::parallel_encoder::e11_tests::e11_runtime_child")
        .stdout(log.try_clone().expect("log clone"))
        .stderr(log)
        .status()
        .expect("supervisor");
    let log = fs::read_to_string(job.join("child.log")).expect("log");
    if !status.success()
        || !job.join("executed").exists()
        || !job.join("verified").exists()
        || !log.contains("test result: ok. 1 passed;")
        || !log.lines().any(|l| l == "E08 watchdog reaped 0 descendants; timeout=false")
    {
        failures.push(format!("{case}: {status}\n{log}"));
    }
    eprint!("{log}");
}

fn input(root: &Path, native: bool) -> Input {
    if native {
        let path = root.join("finite.y4m");
        let mut bytes = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for _ in 0..96 {
            bytes.extend_from_slice(b"FRAME\n");
            bytes.extend(vec![48; 64 * 64 * 3 / 2]);
        }
        fs::write(&path, bytes).expect("native input");
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
        let script = root.join("finite.vpy");
        fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,length=96,format=YUV420P8,fpsnum=24,fpsden=1).set_output()\n").expect("VS script");
        Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(script),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
        .expect("real nonstream input")
    }
}

fn encoder(root: &Path, scene: usize, kind: &str) -> Encoder {
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
    encoder.set_append_color_data(false);
    *encoder.passes_mut().expect("passes") =
        EncoderPasses::All(if kind == "multipass" { 2 } else { 1 });
    encoder.set_executable(Some(root.join(match kind {
        "missing" => "missing-encoder",
        "spawn" => "unspawnable",
        _ => "fake-encoder",
    })));
    for (name, value) in [
        ("e11-root", root.to_str().expect("UTF8").to_owned()),
        ("e11-scene", scene.to_string()),
    ] {
        encoder
            .parameters_mut()
            .insert(name.into(), CLIParameter::new_string("--", " ", &value));
    }
    if kind == "filter" {
        encoder.set_ffmpeg_filter(Some("e11_no_such_filter".to_owned()));
    }
    // Exercise E10 consumer-owned cleanup on generic errors and panics too.
    if matches!(kind, "worker-panic" | "relay-panic" | "early") {
        encoder.set_ffmpeg_filter(Some("null".to_owned()));
    }
    encoder
}

fn metric_callbacks(input: &mut Input, fail: bool) {
    use crate::vapoursynth::plugins::{
        MetricPluginFunction, PluginFunction, standard::plane_stats::PlaneStats,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    let vs = input.decoder().get_vapoursynth_impl().expect("real VS decoder");
    let node = vs
        .get_output(vs.get_output_index(), vs.get_node_modifier())
        .expect("real VS node");
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let (tx, rx) = std::sync::mpsc::channel();
    let values = PlaneStats::collect_frame_values(
        &node,
        tx,
        move |index, value| {
            assert_eq!(*value, 64);
            seen.fetch_add(1, Ordering::SeqCst);
            if fail && index == 0 {
                return Err(PlaneStats::new_error(
                    "e11 metric callback failure".to_owned(),
                ));
            }
            Ok(())
        },
        |frame| Ok(frame.width(0)),
    );
    if fail {
        assert!(
            values
                .expect_err("callback error preserved")
                .to_string()
                .contains("e11 metric callback failure")
        );
        assert!(calls.load(Ordering::SeqCst) > 0);
    } else {
        assert_eq!(values.expect("raw callback semaphore drains"), vec![64; 96]);
        assert_eq!(calls.load(Ordering::SeqCst), 96);
    }
    let _: Vec<_> = rx.try_iter().collect();
    assert!(matches!(
        rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    ));
}

#[test]
fn e11_runtime_child() {
    let Some(root) = std::env::var_os("E08_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let case = std::env::var("E08_CASE").expect("case");
    let (path, kind) = case.split_once('-').expect("path-kind");
    fs::write(root.join("executed"), &case).expect("execution handshake");
    let mut input = input(&root, path == "native");
    assert_eq!(input.streams_concurrently(), path == "native");
    assert_eq!(input.clip_info().expect("clip").num_frames, 96);
    if kind.starts_with("metric") {
        metric_callbacks(&mut input, kind == "metric-error");
        fs::write(
            root.join("verified"),
            "actual async metric callbacks completed",
        )
        .expect("verification");
        return;
    }
    for (name, text) in [
        ("fake-encoder", FAKE.to_owned()),
        (
            "unspawnable",
            format!("#!{}\n", root.join("missing-interpreter").display()),
        ),
    ] {
        fs::write(root.join(name), text).expect("encoder script");
        fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o755))
            .expect("executable");
    }
    let scenes = root.join("scenes");
    fs::create_dir_all(&scenes).expect("outputs");
    fs::write(scenes.join("unrelated.stat"), "sentinel").expect("stats sentinel");
    fs::write(scenes.join("preexisting.264"), "preexisting final").expect("final sentinel");
    if kind == "rename" {
        fs::create_dir_all(scenes.join("0.264")).expect("rename obstruction");
        fs::write(scenes.join("0.264/sentinel"), "preexisting directory")
            .expect("directory sentinel");
    }
    let total = if kind == "save-active" { 8 } else { 4 };
    let tasks = (0..total)
        .map(|index| Task {
            index,
            original_index: index + 7,
            frame_indices: if kind == "decode" && path == "native" {
                vec![9999]
            } else {
                (0..96).collect()
            },
            sub_scenes: None,
            encoder: encoder(&root, index + 7, kind),
            output: scenes.join(format!("{index}.264")),
        })
        .collect();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut rx = Some(rx);
    if kind == "relay-error" {
        drop(rx.take());
    }
    let (mut results_tx, results_rx) = crossbeam_channel::unbounded();
    if kind == "results" {
        let (broken, broken_rx) = crossbeam_channel::unbounded();
        drop(broken_rx);
        results_tx = broken;
    }
    let mut saved = Vec::new();
    let mut on_results = |batch: Vec<ParallelEncoderResult>| -> Result<()> {
        assert!(batch.iter().all(|r| r.result.status.success() && r.bytes == 5));
        saved.extend(batch.iter().map(|r| r.scene));
        if kind == "save-active" {
            while !root.join("started-8").exists() {
                std::thread::yield_now();
            }
            anyhow::bail!("e11 persistence failure with active worker");
        }
        if kind == "save" {
            anyhow::bail!("e11 persistence failure");
        }
        assert_ne!(kind, "save-panic", "e11 persistence panic");
        Ok(())
    };
    let mut stream = task::ResultStream {
        results_tx,
        results_rx: &results_rx,
        finished_scenes: Arc::new(Semaphore::new(0)),
        on_results: &mut on_results,
    };
    let workers = if kind == "zero" {
        0
    } else if matches!(kind, "success2" | "save-active") {
        2
    } else {
        1
    };
    let cancelled = Arc::new(AtomicBool::new(kind == "cancelled"));
    let result = ParallelEncoder::encode_tasks_with_sender(
        &mut input,
        workers,
        tasks,
        tx,
        cancelled,
        &mut stream,
    );
    let success = matches!(kind, "success1" | "success2" | "multipass" | "cancelled");
    if success {
        let results = result.expect("all tasks returned");
        if kind == "cancelled" {
            assert!(results.iter().all(Option::is_none));
        } else {
            assert_eq!(
                results.iter().flatten().map(|r| r.scene).collect::<Vec<_>>(),
                [7, 8, 9, 10]
            );
            saved.sort_unstable();
            assert_eq!(saved, [7, 8, 9, 10], "no result/notification lost");
            for scene in 7..11 {
                assert!(root.join(format!("complete-{scene}")).exists());
            }
        }
    } else {
        let error = result.expect_err("error returned, not panic/deadlock");
        let diagnostic = format!("{error:#}");
        match kind {
            "missing" => assert!(diagnostic.contains("missing-encoder"), "{diagnostic}"),
            "spawn" => assert!(diagnostic.contains("Could not spawn"), "{diagnostic}"),
            "filter" => assert!(diagnostic.contains("e11_no_such_filter"), "{diagnostic}"),
            "early" => {
                let result = error
                    .chain()
                    .find_map(|e| e.downcast_ref::<EncoderResult>())
                    .or_else(|| match error.downcast_ref::<ParallelEncoderError>() {
                        Some(ParallelEncoderError::EncoderFailed { result, .. }) => Some(result),
                        _ => None,
                    })
                    .expect("typed early failure");
                assert_eq!(result.status.code(), Some(42));
                assert!(result.stderr.to_string().contains("E11 early diagnostic"));
                assert!(
                    error.downcast_ref::<EncoderProcessError>().is_some(),
                    "actual broken input pipe: {diagnostic}"
                );
            },
            "status" => {
                let ParallelEncoderError::EncoderFailed { scene, result } =
                    error.downcast_ref::<ParallelEncoderError>().expect("typed status")
                else {
                    panic!("wrong error");
                };
                assert_eq!(*scene, 7);
                assert_eq!(result.status.code(), Some(42));
                assert_eq!(result.stderr.to_string(), "E11 status diagnostic");
            },
            "save" | "save-active" => assert!(
                diagnostic.contains("e11 persistence failure"),
                "{diagnostic}"
            ),
            "worker-panic" | "relay-panic" | "gate-panic" => assert!(
                diagnostic.contains("panicked")
                    && diagnostic.contains("Scene 7")
                    && diagnostic
                        .contains(&format!("e11 {} panic", kind.trim_end_matches("-panic"))),
                "{diagnostic}"
            ),
            "save-panic" => assert!(
                diagnostic.contains("panicked") && diagnostic.contains("e11 persistence panic"),
                "{diagnostic}"
            ),
            "schedule-panic" => assert!(
                diagnostic.contains("Parallel worker setup panicked")
                    && diagnostic.contains("e11 schedule panic"),
                "{diagnostic}"
            ),
            "relay-error" => assert!(diagnostic.contains("progress relay"), "{diagnostic}"),
            "header" | "gate" | "produce" | "size" => assert!(
                diagnostic.contains(&format!("e11 {kind} fault")),
                "{diagnostic}"
            ),
            "zero" => assert!(error.downcast_ref::<ParallelEncoderError>().is_some()),
            "rename" => {
                assert_eq!(
                    error
                        .downcast_ref::<std::io::Error>()
                        .expect("real rename failure")
                        .raw_os_error(),
                    Some(21)
                );
                assert!(
                    root.join("complete-7").exists(),
                    "finite encoder really completed before rename"
                );
            },
            "results" => assert!(diagnostic.contains("disconnected channel"), "{diagnostic}"),
            "open" => assert!(
                diagnostic.contains("No video file found") && diagnostic.contains("finite.y4m"),
                "{diagnostic}"
            ),
            "decode" if path == "native" => assert!(
                diagnostic.contains("frame 9999") && diagnostic.contains("Out of bounds"),
                "{diagnostic}"
            ),
            _ => assert!(!diagnostic.is_empty()),
        }
        if kind == "save-active" {
            assert!(
                root.join("complete-8").exists(),
                "active finite child joined before return"
            );
            assert!(
                !root.join("started-10").exists(),
                "two active jobs are held until stop is observed; no queued encoder starts after wakeup"
            );
            if path == "vs" {
                assert!(
                    !root.join("decoded-11").exists(),
                    "workers+1 lookahead plus the completed job; stop prevents further decode"
                );
            }
        } else if !matches!(kind, "save" | "save-panic") {
            assert!(
                !root.join("started-8").exists(),
                "workers=1: no queued encoder after first failure"
            );
        }
        if path == "vs" && !matches!(kind, "save" | "save-panic" | "save-active" | "zero") {
            assert!(
                !root.join("decoded-9").exists(),
                "lookahead bounded to workers+1; no queued decode after failure"
            );
        }
        if !matches!(kind, "save" | "save-panic" | "save-active" | "results") {
            assert!(saved.is_empty(), "failed completion not saved");
        }
        eprintln!("E11 {case}: original error returned: {diagnostic}");
    }
    if let Some(rx) = rx {
        let _: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            ),
            "all progress helpers joined"
        );
    }
    assert!(
        fs::read_dir(&scenes).expect("cleanup").all(|entry| {
            let name = entry.expect("entry").file_name().to_string_lossy().into_owned();
            !name.contains(".temp.") && !name.starts_with(".av1an-")
        }),
        "all owned temporary outputs/statistics/spools removed"
    );
    assert_eq!(
        fs::read_to_string(scenes.join("unrelated.stat")).expect("stats sentinel"),
        "sentinel"
    );
    assert_eq!(
        fs::read_to_string(scenes.join("preexisting.264")).expect("final sentinel"),
        "preexisting final"
    );
    if kind == "rename" {
        assert_eq!(
            fs::read_to_string(scenes.join("0.264/sentinel")).expect("directory sentinel"),
            "preexisting directory"
        );
    }
    fs::write(root.join("verified"), "all checks ran").expect("verification handshake");
}
