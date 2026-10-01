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
