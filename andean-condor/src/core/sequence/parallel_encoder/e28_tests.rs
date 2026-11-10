//! Counts actual dispatcher workers; only the external codec is synthetic at scale.
use super::*;
use crate::{
    core::encoder::EncoderResult,
    models::encoder::{Encoder, EncoderBase, EncoderPasses},
    utils::semaphore::Semaphore,
};
use std::{
    collections::VecDeque,
    fs,
    io::Cursor,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    sync::{
        Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

struct Probe {
    synthetic: bool,
    handshake: bool,
    failure: bool,
    save_failure: bool,
    workers: usize,
    spawned: AtomicUsize,
    live: AtomicUsize,
    maximum: AtomicUsize,
    headers: Mutex<Vec<usize>>,
    consumed: Mutex<Vec<usize>>,
    held: Mutex<(usize, bool, bool)>,
    changed: Condvar,
    admission: crossbeam_channel::Sender<Arc<Semaphore>>,
}

static PROBE: OnceLock<Probe> = OnceLock::new();

pub(super) struct WorkerCount(&'static Probe);

pub(super) fn worker_started() -> Option<WorkerCount> {
    let probe = PROBE.get()?;
    probe.spawned.fetch_add(1, Ordering::SeqCst);
    let live = probe.live.fetch_add(1, Ordering::SeqCst) + 1;
    probe.maximum.fetch_max(live, Ordering::SeqCst);
    Some(WorkerCount(probe))
}

impl Drop for WorkerCount {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(super) fn before_admission(stop: &lifecycle::Stop, task: &Task) {
    let Some(p) = PROBE.get().filter(|p| p.handshake && task.index == p.workers) else {
        return;
    };
    let held = p.held.lock().expect("held workers");
    drop(p.changed.wait_while(held, |h| h.0 != p.workers).expect("all workers busy"));
    p.admission.send(Arc::clone(&stop.workers)).expect("controller");
}

pub(super) fn header(task: &Task) {
    let Some(p) = PROBE.get() else { return };
    if p.handshake && task.index >= p.workers {
        assert!(
            p.held.lock().expect("held").1,
            "queued header decoded while all workers busy"
        );
        assert!(!p.failure, "queued header decoded after failure");
    }
    p.headers.lock().expect("headers")[task.index] += 1;
}

pub(super) fn before_joins() {
    if let Some(p) = PROBE.get().filter(|p| p.save_failure) {
        p.held.lock().expect("held").2 = true;
        p.changed.notify_all();
    }
}

pub(super) fn consume(
    task: &Task,
    frames: &crossbeam_channel::Receiver<Cursor<Vec<u8>>>,
    temp: &Path,
    stop: &lifecycle::Stop,
) -> Option<Result<EncoderResult>> {
    let p = PROBE.get().filter(|p| p.synthetic)?;
    Some((|| {
        let bytes: Vec<_> = frames.iter().map(Cursor::into_inner).collect();
        assert_eq!(bytes.len(), 2, "one real VS header and one real frame");
        let header = std::str::from_utf8(&bytes[0]).expect("Y4M header");
        assert!(header.starts_with("YUV4MPEG2 "));
        for field in ["W64", "H64", "F24:1"] {
            assert!(
                header.split_whitespace().any(|token| token == field),
                "{header}"
            );
        }
        assert_eq!(&bytes[1][..6], b"FRAME\n");
        assert_eq!(bytes[1].len(), 6 + 64 * 64 * 3 / 2);
        p.consumed.lock().expect("identities")[task.index] += 1;
        if p.handshake && task.index < p.workers {
            let mut held = p.held.lock().expect("held");
            held.0 += 1;
            p.changed.notify_all();
            if (p.failure || p.save_failure) && task.index != 0 {
                drop(held);
                let _ = stop.receiver.recv();
            } else {
                drop(
                    p.changed
                        .wait_while(held, |h| !h.1 || (p.save_failure && !h.2))
                        .expect("release"),
                );
            }
            if p.failure && task.index == 0 {
                anyhow::bail!("e28 active scene {} failure", task.original_index);
            }
        }
        fs::write(temp, task.original_index.to_string())?;
        Ok(EncoderResult {
            encoder: EncoderBase::X264,
            parameters: vec![task.original_index.to_string()],
            status: std::process::ExitStatus::default(),
            stdout: Vec::new().into(),
            stderr: Vec::new().into(),
        })
    })())
}

#[test]
fn e28_bounded_dispatch_runtime() {
    let temp = tempfile::tempdir().expect("fixtures");
    let root = temp.path();
    fs::write(
        root.join("watchdog.rs"),
        include_str!("../../encoder/e08_fixture.rs"),
    )
    .expect("watchdog");
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
        "count1",
        "count3",
        "excess",
        "empty",
        "admission",
        "failure",
        "save",
        "public1",
        "public3",
        "public8",
    ] {
        let job = root.join(case);
        fs::create_dir_all(&job).expect("job");
        let log = fs::File::create(job.join("child.log")).expect("log");
        let status = Command::new(root.join("watchdog"))
            .arg("--e08-watchdog")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(&job)
            .arg(format!("e28-{case}"))
            .arg("core::sequence::parallel_encoder::e28_tests::e28_runtime_child")
            .stdout(log.try_clone().expect("log clone"))
            .stderr(log)
            .status()
            .expect("watchdog");
        let log = fs::read_to_string(job.join("child.log")).expect("child log");
        if !status.success()
            || !job.join("verified").exists()
            || !log.contains("test result: ok. 1 passed;")
            || !log.lines().any(|l| l == "E08 watchdog reaped 0 descendants; timeout=false")
        {
            failures.push(format!("{case}: {status}\n{log}"));
        }
        eprint!("{log}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn e28_runtime_child() {
    let Ok(case) = std::env::var("E08_CASE") else {
        return;
    };
    let Some(case) = case.strip_prefix("e28-") else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os("E08_ROOT").expect("root"));
    fs::write(root.join("executed"), case).expect("execution handshake");
    let (workers, count) = match case {
        "count1" => (1, 2000),
        "count3" => (3, 2000),
        "excess" => (8, 2),
        "empty" => (3, 0),
        "save" => (3, 3),
        "admission" | "failure" => (3, 4),
        "public1" => (1, 5),
        "public3" => (3, 5),
        "public8" => (8, 5),
        _ => panic!("unknown case"),
    };
    let synthetic = !case.starts_with("public");
    let handshake = matches!(case, "admission" | "failure" | "save");
    let failure = case == "failure";
    let save_failure = case == "save";
    let (admission, admitted) = crossbeam_channel::bounded(1);
    assert!(
        PROBE
            .set(Probe {
                synthetic,
                handshake,
                failure,
                save_failure,
                workers: usize::from(workers),
                spawned: AtomicUsize::new(0),
                live: AtomicUsize::new(0),
                maximum: AtomicUsize::new(0),
                headers: Mutex::new(vec![0; count]),
                consumed: Mutex::new(vec![0; count]),
                held: Mutex::new((0, false, false)),
                changed: Condvar::new(),
                admission,
            })
            .is_ok()
    );
    let p = PROBE.get().expect("probe");
    let mut input = e11_tests::input(&root, false);
    assert!(
        !input.streams_concurrently(),
        "real nonstream VS dispatcher"
    );
    let fake = root.join("codec");
    fs::write(&fake, "#!/bin/sh\nset -eu\nout=\nwhile [ \"$#\" -gt 0 ]; do\nif [ \"$1\" = -o ]; then shift; out=$1; fi\nshift\ndone\ncat >/dev/null\nprintf owned >\"$out\"\n").expect("fake codec");
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("executable");
    let tasks: VecDeque<_> = (0..count)
        .map(|position| {
            // Non-handshake cases deliberately dispatch in reverse result-slot order.
            let index = if handshake {
                position
            } else {
                count - 1 - position
            };
            let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
            encoder.set_append_color_data(false);
            encoder.set_executable(Some(fake.clone()));
            *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
            Task {
                index,
                original_index: 7 + index * 3,
                frame_indices: vec![0],
                sub_scenes: None,
                encoder,
                output: root.join(format!("{index}.264")),
            }
        })
        .collect();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut callbacks = vec![0; count];
    let mut on_results = |batch: Vec<ParallelEncoderResult>| -> Result<()> {
        for r in batch {
            assert!(r.result.status.success());
            callbacks[(r.scene - 7) / 3] += 1;
        }
        if save_failure && callbacks[0] == 1 && callbacks[1] == 0 && callbacks[2] == 0 {
            anyhow::bail!("e28 persistence failure with last batch active");
        }
        Ok(())
    };
    let (results_tx, results_rx) = crossbeam_channel::unbounded();
    let mut stream = task::ResultStream {
        results_tx,
        results_rx: &results_rx,
        finished_scenes: Arc::new(Semaphore::new(0)),
        on_results: &mut on_results,
    };
    let result = std::thread::scope(|s| {
        let controller = handshake.then(|| {
            s.spawn(|| {
                if save_failure {
                    let held = p.held.lock().expect("held");
                    let mut held =
                        p.changed.wait_while(held, |h| h.0 != 3).expect("all workers busy");
                    held.1 = true;
                    p.changed.notify_all();
                    return;
                }
                let pool =
                    admitted.recv().expect("queued admission attempted with all workers held");
                let blocked = pool.wait_for_waiter();
                let mut held = p.held.lock().expect("held");
                assert_eq!(held.0, 3, "all workers really occupied");
                let headers = p.headers.lock().expect("headers")[3];
                held.1 = true;
                p.changed.notify_all();
                drop(held);
                assert!(
                    blocked,
                    "scheduler must block on worker admission, before queued decode"
                );
                assert_eq!(headers, 0, "queued header untouched at admission handshake");
            })
        });
        let result = if synthetic {
            ParallelEncoder::encode_tasks_with_sender(
                &mut input,
                workers,
                tasks,
                tx,
                Arc::new(AtomicBool::new(false)),
                &mut stream,
            )
        } else {
            ParallelEncoder::encode_tasks(
                &mut input,
                workers,
                tasks,
                tx,
                Arc::new(AtomicBool::new(false)),
            )
        };
        if let Some(controller) = controller {
            controller.join().expect("controller joined");
        }
        result
    });
    let spawned = p.spawned.load(Ordering::SeqCst);
    let maximum = p.maximum.load(Ordering::SeqCst);
    eprintln!(
        "E28 {case}: tasks={count}, configured={workers}, spawned={spawned}, max_live={maximum}, live_after_join={}",
        p.live.load(Ordering::SeqCst)
    );
    assert_eq!(
        p.live.load(Ordering::SeqCst),
        0,
        "all actual worker guards dropped"
    );
    assert!(
        spawned <= usize::from(workers).min(count),
        "one fixed bounded worker set, not per-scene threads: {spawned}"
    );
    assert_eq!(
        spawned,
        usize::from(workers).min(count),
        "no zero-worker false pass"
    );
    assert!(maximum <= spawned && (count == 0 || maximum > 0));
    if save_failure {
        let error = result.expect_err("callback failure observed before any blocking join");
        assert!(format!("{error:#}").contains("e28 persistence failure with last batch active"));
        assert_eq!(
            callbacks,
            [1, 1, 1],
            "no failed-batch retry; active peers still drained"
        );
        for index in 0..3 {
            assert_eq!(
                fs::read_to_string(root.join(format!("{index}.264"))).expect("active output"),
                (7 + index * 3).to_string()
            );
        }
    } else if failure {
        let error = result.expect_err("active failure closes admission");
        assert!(format!("{error:#}").contains("e28 active scene 7 failure"));
        assert_eq!(*p.headers.lock().expect("headers"), [1, 1, 1, 0]);
        assert_eq!(*p.consumed.lock().expect("consumed"), [1, 1, 1, 0]);
        assert_eq!(
            callbacks,
            [0, 1, 1, 0],
            "successful active peers still persist after stop"
        );
        assert!(!root.join("0.264").exists() && !root.join("3.264").exists());
    } else {
        let results = result.expect("all tasks completed");
        assert_eq!(results.len(), count);
        for (index, result) in results.into_iter().enumerate() {
            let result = result.expect("each result slot filled exactly once");
            assert_eq!(
                result.scene,
                7 + index * 3,
                "sparse scene identity vs result slot"
            );
            assert!(result.result.status.success());
            let expected = if synthetic {
                result.scene.to_string()
            } else {
                "owned".to_owned()
            };
            assert_eq!(
                fs::read_to_string(root.join(format!("{index}.264"))).expect("final output"),
                expected
            );
            assert_eq!(result.bytes, expected.len() as u64);
        }
        assert_eq!(*p.headers.lock().expect("headers"), vec![1; count]);
        if synthetic {
            assert_eq!(*p.consumed.lock().expect("consumed"), vec![1; count]);
            assert_eq!(
                callbacks,
                vec![1; count],
                "each successful task persisted exactly once"
            );
        }
    }
    let _: Vec<_> = rx.try_iter().collect();
    assert!(
        matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ),
        "all progress senders dropped"
    );
    assert!(
        !fs::read_dir(&root).expect("files").any(|e| e
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .contains(".temp."))
    );
    fs::write(
        root.join("verified"),
        "bounded actual dispatch, identities and joins checked",
    )
    .expect("verified");
}
