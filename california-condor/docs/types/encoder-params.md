# Encoder Parameters

Raw encoder CLI strings merged with Condor defaults. Used by `--params` and `--tq-params` (see [init]\(../commands/init.md\), [encode]\(../commands/encode.md\), [target-quality]\(../commands/target-quality.md\), [pipeline](../commands/condor.md)).

## Syntax

Quoted string passed directly to the encoder binary. Repeat semantics: single string per flag.

```bash
$ condor encode --params "--preset 2 --crf 24 --aq-mode 0"
$ condor encode --params "--cpu-used=3 --cq-level=30 --tune=ssim"
$ condor encode --params "--crf 18 --preset slow --tune film"
```

## Merge Rules

- `--params` merges over per-[encoder](./encoder.md) defaults listed in [encoder](./encoder.md#default-parameters).
- `--tq-params` is used for Target Quality probes only; if omitted, `--params` is used. Psychovisual parameters are stripped for metric accuracy.
- Use each encoder's `--help` for valid options; FFmpeg syntax cannot be used (e.g. x264 binary takes `--crf`, not `-crf`).

## Examples

- `> condor encode -e rav1e --params "--speed 4"`
- `> condor target-quality --params "--preset 4" --metric ssimulacra2 --target 85`
