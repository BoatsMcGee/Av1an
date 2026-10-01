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
