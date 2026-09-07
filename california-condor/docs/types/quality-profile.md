# Quality Profile

`PROFILE` selects the probe strategy and statistic for Target Quality and Quality Check. Used by `--profile`, `--target-profile` (see [target-quality]\(../commands/target-quality.md\), [quality-check]\(../commands/quality-check.md\), [pipeline](../commands/condor.md)).

| Value      | Strategy                                              |
| ---------- | ----------------------------------------------------- |
| `fast`     | Measures the average of the middle 11 frames          |
| `standard` | Measures the root-mean-square of the middle 25%       |
| `slow`     | Measures the 10th percentile of all frames            |

## Default

If not specified, `standard` is used.

## Examples

- `> condor target-quality --profile fast --metric ssimulacra2 --target 85`
- `> condor quality-check --profile slow`
