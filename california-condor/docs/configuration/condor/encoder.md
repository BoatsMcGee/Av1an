# condor.encoder

`condor.encoder` in `condor.json`. See [condor](./index.md), CLI reference in [Encoder](../../types/encoder.md), [Encoder Parameters](../../types/encoder-params.md), [Photon Noise](../../types/photon-noise.md).

Externally-tagged enum keyed by encoder base. Exactly one key per object: `AOM`, `RAV1E`, `VPX`, `SVTAV1`, `AVM`, `X264`, `X265`, `VVenC`, `FFmpeg`.

## Common Fields

| Field | Type | Required | Present on | Description |
| ----- | ---- | -------- | ---------- | ----------- |
| `executable` | String (path) or null | Yes | All | Custom encoder binary path, `null` for PATH lookup |
| `pass` | Object | Yes | All except `FFmpeg` | `{ "All": <u8> }` or `{ "Specific": [<u8>, <u8>] }`. Default `{"All": 2}` for AOM/VPX, `{"All": 1}` otherwise |
| `options` | Object (string→CLIParameter) | Yes | All | Encoder CLI options. See [Encoder Parameters](../../types/encoder-params.md) |
| `photon_noise` | Object or null | Yes | AOM, RAV1E, SVTAV1, AVM | See [Photon Noise](../../types/photon-noise.md). VPX/X264/X265/VVenC/FFmpeg have no such field |
| `ffmpeg_filter` | String or null | No | All except `FFmpeg` | FFmpeg filtergraph applied to the frames before they reach this encoder. Defaults to `null`. See [FFmpeg Filter](#ffmpeg-filter) |

`CLIParameter` objects (see [Encoder Parameters](../../types/encoder-params.md)):

- String: `{ "String": { "prefix": "--", "delimiter": "=", "value": "q" } }`
- Number: `{ "Number": { "prefix": "--", "delimiter": " ", "value": 4.0 } }`
- Bool: `{ "Bool": { "prefix": "--", "value": true } }`

`photon_noise` object:

| Field | Type | Required | Description |
| ----- | ---- | -------- | ----------- |
| `iso` | Integer | Yes | Luma ISO strength |
| `chroma_iso` | Integer or null | No | Chroma ISO strength |
| `width` | Integer or null | No | Grain width |
| `height` | Integer or null | No | Grain height |
| `c_y` | Array of Integer or null | No | Luma coefficients |
| `ccb` | Array of Integer or null | No | Cb coefficients |
| `ccr` | Array of Integer or null | No | Cr coefficients |

## Default Passes

| Encoder | Default `pass` |
| ------- | -------------- |
| AOM, VPX | `{ "All": 2 }` |
| RAV1E, SVTAV1, AVM, X264, X265, VVenC | `{ "All": 1 }` |
| FFmpeg | N/A (single pass) |

## Default Quantizer Ranges (for `sequence_config.target_quality.quantizer_range`)

| Encoder | Range |
| ------- | ----- |
| AOM, VPX, SVTAV1 | `(5, 55)` |
| RAV1E | `(50, 140)` |
| AVM | `(5, 250)` |
| X264, X265, VVenC | `(5, 35)` |
| FFmpeg | `(15, 50)` |

## FFmpeg Filter

`ffmpeg_filter` runs the decoded frames through an FFmpeg subprocess before
they reach the encoder. FFmpeg must be installed and available in PATH; the
encode fails validation if it is not found.

- Frames are piped as Y4M into `ffmpeg -vf <graph>` and the filtered Y4M
  stream is piped straight into the encoder, so streaming memory stays
  window-bounded — the filter adds no frame buffering of its own beyond
  FFmpeg's own probe and filter graph buffers.
- Each worker pays FFmpeg startup and a few frames of probe latency per
  scene, plus CPU cost proportional to the filter and worker count. Benchmarker
  calibration encodes with the same filter, so measured speeds account for it.
  Target Quality probes do **not** filter: they are scored against the
  unfiltered reference, so filtering would corrupt the metric the same way it
  does for Quality Check.
- The field lives on the encoder, so each scene can carry its own
  filtergraph. Zone encoding runs one process for a whole pass, so scenes
  with differing filters fall back to per-scene encoding.
- The `FFmpeg` encoder has no `ffmpeg_filter` field: it already accepts `-vf`
  through `options`.
- A filter that changes the picture (`crop`, `hflip`, `negate`, ...) makes
  Quality Check compare an unfiltered reference against the filtered encoded
  output, so scores become meaningless. Quality Check emits a warning when
  any encoded scene carries a filter; harmless filters (for example a pure
  pixel-format conversion) still score fine.

## Examples

SVT-AV1 (init default):

```json
{
    "SVTAV1": {
        "executable": null,
        "pass": { "All": 1 },
        "options": {
            "preset": { "Number": { "prefix": "--", "delimiter": " ", "value": 4.0 } },
            "keyint": { "Number": { "prefix": "--", "delimiter": " ", "value": 0.0 } },
            "scd": { "Number": { "prefix": "--", "delimiter": " ", "value": 0.0 } },
            "rc": { "Number": { "prefix": "--", "delimiter": " ", "value": 0.0 } },
            "crf": { "Number": { "prefix": "--", "delimiter": " ", "value": 25.0 } },
            "progress": { "Number": { "prefix": "--", "delimiter": " ", "value": 2.0 } }
        },
        "photon_noise": null,
        "ffmpeg_filter": null
    }
}
```

AOM with photon noise:

```json
{
    "AOM": {
        "executable": null,
        "pass": { "All": 2 },
        "options": {
            "threads": { "Number": { "prefix": "--", "delimiter": "=", "value": 8.0 } },
            "cpu-used": { "Number": { "prefix": "--", "delimiter": "=", "value": 6.0 } },
            "cq-level": { "Number": { "prefix": "--", "delimiter": "=", "value": 30.0 } },
            "kf-max-dist": { "Number": { "prefix": "--", "delimiter": "=", "value": 9999.0 } },
            "end-usage": { "String": { "prefix": "--", "delimiter": "=", "value": "q" } }
        },
        "photon_noise": { "iso": 800, "chroma_iso": null, "width": null, "height": null, "c_y": null, "ccb": null, "ccr": null },
        "ffmpeg_filter": null
    }
}
```

x264 with a crop filter:

```json
{
    "X264": {
        "executable": null,
        "pass": { "All": 1 },
        "options": {
            "preset": { "Number": { "prefix": "--", "delimiter": " ", "value": 4.0 } },
            "crf": { "Number": { "prefix": "--", "delimiter": " ", "value": 18.0 } }
        },
        "ffmpeg_filter": "crop=iw-16:ih-16"
    }
}
```

FFmpeg (no `pass`, no `photon_noise`):

```json
{
    "FFmpeg": {
        "executable": null,
        "options": {}
    }
}
```

Full per-encoder defaults are listed in [Encoder](../../types/encoder.md#default-parameters).
