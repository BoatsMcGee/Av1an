# sequence_config.bitrate_optimizer

`sequence_config.bitrate_optimizer` in `condor.json`. See [sequence_config](../sequence_config.md). Updated by [`optimize-bitrate`](../../../commands/optimize-bitrate.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `bitrate_sigma_threshold` | Integer (u8) or null | Yes | `null` | Std-dev distance from average scene bitrate to be considered excessively large. CLI `--sigma-threshold` (1–10) |

Example:

```json
{
    "bitrate_sigma_threshold": 3
}
```
