//! Exercise the cached production loader before any layout-sensitive operation.
#![cfg(unix)]

use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use av_metrics_vmaf::{
    BackendPreference, PlaneSource, VideoFormat, VmafConfig, VmafError, VmafScorer,
};
use v_frame::chroma::ChromaSubsampling;

const MARKERS: [&str; 5] = [
    "",
    "vmaf_picture_convert_context_init",
    "vmaf_picture_convert",
    "vmaf_picture_convert_context_close",
    "vmaf_picture_wrap",
];

#[test]
fn m13_rejects_unverified_versions_before_native_setup() {
    run_cases(
        "m13_rejects_unverified_versions_before_native_setup",
        0,
        &[
            "3.2.1",
            "3.1.0",
            "3.3.0",
            "3.2.0-dev",
            "v3.2.1-0-gf85a853",
            "",
            "unknown",
            "non-UTF8",
            "null",
        ],
        false,
    );
}

#[test]
fn m13_rejects_same_version_new_layout_markers_before_native_setup() {
    if std::env::var_os("M13_CHILD").is_some() {
        check_child(false);
        return;
    }
    for marker in 1..MARKERS.len() {
        run_cases(
            "m13_rejects_same_version_new_layout_markers_before_native_setup",
            marker,
            &["3.2.0"],
            false,
        );
    }
}

#[test]
fn m13_accepts_verified_archive_runtime_and_scores() {
    run_cases(
        "m13_accepts_verified_archive_runtime_and_scores",
        0,
        &["3.2.0"],
        true,
    );
}

fn run_cases(test: &str, marker: usize, versions: &[&str], accepted: bool) {
    if std::env::var_os("M13_CHILD").is_some() {
        check_child(accepted);
        return;
    }
    let directory = tempfile::tempdir().expect("temporary M13 runtime fixture");
    let library = directory.path().join(if cfg!(target_os = "macos") {
        "libvmaf.dylib"
    } else {
        "libvmaf.so"
    });
    let output = Command::new("cc")
        .args(["-shared", "-fPIC", "-std=c11", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&library)
        .arg(format!("-DM13_MARKER={marker}"))
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/m13_runtime.c"
        ))
        .output()
        .expect("M13 runtime regression requires cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for version in versions {
        run_child(test, &library, directory.path(), version, MARKERS[marker]);
    }
}

fn run_child(test: &str, library: &Path, directory: &Path, version: &str, marker: &str) {
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", test, "--nocapture"])
        .env("M13_CHILD", "1")
        .env("M13_VERSION", version)
        .env("M13_MARKER_NAME", marker)
        .env("M13_LIBRARY", library)
        .env("VMAF_LIB_DIR", directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("isolated M13 ABI child");
    let start = Instant::now();
    while child.try_wait().expect("child status").is_none() {
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().expect("kill timed out child");
            child.wait().expect("reap timed out child");
            panic!("{version}/{marker}: M13 child exceeded 30s");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("M13 child output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{version}/{marker}: {stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("running 1 test") && stdout.contains("1 passed; 0 failed"),
        "exact child must execute one test: {stdout}"
    );
    eprintln!("{version}/{marker}: {stderr}");
}

fn check_child(accepted: bool) {
    // SAFETY: our isolated fixture stays mapped throughout the counter reads.
    let library = unsafe {
        libloading::Library::new(std::env::var_os("M13_LIBRARY").expect("fixture library"))
    }
    .expect("fixture counters");
    // SAFETY: exact fixture signature and defined indices 0..3, never layout calls.
    let count = unsafe { library.get::<unsafe extern "C" fn(u32) -> u32>(b"m13_count\0") }
        .expect("M13 counter");
    // SAFETY: the mapping remains live; every index is in range.
    let snapshot = || (0..3).map(|index| unsafe { count(index) }).collect::<Vec<_>>();
    let format = VideoFormat {
        width: 8,
        height: 8,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
    };
    for backend in [
        BackendPreference::CpuOnly,
        BackendPreference::Auto,
        BackendPreference::RequireCuda,
    ] {
        if accepted && backend != BackendPreference::CpuOnly {
            break;
        }
        let result = VmafScorer::new(VmafConfig::new().with_backend(backend), format);
        if accepted {
            let mut scorer = result.expect("verified archive runtime accepted");
            let bytes = [128u8; 64];
            let planes = [(8, 8), (4, 4), (4, 4)].map(|(w, h)| {
                PlaneSource::from_bytes(&bytes, w, h, w as isize, 0, 1).expect("planes")
            });
            scorer.submit_pair_raw(planes, planes).expect("submit through actual scorer");
            assert_eq!(
                scorer.finish().expect("score marker"),
                [99.0],
                "accepted ABI reaches native scoring"
            );
            assert_eq!(
                snapshot(),
                [1, 1, 4],
                "accepted constructor and replacement pictures"
            );
            assert_eq!(
                av_metrics_vmaf::libvmaf_version(),
                Some("3.2.0"),
                "accepted runtime remains cached"
            );
            eprintln!("accepted native calls: {:?}", snapshot());
        } else {
            // Drop an erroneously accepted scorer before reporting RED; no unsafe overrun.
            let error = result.err();
            eprintln!("rejected native calls: {:?}; error: {error:?}", snapshot());
            assert_eq!(
                snapshot(),
                [0, 0, 0],
                "reject before init/preallocate/fetch"
            );
            let error = error.expect("incompatible runtime must be rejected");
            let VmafError::IncompatibleLibrary { version, .. } = &error else {
                panic!("dedicated compatibility error: {error:?}");
            };
            let observed = std::env::var("M13_VERSION").expect("observed version");
            assert!(
                version.contains(&observed),
                "preserve observed version: {error}"
            );
            let error = error.to_string();
            assert!(
                error.contains("Incompatible libvmaf"),
                "dedicated compatibility error: {error}"
            );
            assert!(
                error.contains("3.2.0") && error.contains("v3.2.1") && error.contains("Reinstall"),
                "actionable error: {error}"
            );
            let marker = std::env::var("M13_MARKER_NAME").expect("marker");
            assert!(error.contains(&marker), "observed marker: {error}");
        }
    }
}
