//! Full production constructor at a deterministic C boundary, without GPU claims.
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use super::*;
use crate::config::{BackendPreference, VmafFeature};

const EXACT: &str = "scorer::setup_tests::m11_complete_backend_attempts";

#[test]
fn m11_complete_backend_attempts() {
    if std::env::var_os("M11_CHILD").is_some() {
        check_attempt();
        return;
    }
    let directory = tempfile::tempdir().expect("M11 fixture");
    let library = directory.path().join(if cfg!(target_os = "macos") {
        "libvmaf.dylib"
    } else {
        "libvmaf.so"
    });
    let output = Command::new("cc")
        .args(["-shared", "-fPIC", "-std=c99", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&library)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/m11_setup.c"
        ))
        .output()
        .expect("M11 requires cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // A real file satisfies path preflight; only the native parser is doubled.
    std::fs::write(directory.path().join("vmaf_v0.6.1.json"), b"{}").expect("model file");
    // Start with a late CUDA failure: the old constructor cannot retry this.
    for stage in [7, 0, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14] {
        for preference in ["auto", "require", "cpu"] {
            let mut child = Command::new(std::env::current_exe().expect("test executable"))
                .args(["--exact", EXACT, "--nocapture"])
                .env("M11_CHILD", "1")
                .env("M11_STAGE", stage.to_string())
                .env("M11_PREFERENCE", preference)
                .env("M11_LIBRARY", &library)
                .env("VMAF_LIB_DIR", directory.path())
                .env("VMAF_MODEL_PATH", directory.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("isolated M11 child");
            let start = Instant::now();
            while child.try_wait().expect("child status").is_none() {
                if start.elapsed() > Duration::from_secs(30) {
                    child.kill().expect("kill child");
                    child.wait().expect("reap child");
                    panic!("{stage}/{preference}: timeout");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let output = child.wait_with_output().expect("child output");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                output.status.success(),
                "{stage}/{preference}: {stdout}\n{stderr}"
            );
            assert!(
                stdout.contains("running 1 test") && stdout.contains("1 passed; 0 failed"),
                "{stdout}"
            );
            eprintln!("{stage}/{preference}: {stderr}");
        }
    }
}

fn check_attempt() {
    // SAFETY: this is our fixture, kept mapped through cleanup and counter reads.
    let library =
        unsafe { libloading::Library::new(std::env::var_os("M11_LIBRARY").expect("fixture")) }
            .expect("load fixture");
    // SAFETY: signatures/indices are defined by m11_setup.c.
    let (counter, cleanup) = unsafe {
        (
            library
                .get::<unsafe extern "C" fn(u32, u32) -> u32>(b"m11_count\0")
                .expect("count"),
            library.get::<unsafe extern "C" fn()>(b"m11_cleanup\0").expect("cleanup"),
        )
    };
    // SAFETY: all indices below are within the fixture's tables.
    let count = |backend, index| unsafe { counter(backend, index) };
    let stage: u32 = std::env::var("M11_STAGE").expect("stage").parse().expect("number");
    let preference = match std::env::var("M11_PREFERENCE").expect("preference").as_str() {
        "auto" => BackendPreference::Auto,
        "require" => BackendPreference::RequireCuda,
        "cpu" => BackendPreference::CpuOnly,
        _ => unreachable!(),
    };
    let model_path = std::path::PathBuf::from(std::env::var_os("VMAF_MODEL_PATH").expect("models"))
        .join("vmaf_v0.6.1.json");
    let mut config = VmafConfig::new()
        .with_threads(2)
        .with_backend(preference)
        .with_feature(VmafFeature::Psnr);
    if stage != 14 {
        config = config.with_model_path(model_path);
    }
    let format = VideoFormat {
        width: 8,
        height: 8,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
    };
    static API: std::sync::OnceLock<ffi::VmafApi> = std::sync::OnceLock::new();
    let api = API.get_or_init(|| {
        let mut api = *ffi::VmafApi::load().expect("fixture API");
        // SAFETY: the fixture's allocator-matched wrapper deallocator counts
        // the production guard's calls. No public injection API is exposed.
        api.cuda.free_state = Some(unsafe { *library.get(b"m11_free\0").expect("free") });
        api
    });
    let result = VmafScorer::new_with_loader(config, format, || Ok(api));
    let cuda_failed = !matches!(stage, 0 | 10 | 14);
    let cpu = preference == BackendPreference::CpuOnly
        || (preference == BackendPreference::Auto && cuda_failed);
    if (preference == BackendPreference::RequireCuda && cuda_failed) || stage == 11 {
        let error = result.err().expect("failed attempt must fail");
        let status = if cpu {
            -207
        } else if stage == 13 {
            -101
        } else if stage == 11 {
            -107
        } else {
            -(100 + stage as i32)
        };
        if stage == 6 {
            assert!(
                matches!(error, VmafError::ModelNotFound { ref reason, .. } if reason.contains("-106")),
                "{error:?}"
            );
        } else {
            let function = match stage {
                1 | 13 => "vmaf_cuda_state_init",
                2 => "vmaf_init",
                3 => "vmaf_cuda_import_state",
                4 => "vmaf_cuda_preallocate_pictures",
                5 => "vmaf_preallocate_pictures",
                7 | 11 => "vmaf_use_features_from_model",
                8 | 9 => "vmaf_fetch_preallocated_picture",
                12 => "vmaf_model_load_from_path",
                _ => unreachable!(),
            };
            assert!(
                matches!(error, VmafError::CallFailed { function: actual, status: code }
                if actual == function && code == if stage == 12 { 0 } else { status }),
                "{error:?}"
            );
        }
    } else {
        let mut scorer = result.expect("Auto must retry the entire failed CUDA attempt on CPU");
        assert_eq!(
            scorer.backend(),
            if cpu { Backend::Cpu } else { Backend::Cuda },
            "only a complete backend may be selected"
        );
        assert_eq!(
            scorer.config().n_threads,
            2,
            "do not mutate caller configuration"
        );
        let reference = [11u8; 64];
        let distorted = [22u8; 64];
        assert_eq!(
            scorer
                .submit_pair_raw(planes(&reference), planes(&distorted))
                .expect("raw pair"),
            0,
            "first accepted index"
        );
        let frame = v_frame::frame::FrameBuilder::new(8, 8, ChromaSubsampling::Yuv420, 8)
            .build::<u8>()
            .expect("frame");
        assert_eq!(
            scorer.submit_pair(&frame, &frame).expect("typed pair"),
            1,
            "typed path shares the host pool"
        );
        assert_eq!(
            scorer.finish().expect("flush"),
            if cpu { [81.0; 2] } else { [91.0; 2] },
            "both boundary markers survive flush (not metric accuracy)"
        );
        drop(scorer);
    }
    let snapshot: Vec<Vec<_>> = (0..2).map(|b| (0..18).map(|i| count(b, i)).collect()).collect();
    let globals: Vec<_> = (0..7).map(|i| count(2, i)).collect();
    // SAFETY: all Rust handles have dropped. Reclaim tombstones even during RED.
    unsafe { cleanup() };
    eprintln!("M11 CPU/CUDA {snapshot:?}, global {globals:?}");
    assert_eq!(
        globals[6], 0,
        "native boundary/cleanup/order/canary violation"
    );
    assert_eq!(
        count(0, 0),
        u32::from(cpu),
        "exactly one fresh CPU attempt or none"
    );
    // Literal native-boundary expectations, not counts calculated by the builder.
    // init, alloc, close, import, device, host, load, model alloc/destroy,
    // required, fetch, checkout, return, transfer, read, flush, optional, threads.
    let expected_cuda = if preference == BackendPreference::CpuOnly {
        [0; 18]
    } else {
        match stage {
            1 | 13 => [0; 18],
            2 => [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            3 => [1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            4 => [1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            5 => [1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            6 | 12 => [1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            7 | 11 => [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0],
            8 => [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 1, 0],
            9 => [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 1, 1, 0, 0, 0, 1, 0],
            14 => [1, 1, 1, 1, 1, 1, 2, 1, 1, 1, 6, 6, 2, 4, 2, 1, 1, 0],
            0 | 10 => [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 6, 6, 2, 4, 2, 1, 1, 0],
            _ => unreachable!(),
        }
    };
    assert_eq!(
        snapshot[1], expected_cuda,
        "CUDA attempt stops at the failing stage"
    );
    let mut expected_cpu = if !cpu {
        [0; 18]
    } else if stage == 11 {
        [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 2]
    } else {
        [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 6, 6, 2, 4, 2, 1, 1, 2]
    };
    if cpu && stage == 14 {
        expected_cpu[6] = 2;
    }
    assert_eq!(
        snapshot[0], expected_cpu,
        "fresh complete CPU attempt, or exact failure"
    );
    // state init, wrapper alloc/free, original/context/resource release, bad.
    let expected_global = if preference == BackendPreference::CpuOnly {
        [0; 7]
    } else {
        match stage {
            1 => [1, 0, 0, 0, 0, 0, 0],
            13 => [1, 1, 1, 1, 0, 0, 0],
            2 | 3 => [1, 1, 1, 1, 0, 1, 0],
            _ => [1, 1, 1, 0, 1, 1, 0],
        }
    };
    assert_eq!(
        globals, expected_global,
        "one release owner and one wrapper free"
    );
}

fn planes(bytes: &[u8]) -> PlaneSet<'_> {
    [(8, 8), (4, 4), (4, 4)]
        .map(|(w, h)| PlaneSource::from_bytes(bytes, w, h, w as isize, 0, 1).expect("planes"))
}
