# Type Reference

Complex types and structs shared across commands. Each command parameter table links here.

| Type                                                  | Used By                                                                 |
| ----------------------------------------------------- | ----------------------------------------------------------------------- |
| [Decoder](./types/decoder.md)                         | `--decoder`, `--scd-decoder`, `--tq-decoder`                            |
| [Encoder](./types/encoder.md)                         | `--encoder`                                                             |
| [Concatenation](./types/concatenation.md)             | `--concat`, `concatenate --method`                                      |
| [Quality Metric](./types/quality-metric.md)           | `--target-metric`, `--metric`                                           |
| [Quality Profile](./types/quality-profile.md)         | `--profile`, `--target-profile`                                         |
| [Scene Detection](./types/scene-detection.md)         | `detect-scenes --method`, `--min-scene-seconds`, `--max-scene-seconds`  |
| [Filters](./types/filters.md)                         | `--filters`, `--scd-filters`, `--tq-filters`, `--reference-filters`, `--denoised-filters` |
| [Encoder Parameters](./types/encoder-params.md)       | `--params`, `--tq-params`                                               |
| [Photon Noise](./types/photon-noise.md)               | `--photon-noise`, `--chroma-noise`                                      |
| [VapourSynth Arguments](./types/vs-args.md)           | `--vs-args`, `--scd-vs-args`, `--tq-vs-args`                            |
| [Configuration](./configuration/index.md)  | `condor.json`, `--config-file`                                          |
