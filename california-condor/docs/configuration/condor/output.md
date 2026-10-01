# condor.output

`condor.output` in `condor.json`. See [condor](./index.md), [top level](../index.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `path` | String (path) | Yes | — | Output video path. Extension must be supported by the [concatenation method](../../types/concatenation.md) (`mkvmerge`/`ffmpeg` → `.mkv`, `ivf` → `.ivf`) |
| `tags` | Object (string→string) | Yes | `{}` | Container tags |
| `video_tags` | Object (string→string) | Yes | `{}` | Video stream tags |

Example:

```json
{
    "path": "output.mkv",
    "tags": {},
    "video_tags": {}
}
```

With tags:

```json
{
    "path": "output.mkv",
    "tags": { "title": "My Encode" },
    "video_tags": { "language": "eng" }
}
```
