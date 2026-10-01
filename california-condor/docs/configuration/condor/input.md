# condor.input

`condor.input` in `condor.json`. See [condor](./index.md), [top level](../index.md), CLI decoders in [Decoder](../../types/decoder.md), VS args in [VS Args](../../types/vs-args.md).

Externally-tagged enum. Exactly one key per object.

## Variants

| Variant | JSON key | Fields |
| ------- | -------- | ------ |
| Video file | `Video` | `path`, `import_method` |
| VapourSynth source filter | `VapourSynth` | `path`, `import_method`, `cache_path` |
| VapourSynth script | `VapourSynthScript` | `source`, `variables`, `index` |

`init` writes `VapourSynth` with `BestSource` for video inputs, `VapourSynthScript` with `Path` source for `.vpy`/`.py` inputs.

## Video

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `path` | String (path) | Yes | — | Input video path |
| `import_method` | Object | Yes | — | One key: `FFMS2` |

`FFMS2` object:

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `index` | Integer or null | No | `null` | Track index |

Example:

```json
{ "Video": { "path": "input.mp4", "import_method": { "FFMS2": { "index": null } } } }
```

## VapourSynth

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `path` | String (path) | Yes | — | Input video path |
| `import_method` | Object | Yes | — | One key: `LSMASHWorks`, `DGDecNV`, `FFMS2`, `BestSource` |
| `cache_path` | String (path) or null | No | `null` | Index cache path |

Import methods (all take optional fields; omitted fields are `null`):

- `LSMASHWorks`: `{ "index": null }`
- `DGDecNV`: `{ "dgindexnv_executable": null }`
- `FFMS2`: `{ "index": null }`
- `BestSource`: `{ "index": null }` (default on `init`)

Example:

```json
{
    "VapourSynth": {
        "path": "input.mp4",
        "import_method": { "BestSource": { "index": null } },
        "cache_path": null
    }
}
```

CLI mapping: `--decoder bestsource|vs-ffms2|lsmash|dgdecnv|ffms2` selects the method. See [Decoder](../../types/decoder.md).

## VapourSynthScript

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `source` | Object | Yes | — | One key: `Path` (string path) or `Text` (script source) |
| `variables` | Object (string→string) | Yes | `{}` | `--vs-args key=value` pairs. See [VS Args](../../types/vs-args.md) |
| `index` | Integer | Yes | `0` | Output node index |

Examples:

```json
{
    "VapourSynthScript": {
        "source": { "Path": "script.vpy" },
        "variables": { "message": "fluffy kittens" },
        "index": 0
    }
}
```

```json
{
    "VapourSynthScript": {
        "source": { "Text": "import vapoursynth as vs\ncore = vs.core\nclip = core.bs.VideoSource(source='input.mp4')\nclip.set_output()" },
        "variables": {},
        "index": 0
    }
}
```
