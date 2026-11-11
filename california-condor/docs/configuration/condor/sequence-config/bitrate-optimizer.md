# sequence_config.bitrate_optimizer

`sequence_config.bitrate_optimizer` in `condor.json`. See [sequence_config](../sequence_config.md). Updated by [`optimize-bitrate`](../../../commands/optimize-bitrate.md).

Statistics use each scene's latest measured probe at its selected quantizer, with equal weight per eligible scene. Scenes without an exact measured match stay unchanged; no eligible data or zero bitrate standard deviation means no adjustment or save. Outliers use the rounded mean quantizer, which may be unmeasured and does not guarantee a bitrate reduction.

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `bitrate_sigma_threshold` | Integer (u8) or null | Yes | `null` | Std-dev distance from average scene bitrate to be considered excessively large. CLI `--sigma-threshold` (1–10) |

Example:

```json
{
    "bitrate_sigma_threshold": 3
}
```
