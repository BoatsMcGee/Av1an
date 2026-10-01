# sequence_config.quality_check

`sequence_config.quality_check` in `condor.json`. See [sequence_config](../sequence_config.md). Created by [`quality-check`](../../../commands/quality-check.md) (`null` on `init`). Metric shapes are the same as [target-quality](./target-quality.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `metric` | Object | Yes | SSIMULACRA2 default | One key: `VMAF`, `SSIMULACRA2`, `BUTTERAUGLI`, `XPSNR`, `CVVDP`. CLI `--metric` |
| `strategy` | Object or string | Yes | `"Whole"` | Same variants as `probing.strategy` in [target-quality](./target-quality.md). CLI `--profile` presets |
| `statistic` | Object or string | Yes | `"Mean"` | Same variants as `probing.statistic` in [target-quality](./target-quality.md) |
| `input` | Input object or null | Yes | `null` | Override input. Same shape as [condor.input](../input.md) |

Example:

```json
{
    "metric": { "SSIMULACRA2": { "target_range": [74.0, 76.0], "resolution": null, "threads": null } },
    "strategy": { "Subset": { "position": "Middle", "length": { "Percentage": 0.25 } } },
    "statistic": "RootMeanSquare",
    "input": null
}
```
