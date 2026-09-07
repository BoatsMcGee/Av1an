# Configuration

Complete `condor.json` reference. A user should be able to build a valid config file from these documents alone.

| Document                                        | Contents                                                        |
| ----------------------------------------------- | --------------------------------------------------------------- |
| [Top level](./index.md)                         | `$schema`, `input`, `temp`, `input_filters`, `scd_input_filters`, `tq_input_filters`, `condor` |
| [condor](./condor/index.md)                     | `input`, `output`, `encoder`, `scenes`, `sequence_config`       |
| [condor/input](./condor/input.md)               | `Video`, `VapourSynth`, `VapourSynthScript` variants            |
| [condor/output](./condor/output.md)             | `path`, `tags`, `video_tags`                                    |
| [condor/encoder](./condor/encoder.md)           | Per-encoder variants, `pass`, `options`, `photon_noise`         |
| [condor/scene](./condor/scene.md)               | `start_frame`, `end_frame`, `sub_scenes`, per-scene `encoder`, `sequence_data` |
| [condor/sequence-config](./condor/sequence-config/index.md) | All 10 step configs                              |
| [sequence-config/scene-detector](./condor/sequence-config/scene-detector.md) | `method`, `input`                       |
| [sequence-config/noise-detector](./condor/sequence-config/noise-detector.md) | `input`, `reference_filters`, `denoised_filters` |
| [sequence-config/noise-scaler](./condor/sequence-config/noise-scaler.md) | `threshold`, `minimum_scaler`, `maximum_scaler`, `scale_chroma` |
| [sequence-config/benchmarker](./condor/sequence-config/benchmarker.md) | `threshold`, `max_memory`                     |
| [sequence-config/target-quality](./condor/sequence-config/target-quality.md) | `metric`, `maximum_probes`, `quantizer_range`, `interpolators`, `input`, `metric_input`, `probing` |
| [sequence-config/bitrate-optimizer](./condor/sequence-config/bitrate-optimizer.md) | `bitrate_sigma_threshold`               |
| [sequence-config/speed-scaler](./condor/sequence-config/speed-scaler.md) | `speed_quantizers`                          |
| [sequence-config/parallel-encoder](./condor/sequence-config/parallel-encoder.md) | `workers`, `buffer_strategy`, `scenes_directory`, `input` |
| [sequence-config/scene-concatenator](./condor/sequence-config/scene-concatenator.md) | `method`, `scenes_directory`, `output` |
| [sequence-config/quality-check](./condor/sequence-config/quality-check.md) | `metric`, `strategy`, `statistic`, `input` |

Related: CLI usage in [configuration.md](../configuration.md), workflow in [guide](../guide.md), CLI types in [types.md](../types.md) and [types/](../types/decoder.md).
