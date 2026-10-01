# sequence_config.speed_scaler

`sequence_config.speed_scaler` in `condor.json`. See [sequence_config](../sequence_config.md). Updated by [`scale-speed`](../../../commands/scale-speed.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `speed_quantizers` | Array of `[integer, number]` | Yes | `[]` | Speed–quantizer pairs for Convex Hull interpolation. CLI `--quantizers` + `--speeds` (paired positionally) |

Example (from `scale-speed --quantizers 20 --speeds 5 --quantizers 30 --speeds 4 --quantizers 55 --speeds 2`):

```json
{
    "speed_quantizers": [[20, 5.0], [30, 4.0], [55, 2.0]]
}
```
