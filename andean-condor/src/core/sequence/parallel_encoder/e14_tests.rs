//! Removing publication/resume validation must fail these bounded public-path checks.
use super::*;
use crate::{
    core::output::Output,
    models::{
        encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
        scene::Scene,
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    },
    utils::cancellation::Cancellation,
};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command, sync::Mutex};

const ENCODER: &str = r#"#!/bin/sh
set -eu
out= root=
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) out=$2; shift ;;
        --e14-root) root=$2; shift ;;
    esac
    shift
done
printf started >> "$root/starts"
cat > /dev/null
case "$E08_CASE" in
    *wrong*|*duplicate*) printf '3/3 frames\r3/3 frames\r3/3 frames\r' >&2 ;;
    *sparse*) printf '1/3 frames\r' >&2 ;;
esac
case "$E08_CASE" in
    *missing*) exit 0 ;;
    *zero*) : > "$out" ;;
    *corrupt*) printf corrupt > "$out" ;;
    *) cp "$root/fixture" "$out" ;;
esac
case "$E08_CASE" in e14-native-status|e14-vs-status|*optout-status) exit 42 ;; esac
"#;

#[test]
fn e14_bounded_public_artifact_checks() {
    let dir = tempfile::tempdir().expect("fixture");
    let root = dir.path();
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
        for case in [
            "missing",
            "zero",
            "corrupt",
            "truncated",
            "wrong",
            "status",
            "valid",
            "resume-valid",
            "resume-missing",
            "resume-zero",
            "resume-corrupt",
            "resume-wrong",
            "resume-bare",
            "optout-missing",
            "optout-zero",
            "optout-status",
            "filter-strict",
            "filter-optout",
            "av1",
            "vp9",
            "webm",
            "mkv",
            "hevc",
            "sparse",
            "duplicate",
            "probe-force",
            "probe-json",
            "probe-count",
            "probe-status",
            "probe-error",
            "probe-flood",
            "resume-save-error",
            "api-existing",
        ] {
            let case = format!("e14-{path}-{case}");
            let job = root.join(&case);
            fs::create_dir_all(job.join("bin")).expect("job bin");
            if case.contains("probe-") {
                let probe = job.join("bin/ffprobe");
                fs::write(&probe, r#"#!/bin/sh
set -eu
root=$(dirname "$(dirname "$0")")
printf '%s' "$$" > "$root/probe-started"
case "$E08_CASE" in
    *probe-force) printf ready > "$root/probe-ready"; exec sleep 30 ;;
    *probe-json) printf 'not json' ;;
    *probe-count) printf '{"streams":[{"nb_read_frames":"N/A"}]}' ;;
    *probe-status) printf '{"streams":[{"nb_read_frames":"3"}]}'; exit 42 ;;
    *probe-error) printf '{"streams":[{"nb_read_frames":"3"}]}'; printf 'decoder corruption' >&2 ;;
    *probe-flood) dd if=/dev/zero bs=65536 count=2 2>/dev/null; dd if=/dev/zero bs=65536 count=2 >&2 2>/dev/null ;;
esac
"#).expect("probe control");
                fs::set_permissions(probe, fs::Permissions::from_mode(0o755))
                    .expect("probe executable");
            }
            let output = Command::new(root.join("watchdog"))
                .arg("--e08-watchdog")
                .arg(std::env::current_exe().expect("test binary"))
                .arg(&job)
                .arg(&case)
                .arg("core::sequence::parallel_encoder::e14_tests::e14_public_child")
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        job.join("bin").display(),
                        std::env::var("PATH").expect("PATH")
                    ),
                )
                .output()
                .expect("bounded child");
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
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

fn fixture(root: &Path, codec: &str, container: &str, frames: usize) {
    let mut command = Command::new("ffmpeg");
    command
        .args(["-v", "error", "-f", "lavfi", "-i", "color=s=64x64:r=24", "-frames:v"])
        .arg(frames.to_string())
        .args(["-c:v", codec, "-threads", "1"]);
    if codec == "libx265" {
        command.args(["-x265-params", "pools=1:frame-threads=1"]);
    }
    let output = command
        .args(["-f", container])
        .arg(root.join("fixture"))
        .output()
        .expect("ffmpeg fixture");
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn e14_public_child() {
    let Some(root) = std::env::var_os("E08_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let case = std::env::var("E08_CASE").expect("case");
    let rest = case.strip_prefix("e14-").expect("E14 selection");
    let (path, case) = rest.split_once('-').expect("path-case");
    let resume = case.starts_with("resume-");
    let optout = case.starts_with("optout-") || case == "filter-optout";
    let kind = case
        .strip_prefix("resume-")
        .or_else(|| case.strip_prefix("optout-"))
        .unwrap_or(case);
    let (codec, container) = match kind {
        "av1" => ("librav1e", "ivf"),
        "vp9" => ("libvpx-vp9", "ivf"),
        "webm" => ("libvpx-vp9", "webm"),
        "mkv" => ("libx264", "matroska"),
        "hevc" => ("libx265", "hevc"),
        _ => ("libx264", "h264"),
    };
    fixture(
        &root,
        codec,
        container,
        if kind == "wrong" && !resume { 2 } else { 3 },
    );
    if matches!(kind, "av1" | "vp9") {
        use std::io::{Seek, Write};
        let mut file = fs::OpenOptions::new().write(true).open(root.join("fixture")).expect("IVF");
        file.seek(std::io::SeekFrom::Start(24)).expect("frame header");
        file.write_all(&999_u32.to_le_bytes())
            .expect("lying header must not determine count");
    }
    if kind == "truncated" {
        let file = fs::OpenOptions::new().write(true).open(root.join("fixture")).expect("fixture");
        file.set_len(32).expect("truncate");
    }
    fs::write(root.join("encoder"), ENCODER).expect("encoder");
    fs::set_permissions(root.join("encoder"), fs::Permissions::from_mode(0o755))
        .expect("executable");
    let input = e11_tests::input(&root, path == "native");
    let index_before = fs::read(root.join("native.ffindex")).ok();
    let scenes = root.join("scenes");
    fs::create_dir_all(&scenes).expect("scenes");
    fs::write(scenes.join("sentinel"), "caller-owned").expect("sentinel");
    let output = scenes.join("00000.264");
    if kind == "api-existing" {
        fs::write(&output, "caller-owned existing chunk").expect("existing public API output");
    }
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
    encoder.set_append_color_data(false);
    encoder.set_executable(Some(root.join("encoder")));
    encoder.parameters_mut().insert(
        "e14-root".into(),
        CLIParameter::new_string("--", " ", root.to_str().expect("path")),
    );
    if kind.starts_with("filter-") {
        encoder.set_executable(Some(which::which("x264").expect("native x264")));
        encoder.parameters_mut().remove("e14-root");
        encoder
            .parameters_mut()
            .insert("demuxer".into(), CLIParameter::new_string("--", " ", "y4m"));
        encoder.set_ffmpeg_filter(Some("select=not(mod(n\\,2))".into()));
    }
    let data = DefaultSequenceData::default();
    let mut config = DefaultSequenceConfig::default();
    config.parallel_encoder.workers = Some(1);
    config.parallel_encoder.scenes_directory = scenes.clone();
    config.parallel_encoder.validate_frame_count = !optout;
    let saved = Arc::new(Mutex::new(Vec::new()));
    let snapshots = Arc::clone(&saved);
    let fail_save = case == "resume-save-error";
    let mut condor = Condor::new(
        input,
        Output {
            path: root.join("unused"),
            overwrite: false,
            tags: Default::default(),
            video_tags: Default::default(),
        },
        encoder.clone(),
        vec![Scene {
            start_frame: 1,
            end_frame: 4,
            sub_scenes: None,
            encoder,
            sequence_data: data,
        }],
        Some(config),
        Box::new(move |data| {
            snapshots
                .lock()
                .expect("saved")
                .push(data.scenes[0].sequence_data.parallel_encoder.clone());
            if fail_save {
                anyhow::bail!("E14 save failed");
            }
            Ok(())
        }),
    );
    if resume && kind != "bare" {
        let callback = std::mem::replace(&mut condor.save_callback, Box::new(|_| Ok(())));
        // SAFETY: this entry runs in its own isolated subprocess, with no active
        // workers during either environment change.
        unsafe { std::env::set_var("E08_CASE", "e14-seed-valid") };
        let (tx, _rx) = std::sync::mpsc::channel();
        ParallelEncoder::default()
            .execute_cancel(&mut condor, tx, Cancellation::default())
            .expect("owning publication of a genuinely valid chunk before corruption");
        // SAFETY: the isolated child's seed encoder and relays have all joined.
        unsafe { std::env::set_var("E08_CASE", format!("e14-{path}-{case}")) };
        condor.save_callback = callback;
        fs::remove_file(root.join("starts")).expect("clear only seed invocation marker");
        condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(999999);
        match kind {
            "missing" => fs::remove_file(&output).expect("remove certified chunk"),
            "zero" => fs::write(&output, []).expect("truncate certified chunk"),
            "corrupt" => fs::write(&output, b"corrupt").expect("corrupt certified chunk"),
            "wrong" => {
                fs::remove_file(root.join("fixture")).expect("replace fixture");
                fixture(&root, codec, container, 2);
                fs::copy(root.join("fixture"), &output).expect("replace with wrong count");
            },
            _ => (),
        }
    } else if resume {
        fs::copy(root.join("fixture"), &output).expect("unclaimed bare chunk");
    }
    let original = fs::read(&output).ok();
    let prior_data = condor.scenes[0].sequence_data.parallel_encoder.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let token = Cancellation::default();
    let result = std::thread::scope(|scope| {
        let force = (kind == "probe-force").then(|| {
            scope.spawn(|| {
                while !root.join("probe-ready").exists() {
                    std::thread::yield_now();
                }
                token.request().expect("graceful");
                token.request().expect("force");
            })
        });
        let result = if kind == "api-existing" {
            ParallelEncoder::encode_tasks_cancel(
                &mut condor.input,
                1,
                vec![Task {
                    original_index: 0,
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
            .map(|_| ((), vec![]))
        } else {
            ParallelEncoder::default().execute_cancel(&mut condor, tx, token.clone())
        };
        if let Some(force) = force {
            force.join().expect("force controller");
        }
        result
    });
    let statuses: Vec<_> = rx.try_iter().collect();
    assert!(
        matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ),
        "all relays joined"
    );
    let data = &condor.scenes[0].sequence_data.parallel_encoder;
    if matches!(
        kind,
        "valid"
            | "av1"
            | "vp9"
            | "webm"
            | "mkv"
            | "hevc"
            | "filter-optout"
            | "sparse"
            | "duplicate"
    ) {
        result.expect("eligible artifact");
        assert_eq!(data.bytes, Some(output.metadata().expect("output").len()));
        assert!(data.started_on.is_some() && data.completed_on.is_some());
        assert_eq!(statuses.iter().filter(|s| matches!(s, SequenceStatus::Whole(Status::Processing { completion: SequenceCompletion::Custom { name, .. }, .. }) if matches!(name.as_str(), "size" | "resumed-size"))).count(), 1, "exactly one eligibility/size event");
        if kind == "filter-optout" {
            let counted = Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-count_frames",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=nb_read_frames",
                    "-of",
                    "json",
                ])
                .arg(&output)
                .output()
                .expect("independent filtered count");
            assert!(counted.status.success() && counted.stderr.is_empty());
            let json: serde_json::Value =
                serde_json::from_slice(&counted.stdout).expect("count JSON");
            assert_eq!(
                json["streams"][0]["nb_read_frames"], "2",
                "intentional select filter genuinely changes count"
            );
        }
        if resume {
            assert!(
                !root.join("starts").exists(),
                "no encoder for eligible resume"
            );
            assert_eq!(
                (data.started_on, data.completed_on),
                (prior_data.started_on, prior_data.completed_on),
                "retain real history"
            );
            assert_eq!(
                fs::read(root.join("native.ffindex")).ok(),
                index_before,
                "resume validation never creates/changes an index"
            );
            let saved = saved.lock().expect("saved");
            assert_eq!(
                saved.len(),
                1,
                "correct stale byte count through existing save path once"
            );
            assert_eq!(saved[0].bytes, data.bytes);
        }
    } else {
        let error = result.expect_err("ineligible artifact must not succeed");
        if kind == "probe-force" {
            assert_eq!(
                error.downcast_ref::<crate::utils::cancellation::Cancelled>(),
                Some(&crate::utils::cancellation::Cancelled::Forced)
            );
            let pid = fs::read_to_string(root.join("probe-started")).expect("owned probe pid");
            let pid = pid.parse::<u32>().expect("pid");
            assert!(
                !Path::new(&format!("/proc/{pid}")).exists(),
                "direct silent probe reaped before return, not cleaned up by test"
            );
        }
        let text = format!("{error:#}");
        if kind.starts_with("probe-") {
            assert!(
                root.join("probe-started").exists(),
                "selected actual validation child"
            );
            let diagnostic = match kind {
                "probe-json" => "Malformed ffprobe",
                "probe-count" => "missing/invalid decoded frame count",
                "probe-status" => "exit status: 42",
                "probe-error" => "decoder corruption",
                "probe-flood" => "output exceeded",
                _ => "forced cancellation requested",
            };
            assert!(text.contains(diagnostic), "{text}");
        }
        if kind == "save-error" {
            assert!(
                text.contains("Saving artifact eligibility metadata"),
                "{text}"
            );
        } else {
            assert!(text.to_lowercase().contains("scene 0"), "{text}");
        }
        let withheld_history = resume && matches!(kind, "corrupt" | "wrong" | "missing" | "zero");
        if withheld_history {
            assert_eq!(
                (data.bytes, data.started_on, data.completed_on),
                (
                    prior_data.bytes,
                    prior_data.started_on,
                    prior_data.completed_on
                ),
                "withhold current eligibility without destroying repairable historical records"
            );
        } else if kind == "save-error" {
            assert_eq!(
                data.bytes,
                Some(output.metadata().expect("validated output").len())
            );
            assert_eq!(data.provenance, prior_data.provenance);
            assert!(text.contains("E14 save failed"), "{text}");
        } else {
            assert!(
                data.bytes.is_none() && data.started_on.is_none() && data.completed_on.is_none(),
                "no false metadata"
            );
        }
        assert!(!statuses.iter().any(|s| matches!(s, SequenceStatus::Whole(Status::Processing { completion: SequenceCompletion::Custom { name, .. }, .. }) if matches!(name.as_str(), "size" | "resumed-size"))), "no success-size event");
        assert!(
            !statuses.iter().any(|s| matches!(
                s,
                SequenceStatus::Whole(Status::Completed { .. })
                    | SequenceStatus::Subprocess {
                        child: Status::Completed { .. },
                        ..
                    }
            )),
            "progress cannot complete an invalid artifact"
        );
        if resume || kind == "api-existing" {
            if resume {
                assert!(!root.join("starts").exists(), "fail closed, not overwrite");
            }
            assert_eq!(fs::read(&output).ok(), original, "preserve user artifact");
            if kind != "save-error" {
                assert!(
                    text.contains("new job") && text.contains("00000.264"),
                    "actionable: {text}"
                );
            }
            if resume && kind != "bare" {
                let saved = saved.lock().expect("saved");
                if withheld_history {
                    assert!(
                        saved.is_empty(),
                        "no irreversible withdrawal for probe/count failure"
                    );
                } else {
                    assert!(
                        !saved.is_empty(),
                        "validated stale byte correction reaches the failing save"
                    );
                    assert!(
                        saved
                            .iter()
                            .all(|d| d.bytes == data.bytes && d.completed_on == data.completed_on)
                    );
                }
            }
        } else {
            assert!(!output.exists(), "no final artifact");
            assert!(
                saved.lock().expect("saved").is_empty(),
                "no failed completion callback"
            );
        }
    }
    assert_eq!(
        fs::read(scenes.join("sentinel")).expect("sentinel"),
        b"caller-owned"
    );
    assert_eq!(
        fs::read_dir(&scenes).expect("cleanup").count(),
        1 + usize::from(output.exists()),
        "no owned temps, stats or probe sidecars"
    );
    fs::write(root.join("verified"), "artifact and metadata checked").expect("verified");
}

#[test]
fn e14_old_json_is_strict_and_optout_roundtrips_with_schema_default() {
    use crate::models::sequence::parallel_encoder::ParallelEncoderConfig;
    let old = r#"{"workers":1,"scenes_directory":"scenes","input":null}"#;
    let mut config: ParallelEncoderConfig = serde_json::from_str(old).expect("old config");
    assert!(config.validate_frame_count);
    config.validate_frame_count = false;
    let json = serde_json::to_value(config).expect("serialize policy");
    assert_eq!(json["validate_frame_count"], false);
    let config: ParallelEncoderConfig = serde_json::from_value(json).expect("persisted optout");
    assert!(!config.validate_frame_count);
    let schema = schemars::schema_for!(ParallelEncoderConfig);
    let json = serde_json::to_value(schema).expect("generated schema");
    assert_eq!(json["properties"]["validate_frame_count"]["default"], true);
    assert_eq!(
        json["properties"]["validate_frame_count"]["type"],
        "boolean"
    );
    assert!(
        !json["required"]
            .as_array()
            .expect("required")
            .iter()
            .any(|v| v == "validate_frame_count")
    );
}
