# sequence_config.scene_concatenator

`sequence_config.scene_concatenator` in `condor.json`. See [sequence_config](../sequence_config.md). Updated by [`concatenate`](../../../commands/concatenate.md). CLI types in [Concatenation](../../../types/concatenation.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `method` | String | Yes | `"mkvmerge"` | `mkvmerge`, `ffmpeg`, or `ivf`. CLI `--concat` / `concatenate --method` |
| `scenes_directory` | String (path) | Yes | `<temp>/scenes` | Directory of per-scene encodes |
| `output` | String (path) or null | Yes | `null` | Override output path |

Example:

```json
{
    "method": "mkvmerge",
    "scenes_directory": "./a1b2c3d4/scenes",
    "output": null
}
```
