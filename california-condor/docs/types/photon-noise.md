# Photon Noise

Film Grain Synthesis tables generated from an ISO strength. Used by `--photon-noise` and `--chroma-noise` (see [init]\(../commands/init.md\), [encode]\(../commands/encode.md\), [pipeline](../commands/condor.md)). Scaled per scene by [scale-noise]\(../commands/scale-noise.md\) after [detect-noise]\(../commands/detect-noise.md\).

## Struct

| Field        | Type    | Required | Description                                              |
| ------------ | ------- | -------- | -------------------------------------------------------- |
| `iso`        | Integer | Yes      | Luma grain ISO strength (`--photon-noise`)               |
| `chroma_iso` | Integer | No       | Chroma grain ISO strength (`--chroma-noise`, defaults to `iso`) |
| `width`      | Integer | No       | Grain table width                                        |
| `height`     | Integer | No       | Grain table height                                       |
| `c_y`        | List    | No       | Custom luma AR coefficients (max 24)                     |
| `ccb`        | List    | No       | Custom chroma-blue AR coefficients (max 25)              |
| `ccr`        | List    | No       | Custom chroma-red AR coefficients (max 25)               |

The `fgs` [filter](./filters.md) exposes the same struct plus `dynamic_seed` (randomize grain seed per frame).

## Compatibility

Only `aom`, `svt-av1`, `rav1e`, and `avm` encoders (see [encoder](./encoder.md)). Do not combine with internal encoder grain such as `--film-grain` in `svt-av1`.

## Defaults and Guidance

- Minimum ISO `200` recommended for reducing gradient banding.
- [scale-noise]\(../commands/scale-noise.md\) recommended threshold: `0.002`. Scenes below threshold keep unscaled ISO.

## Examples

- `> condor encode --photon-noise 800`
- `> condor encode --photon-noise 800 --chroma-noise 400`
- `> condor scale-noise --threshold 0.002 --minimum-scaler 0.5 --maximum-scaler 2.0 --scale-chroma`
