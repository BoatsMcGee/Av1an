# sequence_config.target_quality

`sequence_config.target_quality` in `condor.json`. See [sequence_config](../sequence_config.md). Created by [`init`](../../../commands/init.md) (when `--target` is given) or [`target-quality`](../../../commands/target-quality.md) (`null` on plain `init`). CLI types in [Quality Metric](../../../types/quality-metric.md), [Quality Profile](../../../types/quality-profile.md), [Encoder Parameters](../../../types/encoder-params.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `metric` | Object | Yes | `{ "SSIMULACRA2": { "target_range": [74.0, 76.0], "resolution": null, "threads": null } }` | One key: `VMAF`, `SSIMULACRA2`, `BUTTERAUGLI`, `XPSNR`, `CVVDP`. CLI `--metric` |
| `maximum_probes` | Integer (u8) | Yes | `4` | Positive max probes per scene (`1..=255`) |
| `quantizer_range` | Array `[u32, u32]` | Yes | Per-encoder (see [condor.encoder](../encoder.md)) | `[min, max]`, `min ≤ max`; both endpoints must pass encoder settings checks. CLI `--min-q`/`--max-q` |
| `interpolators` | Array `[string, string]` | Yes | `["FritschCarlson", "Pchip"]` | Interpolation methods: `"Linear"`, `"Quadratic"`, `"Natural"`, `"FritschCarlson"`, `"Pchip"`, `"Catmull"`, `"Akima"`, `"CubicPolynomial"` |
| `input` | Input object or null | Yes | `null` | Override encode input. Same shape as [condor.input](../input.md) |
| `metric_input` | Input object or null | Yes | `null` | Override metric reference input |
| `probing` | Object | Yes | `{ "encoder_options": null, "strategy": "Whole", "statistic": "Mean" }` | Probe strategy + statistic. CLI `--profile` presets |

`metric` variants (all carry `target_range: [f64, f64]`):

Target endpoints must be finite and ordered (`min ≤ max`); equal target and quantizer
bounds are allowed. Saves and Target Quality preflight validate the global encoder
and every scene's own encoder using the effective probe options. A non-null
`probing.encoder_options` replaces, rather than merges with, scene options; probes
also omit the existing psychovisual settings and FFmpeg filter.

These are offline settings checks, not executable, native bit-depth or capability
checks. FFmpeg's arbitrary native codecs have no universal CRF range here. Invalid
loaded JSON can be inspected, explicitly repaired, or disabled with `target-quality
reset`, but cannot be saved or admitted to Target Quality work unchanged.

The search prefers untested steps inside its current bracket, even when rounding
repeats a previous probe. It tries an unmeasured configured boundary only after
interior steps are exhausted; equal quantizer bounds admit one probe. Steps are
`0.25` for x264/x265 and quarter-capable SVT-AV1 binaries, otherwise `1`.
At exhaustion or `maximum_probes`, the final quantizer remains a measured probe:
highest within the target range, otherwise closest to the target midpoint, with
ties choosing the higher quantizer. The latter is a fallback, not a target hit.
Only validated saved histories are reused, without pruning or changing the JSON
shape. Every recorded TQ pass must match the current selected-frame count and have
finite scores and a defined statistic; a mismatch or corrupt pass is an error
before prediction or reuse. Settings changes do not automatically migrate history.

- `VMAF`: `{ "target_range": [94.0, 96.0], "resolution": [w, h] or null, "scaler": <string>, "threads": <usize>, "model": <path> or null, "features": ["default"|"weighted"|"neg"|"motionless"|"uhd"] }`
- `SSIMULACRA2`: `{ "target_range": [74.0, 76.0], "resolution": [w, h] or null, "threads": <u8> or null }` (default)
- `BUTTERAUGLI`: `{ "target_range": [0.8, 1.2], "resolution": [w, h] or null, "threads": <u8> or null, "intensity_multiplier": <f64> or null, "norm": <u8> or null }`
- `XPSNR`: `{ "target_range": [44.0, 46.0], "resolution": [w, h] or null }`
- `CVVDP`: `{ "target_range": [9.4, 9.6], "resolution": [w, h] or null, "display_model": "standard_4k"|"standard_hdr_pq"|"standard_hdr_hlg"|"standard_hdr_linear"|"standard_hdr_dark"|"standard_hdr_linear_zoom"|"standard_fhd" or null, "resize_to_display": <bool> or null, "disable_temporal": <bool> or null }`

`probing.strategy` variants:

- `"Whole"` — all frames
- `{ "Skip": { "skip": <u32> } }` — every Nth frame, starting with the scene's first frame; N must be positive
- `{ "Subset": { "position": "Start"|"Middle"|"End", "length": { "Percentage": <f64> } | { "Frames": <u32> } } }`

`Percentage` is finite and positive, at most 100: `25.0` means 25%, not `0.25`.
The rounded count is clamped to at least one frame and at most the scene length.
`Frames` must be positive and is also clamped to the scene length. Empty or reversed
scene ranges are errors, not empty measurements.

`probing.statistic` variants: `"Mean"`, `"Median"`, `"Harmonic"`, `{ "Percentile": <f64> }`, `{ "StandardDeviationDistance": { "sigma": <f64> } }`, `"Mode"`, `"Minimum"`, `"Maximum"`, `"RootMeanSquare"`.

Percentiles select sorted index `floor(p / 100 * len)`, capped at the final index: `0` is the minimum and `100` the maximum.

Measured sets must be nonempty, contain exactly one finite score per selected
frame, and produce a finite, defined statistic. Incomplete or invalid batches are
not attached or saved. Percentile parameters must be finite and within `0..=100`;
`sigma` must be finite. Signed finite scores are allowed where the statistic is
defined. Harmonic gives `0` if any sample is zero; otherwise a non-finite or zero
reciprocal sum is an error (`[1, -1]` is undefined; `[-1, 2]` gives `-4`).

With temporal CVVDP enabled, scores are ordered cumulative prefixes: every prefix
must be finite, and the final prefix is the scene result, not a sorted or averaged
prefix. With temporal CVVDP disabled, the configured statistic applies normally.

CLI `--profile` presets: `fast` (mean of middle 11 frames), `standard` (RMS of middle 25%), `slow` (10th percentile of all frames).

### Frame selection and `vmaf`

For `vmaf`, only the frames a strategy selects are decoded and scored, so a sparse selection costs proportionally less work, as it does for the plugin-based metrics. This requires libvmaf at runtime; see the prerequisites list in the [repository README](../../../../README.md).

`Subset` selects a contiguous run, so VMAF's temporal features see consecutive frames exactly as they would in a normal pass. `Skip` does not: libvmaf's motion and temporal extractors compare against the previous *selected* frame rather than the true predecessor, so absolute scores shift slightly. They stay directly comparable across encodes of the same frames, which is what probing needs, but they are not the numbers a continuous pass would produce. Use `Whole` or `Subset` when the absolute value matters and `Skip` when only relative ordering does.

> [!NOTE]
> `CVVDP`'s `disable_temporal` option addresses something related but distinct: it turns the temporal model off inside the metric, rather than changing which frames are compared.

Example:

```json
{
    "metric": { "SSIMULACRA2": { "target_range": [74.0, 76.0], "resolution": null, "threads": null } },
    "maximum_probes": 4,
    "quantizer_range": [5, 55],
    "interpolators": ["FritschCarlson", "Pchip"],
    "input": null,
    "metric_input": null,
    "probing": {
        "encoder_options": null,
        "strategy": { "Subset": { "position": "Middle", "length": { "Percentage": 25.0 } } },
        "statistic": "RootMeanSquare"
    }
}
```
