# sequence_config.parallel_encoder

`sequence_config.parallel_encoder` in `condor.json`. See [sequence_config](../sequence_config.md). Updated by [`encode`](../../../commands/encode.md) and [`condor`](../../../commands/condor.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `workers` | Integer (u8) or null | Yes | `null` | Worker count. `null` means Benchmarker decides. CLI `-w`/`--workers` |
| `scenes_directory` | String (path) | Yes | `<temp>/scenes` | Directory for per-scene encodes |
| `input` | Input object or null | Yes | `null` | Override input. Same shape as [condor.input](../input.md) |
| `validate_frame_count` | Boolean | No | `true` | Decode-count completed chunks with `ffprobe` before publishing or resuming. Set `false` only for filters intentionally changing the frame count. |

Count validation requires `ffprobe` on `PATH` with a decoder for the output codec.
The saved policy also applies to automatic worker discovery and Target Quality
individual/zone outputs. Strict stages check for `ffprobe` before encoding;
opting out removes only that counter prerequisite, not metric decoder requirements.
It counts decoded frames in the actual container, not progress or header counts;
decode errors, unavailable decoders, and missing/malformed counts fail closed.
Setting `validate_frame_count` to `false` disables decode-count verification only:
successful encoder exit and a nonempty regular output file are still required.
With this opt-out, corrupt but nonempty output cannot be decode-verified.

Resume requires a saved completed encoding record and a currently valid chunk.
Bare files are not adopted. Ineligible files are preserved. Missing, empty or
nonregular chunks lose their unusable completion metadata; count/probe/read
failures retain historical records but withhold current eligibility. Repair the
probe or intentional count policy and retry the same job; the historical record
alone never authorizes resume. Otherwise recompute in a new job, or move the
chunk aside and clear its saved completion record. Count validation does not
establish that a chunk matches changed source media or encoder settings.

Strict resume fully decodes each completed chunk again, sequentially, before new
encoding; large jobs can spend substantial time validating. The UI reports
validation progress separately from encoded frames. Graceful cancellation lets
the active check finish, then stops before another check or new encode; Force
terminates the active probe.

Frame buffering is chosen from the input:

- Video inputs and BestSource, FFMS2, or L-SMASH VapourSynth imports stream frames to each worker as it encodes, keeping a few frames in memory per worker.
- DGDecNV imports and nonstreaming VapourSynth scripts wait for a free worker before decoding each scene. Queued scenes are not decoded; an admitted scene can still buffer its frames.

Example:

```json
{
    "workers": null,
    "scenes_directory": "./a1b2c3d4/scenes",
    "input": null,
    "validate_frame_count": true
}
```
