# sequence_config.speed_scaler

`sequence_config.speed_scaler` in `condor.json`. See [sequence_config](../sequence_config.md). Updated by [`scale-speed`](../../../commands/scale-speed.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `speed_quantizers` | Array of `[integer, number]` | Yes | `[]` | `[speed, quantizer]` pairs. CLI `--quantizers` + `--speeds` are paired positionally |

An empty list disables scaling; otherwise provide at least two pairs with finite, unique quantizers (including no duplicate `-0.0`/`0.0`). Fractional quantizer knots in JSON are retained when only speeds are edited. Point order does not matter.

Two points use linear interpolation; three or more use the existing natural cubic spline. Quantizers outside the configured range use the corresponding endpoint speed. Interior estimates are clamped to the configured speed range before rounding; nonfinite scene quantizers or estimates are rejected before any scene changes are saved. Scenes without quantizers are left alone.

Known frontend speed checks (RAV1E `0..=10`, SVT-AV1 `-1..=13`) and existing named-preset clamps apply. These are offline settings checks, not native validation of every encoder or mode; see [Scale Speed](../../../commands/scale-speed.md) for the validation ceiling.

Example (from `scale-speed --quantizers 20 --speeds 5 --quantizers 30 --speeds 4 --quantizers 55 --speeds 2`):

```json
{
    "speed_quantizers": [[5, 20.0], [4, 30.0], [2, 55.0]]
}
```
