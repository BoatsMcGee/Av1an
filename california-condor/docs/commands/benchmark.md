# Benchmark

Benchmark the optimum amount of workers (triggers TUI). See [guide](../guide.md).

```bash
$ condor benchmark
$ condor benchmark --threshold 5
```

| Name                                | Flag          | Type    | Default |
| ----------------------------------- | ------------- | ------- | ------- |
| [Threshold](#threshold---threshold) | `--threshold` | Integer | `5`     |

## Threshold `--threshold`

The minimum speed increase (in percent) required to add an additional worker.

### Possible Values

Integer from `0` to `100`.

### Default

If not specified, `5` is used.

### Examples

- `> condor benchmark --threshold 5`
