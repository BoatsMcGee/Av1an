# Concatenation

`CONCAT` selects how encoded scenes are joined. Used by `--concat` and `concatenate --method` (see [init]\(../commands/init.md\), [pipeline](../commands/condor.md), [concatenate]\(../commands/concatenate.md\)).

| Value      | Tool              | Output Formats | Notes                                                        |
| ---------- | ----------------- | -------------- | ------------------------------------------------------------ |
| `mkvmerge` | MKVToolNix        | `.mkv` only    | Default. Generally best; requires mkvmerge installed.        |
| `ffmpeg`   | FFmpeg            | many           | Supports non-Matroska/IVF; may break audio seeking.          |
| `ivf`      | IVF concatenation | `.ivf` only    | Video only; drops audio, subtitles, chapters, metadata.      |

## Default

If not specified, `mkvmerge` is used.

## Examples

- `> condor concatenate --method mkvmerge`
- `> condor -i input.mp4 -o output.mp4 --concat ffmpeg`
