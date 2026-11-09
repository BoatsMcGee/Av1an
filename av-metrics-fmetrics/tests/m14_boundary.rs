//! Non-skipping real-C boundary tests; no fmetrics/Zig installation needed.
use av_metrics_fmetrics::ffi::*;
#[cfg(unix)]
use av_metrics_fmetrics::{
    ColorInfo, FmetricsConfig, FmetricsError, FmetricsMetric, FmetricsScorer, RgbView,
};
use std::{
    mem::{align_of, offset_of, size_of},
    process::Command,
};
#[cfg(unix)]
use std::{
    process::Stdio,
    time::{Duration, Instant},
};

#[test]
#[cfg(unix)]
fn m14_native_status_and_version_contract() {
    if std::env::var_os("M14_CHILD").is_some() {
        child();
        return;
    }
    let dir = tempfile::tempdir().expect("fixture directory");
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libfmetrics.dylib"
    } else {
        "libfmetrics.so"
    });
    compile(&library, false, &[]);
    for version in ["0.1.0", "0.2.0", "0.1.0-dev", "", "null", "non-UTF8", "m04-test"] {
        for status in [0, 1, 6, 777, -123] {
            if version != "0.1.0" && status != 0 {
                continue;
            }
            let mut child = Command::new(std::env::current_exe().expect("test executable"))
                .args(["--exact", "m14_native_status_and_version_contract", "--nocapture"])
                .env("M14_CHILD", "1")
                .env("M14_VERSION", version)
                .env("M14_STATUS", status.to_string())
                .env("FMETRICS_LIB_PATH", dir.path())
                .env("M14_LIBRARY", &library)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("child");
            let start = Instant::now();
            while child.try_wait().expect("status").is_none() {
                if start.elapsed() > Duration::from_secs(30) {
                    child.kill().expect("kill");
                    child.wait().expect("reap");
                    panic!("M14 child timed out");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let out = child.wait_with_output().expect("output");
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success(),
                "{version}/{status}: {stdout}\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                stdout.contains("1 passed; 0 failed"),
                "exact nonzero test count: {stdout}"
            );
        }
    }
}

#[cfg(unix)]
fn child() {
    // SAFETY: isolated fixture mapping stays alive through all counter calls.
    let lib =
        unsafe { libloading::Library::new(std::env::var_os("M14_LIBRARY").expect("library")) }
            .expect("mapping");
    // SAFETY: exact fixture counter signature and indices 0/1.
    let counter =
        unsafe { lib.get::<unsafe extern "C" fn(u32) -> u32>(b"m14_count\0") }.expect("counter");
    // SAFETY: only defined indices 0/1 are used while the fixture stays mapped.
    let count = |index| unsafe { counter(index) };
    let color = ColorInfo {
        matrix: av_metrics_fmetrics::YuvMatrix::Bt709,
        range: av_metrics_fmetrics::SampleRange::Limited,
        bit_depth: 8,
        transfer: av_decoders::TransferCharacteristic::IEC61966_2_1,
        colorspace: FmetricsColorspace::Srgb,
        hdr: false,
    };
    let result = FmetricsScorer::new(FmetricsConfig::new(FmetricsMetric::Ssimulacra2), color);
    if std::env::var("M14_VERSION").expect("version") != "0.1.0" {
        assert!(
            matches!(&result, Err(FmetricsError::IncompatibleLibrary { .. })),
            "reject incompatible ABI"
        );
        let error = result.expect_err("incompatible version").to_string();
        assert!(
            error.contains(&std::env::var("M14_VERSION").expect("version"))
                && error.contains("0.1.0"),
            "observed version and actionable pinned build: {error}"
        );
        assert_eq!(count(0), 0, "reject before workspace allocation");
        return;
    }
    let scorer = result.expect("pinned ABI");
    let bytes = [128u8; 32 * 32 * 3];
    let image = RgbView::new(&bytes, 32, 32, false).expect("RGB");
    let result = scorer.submit_rgb(&image, &image);
    let raw: i32 = std::env::var("M14_STATUS").expect("status").parse().expect("integer");
    if raw == 0 {
        assert_eq!(
            result.expect("score").ssimulacra2,
            Some(42.0),
            "known success marker"
        );
    } else {
        let error = result.expect_err("native failure");
        assert!(
            matches!(&error, FmetricsError::CallFailed { code, .. } if *code == raw),
            "{error:?}"
        );
        if (1..=6).contains(&raw) {
            assert!(
                error.to_string().contains("known fmetrics status"),
                "retain native known-code formatting"
            );
        } else {
            assert!(
                error.to_string().contains(&raw.to_string()),
                "retain raw unknown code"
            );
        }
    }
    drop(scorer);
    assert_eq!(count(0), count(1), "all workspaces destroyed");
}

fn compile(path: &std::path::Path, probe: bool, flags: &[&str]) {
    let mut cmd = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()));
    cmd.args(["-std=c11", "-Wall", "-Wextra", "-Werror"]);
    cmd.args(flags);
    if probe {
        cmd.arg("-DM14_PROBE");
    } else {
        cmd.args(["-shared", "-fPIC"]);
    }
    let out = cmd
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/m14_boundary.c"))
        .arg("-o")
        .arg(path)
        .output()
        .expect("C compiler required");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn m14_real_header_layouts_and_signatures() {
    let dir = tempfile::tempdir().expect("probe directory");
    let exe = dir.path().join(if cfg!(windows) { "probe.exe" } else { "probe" });
    compile(&exe, true, &[]);
    let out = Command::new(exe).output().expect("probe");
    assert!(out.status.success(), "C header probe exited successfully");
    let actual: Vec<usize> = String::from_utf8(out.stdout)
        .expect("ASCII")
        .split_whitespace()
        .map(|n| n.parse().expect("layout"))
        .collect();
    let mut expected = Vec::new();
    macro_rules! ty {
        ($t:ty) => {
            expected.extend([size_of::<$t>(), align_of::<$t>()]);
        };
    }
    macro_rules! field {
        ($t:ty, $f:ident, $ft:ty) => {
            expected.extend([offset_of!($t, $f), size_of::<$ft>()]);
        };
    }
    ty!(FmetricsErr);
    ty!(FmetricsPixFmt);
    ty!(FmetricsColorspace);
    ty!(FmetricsCvvdpDisplayModel);
    ty!(FmetricsImg);
    field!(FmetricsImg, data, *const std::ffi::c_void);
    field!(FmetricsImg, width, u32);
    field!(FmetricsImg, height, u32);
    field!(FmetricsImg, stride, u32);
    field!(FmetricsImg, format, FmetricsPixFmt);
    field!(FmetricsImg, colorspace, FmetricsColorspace);
    field!(FmetricsImg, hdr, bool);
    ty!(FmetricsButteraugliOptions);
    field!(FmetricsButteraugliOptions, intensity_target, f32);
    field!(FmetricsButteraugliOptions, pnorm, i32);
    ty!(FmetricsCvvdpDisplayParams);
    field!(FmetricsCvvdpDisplayParams, resolution_width, i32);
    field!(FmetricsCvvdpDisplayParams, resolution_height, i32);
    field!(FmetricsCvvdpDisplayParams, viewing_distance_meters, f32);
    field!(FmetricsCvvdpDisplayParams, diagonal_size_inches, f32);
    field!(FmetricsCvvdpDisplayParams, max_luminance, f32);
    field!(FmetricsCvvdpDisplayParams, contrast, f32);
    field!(FmetricsCvvdpDisplayParams, ambient_light, f32);
    field!(FmetricsCvvdpDisplayParams, reflectivity, f32);
    field!(FmetricsCvvdpDisplayParams, is_hdr, bool);
    ty!(FmetricsCvvdpResult);
    field!(FmetricsCvvdpResult, jod, f64);
    field!(FmetricsCvvdpResult, quality, f64);
    expected.extend([0, 1, 2, 3, 4, 5, 6, 1, 2, 3, 1, 2, 0, 27]);
    assert_eq!(actual, expected, "real pinned public C schema/signatures");
    eprintln!(
        "M14 fmetrics: {} real-header values, all 17 exported signatures",
        actual.len()
    );
    let drift = dir.path().join(if cfg!(windows) {
        "short-enums.exe"
    } else {
        "short-enums"
    });
    compile(&drift, true, &["-fshort-enums"]);
    let out = Command::new(drift).output().expect("ABI drift probe");
    assert!(out.status.success(), "C-only drift probe must run");
    let short: Vec<usize> = String::from_utf8(out.stdout)
        .expect("ASCII")
        .split_whitespace()
        .map(|n| n.parse().expect("layout"))
        .collect();
    assert_ne!(
        short, expected,
        "same oracle rejects nonstandard enum-width ABI"
    );
    assert_ne!(
        short[0],
        size_of::<FmetricsErr>(),
        "detect C status width drift"
    );
}
