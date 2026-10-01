# Scale Noise

Scale Photon Noise ISO per scene based on Noise Detector results. Scenes must have Photon Noise (`--photon-noise`/`--chroma-noise`) configured. Scenes below the threshold are not scaled. See [guide](../guide.md) and [Photon Noise](../types/photon-noise.md).

```bash
$ condor scale-noise --threshold 0.002
$ condor scale-noise --threshold 0.002 --minimum-scaler 0.5 --maximum-scaler 2.0 --scale-chroma
```

| Name                                            | Flag               | Type  | Default |
| ----------------------------------------------- | ------------------ | ----- | ------- |
| [Threshold](#threshold---threshold)             | `--threshold`      | Float |         |
| [Minimum Scaler](#minimum-scaler---minimum-scaler) | `--minimum-scaler` | Float |      |
| [Maximum Scaler](#maximum-scaler---maximum-scaler) | `--maximum-scaler` | Float |      |
| [Scale Chroma](#scale-chroma---scale-chroma)    | `--scale-chroma`   |       |         |

## Threshold `--threshold`

Minimum noise value for a scene to scale Photon Noise ISO.

### Examples

- `> condor scale-noise --threshold 0.002` - Recommended value

## Minimum Scaler `--minimum-scaler`

Minimum scale factor for Photon Noise ISO scaling.

## Maximum Scaler `--maximum-scaler`

Maximum scale factor for Photon Noise ISO scaling.

## Scale Chroma `--scale-chroma`

Scale Chroma ISO.
