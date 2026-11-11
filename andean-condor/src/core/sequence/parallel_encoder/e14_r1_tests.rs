//! Fixed-work policy and publication regressions; no throughput comparisons.
use super::*;
use crate::{
    core::{
        output::Output,
        sequence::{benchmarker::Benchmarker, target_quality::TargetQuality},
    },
    models::{
        encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
        scene::Scene,
        sequence::{
            DefaultSequenceConfig, DefaultSequenceData,
            scene_concatenator::ConcatMethod,
            target_quality::{
                TargetQualityConfig,
                types::{QualityMetric, VmafFeature},
            },
        },
    },
    utils::cancellation::Cancellation,
};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
    process::Command,
    sync::Mutex,
};

#[derive(Clone, Default, serde::Serialize)]
struct Config {
    #[serde(flatten)]
    inner: DefaultSequenceConfig,
    benchmarker: crate::models::sequence::benchmarker::BenchmarkerConfig,
}
impl std::ops::Deref for Config {
    type Target = DefaultSequenceConfig;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl std::ops::DerefMut for Config {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
impl crate::models::sequence::SequenceConfigHandler for Config {}
impl crate::models::sequence::benchmarker::BenchmarkerConfigHandler for Config {
    fn benchmarker(
        &self,
    ) -> anyhow::Result<&crate::models::sequence::benchmarker::BenchmarkerConfig> {
        Ok(&self.benchmarker)
    }
    fn benchmarker_mut(
        &mut self,
    ) -> anyhow::Result<&mut crate::models::sequence::benchmarker::BenchmarkerConfig> {
        Ok(&mut self.benchmarker)
    }
}
impl crate::models::sequence::parallel_encoder::ParallelEncoderConfigHandler for Config {
    fn parallel_encoder(
        &self,
    ) -> anyhow::Result<&crate::models::sequence::parallel_encoder::ParallelEncoderConfig> {
        Ok(&self.inner.parallel_encoder)
    }
    fn parallel_encoder_mut(
        &mut self,
    ) -> anyhow::Result<&mut crate::models::sequence::parallel_encoder::ParallelEncoderConfig> {
        Ok(&mut self.inner.parallel_encoder)
    }
}
impl crate::models::sequence::target_quality::TargetQualityConfigHandler for Config {
    fn target_quality(&self) -> anyhow::Result<&Option<TargetQualityConfig>> {
        Ok(&self.inner.target_quality)
    }
    fn target_quality_mut(&mut self) -> anyhow::Result<&mut Option<TargetQualityConfig>> {
        Ok(&mut self.inner.target_quality)
    }
}
impl crate::models::sequence::scene_concatenator::SceneConcatenatorConfigHandler for Config {
    fn protected_sources(&self) -> Vec<std::path::PathBuf> {
        crate::models::sequence::scene_concatenator::SceneConcatenatorConfigHandler::protected_sources(&self.inner)
    }

    fn scene_concatenator(
        &self,
    ) -> anyhow::Result<&crate::models::sequence::scene_concatenator::SceneConcatenatorConfig> {
        Ok(&self.inner.scene_concatenator)
    }
    fn scene_concatenator_mut(
        &mut self,
    ) -> anyhow::Result<&mut crate::models::sequence::scene_concatenator::SceneConcatenatorConfig>
    {
        Ok(&mut self.inner.scene_concatenator)
    }
}

pub(in crate::core::sequence) fn fixed_batch() -> Option<(usize, usize)> {
    std::env::var("E08_CASE")
        .is_ok_and(|case| case.starts_with("r1-worker-"))
        .then_some((1, 3))
}
pub(in crate::core::sequence) fn finish_fixed_batch(token: &Cancellation) {
    if fixed_batch().is_some() {
        fs::write(
            Path::new(&std::env::var_os("E08_ROOT").expect("root")).join("fixed-batch"),
            "one batch, no threshold or throughput comparison",
        )
        .expect("batch handshake");
        token.request().expect("stop before another trial");
    }
}
pub(super) fn after_rename(output: &Path) {
    if std::env::var("E08_CASE").is_ok_and(|case| case.starts_with("r1-observer-")) {
        let root = std::path::PathBuf::from(std::env::var_os("E08_ROOT").expect("root"));
        assert!(output.is_file(), "real rename precedes receiver failure");
        fs::write(root.join("renamed"), "published").expect("rename handshake");
        while !root.join("observer-dropped").exists() {
            std::thread::yield_now();
        }
    }
}

const CODEC: &str = r#"#!/bin/sh
out= previous=
for argument in "$@"; do
    if [ "$previous" = -o ]; then out=$argument; fi
    previous=$argument
done
if [ -n "$out" ] && [ "$out" != /dev/null ]; then
    printf '%s\n' "$out" >> "$E08_ROOT/starts"
    case "$E08_CASE" in
        *-zero) cat >/dev/null; : > "$out"; exit 0 ;;
        *-status) cat >/dev/null; printf partial > "$out"; exit 42 ;;
    esac
fi
"$E14_X264" "$@"
status=$?
if [ -n "$out" ] && [ "$out" != /dev/null ] && [ -f "$out" ]; then cp "$out" "$E08_ROOT/encoded"; fi
exit "$status"
"#;

#[test]
fn e14_r1_bounded_policy_resume_and_observers() {
    let dir = tempfile::tempdir().expect("fixture");
    fs::write(
        dir.path().join("watchdog.rs"),
        include_str!("../../encoder/e08_fixture.rs"),
    )
    .expect("supervisor");
    assert!(
        Command::new("rustc")
            .arg(dir.path().join("watchdog.rs"))
            .arg("-o")
            .arg(dir.path().join("watchdog"))
            .status()
            .expect("rustc")
            .success()
    );
    let mut failures = vec![];
    for case in [
        "worker-optout",
        "worker-no-probe-optout",
        "worker-strict",
        "worker-zero",
        "worker-status",
        "preflight-worker",
        "preflight-tq",
        "preflight-tq-api",
        "preflight-parallel",
        "preflight-zone",
        "preflight-encoder",
        "tq-individual-optout",
        "tq-zone-optout",
        "tq-individual-zero",
        "tq-zone-status",
        "resume-policy",
        "resume-probe",
        "resume-graceful",
        "observer-native",
        "observer-vs",
    ] {
        let job = dir.path().join(case);
        fs::create_dir_all(job.join("bin")).expect("bin");
        for tool in ["ffmpeg", "cat", "cp", "x264", "sleep"] {
            symlink(
                which::which(tool).expect("installed fixture tool"),
                job.join("bin").join(tool),
            )
            .expect("isolated PATH");
        }
        if matches!(case, "worker-optout" | "worker-strict")
            || case.starts_with("observer-")
            || case == "resume-policy"
        {
            symlink(
                which::which("ffprobe").expect("installed ffprobe"),
                job.join("bin/ffprobe"),
            )
            .expect("probe");
        } else if case.starts_with("resume-") {
            fs::write(
                job.join("bin/ffprobe"),
                r#"#!/bin/sh
printf probe >> "$E08_ROOT/probes"
if [ -f "$E08_ROOT/probe-bad" ]; then printf '{"streams":[{"nb_read_frames":"N/A"}]}'; exit 0; fi
if [ "$E08_CASE" = r1-resume-graceful ]; then
    printf ready > "$E08_ROOT/probe-active"
    for attempt in 1 2 3 4 5; do
        [ -f "$E08_ROOT/release-probe" ] && break
        sleep 1
    done
    [ -f "$E08_ROOT/release-probe" ] || exit 124
fi
exec "$E14_FFPROBE" "$@"
"#,
            )
            .expect("probe control");
            fs::set_permissions(job.join("bin/ffprobe"), fs::Permissions::from_mode(0o755))
                .expect("probe executable");
        }
        let output = Command::new(dir.path().join("watchdog"))
            .arg("--e08-watchdog")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(&job)
            .arg(format!("r1-{case}"))
            .arg("core::sequence::parallel_encoder::e14_r1_tests::e14_r1_child")
            .env("PATH", job.join("bin"))
            .env("E14_X264", which::which("x264").expect("x264"))
            .env("E14_FFPROBE", which::which("ffprobe").expect("ffprobe"))
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
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn size(status: &SequenceStatus) -> bool {
    matches!(status, SequenceStatus::Whole(Status::Processing { completion: SequenceCompletion::Custom { name, .. }, .. }) if matches!(name.as_str(), "size" | "resumed-size"))
}

#[test]
fn e14_r1_child() {
    let Some(root) = std::env::var_os("E08_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let case = std::env::var("E08_CASE").expect("case");
    let case = case.strip_prefix("r1-").expect("R1 selection");
    fs::write(root.join("codec"), CODEC).expect("codec");
    fs::set_permissions(root.join("codec"), fs::Permissions::from_mode(0o755)).expect("executable");
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
    encoder.set_executable(Some(root.join("codec")));
    encoder.set_append_color_data(false);
    encoder.set_quantizer(20.0);
    encoder
        .parameters_mut()
        .insert("demuxer".into(), CLIParameter::new_string("--", " ", "y4m"));
    encoder.parameters_mut().insert(
        "preset".into(),
        CLIParameter::new_string("--", " ", "ultrafast"),
    );
    encoder
        .parameters_mut()
        .insert("bframes".into(), CLIParameter::new_number("--", " ", 0.0));
    if case.starts_with("worker-") {
        encoder.set_ffmpeg_filter(Some("select=not(mod(n\\,2))".into()));
    }
    let input = e11_tests::input(&root, case != "observer-vs");
    let scenes_dir = root.join("scenes");
    fs::create_dir_all(scenes_dir.join("Benchmarker")).expect("scratch");
    fs::write(scenes_dir.join("Benchmarker/sentinel"), "caller-owned").expect("scratch sentinel");
    let mut config = Config::default();
    config.parallel_encoder.scenes_directory = scenes_dir.clone();
    config.parallel_encoder.workers = if case.starts_with("worker-") {
        None
    } else {
        Some(1)
    };
    config.parallel_encoder.validate_frame_count = !case.ends_with("optout")
        && !matches!(
            case,
            "worker-zero" | "worker-status" | "tq-individual-zero" | "tq-zone-status"
        );
    if case.starts_with("tq-") || case == "preflight-tq" {
        config.target_quality = Some(TargetQualityConfig {
            maximum_probes: 1,
            metric: QualityMetric::VMAF {
                target_range: (-1000.0, 1000.0),
                resolution: None,
                scaler: "bicubic".into(),
                threads: 1,
                model: None,
                features: vec![VmafFeature::Default],
            },
            ..Default::default()
        });
        config.scene_concatenator.method = ConcatMethod::FFmpeg(Default::default());
    }
    let saved = Arc::new(Mutex::new(vec![]));
    let snapshots = Arc::clone(&saved);
    let save_path = root.join("saved-job.json");
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
            start_frame: 0,
            end_frame: 3,
            sub_scenes: None,
            encoder,
            sequence_data: DefaultSequenceData::default(),
        }],
        Some(config),
        Box::new(move |data| {
            fs::write(&save_path, serde_json::to_vec(&data)?)?;
            snapshots.lock().expect("snapshots").push(
                data.scenes
                    .iter()
                    .map(|s| s.sequence_data.parallel_encoder.clone())
                    .collect::<Vec<_>>(),
            );
            Ok(())
        }),
    );
    let token = Cancellation::default();
    let (tx, rx) = std::sync::mpsc::channel();
    if case.starts_with("preflight-") {
        let result = match case {
            "preflight-worker" => Benchmarker::default().validate(&mut condor).map(|_| ()),
            "preflight-tq" => TargetQuality::default().validate(&mut condor).map(|_| ()),
            "preflight-tq-api" => TargetQuality::probe_pass_cancel(
                1,
                &scenes_dir,
                &TargetQualityConfig::default(),
                &mut condor.input,
                None,
                1,
                &ConcatMethod::FFmpeg(Default::default()),
                &[crate::core::sequence::target_quality::Task {
                    original_index: 0,
                    frame_indices: vec![0, 1, 2],
                    encoder: condor.encoder.clone(),
                    output: scenes_dir.join("new.264"),
                    passes: vec![],
                }],
                tx,
                &token,
            )
            .map(|_| ()),
            "preflight-parallel" | "preflight-encoder" => {
                if case == "preflight-encoder" {
                    condor.scenes[0].encoder.set_executable(Some(root.join("missing-encoder")));
                }
                ParallelEncoder::encode_tasks_cancel(
                    &mut condor.input,
                    1,
                    vec![Task {
                        index: 0,
                        original_index: 0,
                        frame_indices: vec![0, 1, 2],
                        sub_scenes: None,
                        encoder: condor.scenes[0].encoder.clone(),
                        output: scenes_dir.join("new.264"),
                    }]
                    .into(),
                    tx,
                    token,
                )
                .map(|_| ())
            },
            _ => {
                use crate::core::sequence::zone_encoder::{ZoneEncoder, ZonePlan};
                let task = Task {
                    index: 0,
                    original_index: 0,
                    frame_indices: vec![0, 1, 2],
                    sub_scenes: None,
                    encoder: condor.encoder.clone(),
                    output: scenes_dir.join("new.264"),
                };
                let plan = ZonePlan::try_build_cancel(std::slice::from_ref(&task), &token)
                    .expect("real zone plan");
                ZoneEncoder::encode_tasks_cancel(
                    &mut condor.input,
                    vec![task].into(),
                    plan,
                    tx,
                    token,
                )
                .map(|_| ())
            },
        };
        let text = format!("{:#}", result.expect_err("early prerequisite failure"));
        assert!(
            text.contains(if case == "preflight-encoder" {
                "missing-encoder"
            } else {
                "ffprobe"
            }),
            "{text}"
        );
        assert!(
            !root.join("starts").exists(),
            "preflight before actual encode"
        );
    } else if case.starts_with("worker-") {
        Benchmarker::default()
            .validate(&mut condor)
            .expect("stage prerequisites with configured count policy");
        let result = Benchmarker::default().execute_cancel(&mut condor, tx, token);
        if case.ends_with("optout") {
            result.expect("saved optout forwarded through default workers discovery");
            assert!(
                root.join("fixed-batch").exists(),
                "fixed single functional batch, no thresholds"
            );
            assert_eq!(
                fs::read_to_string(root.join("starts")).expect("started").lines().count(),
                1
            );
            let path = fs::read_to_string(root.join("starts")).expect("placement");
            assert!(
                Path::new(path.trim()).starts_with(scenes_dir.join("Benchmarker")),
                "job-local trial ownership: {path}"
            );
            assert!(!Path::new(path.trim()).exists(), "owned trial cleaned");
            assert_eq!(
                fs::read(scenes_dir.join("Benchmarker/sentinel")).expect("sentinel"),
                b"caller-owned"
            );
            let counted = Command::new(std::env::var_os("E14_FFPROBE").expect("real probe"))
                .args([
                    "-v",
                    "error",
                    "-count_frames",
                    "-show_entries",
                    "stream=nb_read_frames",
                    "-of",
                    "json",
                ])
                .arg(root.join("encoded"))
                .output()
                .expect("independent actual count");
            assert!(counted.status.success() && counted.stderr.is_empty());
            let count: serde_json::Value =
                serde_json::from_slice(&counted.stdout).expect("count JSON");
            assert_eq!(
                count["streams"][0]["nb_read_frames"], "2",
                "fixed work filter genuinely changes count 3 to 2"
            );
        } else {
            let error = result.expect_err("strict count/status/nonempty remain mandatory");
            assert!(
                !root.join("fixed-batch").exists(),
                "failed batch not completed"
            );
            assert!(
                format!("{error:#}").contains(match case {
                    "worker-strict" => "decoded frame count",
                    "worker-zero" => "nonempty regular file",
                    _ => "42",
                }),
                "{error:#}"
            );
        }
    } else if case.starts_with("tq-") {
        if case.starts_with("tq-individual-") {
            let mut other = condor.scenes[0].clone();
            other.start_frame = 3;
            other.end_frame = 6;
            other.encoder.parameters_mut().insert(
                "preset".into(),
                CLIParameter::new_string("--", " ", "veryfast"),
            );
            condor.scenes.push(other);
        } else {
            let mut other = condor.scenes[0].clone();
            other.start_frame = 3;
            other.end_frame = 6;
            condor.scenes.push(other);
        }
        let target_dir = scenes_dir.join("Target Quality");
        fs::create_dir_all(target_dir.join("1/Scene Concatenator")).expect("pass/concat scratch");
        TargetQuality::default()
            .validate(&mut condor)
            .expect("optout requires no ffprobe, normal stage prerequisites remain");
        let result = TargetQuality::default().execute_cancel(&mut condor, tx, token);
        if case.ends_with("optout") {
            result.expect("configured optout forwarded to actual TQ encode and scoring");
            assert!(
                condor.scenes.iter().all(|s| s.sequence_data.target_quality.passes.len() == 1),
                "real scored pass"
            );
            let paths = fs::read_to_string(root.join("starts")).expect("actual encodes");
            assert_eq!(
                paths.lines().count(),
                if case == "tq-zone-optout" { 1 } else { 2 },
                "actual zone/individual dispatch selected"
            );
        } else {
            let error = result.expect_err("count optout cannot waive status/nonempty");
            assert!(
                format!("{error:#}").contains(if case.ends_with("zero") {
                    "nonempty regular file"
                } else {
                    "42"
                }),
                "{error:#}"
            );
            assert!(condor.scenes.iter().all(|s| s.sequence_data.target_quality.passes.is_empty()));
        }
    } else if case.starts_with("resume-") {
        let final_path = scenes_dir.join("00000.264");
        let frames = if case == "resume-policy" { "2" } else { "3" };
        assert!(
            Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=s=64x64:r=24",
                    "-frames:v",
                    frames,
                    "-c:v",
                    "libx264",
                    "-f",
                    "h264"
                ])
                .arg(&final_path)
                .status()
                .expect("real artifact")
                .success()
        );
        let data = &mut condor.scenes[0].sequence_data.parallel_encoder;
        data.bytes = Some(final_path.metadata().expect("bytes").len());
        data.started_on = Some(10);
        data.completed_on = Some(20);
        let history = serde_json::to_value(&*data).expect("history");
        let saved_job = serde_json::to_vec(&condor.as_data()).expect("saved historical job");
        fs::write(root.join("saved-job.json"), &saved_job)
            .expect("persist history before validation");
        if case == "resume-graceful" {
            let other = condor.scenes[0].clone();
            condor.scenes.push(other);
            fs::copy(&final_path, scenes_dir.join("00001.264")).expect("second candidate");
            let mut queued = condor.scenes[0].clone();
            queued.sequence_data = Default::default();
            condor.scenes.push(queued);
            fs::write(
                root.join("saved-job.json"),
                serde_json::to_vec(&condor.as_data()).expect("three-scene history"),
            )
            .expect("persist all resume candidates");
            std::thread::scope(|scope| {
                let control_root = &root;
                let control_token = token.clone();
                let cancel = scope.spawn(move || {
                    let first = rx.recv().expect("validation progress before probe");
                    assert!(matches!(&first, SequenceStatus::Whole(Status::Processing { completion: SequenceCompletion::Custom { name, completed: 0.0, total: 3.0 }, .. }) if name == "validating"), "explicit candidate progress: {first:?}");
                    while !control_root.join("probe-active").exists() { std::thread::yield_now(); }
                    control_token.request().expect("graceful during first active candidate");
                    fs::write(control_root.join("release-probe"), "finish active probe").expect("release");
                    rx.iter().collect::<Vec<_>>()
                });
                ParallelEncoder::default()
                    .execute_cancel(&mut condor, tx, token.clone())
                    .expect("graceful stops after finite active candidate");
                let events = cancel.join().expect("progress observer");
                assert_eq!(
                    events.iter().filter(|s| size(s)).count(),
                    1,
                    "only first validated candidate eligible"
                );
            });
            assert_eq!(
                fs::read_to_string(root.join("probes")).expect("probe count"),
                "probe",
                "no future candidate admitted"
            );
            assert!(
                !root.join("starts").exists(),
                "no new encode after Graceful"
            );
            for scene in &condor.scenes[..2] {
                assert_eq!(
                    serde_json::to_value(&scene.sequence_data.parallel_encoder).expect("history"),
                    history
                );
            }
        } else {
            if case == "resume-probe" {
                fs::write(root.join("probe-bad"), "transient").expect("probe fault");
            }
            let error = ParallelEncoder::default()
                .execute_cancel(&mut condor, tx, token.clone())
                .expect_err("first check fails closed");
            assert!(format!("{error:#}").contains(if case == "resume-policy" {
                "decoded frame count"
            } else {
                "missing/invalid"
            }));
            assert!(!rx.iter().any(|s| size(&s)), "withhold current eligibility");
            assert_eq!(
                serde_json::to_value(&condor.scenes[0].sequence_data.parallel_encoder)
                    .expect("history"),
                history,
                "recoverable failure must not erase history"
            );
            assert!(
                saved.lock().expect("saved").is_empty(),
                "no destructive save"
            );
            assert_eq!(
                fs::read(root.join("saved-job.json")).expect("persisted history"),
                saved_job,
                "recoverable rejection preserves the actual saved job"
            );
            if case == "resume-policy" {
                condor.sequence_config.parallel_encoder.validate_frame_count = false;
                fs::write(
                    root.join("saved-job.json"),
                    serde_json::to_vec(&condor.as_data()).expect("repaired config"),
                )
                .expect("save explicit policy repair");
            } else {
                fs::remove_file(root.join("probe-bad")).expect("restore healthy probe");
            }
            let (tx, rx) = std::sync::mpsc::channel();
            ParallelEncoder::default()
                .execute_cancel(&mut condor, tx, token)
                .expect("same job recovers after policy/tool repair");
            assert_eq!(rx.iter().filter(size).count(), 1);
            assert_eq!(
                serde_json::to_value(&condor.scenes[0].sequence_data.parallel_encoder)
                    .expect("history"),
                history
            );
            assert!(
                !root.join("starts").exists(),
                "retry reuses historical artifact, no encoder"
            );
        }
    } else {
        std::thread::scope(|scope| {
            let observer = scope.spawn(|| {
                while !root.join("renamed").exists() {
                    std::thread::yield_now();
                }
                drop(rx);
                fs::write(root.join("observer-dropped"), "real receiver disconnected")
                    .expect("observer handshake");
            });
            ParallelEncoder::default()
                .execute_cancel(&mut condor, tx, token)
                .expect("optional observer cannot invalidate publication");
            observer.join().expect("observer joined");
        });
        let data = &condor.scenes[0].sequence_data.parallel_encoder;
        assert_eq!(
            data.bytes,
            Some(scenes_dir.join("00000.264").metadata().expect("published chunk").len())
        );
        assert!(data.started_on.is_some() && data.completed_on.is_some());
        let persisted: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join("saved-job.json")).expect("persisted completed result"),
        )
        .expect("saved JSON");
        assert_eq!(
            persisted["scenes"][0]["sequence_data"]["parallel_encoder"]["bytes"],
            data.bytes.expect("completed bytes")
        );
        assert!(
            saved.lock().expect("saved").iter().any(|batch| batch[0].bytes == data.bytes),
            "published result persisted"
        );
    }
    assert!(
        !scenes_dir.join("00000.temp.264").exists(),
        "consumer/probe/relay owners released"
    );
    fs::write(
        root.join("verified"),
        "R1 functional boundary assertions passed",
    )
    .expect("verified");
}
