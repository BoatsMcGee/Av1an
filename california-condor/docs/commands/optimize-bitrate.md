# Optimize Bitrate

Optimize bitrate for scenes that exceed normal bitrate after Target Quality. Scenes with bitrate above the normal threshold are optimized by clamping the quantizer to the average quantizer value. See [guide](../guide.md).

```bash
$ condor optimize-bitrate
$ condor optimize-bitrate --sigma-threshold 3
```

| Name                                                    | Flag                | Type    | Default |
| ------------------------------------------------------- | ------------------- | ------- | ------- |
| [Sigma Threshold](#sigma-threshold---sigma-threshold)   | `--sigma-threshold` | Integer |         |

## Sigma Threshold `--sigma-threshold`

Minimum bitrate sigma (σ) threshold for a scene to be optimized.

### Possible Values

Integer from `1` to `10`.

### Examples

- `> condor optimize-bitrate --sigma-threshold 3`

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-optimize-bitrate-light.avif"><picture><source srcset="../media/tui/help-optimize-bitrate-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-optimize-bitrate-light.avif" alt="condor optimize-bitrate --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor optimize-bitrate --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-optimize-bitrate-verbose-light.avif"><picture><source srcset="../media/tui/help-optimize-bitrate-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-optimize-bitrate-verbose-light.avif" alt="condor optimize-bitrate --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor optimize-bitrate --help --verbose</p>
    </td>
  </tr>
</table>
