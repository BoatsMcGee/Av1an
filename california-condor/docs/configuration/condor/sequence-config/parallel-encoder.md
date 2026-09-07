# sequence_config.parallel_encoder

`sequence_config.parallel_encoder` in `condor.json`. See [sequence_config](../sequence_config.md). Updated by [`encode`](../../../commands/encode.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `workers` | Integer (u8) or null | Yes | `null` | Worker count. `null` means Benchmarker decides. CLI `-w`/`--workers` |
| `buffer_strategy` | Object | Yes | `{ "Workers": 1 }` | One key: `None`, `{ "Workers": <u8> }`, or `Maximum` |
| `scenes_directory` | String (path) | Yes | `<temp>/scenes` | Directory for per-scene encodes |
| `input` | Input object or null | Yes | `null` | Override input. Same shape as [condor.input](../input.md) |

`buffer_strategy` variants:

- `{ "None": null }` — no buffering beyond workers
- `{ "Workers": 1 }` — buffer one extra scene per worker (default)
- `{ "Maximum": null }` — buffer `workers * 2`

Example:

```json
{
    "workers": null,
    "buffer_strategy": { "Workers": 1 },
    "scenes_directory": "./a1b2c3d4/scenes",
    "input": null
}
```
