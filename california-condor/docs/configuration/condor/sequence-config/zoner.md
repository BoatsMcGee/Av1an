# sequence_config.zoner

`sequence_config.zoner` in `condor.json`. See [sequence_config](../sequence_config.md). Created by [`zone`](../../../commands/zone.md) or [`condor --zones`](../../../commands/condor.md#zones---zones) (`null` on `init`), and applied by [`condor`](../../../commands/condor.md) right after Scene Detection.

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `zones` | Array | No | `[]` | The zone overrides, applied in start-frame order. Each item is a [Zone](../../../types/zone.md) |

CLI mapping: `zone ZONE...` and `condor --zones ZONE...`, where each value is a zone definition or a path to a JSON, JSON lines, or text zone file. Values replace the zones already configured.

## Implementation structure

Zoner belongs entirely to `california-condor`; it does not add a sequence or zone configuration to `andean-condor`.

- `src/zoner/zone.rs` owns the zone model, encoder overrides, and scene splitting.
- `src/zoner/parse.rs` reads inline definitions and JSON, JSON lines, or text files, reusing the encoder parameter parser.
- `src/zoner/mod.rs` owns `ZonerConfig`, `ZonerData`, range validation, ordered application, and the `Sequence` adapter. `sequence_data.zoner.lifecycle` on the first scene stores `baseline`, structured `zones`, and `applied` definitions. Definitions contain only ranges, sub-scenes and encoders, never nested scene results.
- The handler stages definitions in memory. The sequence publishes scenes, lifecycle and invalidation together only after validation/application. Known unchanged definitions do not reapply or save. Changed/removed definitions rebuild from the original baseline. `zone --dry-run` uses the same sequence with a no-op save callback and disposable state. The pipeline applies zones immediately after Scene Detector and before noise processing. All CLI sequence runners share one configuration-save callback; direct callers must retain their job lock through the callback.
- `andean-condor` only supplies generic encoder accessors and equality derives, not zoning policy.

Affected derived results are cleared before publication; old claims/files set the core-readable `sequence_data.parallel_encoder.invalidated` barrier. Files are never automatically removed. Explicit result reset clears the marker, not files or inherited encoder settings: recompute only in an empty/new work directory.

Legacy zoning flags without lifecycle proof refuse. Rebasing also refuses if manual or downstream encoder edits no longer match the last applied scene definitions. Preserve edits and reset/redetect the whole scene list in an empty work directory, or start a distinct job; clearing only the flag cannot recover original settings.

Example:

```json
{
    "zones": [
        {
            "start_frame": null,
            "end_frame": 200,
            "maximum_frames": 240,
            "encoder": {
                "Merge": {
                    "base": "SVTAV1",
                    "options": { "Merge": "--preset 3 --crf 23" },
                    "photon_noise": { "Merge": { "iso": 400 } }
                }
            }
        }
    ]
}
```
