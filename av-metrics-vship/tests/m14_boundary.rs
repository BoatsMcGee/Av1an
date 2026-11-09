//! Real-header ABI sinks; CPU backend is a valid tag, not a GPU accuracy claim.
use av_metrics_vship::ffi::*;
#[cfg(unix)]
use av_metrics_vship::{VideoFormat, VshipConfig, VshipError, VshipMetric, VshipScorer};
use std::{
    mem::{align_of, offset_of, size_of},
    process::Command,
};
#[cfg(unix)]
use std::{
    process::Stdio,
    time::{Duration, Instant},
};
#[cfg(unix)]
use v_frame::{chroma::ChromaSubsampling, frame::FrameBuilder};

#[test]
#[cfg(unix)]
fn m14_native_discriminants_and_admission() {
    if std::env::var_os("M14_CHILD").is_some() {
        child();
        return;
    }
    let dir = tempfile::tempdir().expect("fixture directory");
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libvship.dylib"
    } else {
        "libvship.so"
    });
    compile(&library, false, &[]);
    // backend, patch, probe status, compute status, score tag (-1 = retain).
    let mut cases =
        vec![(777, 2, 0, 0, -1), (-123, 2, 0, 0, -1), (3, 3, 0, 0, -1), (3, 1, 0, 0, -1)];
    for backend in 0..4 {
        cases.push((backend, 2, 0, 0, -1));
    }
    for status in [1, 17, 777, -123] {
        cases.push((3, 2, status, 0, -1));
        cases.push((3, 2, 0, status, -1));
    }
    for tag in [0, 1, 4, 5, 6, 777, -123] {
        cases.push((3, 2, 0, 0, tag));
    }
    for (backend, patch, probe, status, tag) in cases {
        let mut child = Command::new(std::env::current_exe().expect("executable"));
        child
            .args(["--exact", "m14_native_discriminants_and_admission", "--nocapture"])
            .env("M14_CHILD", "1")
            .env("M14_BACKEND", backend.to_string())
            .env("M14_PATCH", patch.to_string())
            .env("M14_PROBE_STATUS", probe.to_string())
            .env("M14_COMPUTE_STATUS", status.to_string())
            .env("VSHIP_PLUGIN_PATH", dir.path())
            .env("M14_LIBRARY", &library)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if tag != -1 {
            child.env("M14_TAG", tag.to_string());
        } else {
            child.env_remove("M14_TAG");
        }
        let mut child = child.spawn().expect("child");
        let start = Instant::now();
        while child.try_wait().expect("status").is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().expect("kill");
                child.wait().expect("reap");
                panic!("M14 timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let out = child.wait_with_output().expect("output");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{backend}/{patch}/{probe}/{status}/{tag}: {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            stdout.contains("1 passed; 0 failed"),
            "exact nonzero test count: {stdout}"
        );
    }
}
#[cfg(unix)]
fn child() {
    let setting = |name| std::env::var(name).expect("setting").parse::<i32>().expect("integer");
    // SAFETY: isolated fixture and exact counter signature, mapping remains live.
    let lib =
        unsafe { libloading::Library::new(std::env::var_os("M14_LIBRARY").expect("library")) }
            .expect("mapping");
    // SAFETY: signature defined in the fixture, valid indices 0..4.
    let counter =
        unsafe { lib.get::<unsafe extern "C" fn(u32) -> u32>(b"m14_count\0") }.expect("counter");
    // SAFETY: only defined fixture indices 0..4 are used while mapped.
    let count = |index| unsafe { counter(index) };
    let format = VideoFormat {
        width: 32,
        height: 32,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
        color_range: None,
        color_primaries: None,
        color_transfer_characteristic: None,
        color_space: None,
        chroma_location: None,
    };
    for metric in VshipMetric::all() {
        let result = VshipScorer::new(
            VshipConfig::new().with_metric(metric).with_handler_threads(1),
            format,
            format,
            None,
        );
        if !(0..4).contains(&setting("M14_BACKEND")) || setting("M14_PATCH") != 2 {
            assert!(
                matches!(&result, Err(VshipError::IncompatibleLibrary { .. })),
                "ABI rejection"
            );
            assert_eq!(
                (count(0), count(1)),
                (0, 0),
                "reject before GPU probe and handler allocation"
            );
            if !(0..4).contains(&setting("M14_BACKEND")) {
                assert!(
                    result
                        .err()
                        .expect("error")
                        .to_string()
                        .contains(&setting("M14_BACKEND").to_string()),
                    "retain unknown backend integer"
                );
            }
            continue;
        }
        let probe = setting("M14_PROBE_STATUS");
        if probe != 0 {
            assert!(
                matches!(result,Err(VshipError::CallFailed { status,.. }) if status==probe),
                "raw probe status"
            );
            assert_eq!(count(1), 0, "probe failure creates no handler");
            continue;
        }
        let mut scorer = result.expect("pinned ABI");
        assert_eq!(
            scorer.backend() as i32,
            setting("M14_BACKEND"),
            "checked backend roundtrip"
        );
        let frame = FrameBuilder::new(32, 32, ChromaSubsampling::Yuv420, 8)
            .build::<u8>()
            .expect("frame");
        let result = scorer.submit_pair(&frame, &frame);
        let status = setting("M14_COMPUTE_STATUS");
        let tag = std::env::var("M14_TAG").ok().map(|s| s.parse::<i32>().expect("tag"));
        let expected = match metric {
            VshipMetric::Ssimulacra2 => 4,
            VshipMetric::Butteraugli => 5,
            VshipMetric::Cvvdp => 6,
        };
        if status != 0 {
            let error = result.expect_err("status");
            assert!(
                matches!(&error,VshipError::CallFailed {status:s,..} if *s==status),
                "{error:?}"
            );
            if (1..=17).contains(&status) {
                assert!(
                    error.to_string().contains("known vship status"),
                    "known-code native formatting"
                );
            }
        } else if tag.is_some_and(|t| t != expected) {
            assert!(
                matches!(result,Err(VshipError::InvalidScoreTag {actual,expected:e}) if Some(actual)==tag && e==expected),
                "reject wrong or unknown native output tag"
            );
        } else {
            assert_eq!(
                result.expect("score").value(metric),
                Some(42.0),
                "known metric output marker"
            );
        }
        drop(scorer);
        assert_eq!(count(1), count(2), "all handlers freed");
    }
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
    macro_rules! fields {($t:ty; $($f:ident:$ft:ty),+)=>{$(expected.extend([offset_of!($t,$f),size_of::<$ft>()]);)+};}
    ty!(i32);
    ty!(VshipBackend);
    ty!(VshipStructType);
    ty!(VshipSample);
    ty!(VshipRange);
    ty!(VshipChromaLocation);
    ty!(VshipColorFamily);
    ty!(VshipYuvMatrix);
    ty!(VshipTransferFunction);
    ty!(VshipPrimaries);
    ty!(VshipVersion);
    fields!(VshipVersion;major:i32,minor:i32,minor_minor:i32,backend:i32);
    ty!(VshipColorspace);
    fields!(VshipColorspace;width:i64,height:i64,target_width:i64,target_height:i64,sample:VshipSample,range:VshipRange,subsampling:VshipChromaSubsample,chroma_location:VshipChromaLocation,color_family:VshipColorFamily,yuv_matrix:VshipYuvMatrix,transfer_function:VshipTransferFunction,primaries:VshipPrimaries,crop:VshipCropRectangle);
    ty!(VshipInitSsimulacra2_1);
    fields!(VshipInitSsimulacra2_1;struct_type:VshipStructType,src_colorspace:VshipColorspace,dis_colorspace:VshipColorspace,gpu_id:i32);
    ty!(VshipInitButteraugli_1);
    fields!(VshipInitButteraugli_1;struct_type:VshipStructType,src_colorspace:VshipColorspace,dis_colorspace:VshipColorspace,q_norm:i32,intensity_multiplier:f32,gpu_id:i32);
    ty!(VshipInitCvvdp_1);
    fields!(VshipInitCvvdp_1;struct_type:VshipStructType,src_colorspace:VshipColorspace,dis_colorspace:VshipColorspace,fps:f32,resize_to_display:bool,model_key_cstr:*const std::ffi::c_char,model_config_json_cstr:*const std::ffi::c_char,gpu_id:i32);
    ty!(VshipScoreSsimulacra2);
    fields!(VshipScoreSsimulacra2;struct_type:i32,score:f64);
    ty!(VshipScoreButteraugli);
    fields!(VshipScoreButteraugli;struct_type:i32,norm_q:f64,norm_3:f64,norm_inf:f64,dstp:*const u8,dst_stride:i64);
    ty!(VshipScoreCvvdp);
    fields!(VshipScoreCvvdp;struct_type:i32,score:f64,dstp:*const u8,dst_stride:i64);
    ty!(VshipDeviceInfo);
    fields!(VshipDeviceInfo;name:[std::ffi::c_char;256],vram_size:u64,integrated:i32,multi_processor_count:i32,warp_size:i32,vulkan_feature_matrix:VshipVulkanFeatureMatrix);
    expected.extend([0, 17, 0, 1, 2, 3, 0, 1, 2, 3, 4, 5, 6]);
    assert_eq!(actual, expected, "real pinned public C schema/signatures");
    eprintln!(
        "M14 Vship: {} real-header values, all 14 resolved signatures",
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
    assert_ne!(short[0], size_of::<i32>(), "detect C status width drift");
}
