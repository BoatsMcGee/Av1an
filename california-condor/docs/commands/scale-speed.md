# Scale Speed

Apply speed based on quantizer per scene using Convex Hull interpolation. Speeds for scenes with quantizers between points are interpolated between the nearest points. See [guide](../guide.md).

```bash
$ condor scale-speed --encoder svt-av1 --quantizers 20 --speeds 5 --quantizers 30 --speeds 4 --quantizers 55 --speeds 2
```

| Name                                      | Flag           | Type         | Default |
| ----------------------------------------- | -------------- | ------------ | ------- |
| [Quantizers](#quantizers---quantizers)    | `--quantizers` | Integer List |         |
| [Speeds](#speeds---speeds)                | `--speeds`     | Integer List |         |

## Quantizers `--quantizers`

Quantizer values for speed-quantizer pairs. Must match number of speeds. Requires `--speeds`.

### Examples

- `> condor scale-speed --encoder svt-av1 --quantizers 20 --speeds 5 --quantizers 30 --speeds 4 --quantizers 55 --speeds 2`
- `> condor scale-speed --encoder aom --quantizers 10 --speeds 6 --quantizers 25 --speeds 4 --quantizers 40 --speeds 3`

## Speeds `--speeds`

Speed values for speed-quantizer pairs. Must match number of quantizers. Requires `--quantizers`.

### Examples

- `> condor scale-speed --encoder svt-av1 --quantizers 20 --speeds 5 --quantizers 30 --speeds 4 --quantizers 55 --speeds 2`
- `> condor scale-speed --encoder aom --quantizers 10 --speeds 6 --quantizers 25 --speeds 4 --quantizers 40 --speeds 3`

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-scale-speed-light.avif"><picture><source srcset="../media/tui/help-scale-speed-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-scale-speed-light.avif" alt="condor scale-speed --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor scale-speed --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-scale-speed-verbose-light.avif"><picture><source srcset="../media/tui/help-scale-speed-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-scale-speed-verbose-light.avif" alt="condor scale-speed --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor scale-speed --help --verbose</p>
    </td>
  </tr>
</table>
