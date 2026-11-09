//! Real GPU/plugin acceptance. ABI marker libraries cannot establish accuracy.
#[cfg(test)]
mod tests {
    use andean_condor::{
        metrics::{probe::OutputIndexing, vship},
        models::sequence::target_quality::types::QualityMetric,
        vapoursynth::{
            VapourSynthError, get_core,
            plugins::vship::cvvdp::{CVVDP, DisplayModel},
        },
    };

    // Fresh three-frame, nonuniform SDR fixtures with deliberately different errors.
    // Both native and plugin consume identical YUV444P8, BT.709/sRGB limited data,
    // 24 fps, standard_fhd, no resize, GPU 0 and the same temporal flag.
    const INPUT: &str = "import vapoursynth as vs
core = vs.core
def tile(y):
    return core.std.BlankClip(width=128, height=128, length=1, fpsnum=24, fpsden=1, format=vs.YUV444P8, color=[y,128,128])
def picture(n, shift):
    ys = [40+n*8, 180-n*9, 90+n*13, 140-n*7]
    t = [tile(y+shift) for y in ys]
    return core.std.StackVertical([core.std.StackHorizontal(t[:2]),core.std.StackHorizontal(t[2:])])
r = core.std.Splice([picture(n,0) for n in range(3)]).std.SetFrameProps(_Matrix=1, _Transfer=13, _Primaries=1, _Range=1, _ColorRange=1)
d = core.std.Splice([picture(n,s) for n,s in enumerate([1,30,7])]).std.SetFrameProps(_Matrix=1, _Transfer=13, _Primaries=1, _Range=1, _ColorRange=1)
";

    fn decoder(output: &str) -> anyhow::Result<av_decoders::Decoder> {
        Ok(av_decoders::Decoder::from_script(
            &format!("{INPUT}{output}"),
            Default::default(),
        )?)
    }

    fn native(disabled: bool, selected: &[usize]) -> anyhow::Result<Vec<f64>> {
        let mut reference = decoder("r.set_output()")?;
        let mut distorted = decoder("d.set_output()")?;
        vship::score_probed_frames(
            &mut reference,
            &mut distorted,
            &QualityMetric::CVVDP {
                target_range: (0.0, 10.0),
                resolution: None,
                display_model: Some(DisplayModel::StandardFHD),
                resize_to_display: Some(false),
                disable_temporal: Some(disabled),
                gpu_id: Some(0),
            },
            selected,
            OutputIndexing::Aligned,
            None,
            |_, _| {},
        )
    }

    fn plugin(disabled: bool) -> anyhow::Result<Vec<f64>> {
        let mut decoder = decoder("r.set_output(0)\nd.set_output(1)")?;
        let vs = decoder.get_vapoursynth_impl().expect("VS fixture");
        let core = get_core(&vs.env)?;
        let reference = vs.get_output(0, vs.get_node_modifier())?;
        let distorted = vs.get_output(1, vs.get_node_modifier())?;
        // Actual production plugin wrapper, not a copied spelling of its arguments.
        let node = CVVDP {
            gpu_id: Some(0),
            model_name: Some(DisplayModel::StandardFHD),
            resize_to_display: Some(false),
            disable_temporal: Some(disabled),
            distmap: Some(false),
            ..Default::default()
        }
        .invoke(core, &reference, &distorted)?;
        (0..3)
            .map(|index| Ok(node.get_frame(index)?.props().get_float("_CVVDP")?))
            .collect()
    }

    #[test]
    fn m15_native_vs_plugin() {
        if !av_metrics_vship::is_available() {
            assert!(
                std::env::var_os("AV_METRICS_REQUIRE_CVVDP_PARITY").is_none(),
                "required M15 parity needs pinned Vship 5.1.2, supported GPU and VS plugin"
            );
            eprintln!("UNEXECUTED M15 numerical parity: no supported Vship/GPU runtime");
            return;
        }
        let input = decoder("r.set_output()").expect("fresh GPU fixture");
        let format = av_metrics_vship::VideoFormat::from_details(input.get_video_details());
        let scorer = av_metrics_vship::VshipScorer::new(
            av_metrics_vship::VshipConfig::new()
                .with_metric(av_metrics_vship::VshipMetric::Cvvdp)
                .with_gpu_id(0)
                .with_fps(24.0),
            format,
            format,
            None,
        )
        .expect("GPU handler");
        assert!(
            matches!(
                scorer.backend(),
                av_metrics_vship::VshipBackend::Hip
                    | av_metrics_vship::VshipBackend::Cuda
                    | av_metrics_vship::VshipBackend::Vulkan
            ),
            "M15 numerical acceptance requires a real GPU backend, not CPU/ABI markers"
        );
        drop(scorer);
        let disabled_plugin = match plugin(true) {
            Ok(scores) => scores,
            Err(error) => {
                assert!(
                    matches!(
                        error.downcast_ref::<VapourSynthError>(),
                        Some(
                            VapourSynthError::PluginNotFound { .. }
                                | VapourSynthError::PluginLoadError { .. }
                                | VapourSynthError::PluginFunctionNotFound { .. }
                        )
                    ),
                    "M15 plugin execution failed (not missing runtime): {error:#}"
                );
                assert!(
                    std::env::var_os("AV_METRICS_REQUIRE_CVVDP_PARITY").is_none(),
                    "required M15 plugin parity unavailable: {error:#}"
                );
                eprintln!("UNEXECUTED M15 numerical parity: plugin unavailable: {error:#}");
                return;
            },
        };
        let disabled = native(true, &[0, 1, 2]).expect("disabled native CVVDP");
        let enabled = native(false, &[0, 1, 2]).expect("enabled native CVVDP");
        let enabled_plugin = plugin(false).expect("enabled plugin CVVDP");
        // Provisional same-backend absolute JOD tolerance; not locally calibrated.
        let close = |a: f64, b: f64| {
            assert!(
                a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-4,
                "M15 parity outside provisional 1e-4 JOD tolerance: {a} vs {b}"
            );
        };
        for index in 0..3 {
            close(disabled[index], disabled_plugin[index]);
            close(
                disabled[index],
                native(true, &[index]).expect("fresh independent scorer")[0],
            );
            close(enabled[index], enabled_plugin[index]);
        }
        assert!(
            enabled.iter().zip(&disabled).skip(1).any(|(a, b)| (a - b).abs() > 1e-6),
            "fixture must distinguish enabled history/accumulation from independent frames"
        );
    }
}
