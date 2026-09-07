# sequence_config.noise_scaler

`sequence_config.noise_scaler` in `condor.json`. See [sequence_config](../sequence_config.md). Created by [`scale-noise`](../../../commands/scale-noise.md) (`null` on `init`).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `threshold` | Number (f64) | Yes | — | Minimum noise value for a scene to scale Photon Noise ISO. Recommended `0.002` |
| `minimum_scaler` | Number (f64) | Yes | — | Minimum scale factor for Photon Noise ISO scaling |
| `maximum_scaler` | Number (f64) | Yes | — | Maximum scale factor for Photon Noise ISO scaling |
| `scale_chroma` | Boolean | Yes | — | Whether to also scale chroma noise |

CLI mapping: `scale-noise --threshold <float> --minimum-scaler <float> --maximum-scaler <float> --scale-chroma`.

Example:

```json
{
    "threshold": 0.002,
    "minimum_scaler": 0.5,
    "maximum_scaler": 2.0,
    "scale_chroma": true
}
```
