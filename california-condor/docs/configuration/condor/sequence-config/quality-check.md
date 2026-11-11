# sequence_config.quality_check

`sequence_config.quality_check` in `condor.json`. See [sequence_config](../sequence_config.md). Created by [`quality-check`](../../../commands/quality-check.md) (`null` on `init`). Metric shapes are the same as [target-quality](./target-quality.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `metric` | Object | Yes | SSIMULACRA2 default | One key: `VMAF`, `SSIMULACRA2`, `BUTTERAUGLI`, `XPSNR`, `CVVDP`. CLI `--metric` |
| `strategy` | Object or string | Yes | `"Whole"` | Same variants as `probing.strategy` in [target-quality](./target-quality.md). CLI `--profile` presets |
| `statistic` | Object or string | Yes | `"Mean"` | Same variants as `probing.statistic` in [target-quality](./target-quality.md) |
| `input` | Input object or null | Yes | `null` | Override input. Same shape as [condor.input](../input.md) |

New measurements require a nonempty set with exactly one finite score per selected
frame and a finite, defined statistic. Invalid batches are not attached or saved.
Signed finite scores are allowed where defined. Harmonic gives `0` if any sample
is zero; otherwise a non-finite or zero reciprocal sum is an error. With temporal
CVVDP enabled, every ordered cumulative prefix must be finite and the final prefix
is the scene result; other modes use the configured statistic.

Saved QC score counts that differ from the current selection are remeasured, for
example after changing profiles. The old record is kept if replacement measurement
fails; only a fully valid replacement batch is attached and saved. A matching-count
claimed record must still have finite values and a defined statistic; corruption is
an error, not completion by count alone. The default empty record with zero
timestamps is unmeasured. Count matching does not track settings provenance or
migrate history. TQ instead rejects claimed history whose count no longer matches;
see [target-quality](./target-quality.md).

Example:

```json
{
    "metric": { "SSIMULACRA2": { "target_range": [74.0, 76.0], "resolution": null, "threads": null } },
    "strategy": { "Subset": { "position": "Middle", "length": { "Percentage": 25.0 } } },
    "statistic": "RootMeanSquare",
    "input": null
}
```
