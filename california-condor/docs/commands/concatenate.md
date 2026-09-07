# Concatenate

Concatenate encoded scenes into output video (triggers TUI). See [guide](../guide.md) and [type reference](../types.md).

```bash
$ condor concatenate
$ condor concatenate --method mkvmerge
```

| Name                              | Flag       | Type     | Default    |
| --------------------------------- | ---------- | -------- | ---------- |
| [Method](#method---method)        | `--method` | [`CONCAT`](../types/concatenation.md) | `mkvmerge` |

## Method `--method`

Method used for concatenating the encoded chunks into the output file. See [Concatenation](../types/concatenation.md).

### Possible Values

- `mkvmerge` - MKVToolNix mkvmerge, Matroska (`.mkv`) only, generally best
- `ffmpeg` - FFmpeg, supports more formats but may break audio seeking
- `ivf` - Indeo Video Format (`.ivf`), video only

### Default

If not specified, `mkvmerge` is used.

### Examples

- `> condor concatenate --method mkvmerge`
- `> condor concatenate --method ffmpeg`
