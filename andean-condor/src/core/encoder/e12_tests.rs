//! Every case runs the public owner or its precise owning seam in an isolated process.
use super::*;
use crate::core::{
    Condor,
    output::Output,
    sequence::{
        Sequence,
        ffmpeg_filter::FfmpegFilter,
        parallel_encoder::{ParallelEncoder, Task},
        scene_concatenator::SceneConcatenator,
        serial_encoder::SerialEncoder,
        zone_encoder::{ZoneEncoder, ZonePlan},
    },
};
use crate::models::{
    input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
    scene::Scene,
    sequence::{DefaultSequenceConfig, DefaultSequenceData},
};
use std::fs;

#[test]
fn e12_bounded_scene_force() {
    let dir = tempfile::tempdir().expect("fixture");
    fs::write(dir.path().join("fake.rs"), include_str!("e12_fixture.rs")).expect("source");
    e02_tests::run_bounded(
        Command::new("rustc")
            .arg(dir.path().join("fake.rs"))
            .arg("-o")
            .arg(dir.path().join("fake")),
        &dir.path().join("rustc.log"),
    );
    let mut failures = Vec::new();
    for case in [
        "force-vs",
        "force-native",
        "graceful-vs",
        "graceful-native",
        "graceful-sigint",
        "registration",
        "leader-exits",
        "capture",
        "stream-idle",
        "encoder-existing",
        "stdin-full",
        "replay-full",
        "serial",
        "zone",
        "zone-probe",
        "filter-empty",
        "filter-full",
        "dg",
        "ffmpeg",
        "ffmpeg-existing",
        "mkvmerge",
        "mkv-group",
        "pre-spawn",
        "guard-drop",
        "guard-unwind",
        "after-completion",
        "ffms-index",
    ] {
        let root = dir.path().join(case);
        fs::create_dir_all(&root).expect("case directory");
        // Reuse an immutable binary while the unfiltered suite forks/execs in parallel.
        std::os::unix::fs::symlink(dir.path().join("fake"), root.join("fake"))
            .expect("fixture executable");
        let output = Command::new(root.join("fake"))
            .arg("supervise")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(&root)
            .arg(case)
            .output()
            .unwrap_or_else(|error| panic!("bounded supervisor {case}: {error}"));
        let log = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !output.status.success()
            || !log
                .lines()
                .any(|line| line == format!("E12 case={case} selected=true rescued=0"))
        {
            failures.push(format!("{case}: {log}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn wait_file(root: &Path, name: &str) {
    while !root.join(name).exists() {
        thread::yield_now();
    }
}

fn input(root: &Path, native: bool) -> Input {
    if native {
        let path = root.join("input.y4m");
        let mut bytes = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for _ in 0..2 {
            bytes.extend_from_slice(b"FRAME\n");
            bytes.extend(vec![48; 64 * 64 * 3 / 2]);
        }
        fs::write(&path, bytes).expect("native input");
        Input::from_video(&InputModel::Video {
            path,
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: Some(root.join("input.ffindex")),
            },
            filters: vec![],
        })
        .expect("native FFMS2")
    } else {
        let path = root.join("input.vpy");
        fs::write(&path, format!("from vapoursynth import core, YUV420P8\nc=core.std.BlankClip(width=64,height=64,length=2,format=YUV420P8)\ndef mark(n):\n if n==1: open({:?},'w').write('decoded')\n return c\ncore.std.FrameEval(c,mark).set_output()\n", root.join("queued-decoded").to_str().expect("path"))).expect("script");
        Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(path),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
        .expect("VS input")
    }
}

fn encoder(root: &Path, multipass: bool) -> Encoder {
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
    encoder.set_executable(Some(root.join("fake")));
    encoder.set_append_color_data(false);
    encoder.set_quantizer(23.0);
    if multipass {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    }
    encoder
}

fn reap(root: &Path) {
    if let Ok(pid) = fs::read_to_string(root.join("grandchild")) {
        let pid: i32 = pid.parse().expect("grandchild PID");
        let mut status = 0;
        // SAFETY: Only the known adopted fixture grandchild is reaped. Its finite
        // emergency lifetime exceeds the supervisor bound; natural exit cannot pass.
        let reaped = unsafe { libc::waitpid(pid, &mut status, 0) };
        assert_eq!(
            reaped, pid,
            "grandchild must be dead, not killed by supervisor"
        );
        assert!(libc::WIFSIGNALED(status));
        assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
    }
    if let Ok(pid) = fs::read_to_string(root.join("leader")) {
        let pid: i32 = pid.parse().expect("leader PID");
        // SAFETY: Signal zero checks existence only; it cannot kill an unrelated process.
        let exists = unsafe { libc::kill(pid, 0) };
        assert_eq!(exists, -1, "direct child must be reaped");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
    assert_eq!(
        fs::read(root.join("sentinel")).expect("sentinel"),
        b"caller-owned"
    );
    assert!(
        !fs::read_dir(root).expect("listing").any(|e| e
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".av1an-stats-"))
    );
}

#[test]
fn e12_scene_child() {
    let Some(root) = std::env::var_os("E12_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let case = std::env::var("E12_CASE").expect("case");
    fs::write(root.join("selected"), "yes").expect("handshake");
    fs::write(root.join("sentinel"), "caller-owned").expect("sentinel");
    if case == "encoder-existing" {
        fs::write(root.join("out.264"), "caller-owned").expect("existing output");
    }
    if case == "ffmpeg-existing" {
        fs::write(root.join("out.mkv"), "caller-owned").expect("existing output");
    }
    // SAFETY: This test runs alone in a supervisor-owned process. Adopt only its
    // fixture grandchildren, and ignore terminal-style SIGINT in the test group.
    unsafe {
        assert_eq!(libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0), 0);
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }
    let cancellation = Cancellation::default();
    if case == "guard-drop" || case == "guard-unwind" || case == "after-completion" {
        let completed = case == "after-completion";
        let cleaned = std::panic::catch_unwind(|| {
            let mut command = Command::new(root.join("fake"));
            command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            if completed {
                command.arg("--complete");
            }
            let mut process =
                OwnedProcess::spawn(&mut command, &cancellation).expect("owned child");
            if completed {
                assert!(process.wait().expect("normal reap").success());
                cancellation.request().expect("graceful after reap");
                cancellation.request().expect("force after reap is disarmed");
            } else {
                wait_file(&root, "grandchild");
                assert_ne!(case, "guard-unwind", "deliberate guard unwind");
            }
        });
        assert_eq!(cleaned.is_err(), case == "guard-unwind");
        reap(&root);
        return;
    }
    if case == "ffms-index" {
        let path = root.join("finite.y4m");
        let mut bytes = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for _ in 0..32 {
            bytes.extend_from_slice(b"FRAME\n");
            bytes.extend(vec![48; 64 * 64 * 3 / 2]);
        }
        fs::write(&path, bytes).expect("native source");
        let cache = root.join("interrupted.ffindex");
        let data = InputModel::Video {
            path,
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: Some(cache.clone()),
            },
            filters: vec![],
        };
        let mut observed = false;
        let result = Input::ensure_indexed_cancel(
            &data,
            |progress| {
                if matches!(progress, crate::core::input::OpenProgress::Indexing { .. })
                    && !observed
                {
                    observed = true;
                    cancellation.request().expect("graceful native index");
                    cancellation.request().expect("force native index");
                }
            },
            &cancellation,
        );
        assert!(observed, "real FFMS2 indexing callback ran");
        assert_eq!(
            result.err().expect("native indexing force").downcast_ref::<Cancelled>(),
            Some(&Cancelled::Forced)
        );
        assert!(!cache.exists(), "interrupted new index cannot be reused");
        reap(&root);
        return;
    }
    if case == "pre-spawn" {
        cancellation.request().expect("graceful");
        let (_tx, rx) = crossbeam_channel::bounded(1);
        let (progress, _) = mpsc::channel();
        assert_eq!(
            encoder(&root, true)
                .encode_with_stream_cancel(
                    rx,
                    &root.join("out.264"),
                    progress,
                    &tests::clip_info(),
                    &cancellation
                )
                .expect_err("graceful cannot admit a fresh scene")
                .downcast_ref::<Cancelled>(),
            Some(&Cancelled::Graceful)
        );
        cancellation.request().expect("forced");
        let (_tx, rx) = crossbeam_channel::bounded(1);
        let (progress, _) = mpsc::channel();
        assert!(
            encoder(&root, true)
                .encode_with_stream_cancel(
                    rx,
                    &root.join("out.264"),
                    progress,
                    &tests::clip_info(),
                    &cancellation
                )
                .expect_err("pre-spawn force")
                .is::<Cancelled>()
        );
        assert!(!root.join("leader").exists());
        reap(&root);
        return;
    }
    // All PATH mutation precedes controller/relay threads in this isolated process.
    for name in ["ffmpeg", "mkvmerge"] {
        std::os::unix::fs::symlink(root.join("fake"), root.join(name)).expect("fixture launcher");
    }
    let path = std::env::join_paths(std::iter::once(root.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").expect("PATH"),
    )))
    .expect("PATH");
    // SAFETY: No other test runs in this process, and no threads have been started.
    unsafe {
        std::env::set_var("PATH", path);
    }
    let root2 = root.clone();
    let signal = cancellation.clone();
    let kind = case.clone();
    let controller = thread::spawn(move || {
        if kind == "capture" {
            while !fs::read_dir(&root2).expect("listing").any(|e| {
                e.expect("entry").file_name().to_string_lossy().starts_with(".av1an-stats-")
            }) {
                thread::yield_now();
            }
        } else if kind.starts_with("graceful") {
            wait_file(&root2, "active");
        } else {
            wait_file(&root2, "grandchild");
        }
        if kind == "stdin-full" {
            wait_file(&root2, "queued");
        }
        if kind == "filter-full" {
            wait_file(&root2, "full-output");
        }
        if kind == "leader-exits" {
            let pid: u32 =
                fs::read_to_string(root2.join("leader")).expect("PID").parse().expect("PID");
            loop {
                // SAFETY: Observe only our still-owned direct child, without reaping it.
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                // SAFETY: Observe the known direct child into a valid siginfo, without reaping.
                let observed = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        pid,
                        &mut info,
                        libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                    )
                };
                assert_eq!(observed, 0);
                // SAFETY: Successful waitid initializes the SIGCHLD union member.
                if unsafe { info.si_pid() } != 0 {
                    break;
                }
                thread::yield_now();
            }
        }
        signal.request().expect("graceful");
        if kind.starts_with("graceful") {
            if kind == "graceful-sigint" {
                // SAFETY: The supervisor gave this test its own group; managed children lead other groups.
                assert_eq!(unsafe { libc::kill(0, libc::SIGINT) }, 0);
            }
            fs::write(root2.join("release"), "finish active scene").expect("release");
        } else {
            signal.request().expect("force");
        }
    });
    let result: Result<()> = if case.starts_with("filter") {
        let (_tx, rx) = crossbeam_channel::bounded(1);
        let mut stage = FfmpegFilter::spawn_cancel(
            rx,
            1,
            "null",
            Arc::new(std::sync::Mutex::new(None)),
            &cancellation,
        )
        .expect("filter");
        let output = stage.take_receiver().expect("receiver");
        if case == "filter-full" {
            while output.is_empty() {
                thread::yield_now();
            }
            fs::write(root.join("full-output"), "bounded output full").expect("full handshake");
            assert!(cancellation.forced().recv().is_err());
        } else {
            let _ = output.recv();
        }
        drop(output);
        stage.check()
    } else if case == "dg" {
        fs::write(root.join("source"), "finite fixture source").expect("source");
        crate::vapoursynth::plugins::dgdecodenv::DGSource::index_video_cancel(
            &root.join("source"),
            Some(&root.join("out.dgi")),
            Some(&root.join("fake")),
            None,
            &cancellation,
        )
        .map(|_| ())
    } else if case.starts_with("ffmpeg") || case.starts_with("mkv") {
        fs::create_dir_all(root.join("Scene Concatenator")).expect("scratch");
        let scenes = vec![root.join("scene.264"); if case == "mkv-group" { 513 } else { 1 }];
        let (tx, _rx) = mpsc::channel();
        if case.starts_with("ffmpeg") {
            SceneConcatenator::ffmpeg_cancel(
                &root,
                &root.join("out.mkv"),
                &scenes,
                None,
                1,
                av_format::rational::Ratio::new(24, 1),
                None,
                &tx,
                &cancellation,
            )
        } else {
            SceneConcatenator::mkvmerge_cancel(
                &root,
                &root.join("out.mkv"),
                &scenes,
                None,
                av_format::rational::Ratio::new(24, 1),
                None,
                &tx,
                &cancellation,
            )
        }
    } else if ["capture", "stream-idle", "encoder-existing", "stdin-full", "replay-full"]
        .contains(&case.as_str())
    {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let full = case == "stdin-full" || case == "replay-full";
        let _keep_alive = (!full).then(|| tx.clone());
        let producer = full.then(|| {
            let replay = case == "replay-full";
            let root = root.clone();
            thread::spawn(move || {
                tx.send(Cursor::new(vec![1; 16 * 1024 * 1024])).expect("first chunk");
                if !replay {
                    tx.send(Cursor::new(vec![2; 1024])).expect("queued chunk");
                    fs::write(root.join("queued"), "yes").expect("queue handshake");
                    assert!(tx.send(Cursor::new(vec![3; 1024])).is_err());
                }
            })
        });
        let (progress, _rx) = mpsc::channel();
        let result = encoder(&root, case == "capture" || case == "replay-full")
            .encode_with_stream_cancel(
                rx,
                &root.join("out.264"),
                progress,
                &tests::clip_info(),
                &cancellation,
            )
            .map(|_| ());
        if let Some(producer) = producer {
            producer.join().expect("bounded producer joined");
        }
        result
    } else {
        let mut input = input(&root, case.ends_with("native"));
        let encoder = encoder(&root, case.starts_with("graceful"));
        let scenes = (0..2)
            .map(|i| Scene {
                start_frame: i,
                end_frame: i + 1,
                sub_scenes: None,
                encoder: encoder.clone(),
                sequence_data: DefaultSequenceData::default(),
            })
            .collect();
        let mut config = DefaultSequenceConfig::default();
        config.parallel_encoder.scenes_directory = root.clone();
        config.parallel_encoder.workers = Some(1);
        let saved = root.join("saved.json");
        let (progress, _rx) = mpsc::channel();
        if case == "zone" || case == "zone-probe" {
            let tasks: Vec<_> = (0..2)
                .map(|i| Task {
                    original_index: i,
                    index: i,
                    frame_indices: vec![i],
                    sub_scenes: None,
                    encoder: encoder.clone(),
                    output: root.join(format!("{i}.264")),
                })
                .collect();
            let plan = ZonePlan::try_build_cancel(&tasks, &cancellation);
            plan.and_then(|plan| {
                ZoneEncoder::encode_tasks_cancel(
                    &mut input,
                    tasks.into(),
                    plan,
                    progress,
                    cancellation,
                )
            })
            .map(|_| ())
        } else {
            let mut condor = Condor::new(
                input,
                Output {
                    path: root.join("unused"),
                    tags: HashMap::new(),
                    video_tags: HashMap::new(),
                },
                encoder,
                scenes,
                Some(config),
                Box::new(move |data| {
                    fs::write(&saved, serde_json::to_vec(&data)?)?;
                    Ok(())
                }),
            );
            let result = if case == "serial" {
                SerialEncoder::new(&root).execute_cancel(&mut condor, progress, cancellation)
            } else {
                ParallelEncoder::default().execute_cancel(&mut condor, progress, cancellation)
            };
            if case.starts_with("graceful") {
                assert_eq!(
                    fs::read_to_string(root.join("passes")).expect("passes"),
                    "1\n2\n"
                );
                assert!(root.join("00000.264").exists());
                assert!(condor.scenes[0].sequence_data.parallel_encoder.bytes.is_some());
                assert!(condor.scenes[1].sequence_data.parallel_encoder.bytes.is_none());
                assert!(root.join("saved.json").exists());
            } else {
                assert!(
                    condor.scenes.iter().all(|scene| scene
                        .sequence_data
                        .parallel_encoder
                        .bytes
                        .is_none())
                );
                assert!(
                    !root.join("saved.json").exists(),
                    "no success callback for forced scenes"
                );
            }
            result.map(|_| ())
        }
    };
    controller.join().expect("controller joined");
    if case.starts_with("graceful") {
        result.expect("active All2 completes");
        assert!(!root.join("queued-decoded").exists());
    } else {
        let error = result.expect_err("force is not success");
        assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
        if [
            "force-vs",
            "force-native",
            "stdin-full",
            "stream-idle",
            "replay-full",
            "serial",
            "zone",
        ]
        .contains(&case.as_str())
        {
            assert!(
                error.chain().any(|cause| cause.is::<EncoderResult>()),
                "forced encoder diagnostics/status must remain typed: {error:#}"
            );
        }
        assert!(!root.join("00000.264").exists());
        if case == "encoder-existing" {
            assert_eq!(
                fs::read(root.join("out.264")).expect("existing output"),
                b"caller-owned"
            );
        } else {
            assert!(!root.join("out.264").exists());
        }
        assert!(!root.join("out.dgi").exists());
        if case == "ffmpeg-existing" {
            assert_eq!(
                fs::read(root.join("out.mkv")).expect("existing output"),
                b"caller-owned"
            );
        } else {
            assert!(!root.join("out.mkv").exists());
        }
    }
    assert!(!root.join("00001.264").exists());
    assert!(!root.join("zone-pass.temp.264").exists());
    assert!(!root.join("Scene Concatenator/00000.mkv").exists());
    reap(&root);
}

#[test]
fn e12_force_wakes_full_replay_and_prevents_next_pass() {
    let cancellation = Cancellation::default();
    let (tx, rx) = crossbeam_channel::bounded(1);
    tx.send(Cursor::new(vec![1; 1024 * 1024])).expect("spool input");
    drop(tx);
    let mut calls = 0;
    let error = spool::encode_cancel(
        Cursor::new(Vec::new()),
        rx,
        Path::new("unused"),
        2,
        &cancellation,
        |_, frames| -> Result<()> {
            calls += 1;
            while frames.len() != 2 {
                thread::yield_now();
            }
            cancellation.request()?;
            cancellation.request()?;
            Err(Cancelled::Forced.into())
        },
    )
    .expect_err("force unblocks the bounded replay producer");
    assert_eq!(calls, 1);
    assert!(error.is::<Cancelled>());

    let cancellation = Cancellation::default();
    let (tx, rx) = crossbeam_channel::bounded(1);
    tx.send(Cursor::new(vec![1; 32])).expect("spool input");
    drop(tx);
    let mut calls = 0;
    let error = spool::encode_cancel(
        Cursor::new(Vec::new()),
        rx,
        Path::new("unused"),
        2,
        &cancellation,
        |pass, frames| {
            calls += 1;
            assert_eq!(pass, 1, "force at pass transition must prevent pass 2");
            assert_eq!(
                frames.into_iter().map(|f| f.into_inner().len()).sum::<usize>(),
                32
            );
            cancellation.request()?;
            cancellation.request()?;
            Ok(())
        },
    )
    .expect_err("force checked before the next pass");
    assert_eq!(calls, 1);
    assert!(error.is::<Cancelled>());
}
