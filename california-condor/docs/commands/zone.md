# Zone

Apply zone overrides to the scenes inside their frame ranges. Zones are the successor to Av1an's zones: each one marks the scenes it shares a frame with, merges its encoder settings into the scenes' own (or replaces their encoder outright), and caps how long those scenes may be. See [guide](../guide.md), [Zone](../types/zone.md), and [zoner](../configuration/condor/sequence-config/zoner.md).

```bash
$ condor zone '-1 200 svt-av1 "--preset 3 --crf 23"'
$ condor zone '-1 200 replace aom("/tools/aomenc") "--cpu-used=4" --maximum-frames 240'
$ condor zone ./zones.json
$ condor zone --dry-run '-1 200 svt-av1 "--preset 3" --maximum-frames 96'
```

| Name                | Flag      | Type                    | Default |
| ------------------- | --------- | ----------------------- | ------- |
| [Zones](#zones-zone) | `ZONE...` | [Zone](../types/zone.md) |         |
| [Dry run](#dry-run---dry-run) | `--dry-run` | Boolean | `false` |

## Zones `ZONE...`

One or more zone definitions, or paths to zone files. Each definition reads

`<start> <end> [replace|merge] [encoder[(executable)]] ["options"] [flags]`

where the frames are half-open with `-1` for the input's first or last frame, `replace` replaces the scene's encoder instead of merging into it, the encoder accepts its binary in parentheses, the quoted string holds encoder parameters (as in [`--params`](../types/encoder-params.md)), and the flags are `--maximum-frames`, `--ffmpeg-filter`, `--append-color-data`, `--iso`, `--chroma-iso`, `--width`, `--height`, `--sy`, `--ccb` and `--ccr`. See [Zone](../types/zone.md) for every part.

A value that names an existing file is read as a zone file: `.json` for `{ "zones": [ ... ] }` or a bare array, `.jsonl`/`.ndjson` for one zone per line, and anything else as a text file of definitions with `#` comments.

### Examples

- `> condor zone '-1 200 svt-av1 "--preset 3 --crf 23"'` - Merges into the scenes of the first 200 frames
- `> condor zone '-1 200 replace svt-av1("/tools/fish") "--preset 3 --crf 23" --iso 400'` - Replaces the encoder of those scenes, with Photon Noise
- `> condor zone '-1 200 x264 "--crf 18" --append-color-data off'` - Leaves the colour and HDR metadata out of that zone's encoder parameters
- `> condor zone ./zones.txt` - Applies every definition in a text file
- `> condor zone` - Applies the zones already in the configuration

## Dry run `--dry-run`

Preview changes with the same validation, INFO scene/split reports, and warnings as a normal run. Supply definitions or files, or use `condor zone --dry-run` to preview the configured zones. The output is explicitly marked as a dry-run.

No zone definitions or scene changes are saved, and no encode/probe outputs are modified. Normal log files and input-opening/index-cache activity still occur; dry-run does not mean zero filesystem activity.

## Zoned Scenes

After validation and application succeed, zones, scenes, and invalidation metadata are saved together. The first pass stores the original per-scene ranges, sub-scenes and encoders, plus the structured applied definitions. Known unchanged definitions skip without another save, including after reloading the job. Changed or removed overrides rebuild from that original baseline, not from global defaults or previous overrides. To remove all definitions, set `condor.sequence_config.zoner` to `null` (or its `zones` to `[]`) and run `condor zone`.

A failed command or dry-run leaves configuration bytes unchanged. Scenes still take whole overlapping zones: no zone-boundary splitting is introduced. Scenes longer than a zone's `maximum_frames` are split up.

Scene-list edits carry the existing lifecycle onto the new first scene when the
old first scene is deleted or displaced. They preserve the old applied definitions,
so unchanged zones still skip without saving, but changed-zone rebasing cannot
overwrite those manual edits. Resetting the whole list clears its lifecycle.

A merge whose encoder base differs from the scene's leaves that scene unchanged with a warning, rather than put one encoder's parameters on another's command line.

`condor zone` needs scenes to zone: run [detect-scenes](./detect-scenes.md) first, or let [`condor`](./condor.md) run detection before its Zoner step.

Actual changes clear affected probe/selection, encode and quality-check records before saving. Valid range-mapped noise measurements and proven scaling are retained; withdrawn scaling must restore its exact original PhotonNoise or refuse. Shifted indices count as changes even when a moved range is identical. Invalidated claims/artifacts set a persisted barrier even if old files are later absent. Files, source and final output are preserved; there is no automatic cleanup. See [scene invalidation and recovery](scenes.md#invalidation-and-recovery).

Legacy `zoned: true` without original/applied proof cannot safely skip or restore settings. Manual scene edits (or downstream encoder changes) can also make a later rebase unsafe. These cases preserve edits and refuse: reset/redetect the **whole scene list** in a new/empty work directory, or [start a separate job](clean.md#start-a-separate-job) with a distinct config, empty temp directory and output. Clearing only the old zoning flag is not a safe migration.

## Console reporting

Zoner reports its work at `INFO` level, both for `condor zone` and in the full pipeline:

- Each encoder change identifies the zone and scene's half-open frame range.
- Each split identifies the original scene, the resulting frame ranges, and how many scenes were added.
- Each zone reports matched, modified, split, added, and matched-but-unchanged scene counts. A scene changed and split by the same zone counts as one modified scene. No-op overrides and skipped encoder mismatches do not count as changes.
- The final summary reports the number of zones, scene changes across zones, total scenes added, before/after scene counts, and application warnings. Additional stale-result warnings are reported separately. A scene modified by multiple zones contributes a change for each zone; this is not a count of unique original scenes.
- A run with proven unchanged definitions reports that at `INFO` and applies nothing.

The command modifies scene configuration; no encoding or artifact deletion happens.

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-zone-light.avif"><picture><source srcset="../media/tui/help-zone-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-zone-light.avif" alt="condor zone --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor zone --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-zone-verbose-light.avif"><picture><source srcset="../media/tui/help-zone-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-zone-verbose-light.avif" alt="condor zone --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor zone --help --verbose</p>
    </td>
  </tr>
</table>
