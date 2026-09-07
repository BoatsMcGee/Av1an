# sequence_config.benchmarker

`sequence_config.benchmarker` in `condor.json`. See [sequence_config](../sequence_config.md). Updated by [`benchmark`](../../../commands/benchmark.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `threshold` | Integer (u8) | Yes | `5` | Percent of speed increase required to add an additional worker. CLI `--threshold` (0–100) |
| `max_memory` | Integer (u32) or null | Yes | `null` | Maximum RAM in megabytes allowed across all workers |

Example:

```json
{
    "threshold": 5,
    "max_memory": null
}
```
