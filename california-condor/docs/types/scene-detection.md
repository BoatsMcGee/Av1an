# Scene Detection

`METHOD` selects the scene detection algorithm. Used by `detect-scenes --method` (see [detect-scenes]\(../commands/detect-scenes.md\)).

| Value      | Behavior                                                        |
| ---------- | --------------------------------------------------------------- |
| `none`     | No detection; chunks scenes by maximum length                   |
| `fast`     | av-scenechange fast algorithm                                   |
| `standard` | av-scenechange standard algorithm                               |

## Defaults

- `--method`: `standard`
- `--min-scene-seconds`: `1`
- `--max-scene-seconds`: `10`

Scene length bounds apply to all methods, including `none` (which cuts purely by maximum length).

## Examples

- `> condor detect-scenes --method fast --min-scene-seconds 1 --max-scene-seconds 10`
- `> condor detect-scenes --method none --max-scene-seconds 5`
