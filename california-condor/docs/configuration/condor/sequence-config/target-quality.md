# sequence_config.target_quality

`sequence_config.target_quality` in `condor.json`. See [sequence_config](../sequence_config.md). Created by [`init`](../../../commands/init.md) (when `--target` is given) or [`target-quality`](../../../commands/target-quality.md) (`null` on plain `init`). CLI types in [Quality Metric](../../../types/quality-metric.md), [Quality Profile](../../../types/quality-profile.md), [Encoder Parameters](../../../types/encoder-params.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `metric` | Object | Yes | `{ "SSIMULACRA2": { "target_range": [74.0, 76.0], "resolution": null, "threads": null } }` | One key: `VMAF`, `SSIMULACRA2`, `BUTTERAUGLI`, `XPSNR`, `CVVDP`. CLI `--metric` |
| `maximum_probes` | Integer (u8) | Yes | `4` | Max probes per scene |
| `quantizer_range` | Array `[u32, u32]` | Yes | Per-encoder (see [condor.encoder](../encoder.md)) | `[min, max]`. CLI `--min-q`/`--max-q` |
| `interpolators` | Array `[string, string]` | Yes | `["natural", "pchip"]` | Interpolation methods: `linear`, `quadratic`, `natural`, `pchip`, `catmull`, `akima`, `cubicpolynomial` |
| `input` | Input object or null | Yes | `null` | Override encode input. Same shape as [condor.input](../input.md) |
| `metric_input` | Input object or null | Yes | `null` | Override metric reference input |
| `probing` | Object | Yes | `{ "encoder_options": null, "strategy": "Whole", "statistic": "Mean" }` | Probe strategy + statistic. CLI `--profile` presets |

`metric` variants (all carry `target_range: [f64, f64]`):

- `VMAF`: `{ "target_range": [94.0, 96.0], "resolution": [w, h] or null, "scaler": <string>, "filter": <string> or null, "threads": <usize>, "model": <path> or null, "features": ["default"|"weighted"|"neg"|"motionless"|"uhd"] }`
- `SSIMULACRA2`: `{ "target_range": [74.0, 76.0], "resolution": [w, h] or null, "threads": <u8> or null }` (default)
- `BUTTERAUGLI`: `{ "target_range": [0.8, 1.2], "resolution": [w, h] or null, "threads": <u8> or null, "intensity_multiplier": <f64> or null, "norm": <u8> or null }`
- `XPSNR`: `{ "target_range": [44.0, 46.0], "resolution": [w, h] or null }`
- `CVVDP`: `{ "target_range": [9.4, 9.6], "resolution": [w, h] or null, "display_model": "standard_4k"|"standard_hdr_pq"|"standard_hdr_hlg"|"standard_hdr_linear"|"standard_hdr_dark"|"standard_hdr_linear_zoom"|"standard_fhd" or null, "resize_to_display": <bool> or null, "disable_temporal": <bool> or null }`

`probing.strategy` variants:

- `"Whole"` — all frames
- `{ "Skip": { "skip": <u32> } }` — every Nth frame
- `{ "Subset": { "position": "Start"|"Middle"|"End", "length": { "Percentage": <f64> } | { "Frames": <u32> } } }`

`probing.statistic` variants: `"Mean"`, `"Median"`, `"Harmonic"`, `{ "Percentile": <f64> }`, `{ "StandardDeviationDistance": { "sigma": <f64> } }`, `"Mode"`, `"Minimum"`, `"Maximum"`, `"RootMeanSquare"`.

CLI `--profile` presets: `fast` (mean of middle 11 frames), `standard` (RMS of middle 25%), `slow` (10th percentile of all frames).

Example:

```json
{
    "metric": { "SSIMULACRA2": { "target_range": [74.0, 76.0], "resolution": null, "threads": null } },
    "maximum_probes": 4,
    "quantizer_range": [5, 55],
    "interpolators": ["natural", "pchip"],
    "input": null,
    "metric_input": null,
    "probing": {
        "encoder_options": null,
        "strategy": { "Subset": { "position": "Middle", "length": { "Percentage": 0.25 } } },
        "statistic": "RootMeanSquare"
    }
}
```
