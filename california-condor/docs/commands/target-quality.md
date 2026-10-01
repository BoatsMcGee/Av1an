# Target Quality

Calculate the optimum quantizer per scene for a given metric target (triggers TUI). See [guide](../guide.md) and [type reference](../types.md).

```bash
$ condor target-quality --metric ssimulacra2 --target 85
$ condor target-quality -i input.mp4 --metric butteraugli --target 90 --profile standard --min-q 10 --max-q 50
```

| Name                                                  | Flag          | Type        | Default       |
| ----------------------------------------------------- | ------------- | ----------- | ------------- |
| [Input](#input--i)                                    | `-i`, `--input` | Path      | Config input  |
| [Decoder](#decoder---decoder)                         | `--decoder`   | [`DECODER`](../types/decoder.md)   | `bestsource`  |
| [Filters](#filters---filters)                         | `--filters`   | [Filters](../types/filters.md)     |               |
| [VapourSynth Arguments](#vapoursynth-arguments---vs-args) | `--vs-args` | [String List](../types/vs-args.md) |             |
| [Encoder Parameters](#encoder-parameters---params)    | `--params`    | [String](../types/encoder-params.md)      | Based on Encoder |
| [Metric](#metric---metric)                            | `--metric`    | [`METRIC`](../types/quality-metric.md)    | `ssimulacra2` |
| [Target](#target---target)                            | `--target`    | Float       |               |
| [Minimum Quantizer](#minimum-quantizer---min-q)       | `--min-q`     | Integer     | Based on Encoder |
| [Maximum Quantizer](#maximum-quantizer---max-q)       | `--max-q`     | Integer     | Based on Encoder |
| [Profile](#profile---profile)                         | `--profile`   | [`PROFILE`](../types/quality-profile.md)   | `standard`    |

## Input `-i`

Path to the input file to encode and measure quality in Target Quality. Can be a video or VapourSynth script (`.py` or `.vpy`).

### Examples

- `> condor target-quality -i ./video.mp4 --metric ssimulacra2 --target 85`

## Decoder `--decoder`

Method used for decoding the input video. Methods besides `ffms2` require external VapourSynth plugins. Defaults to `bestsource`. See [Decoder](../types/decoder.md).

### Possible Values

- `bestsource`
- `vs-ffms2`
- `lsmash`
- `dgdecnv`
- `ffms2`

## Filters `--filters`

VapourSynth filters to apply to the Target Quality input. See [Filters](../types/filters.md).

## VapourSynth Arguments `--vs-args`

VapourSynth/Python arguments to pass to the input script environment. See [VS Args](../types/vs-args.md).

## Encoder Parameters `--params`

Parameters for the video encoder used in Target Quality. Passed directly to the encoder binary, merged with Condor defaults; psychovisual parameters are omitted. See [Encoder Parameters](../types/encoder-params.md).

### Examples

- `> condor target-quality --params "--preset 2 --crf 24 --aq-mode 0"`

## Metric `--metric`

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

- `> condor target-quality --metric ssimulacra2 --target 85`

## Minimum Quantizer `--min-q`

The lowest quantizer for Target Quality to try when searching for the optimal quantizer.

### Default

Default depends on the specified encoder (`--encoder`).

## Maximum Quantizer `--max-q`

The highest quantizer for Target Quality to try when searching for the optimal quantizer.

### Default

Default depends on the specified encoder (`--encoder`).

## Profile `--profile`

The preset profile to choose the Target Quality Probe Strategy and Statistic. See [Quality Profile](../types/quality-profile.md).

### Possible Values

- `fast` - Measures the average of the middle 11 frames
- `standard` - Measures the root-mean-square of the middle 25% of frames
- `slow` - Measures the 10th percentile of all frames

### Default

If not specified, `standard` is used.
