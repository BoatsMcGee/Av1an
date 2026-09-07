# Encode

Encode scenes in parallel (triggers TUI). See [guide](../guide.md) and [type reference](../types.md).

```bash
$ condor encode
$ condor encode -i input.mp4 -e svt-av1 -w 4 --params "--preset 4" --photon-noise 800
```

| Name                                                  | Flag              | Type        | Default                |
| ----------------------------------------------------- | ----------------- | ----------- | ---------------------- |
| [Input](#input--i)                                    | `-i`, `--input`   | Path        | Config input           |
| [Decoder](#decoder---decoder)                         | `--decoder`       | [`DECODER`](../types/decoder.md)   | `bestsource`           |
| [Filters](#filters---filters)                         | `--filters`       | [Filters](../types/filters.md)     | `resize:scaler=bicubic;format=yuv420p10le` |
| [VapourSynth Arguments](#vapoursynth-arguments---vs-args) | `--vs-args`   | [String List](../types/vs-args.md) |                        |
| [Workers](#workers--w---workers)                      | `-w`, `--workers` | Integer     | Benchmarker decides    |
| [Encoder](#encoder--e---encoder)                       | `-e`, `--encoder` | [`ENCODER`](../types/encoder.md)   | `svt-av1`              |
| [Passes](#passes---passes)                            | `--passes`        | Integer     | `2` for aom/vpx, else `1` |
| [Encoder Parameters](#encoder-parameters---params)    | `--params`        | [String](../types/encoder-params.md)      | Based on Encoder       |
| [Photon Noise](#photon-noise---photon-noise)          | `--photon-noise`  | [Integer](../types/photon-noise.md)     |                        |
| [Chroma Noise](#chroma-noise---chroma-noise)          | `--chroma-noise`  | [Integer](../types/photon-noise.md)     |                        |

## Input `-i`

Path to the input file to encode. Can be a video or VapourSynth script (`.py` or `.vpy`).

### Examples

- `> condor encode -i ./video.mp4`

## Decoder `--decoder`

Method used for decoding the input video. Methods besides `ffms2` require external VapourSynth plugins. Defaults to `bestsource`. See [Decoder](../types/decoder.md).

### Possible Values

- `bestsource`
- `vs-ffms2`
- `lsmash`
- `dgdecnv`
- `ffms2`

## Filters `--filters`

VapourSynth filters to apply to the input. See [Filters](../types/filters.md).

### Default

If not specified, `resize:scaler=bicubic;format=yuv420p10le` (YUV 4:2:0 10-bit) is used.

## VapourSynth Arguments `--vs-args`

VapourSynth/Python arguments to pass to the input script environment. See [VS Args](../types/vs-args.md).

## Workers `-w`, `--workers`

The amount of encoder processes to use at once.

### Default

If not specified, Benchmarker is used to determine the optimal number of workers.

## Encoder `-e`, `--encoder`

Video encoder to use. See [Encoder](../types/encoder.md).

### Possible Values

- `aom`
- `rav1e`
- `vpx`
- `svt-av1`
- `avm`
- `x264`
- `x265`
- `vvenc`
- `ffmpeg`

### Default

If not specified, `svt-av1` is used.

## Passes `--passes`

Number of encoder passes.

### Default

If not specified, `2` for `aom` and `vpx` encoders, otherwise `1`. `aom` and `vpx` benefit from two-pass mode even with constant quality mode.

## Encoder Parameters `--params`

Parameters for the video encoder (`--encoder`). Passed directly to the encoder binary and merged with Condor defaults. See [Encoder Parameters](../types/encoder-params.md).

### Examples

- `> condor encode --params "--preset 2 --crf 24 --aq-mode 0"`
- `> condor encode --params "--cpu-used=3 --cq-level=30 --tune=ssim"`

## Photon Noise `--photon-noise`

Generate and apply a photon noise table using Film Grain Synthesis with the specified ISO strength. Only compatible with `aom`, `svt-av1`, `rav1e`, and `avm`. A minimum ISO of `200` is recommended for reducing gradient banding. See [Photon Noise](../types/photon-noise.md).

## Chroma Noise `--chroma-noise`

Apply chroma noise of the specified ISO strength to the photon noise table using Film Grain Synthesis. Only compatible with `aom`, `svt-av1`, `rav1e`, and `avm`. See [Photon Noise](../types/photon-noise.md).
