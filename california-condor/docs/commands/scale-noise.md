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

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-scale-noise-light.avif"><picture><source srcset="../media/tui/help-scale-noise-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-scale-noise-light.avif" alt="condor scale-noise --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor scale-noise --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-scale-noise-verbose-light.avif"><picture><source srcset="../media/tui/help-scale-noise-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-scale-noise-verbose-light.avif" alt="condor scale-noise --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor scale-noise --help --verbose</p>
    </td>
  </tr>
</table>
