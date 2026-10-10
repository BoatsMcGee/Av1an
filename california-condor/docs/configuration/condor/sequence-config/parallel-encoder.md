# sequence_config.parallel_encoder

`sequence_config.parallel_encoder` in `condor.json`. See [sequence_config](../sequence_config.md). Updated by [`encode`](../../../commands/encode.md) and [`condor`](../../../commands/condor.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `workers` | Integer (u8) or null | Yes | `null` | Worker count. `null` means Benchmarker decides. CLI `-w`/`--workers` |
| `scenes_directory` | String (path) | Yes | `<temp>/scenes` | Directory for per-scene encodes |
| `input` | Input object or null | Yes | `null` | Override input. Same shape as [condor.input](../input.md) |

Frame buffering is chosen from the input:

- Video inputs and BestSource, FFMS2, or L-SMASH VapourSynth imports stream frames to each worker as it encodes, keeping a few frames in memory per worker.
- DGDecNV imports and VapourSynth scripts decode one scene ahead of the workers.

Example:

```json
{
    "workers": null,
    "scenes_directory": "./a1b2c3d4/scenes",
    "input": null
}
```
