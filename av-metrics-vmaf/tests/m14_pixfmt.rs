//! Native unknown pixel-format writes run only with integer-safe descriptors.
#![cfg(unix)]
use av_metrics_vmaf::{
    BackendPreference, PlaneSource, VideoFormat, VmafConfig, VmafError, VmafScorer,
};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use v_frame::chroma::ChromaSubsampling;

#[test]
fn m14_native_pool_format_owns_before_validation() {
    if std::env::var_os("M14_CHILD").is_some() {
        child();
        return;
    }
    let dir = tempfile::tempdir().expect("fixture directory");
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libvmaf.dylib"
    } else {
        "libvmaf.so"
    });
    let out = Command::new("cc")
        .args(["-shared", "-fPIC", "-std=c11", "-Wall", "-Wextra", "-Werror", "-pthread"])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/m14_pixfmt.c"
        ))
        .arg("-o")
        .arg(&library)
        .output()
        .expect("C compiler required");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for (fetch, failed) in [(1, false), (2, false), (3, false), (4, false), (2, true)] {
        for format in [777, -123, 0, 4] {
            let mut child = Command::new(std::env::current_exe().expect("executable"))
                .args(["--exact", "m14_native_pool_format_owns_before_validation", "--nocapture"])
                .env("M14_CHILD", "1")
                .env("M14_FETCH", fetch.to_string())
                .env("M14_PIXFMT", format.to_string())
                .env("M11_STAGE", if failed { "9" } else { "0" })
                .env("M12_CHILD", "1")
                .env("M14_LIBRARY", &library)
                .env("VMAF_LIB_DIR", dir.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("child");
            let start = Instant::now();
            while child.try_wait().expect("status").is_none() {
                if start.elapsed() > Duration::from_secs(30) {
                    child.kill().expect("kill");
                    child.wait().expect("reap");
                    panic!("pool close hung");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let out = child.wait_with_output().expect("output");
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success(),
                "{fetch}/{format}/{failed}: {stdout}\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                stdout.contains("1 passed; 0 failed"),
                "exact nonzero child: {stdout}"
            );
        }
    }
}
fn child() {
    // SAFETY: exact fixture mapping remains live through cleanup and counters.
    let lib =
        unsafe { libloading::Library::new(std::env::var_os("M14_LIBRARY").expect("library")) }
            .expect("mapping");
    // SAFETY: exact fixture signatures with valid indices below.
    let counter = unsafe { lib.get::<unsafe extern "C" fn(u32, u32) -> u32>(b"m11_count\0") }
        .expect("counter");
    // SAFETY: fixture signatures, called while mapped.
    let invalid = unsafe { lib.get::<unsafe extern "C" fn() -> u32>(b"m14_invalid_returns\0") }
        .expect("invalid returns");
    // SAFETY: cleanup runs once after every Rust owner is dropped.
    let cleanup = unsafe { lib.get::<unsafe extern "C" fn()>(b"m11_cleanup\0") }.expect("cleanup");
    // SAFETY: all used counter indices are defined by m11_setup.c.
    let count = |b, i| unsafe { counter(b, i) };
    let format = VideoFormat {
        width: 8,
        height: 8,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
    };
    let fetch: u32 = std::env::var("M14_FETCH").expect("fetch").parse().expect("integer");
    let failed = std::env::var("M11_STAGE").expect("stage") == "9";
    let result = VmafScorer::new(
        VmafConfig::new().with_backend(BackendPreference::CpuOnly).with_threads(2),
        format,
    );
    let error = if fetch <= 2 {
        result.err().expect("bad initial picture")
    } else {
        let mut scorer = result.expect("initial pair");
        let a = [11u8; 64];
        let b = [22u8; 64];
        let planes = |bytes| {
            [(8, 8), (4, 4), (4, 4)].map(|(w, h)| {
                PlaneSource::from_bytes(bytes, w, h, w as isize, 0, 1).expect("planes")
            })
        };
        let error = scorer.submit_pair_raw(planes(&a), planes(&b)).expect_err("bad replacement");
        assert!(
            scorer.submit_pair_raw(planes(&a), planes(&b)).is_err(),
            "no retry after replacement failure"
        );
        drop(scorer);
        error
    };
    if failed {
        assert!(
            matches!(error, VmafError::CallFailed { status: -109, .. }),
            "preserve failed-fetch status before inspecting unowned output"
        );
    } else {
        assert!(
            matches!(&error, VmafError::UnsupportedFormat { .. }),
            "{error:?}"
        );
        assert!(
            error.to_string().contains(&std::env::var("M14_PIXFMT").expect("format")),
            "preserve raw pixel-format integer"
        );
    }
    assert_eq!(
        count(0, 14),
        u32::from(fetch > 2),
        "no read_pictures for invalid initial format"
    );
    assert_eq!(
        count(0, 11),
        count(0, 12) + count(0, 13),
        "every successful checkout returned/transferred once"
    );
    assert_eq!(count(0, 2), 1, "context close unblocked");
    assert_eq!(count(2, 6), 0, "ownership/canary violations");
    // SAFETY: fixture remains mapped; failed fetch has no owning return.
    let invalid_returns = unsafe { invalid() };
    assert_eq!(
        invalid_returns,
        u32::from(!failed),
        "adopt successful invalid picture exactly once, never failed fetch output"
    );
    // SAFETY: all native/Rust owners are gone, free tombstones exactly once.
    unsafe { cleanup() };
}
