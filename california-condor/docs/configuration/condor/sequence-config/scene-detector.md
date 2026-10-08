# sequence_config.scene_detector

`sequence_config.scene_detector` in `condor.json`. See [sequence_config](../sequence_config.md). Written by [`init`](../../../commands/init.md), updated by [`detect-scenes`](../../../commands/detect-scenes.md). CLI types in [Scene Detection](../../../types/scene-detection.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `method` | Object | Yes | `{ "AVSceneChange": { "minimum_length": <fps>, "maximum_length": <fps*10>, "method": "Standard" } }` | One key: `None`, `AVSceneChange`, or `TransNetV2` |
| `input` | Input object or null | Yes | `null` | Override input for detection. Same shape as [condor.input](../input.md) |

`method` variants:

- `None`: `{ "None": { "minimum_length": <usize frames>, "maximum_length": <usize frames> } }` — uniform splitting, no content analysis
- `AVSceneChange`: `{ "AVSceneChange": { "minimum_length": <usize frames>, "maximum_length": <usize frames>, "method": "Fast" | "Standard", "save_scores": <bool> } }`
- `TransNetV2`: `{ "TransNetV2": { "minimum_length": <usize frames>, "maximum_length": <usize frames>, "model_path": <path or null>, "save_scores": <bool> } }` — neural network detection via ONNX Runtime; `model_path` pins the model file (see [Scene Detection](../../types/scene-detection.md)), otherwise it is searched beside the executable. The `andean-condor/scripts/install-transnetv2-model-*` scripts download a copy and print the `TRANSNETV2_MODEL_PATH` to set

`save_scores` keeps the method's per-frame scores in each scene's `sequence_data.scene_detection` ([condor.scenes](../scene.md)). It defaults to `false`: the scores are unused and would flood `condor.json`.

CLI mapping: `detect-scenes --method fast|standard|transnetv2|none --min-scene-seconds <int> --max-scene-seconds <int>` converts seconds to frames via input fps. `init` uses `standard`, 1s minimum, 10s maximum.

Example (24 fps input):

```json
{
    "method": { "AVSceneChange": { "minimum_length": 24, "maximum_length": 240, "method": "Standard" } },
    "input": null
}
```

With override input:

```json
{
    "method": { "AVSceneChange": { "minimum_length": 24, "maximum_length": 240, "method": "Fast" } },
    "input": { "VapourSynth": { "path": "scd.mp4", "import_method": { "BestSource": { "index": null } }, "cache_path": null } }
}
```
