# Init

Create a new `condor.json` configuration. See [guide](../guide.md), [configuration](../configuration.md), and [type reference](../types.md).

```bash
$ condor init "input.mp4" "output.mkv"
$ condor init input.mp4 output.mkv --encoder svt-av1 --target-metric ssimulacra2 --target 85
```

| Name                                                        | Flag              | Type      | Default                |
| ----------------------------------------------------------- | ----------------- | --------- | ---------------------- |
| [Input](#input)                                             | `INPUT`           | Path      |                        |
| [Output](#output)                                           | `OUTPUT`          | Path      |                        |
| [Decoder](#decoder---decoder)                               | `--decoder`       | [`DECODER`](../types/decoder.md) | `bestsource`           |
| [Filters](#filters---filters)                               | `--filters`       | [Filters](../types/filters.md)   | `resize:scaler=bicubic;format=yuv420p10le` |
| [VapourSynth Arguments](#vapoursynth-arguments---vs-args)   | `--vs-args`       | [String List](../types/vs-args.md) |                      |
| [Concatenation](#concatenation---concat)                     | `--concat`        | [`CONCAT`](../types/concatenation.md)  | `mkvmerge`             |
| [Workers](#workers--w---workers)                            | `-w`, `--workers` | Integer   | Benchmarker decides    |
| [Encoder](#encoder--e---encoder)                             | `-e`, `--encoder` | [`ENCODER`](../types/encoder.md) | `svt-av1`              |
| [Encoder Parameters](#encoder-parameters---params)          | `--params`        | [String](../types/encoder-params.md)    | Based on Encoder       |
| [Photon Noise](#photon-noise---photon-noise)                | `--photon-noise`  | [Integer](../types/photon-noise.md)   |                        |
| [Target Metric](#target-metric---target-metric)              | `--target-metric` | [`METRIC`](../types/quality-metric.md)  | `ssimulacra2`          |
| [Target](#target---target)                                   | `--target`        | Float     |                        |

## Input

Path to the input file to encode. Can be a video or VapourSynth script (`.py` or `.vpy`).

### Examples

- `> condor init ./video.mp4 ./output.mkv`
- `> condor init ./script.vpy ./output.mkv`

## Output

Path to the output video file. Extension must be supported by `--concat`.

### Examples

- `> condor init input.mp4 ./output.mkv`
- `> condor init input.mp4 /mnt/encoded/flock.ivf`

## Decoder `--decoder`

Method used for decoding the input video. Methods besides `ffms2` require external VapourSynth plugins. See [Decoder](../types/decoder.md).

### Possible Values

- `bestsource` - BestSource VapourSynth plugin
- `vs-ffms2` - FFmpegSource VapourSynth plugin
- `lsmash` - L-SMASH-Works VapourSynth plugin
- `dgdecnv` - DGDecodeNV VapourSynth plugin
- `ffms2` - FFmpegSource (integrated)

### Default

If not specified, `bestsource` is used.

## Filters `--filters`

VapourSynth filters to apply to the input. See [Filters](../types/filters.md).

### Default

If not specified, `resize:scaler=bicubic;format=yuv420p10le` (YUV 4:2:0 10-bit) is used.

### Examples

- `> condor init input.mp4 output.mkv --filters "resize:scaler=bilinear;width=1920;height=1080;format=yuv420p10le"`
- `> condor init input.mp4 output.mkv --filters "crop:top=140;bottom=140;"`

## VapourSynth Arguments `--vs-args`

VapourSynth/Python arguments to pass to the input script environment. See [VS Args](../types/vs-args.md).

### Examples

- `> condor init script.vpy output.mkv --vs-args "message=fluffy kittens" --vs-args "head=empty"`

## Concatenation `--concat`

Method used for concatenating encoded chunks into the output file. See [Concatenation](../types/concatenation.md).

### Possible Values

- `mkvmerge` - MKVToolNix mkvmerge, Matroska (`.mkv`) only, generally best
- `ffmpeg` - FFmpeg, supports more formats but may break audio seeking
- `ivf` - Indeo Video Format (`.ivf`), video only

### Default

If not specified, `mkvmerge` is used.

## Workers `-w`, `--workers`

The amount of encoder processes to use at once.

### Default

If not specified, Benchmarker is used to determine the optimal number of workers.

## Encoder `-e`, `--encoder`

Video encoder to use. See [Encoder](../types/encoder.md).

### Possible Values

- `aom` - Alliance for Open Media AV1 encoder
- `rav1e` - Rust AV1 encoder
- `vpx` - WebM VP8/VP9 encoder
- `svt-av1` - Scalable Video Technology for AV1
- `avm` - Alliance for Open Media Video Model AV2 encoder
- `x264` - x264 H.264 encoder
- `x265` - x265 H.265 encoder
- `vvenc` - Fraunhofer Versatile Video Encoder H.266 encoder
- `ffmpeg` - FFmpeg

### Default

If not specified, `svt-av1` is used.

## Encoder Parameters `--params`

Parameters for the video encoder (`--encoder`). Passed directly to the encoder binary and merged with Condor defaults. See [Encoder Parameters](../types/encoder-params.md).

### Examples

- `> condor init input.mp4 output.mkv --params "--preset 2 --crf 24 --aq-mode 0"`
- `> condor init input.mp4 output.mkv --params "--cpu-used=3 --cq-level=30 --tune=ssim"`

## Photon Noise `--photon-noise`

Generate and apply a photon noise table using Film Grain Synthesis with the specified ISO strength. Only compatible with `aom`, `svt-av1`, `rav1e`, and `avm`. Do not use with internal encoder grain (e.g. `--film-grain` in `svt-av1`). See [Photon Noise](../types/photon-noise.md).

### Examples

- `> condor init input.mp4 output.mkv --photon-noise 800`

## Target Metric `--target-metric`

The quality metric used for Target Quality. See [Quality Metric](../types/quality-metric.md).

### Possible Values

- `ssimulacra2` - Structural SIMilarity Unveiling Local And Compression Related Artifacts
- `butteraugli` - butteraugli Infinite-Norm
- `butteraugli-3` - butteraugli 3-Norm
- `xpsnr` - Extended Perceptually Weighted PSNR (min of Y, U, V)
- `cvvdp` - ColorVideoVDP
- `vmaf` - Video Multi-Method Assessment Fusion (unimplemented)

### Default

If not specified, `ssimulacra2` is used.

## Target `--target`

The quality metric score that Target Quality will aim for.

### Examples

- `> condor init input.mp4 output.mkv --target-metric ssimulacra2 --target 85`
