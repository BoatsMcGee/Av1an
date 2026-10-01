# condor.json

Top-level `condor.json` schema reference. Created by [`init`](../commands/init.md), managed via [configuration.md](../configuration.md), consumed by every step in [guide](../guide.md). For the nested `condor` object see [condor](./condor/index.md).

## Fields

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `$schema` | String (URL) | No | Release `configuration.schema.json` URL | JSON Schema URL for IDE validation (see [configuration.md](../configuration.md#external-validation)) |
| `input` | String (path) | Yes | — | Original input path. Duplicated in case `condor.input` instantiates a VapourSynth script input |
| `temp` | String (path) | Yes | — | Temporary directory. `init` defaults to `<cwd>/<hash-of-input-name>`; override with `--temp` |
| `input_filters` | Array of `VapourSynthFilter` objects | Yes | See below | Encode input filters. CLI `--filters` strings are parsed into these objects. See [Filters](../types/filters.md#json-shape) |
| `scd_input_filters` | Array of `VapourSynthFilter` objects | Yes | `[]` | Scene detection filter overrides. CLI `--scd-filters`. See [Filters](../types/filters.md#json-shape) |
| `tq_input_filters` | Array of `VapourSynthFilter` objects | Yes | `[]` | Target quality filter overrides. CLI `--tq-filters`. See [Filters](../types/filters.md#json-shape) |
| `condor` | Object | Yes | — | Sequence state. See [condor](./condor/index.md) |

`input_filters` default on `init` (one resize to 10-bit):

```json
[
    { "Resize": { "scaler": "Bicubic", "width": null, "height": null, "format": "YUV420P10LE" } }
]
```

CLI strings use the syntax documented in [Filters](../types/filters.md) (e.g. `"resize:scaler=bilinear;width=1920;height=1080;format=yuv420p10le"`, `"crop:top=140;bottom=140;"`, `"trim:start=24;end=240;"`). Each string is parsed (`FromStr`) into one object in the array above. Full object shapes for every variant (`Crop`, `Resize`, `Trim`, `Rescale`, `WNNM`, `Bilateral`, `Degrain`, `ZoomDegrain`, `FGS`) are listed in [Filters](../types/filters.md#json-shape).

## Defaults on Init

- Encoder: `svt-av1` with defaults in [Encoder](../types/encoder.md#default-parameters)
- Input filter: `resize:scaler=bicubic;format=yuv420p10le`
- Scene detector: `standard`, 1s minimum, 10s maximum
- `scenes`: `[]`
- `noise_detector`, `noise_scaler`, `target_quality`, `quality_check`: `null`

## Minimal Example

```json
{
    "$schema": "https://github.com/rust-av/Av1an/releases/download/latest/configuration.schema.json",
    "input": "input.mp4",
    "temp": "./a1b2c3d4",
    "input_filters": [
        { "Resize": { "scaler": "Bicubic", "width": null, "height": null, "format": "YUV420P10LE" } }
    ],
    "scd_input_filters": [],
    "tq_input_filters": [],
    "condor": {
        "input": {
            "Video": {
                "path": "input.mp4",
                "import_method": { "FFMS2": { "index": null } }
            }
        },
        "output": { "path": "output.mkv", "tags": {}, "video_tags": {} },
        "encoder": {
            "SVTAV1": {
                "executable": null,
                "pass": { "All": 1 },
                "options": {
                    "preset": { "Number": { "prefix": "--", "delimiter": " ", "value": 4.0 } },
                    "crf": { "Number": { "prefix": "--", "delimiter": " ", "value": 25.0 } }
                },
                "photon_noise": null
            }
        },
        "scenes": [],
        "sequence_config": {
            "scene_detector": {
                "method": { "AVSceneChange": { "minimum_length": 24, "maximum_length": 240, "method": "Standard" } },
                "input": null
            },
            "noise_detector": null,
            "noise_scaler": null,
            "benchmarker": { "threshold": 5, "max_memory": null },
            "parallel_encoder": { "workers": null, "buffer_strategy": { "Workers": 1 }, "scenes_directory": "./a1b2c3d4/scenes", "input": null },
            "scene_concatenator": { "method": "mkvmerge", "scenes_directory": "./a1b2c3d4/scenes", "output": null },
            "target_quality": null,
            "quality_check": null,
            "bitrate_optimizer": { "bitrate_sigma_threshold": null },
            "speed_scaler": { "speed_quantizers": [] }
        }
    }
}
```

Regenerate the authoritative schema with `cargo run -p california-condor --bin generate-schema`.
