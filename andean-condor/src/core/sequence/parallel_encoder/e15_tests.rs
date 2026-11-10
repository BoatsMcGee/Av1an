//! Removing scene replay or retry isolation breaks these real public jobs.
use super::*;
use crate::{
    core::output::Output,
    models::{
        encoder::{Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter},
        scene::Scene,
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    },
    utils::cancellation::Cancellation,
};
use std::{fs, os::unix::fs::PermissionsExt, process::Command, sync::Mutex};

pub(super) fn after_failure(token: &Cancellation) {
    let Ok(case) = std::env::var("E08_CASE") else {
        return;
    };
    if !case.starts_with("e15-") {
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os("E08_ROOT").expect("root"));
    for output in fs::read_to_string(root.join("outputs")).unwrap_or_default().lines() {
        if std::path::Path::new(output).starts_with(root.join("scenes")) {
            assert!(
                !std::path::Path::new(output).exists(),
                "failed attempt removed after joins"
            );
        }
    }
    for stats in fs::read_to_string(root.join("stat-paths"))
        .unwrap_or_default()
        .lines()
        .filter(|s| !s.is_empty())
    {
        assert!(
            !std::path::Path::new(stats).exists(),
            "failed statistics removed after reap"
        );
    }
    if case.ends_with("graceful") && !case.ends_with("all2-graceful") {
        token.request().expect("graceful at retry admission");
    }
}

const ENCODER: &str = r#"#!/bin/sh
set -eu
out= root= stats= pass=1
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) out=$2; shift ;;
        --stats) stats=$2; shift ;;
        --pass) pass=$2; shift ;;
        --e15-root) root=$2; shift ;;
    esac
    shift
done
n=0
if [ -f "$root/count" ]; then n=$(cat "$root/count"); fi
n=$((n+1))
printf '%s' "$n" > "$root/count"
printf started > "$root/started"
case "$E08_CASE" in
    *peer-exhaust)
        case "$root" in
            */peer-1) while [ ! -f "$E08_ROOT/stop-observed" ]; do :; done ;;
            */peer-2) exit 91 ;;
            *) while [ ! -f "$root/peer-1/started" ]; do :; done ;;
        esac ;;
esac
printf '%s\n' "$pass" >> "$root/passes"
printf '%s\n' "$out" >> "$root/outputs"
printf '%s\n' "$stats" >> "$root/stat-paths"
if [ "$pass" = 1 ]; then test ! -f "$out"; fi
case "$E08_CASE" in
    *force)
        sleep 30 &
        printf '%s' "$!" > "$root/grandchild"
        printf '%s' "$$" > "$root/ready"
        wait ;;
    *all2-graceful)
        if [ "$pass" = 1 ]; then
            printf '%s' "$$" > "$root/ready"
            while [ ! -f "$root/release" ]; do :; done
        fi ;;
esac
cat > "$root/input-$n"
if [ "$pass" = 1 ] && [ -n "$stats" ]; then
    test ! -f "$stats"
    printf '%s' "$n" > "$stats"
elif [ -n "$stats" ]; then
    test -s "$stats"
fi
case "$E08_CASE" in
    *all2) fail=2 ;;
    *) fail=1 ;;
esac
case "$E08_CASE" in
    *exhaust*) fail=99 ;;
    *filter-failure) fail=99 ;;
    *all2-graceful|*save|*publication) fail=0 ;;
esac
case "$root" in */peer-*) fail=0 ;; esac
if [ "$n" -le "$fail" ] && { [ "$pass" = 2 ] || [ -z "$stats" ]; }; then
    case "$E08_CASE" in
        *missing) exit 0 ;;
        *zero) : > "$out"; exit 0 ;;
        *wrong) cp "$root/wrong" "$out"; exit 0 ;;
    esac
    printf partial > "$out"
    printf 'last-codec-diagnostic\377' >&2
    exit 42
fi
cp "$root/fixture" "$out"
case "$E08_CASE" in *publication) printf caller-final > "$root/scenes/00000.264" ;; esac
"#;

#[test]
fn e15_saved_budget_and_schema() {
    use crate::models::sequence::parallel_encoder::ParallelEncoderConfig;
    let config: ParallelEncoderConfig =
        serde_json::from_str(r#"{"workers":1,"scenes_directory":"scenes","input":null}"#)
            .expect("old JSON");
    let json = serde_json::to_value(config).expect("saved");
    assert_eq!(json["max_tries"], 3);
    let schema =
        serde_json::to_value(schemars::schema_for!(ParallelEncoderConfig)).expect("schema");
    assert_eq!(schema["properties"]["max_tries"]["default"], 3);
    assert_eq!(schema["properties"]["max_tries"]["minimum"], 1);
}

#[test]
fn e15_bounded_public_retry_jobs() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    fs::write(
        root.join("watchdog.rs"),
        include_str!("../../encoder/e08_fixture.rs"),
    )
    .expect("supervisor");
    assert!(
        Command::new("rustc")
            .arg(root.join("watchdog.rs"))
            .arg("-o")
            .arg(root.join("watchdog"))
            .status()
            .expect("rustc")
            .success()
    );
    let mut failures = Vec::new();
    for path in ["native", "vs"] {
        for kind in [
            "once",
            "filter",
            "filter-failure",
            "all2",
            "exhaust1",
            "exhaust2",
            "exhaust3",
            "missing",
            "zero",
            "wrong",
            "exhaust-missing",
            "exhaust-zero",
            "exhaust-wrong",
            "graceful",
            "all2-graceful",
            "force",
            "cancelled",
            "budget0",
            "preflight",
            "source",
            "save",
            "publication",
            "peers",
            "peer-exhaust",
            "direct",
            "direct-exhaust",
        ] {
            let case = format!("e15-{path}-{kind}");
            let job = root.join(&case);
            fs::create_dir_all(&job).expect("job");
            let output = Command::new(root.join("watchdog"))
                .arg("--e08-watchdog")
                .arg(std::env::current_exe().expect("binary"))
                .arg(&job)
                .arg(&case)
                .arg("core::sequence::parallel_encoder::e15_tests::e15_public_child")
                .output()
                .expect("bounded child");
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            eprintln!("E15 {case}: {text}");
            if !output.status.success()
                || !job.join("verified").exists()
                || !text.contains("test result: ok. 1 passed;")
                || !text
                    .lines()
                    .any(|line| line == "E08 watchdog reaped 0 descendants; timeout=false")
            {
                failures.push(format!("{case}: {text}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn e15_public_child() {
    let Some(root) = std::env::var_os("E08_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let case = std::env::var("E08_CASE").expect("case");
    let Some(rest) = case.strip_prefix("e15-") else {
        return;
    };
    let (path, kind) = rest.split_once('-').expect("path-kind");
    assert_eq!(
        // SAFETY: This isolated child adopts only its own fixture descendants.
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
        0
    );
    let fixture = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=s=64x64:r=24",
            "-frames:v",
            "3",
            "-c:v",
            "libx264",
            "-threads",
            "1",
            "-f",
            "h264",
        ])
        .arg(root.join("fixture"))
        .output()
        .expect("fixture");
    assert!(fixture.status.success(), "{fixture:?}");
    if kind.ends_with("wrong") {
        let fixture = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=s=64x64:r=24",
                "-frames:v",
                "2",
                "-c:v",
                "libx264",
                "-threads",
                "1",
                "-f",
                "h264",
            ])
            .arg(root.join("wrong"))
            .output()
            .expect("wrong fixture");
        assert!(fixture.status.success());
    }
    fs::write(root.join("encoder"), ENCODER).expect("encoder");
    fs::set_permissions(root.join("encoder"), fs::Permissions::from_mode(0o755))
        .expect("executable");
    let input = e11_tests::input(&root, path == "native");
    let index_before = fs::read(root.join("native.ffindex")).ok();
    let scenes = root.join("scenes");
    fs::create_dir_all(&scenes).expect("scratch");
    fs::write(scenes.join("00000.temp.264"), b"caller-owned").expect("sentinel");
    let output = scenes.join("00000.264");
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
    encoder.set_append_color_data(false);
    encoder.set_executable(Some(root.join("encoder")));
    encoder.parameters_mut().insert(
        "e15-root".into(),
        CLIParameter::new_string("--", " ", root.to_str().expect("UTF8")),
    );
    if kind == "filter" {
        encoder.set_ffmpeg_filter(Some("null".into()));
    }
    if kind == "filter-failure" {
        encoder.set_ffmpeg_filter(Some("e15_missing_filter".into()));
    }
    if kind.starts_with("all2") {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    }
    if kind == "preflight" {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(3);
    }
    let tries = kind
        .strip_prefix("exhaust")
        .map(|n| n.parse::<u32>().unwrap_or(2))
        .or_else(|| (kind == "filter-failure").then_some(3));
    let tries = tries.or_else(|| (kind == "direct-exhaust").then_some(3));
    let mut config = DefaultSequenceConfig::default();
    let mut json = serde_json::to_value(&config.parallel_encoder).expect("JSON");
    if let Some(tries) = tries {
        json["max_tries"] = tries.into();
    }
    if kind == "budget0" {
        json["max_tries"] = 0.into();
    }
    config.parallel_encoder = serde_json::from_value(json).expect("saved config");
    config.parallel_encoder.workers = Some(1);
    config.parallel_encoder.scenes_directory = scenes.clone();
    let peers = matches!(kind, "peers" | "peer-exhaust");
    if peers {
        config.parallel_encoder.workers = Some(2);
    }
    let saved = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&saved);
    let fail_save = kind == "save";
    let mut condor = Condor::new(
        input,
        Output {
            path: root.join("unused"),
            tags: Default::default(),
            video_tags: Default::default(),
        },
        encoder.clone(),
        vec![Scene {
            start_frame: 1,
            end_frame: if kind == "source" { 98 } else { 4 },
            sub_scenes: None,
            encoder,
            sequence_data: DefaultSequenceData::default(),
        }],
        Some(config),
        Box::new(move |data| {
            observed
                .lock()
                .expect("snapshots")
                .push(data.scenes[0].sequence_data.parallel_encoder.clone());
            if fail_save {
                anyhow::bail!("e15 result save failure");
            }
            Ok(())
        }),
    );
    if peers {
        for index in 1..=2 {
            let peer = root.join(format!("peer-{index}"));
            fs::create_dir_all(&peer).expect("peer input logs");
            fs::copy(root.join("fixture"), peer.join("fixture")).expect("peer output fixture");
            let mut encoder = condor.encoder.clone();
            encoder.parameters_mut().insert(
                "e15-root".into(),
                CLIParameter::new_string("--", " ", peer.to_str().expect("UTF8")),
            );
            condor.scenes.push(Scene {
                start_frame: 1,
                end_frame: 4,
                sub_scenes: None,
                encoder,
                sequence_data: DefaultSequenceData::default(),
            });
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    if kind == "budget0" {
        let scratch = root.join("must-not-create");
        condor.sequence_config.parallel_encoder.scenes_directory = scratch.clone();
        let error = ParallelEncoder::default()
            .initialize(&mut condor, tx.clone())
            .expect_err("zero before initialization IO");
        assert!(error.to_string().contains("max_tries"));
        assert!(!scratch.exists(), "zero must not create scratch");
    }
    let token = Cancellation::default();
    if kind == "cancelled" {
        token.request().expect("already cancelled");
    }
    let result = std::thread::scope(|scope| {
        let controller = matches!(kind, "force" | "all2-graceful").then(|| {
            scope.spawn(|| {
                while !root.join("ready").exists() {
                    std::thread::yield_now();
                }
                token.request().expect("first request");
                if kind == "force" {
                    token.request().expect("force request");
                }
                fs::write(root.join("release"), "continue").expect("release active attempt");
            })
        });
        let result = if kind.starts_with("direct") {
            ParallelEncoder::encode_tasks_cancel(
                &mut condor.input,
                1,
                vec![Task {
                    original_index: 7,
                    index: 0,
                    frame_indices: vec![1, 2, 3],
                    sub_scenes: None,
                    encoder: condor.encoder.clone(),
                    output: output.clone(),
                }]
                .into(),
                tx,
                token.clone(),
            )
            .map(|results| {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].as_ref().expect("direct result").scene, 7);
                ((), vec![])
            })
        } else {
            ParallelEncoder::default().execute_cancel(&mut condor, tx, token.clone())
        };
        if let Some(controller) = controller {
            controller.join().expect("controller");
        }
        result
    });
    let calls: u32 = fs::read_to_string(root.join("count"))
        .ok()
        .map_or(0, |s| s.parse().expect("count"));
    let events: Vec<_> = rx.try_iter().collect();
    if peers {
        if kind == "peers" {
            result.expect("transient failure must not stop queued/healthy scenes");
            assert_eq!(calls, 2);
            assert!(
                condor.scenes.iter().all(|scene| scene
                    .sequence_data
                    .parallel_encoder
                    .bytes
                    .is_some())
            );
            assert!(
                root.join("peer-2/started").exists(),
                "queued work still admitted"
            );
        } else {
            let error = result.expect_err("scene zero exhausts exactly three attempts");
            assert_eq!(calls, 3);
            assert!(format!("{error:#}").contains("attempt 3/3"));
            assert!(condor.scenes[0].sequence_data.parallel_encoder.bytes.is_none());
            assert!(
                condor.scenes[1].sequence_data.parallel_encoder.bytes.is_some(),
                "successful active peer saved after terminal failure"
            );
            assert!(
                !root.join("peer-2/started").exists(),
                "queued work stopped only on exhaustion"
            );
            assert_eq!(
                saved.lock().expect("saved").len(),
                1,
                "active peer saved once"
            );
        }
        assert_eq!(
            events
                .iter()
                .filter(|s| matches!(
                    s,
                    SequenceStatus::Subprocess {
                        child: Status::Completed { .. },
                        ..
                    }
                ))
                .count(),
            if kind == "peers" { 3 } else { 1 }
        );
    } else if matches!(kind, "budget0" | "preflight" | "cancelled") {
        if kind == "cancelled" {
            result.expect("no scenes admitted");
        } else {
            result.expect_err("invalid config before input/spawn");
        }
        assert_eq!(calls, 0);
        assert!(saved.lock().expect("saved").is_empty());
        assert_eq!(
            fs::read(root.join("native.ffindex")).ok(),
            index_before,
            "no input consumption/index changes"
        );
    } else if matches!(kind, "source" | "save" | "publication" | "force") {
        let error = result.expect_err("terminal failure");
        assert!(calls <= 1, "no retry for non-attempt failure: {error:#}");
        if kind == "force" {
            assert_eq!(calls, 1);
            assert_eq!(
                error.downcast_ref::<crate::utils::cancellation::Cancelled>(),
                Some(&crate::utils::cancellation::Cancelled::Forced)
            );
            let pid: i32 = fs::read_to_string(root.join("grandchild"))
                .expect("grandchild")
                .parse()
                .expect("pid");
            let mut status = 0;
            // SAFETY: Reap the known adopted grandchild, never signal or rescue it.
            assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
            assert!(libc::WIFSIGNALED(status));
            assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
            for file in ["ready", "grandchild"] {
                let pid = fs::read_to_string(root.join(file)).expect("actual child pid");
                assert!(
                    !std::path::Path::new(&format!("/proc/{pid}")).exists(),
                    "owned process reaped"
                );
            }
        }
        if kind == "publication" {
            assert_eq!(fs::read(&output).expect("existing final"), b"caller-final");
        }
        if kind != "save" {
            assert!(saved.lock().expect("saved").is_empty());
        }
    } else if tries.is_some() || kind == "graceful" {
        let tries = tries.unwrap_or(1);
        let error = result.expect_err("exhausted");
        if kind == "graceful" {
            assert_eq!(
                error.downcast_ref::<crate::utils::cancellation::Cancelled>(),
                Some(&crate::utils::cancellation::Cancelled::Graceful)
            );
        }
        assert_eq!(calls, tries, "total attempts, not extra retries");
        assert!(
            format!("{error:#}").contains(&format!(
                "attempt {tries}/{}",
                if kind == "graceful" { 3 } else { tries }
            )),
            "{error:#}"
        );
        if !kind.starts_with("exhaust-") {
            let result = match error.downcast_ref::<ParallelEncoderError>() {
                Some(ParallelEncoderError::EncoderFailed { result, .. }) => result,
                _ => error
                    .downcast_ref::<crate::core::encoder::EncoderProcessError>()
                    .expect("typed error")
                    .result
                    .as_ref()
                    .expect("status"),
            };
            assert_eq!(result.status.code(), Some(42));
            assert!(
                matches!(&result.stderr, crate::core::encoder::StringOrBytes::Bytes(bytes) if bytes == b"last-codec-diagnostic\xff")
            );
        }
        assert!(!output.exists());
        assert!(saved.lock().expect("saved").is_empty());
        assert!(condor.scenes[0].sequence_data.parallel_encoder.bytes.is_none());
    } else {
        result.expect("public job retries a transient scene failure");
        assert_eq!(calls, if kind == "all2" { 4 } else { 2 });
        assert_eq!(
            fs::read(&output).expect("published"),
            fs::read(root.join("fixture")).expect("fixture")
        );
        assert_eq!(
            events
                .iter()
                .filter(|s| matches!(
                    s,
                    SequenceStatus::Subprocess {
                        child: Status::Completed { .. },
                        ..
                    }
                ))
                .count(),
            1
        );
        assert!(
            saved
                .lock()
                .expect("saved")
                .iter()
                .all(|d| d.bytes.is_some() && d.completed_on.is_some())
        );
        assert_eq!(
            saved.lock().expect("saved").len(),
            usize::from(kind != "direct"),
            "Sequence persists once; the direct API leaves persistence to its caller"
        );
        let input = fs::read(root.join("input-1")).expect("literal scene input");
        let header_end = input.iter().position(|byte| *byte == b'\n').expect("Y4M header") + 1;
        let header: Vec<_> = std::str::from_utf8(&input[..header_end])
            .expect("header")
            .split_whitespace()
            .collect();
        for token in ["YUV4MPEG2", "W64", "H64", "F24:1"] {
            assert!(header.contains(&token));
        }
        assert_eq!(
            input.len() - header_end,
            3 * (6 + 6144),
            "three literal 64x64 4:2:0 frames, not empty replay"
        );
        for frame in input[header_end..].as_chunks::<{ 6 + 6144 }>().0 {
            assert_eq!(&frame[..6], b"FRAME\n");
            if path == "native" {
                assert!(frame[6..].iter().all(|byte| *byte == 48));
            }
        }
        for n in 2..=calls {
            assert_eq!(
                fs::read(root.join("input-1")).expect("input"),
                fs::read(root.join(format!("input-{n}"))).expect("replayed input")
            );
        }
        if kind == "all2" {
            assert_eq!(
                fs::read_to_string(root.join("passes")).expect("pass order"),
                "1\n2\n1\n2\n"
            );
        }
        if kind == "all2-graceful" {
            assert_eq!(
                fs::read_to_string(root.join("passes")).expect("active pass order"),
                "1\n2\n"
            );
        }
    }
    assert_eq!(
        fs::read(scenes.join("00000.temp.264")).expect("caller temp"),
        b"caller-owned"
    );
    assert_eq!(
        fs::read_dir(scenes).expect("scratch").count(),
        1 + if peers {
            if kind == "peers" { 3 } else { 1 }
        } else {
            usize::from(output.exists())
        },
        "no owned output/stats/spool leftovers"
    );
    assert!(
        matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ),
        "all owners joined"
    );
    fs::write(root.join("verified"), "public job checked").expect("verified");
}
