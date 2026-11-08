//! Real decode/probe/scorer transport with doubles only at the native ABI.
//! Child processes isolate each library loader's OnceLock and environment.
#![cfg(unix)]

#[cfg(test)]
mod tests {
    use std::process::Command;

    use andean_condor::{
        metrics::{fmetrics, probe::OutputIndexing, vship},
        models::sequence::target_quality::types::QualityMetric,
    };

    #[test]
    fn m04_native_driver_metadata_transport() {
        if std::env::var_os("M04_NATIVE_CHILD").is_some() {
            check_drivers();
            return;
        }
        let directory = tempfile::tempdir().expect("temporary native ABI sink");
        let library = directory.path().join(if cfg!(target_os = "macos") {
            "libfmetrics.dylib"
        } else {
            "libfmetrics.so"
        });
        let output = Command::new("cc")
            .args(["-shared", "-fPIC", "-o"])
            .arg(&library)
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/m04_native.c"
            ))
            .output()
            .expect("M04 ABI transport check requires a C compiler (cc)");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::copy(
            &library,
            directory.path().join(if cfg!(target_os = "macos") {
                "libvship.dylib"
            } else {
                "libvship.so"
            }),
        )
        .expect("copy libvship ABI sink");
        let output = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "tests::m04_native_driver_metadata_transport", "--nocapture"])
            .env("M04_NATIVE_CHILD", "1")
            .env("FMETRICS_LIB_PATH", directory.path())
            .env("VSHIP_PLUGIN_PATH", directory.path())
            .output()
            .expect("run isolated transport test");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn decoder(properties: &str) -> av_decoders::Decoder {
        av_decoders::Decoder::from_script(&format!(
        "import vapoursynth as vs\nvs.core.std.BlankClip(width=32, height=32, length=1, format=vs.YUV420P8, color=[100,140,160]).std.SetFrameProps({properties}).set_output()"
    ), Default::default()).expect("open color-tagged fixture")
    }

    fn check_drivers() {
        let ssimulacra2 = QualityMetric::SSIMULACRA2 {
            target_range: (74.0, 76.0),
            resolution: None,
            threads: Some(1),
            gpu_id: Some(0),
        };
        let cvvdp = QualityMetric::CVVDP {
            target_range: (40.0, 50.0),
            resolution: None,
            display_model: None,
            resize_to_display: None,
            disable_temporal: None,
            gpu_id: None,
        };
        for metric in [&ssimulacra2, &cvvdp] {
            let mut reference = decoder("_Range=1, _Matrix=6, _Primaries=1, _Transfer=8");
            let mut distorted = decoder("_Range=0, _Matrix=9, _Primaries=1, _Transfer=13");
            let scores = fmetrics::score_probed_frames(
                &mut reference,
                &mut distorted,
                metric,
                &[0],
                OutputIndexing::Aligned,
                None,
                |_, _| {},
            )
            .expect("independent RGB metadata reaches fmetrics ABI");
            assert_eq!(scores, [42.0]);
        }
        let mut reference =
            decoder("_Range=1, _Matrix=6, _Primaries=1, _Transfer=13, _ChromaLocation=1");
        let mut distorted =
            decoder("_Range=0, _Matrix=9, _Primaries=9, _Transfer=16, _ChromaLocation=2");
        let scores = vship::score_probed_frames(
            &mut reference,
            &mut distorted,
            &ssimulacra2,
            &[0],
            OutputIndexing::Aligned,
            None,
            |_, _| {},
        )
        .expect("independent HDR/SDR metadata reaches libvship ABI");
        assert_eq!(scores, [42.0]);
    }
}
