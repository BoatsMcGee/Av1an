# Filters

VapourSynth filter chains applied to inputs. Used by `--filters`, `--scd-filters`, `--tq-filters`, `--reference-filters`, `--denoised-filters` (see [init]\(../commands/init.md\), [encode]\(../commands/encode.md\), [pipeline](../commands/condor.md), [detect-scenes]\(../commands/detect-scenes.md\), [detect-noise]\(../commands/detect-noise.md\), [target-quality]\(../commands/target-quality.md\), [quality-check]\(../commands/quality-check.md\)).

## Syntax

One or more `name:key=value;key=value;` chains, passed as repeated flags or a single string:

```bash
$ condor encode --filters "resize:scaler=bilinear;width=1920;height=1080;format=yuv420p10le"
$ condor detect-scenes --filters "resize:scaler=bilinear;width=540;height=960;"
$ condor encode --filters "crop:top=140;bottom=140;" --filters "trim:start=24;end=240;"
```

Keys are case-insensitive. Omitted keys keep source values. List values use commas (e.g. `sigma=3.0,0.0,0.0`).

## Filter Reference

| Filter      | Arguments                                                                 | Notes                                    |
| ----------- | ------------------------------------------------------------------------- | ---------------------------------------- |
| `resize`    | `scaler?`, `width?`, `height?`, `format?`                                 | See [Scaler](#scaler) and [Format](#format) |
| `crop`      | `top?`, `bottom?`, `left?`, `right?` (pixels)                             |                                          |
| `trim`      | `start?`, `end?` (frames)                                                 |                                          |
| `rescale`   | `kernel` (required), `width` (required), `height` (required), `doubler` (required) | Requires vs-jetpack + vodesfunc. See [Kernel](#kernel) and [Doubler](#doubler) |
| `wnnm`      | `sigma?`, `block_size?`, `block_step?`, `group_size?`, `bm_range?`, `radius?`, `ps_num?`, `ps_range?`, `residual?`, `adaptive_aggregation?` | WNNM denoise. Requires vszip. |
| `bilateral` | `sigma_s?`, `sigma_r?`, `planes?`, `algorithm?`, `pbficnum?`              | Bilateral filter. Requires vszip.        |
| `degrain`   | MVUtensils motion-compensated denoise (many optional keys: `blksize`, `overlap`, `pel`, `radius`, `thsad`, `thsad2`, `planes`, `limit`, `thscd1`, `thscd2`, `recalculate`, ...) | Composite Super → Analyse → Degrain chain |
| `zoom_degrain` | ZooMVTools motion-compensated denoise (many optional keys: `hpad`, `vpad`, `pel`, `blksize`, `thsad`, `thsadc`, `plane`, `limit`, `recalculate`, ...) | Composite ZMVSuper → Analyse → Degrain chain |
| `fgs`       | `iso` (required), `chroma_iso?`, `cy?`, `ccb?`, `ccr?`, `dynamic_seed?`   | Film Grain Synthesis via dav1d grain engine. See [Photon Noise](./photon-noise.md) |

## Scaler

`resize` scaler values:

| Value      |
| ---------- |
| `bicubic`  |
| `bilinear` |
| `bob`      |
| `lanczos`  |
| `point`    |
| `spline16` |
| `spline36` |
| `spline64` |

## Format

`resize` format values are FFmpeg pixel formats (`-pix_fmt` strings):

| Value          | Description              |
| -------------- | ------------------------ |
| `yuv420p`      | YUV 4:2:0 8-bit          |
| `yuv420p10le`  | YUV 4:2:0 10-bit         |
| `yuv420p12le`  | YUV 4:2:0 12-bit         |
| `yuv422p`      | YUV 4:2:2 8-bit          |
| `yuv422p10le`  | YUV 4:2:2 10-bit         |
| `yuv422p12le`  | YUV 4:2:2 12-bit         |
| `yuv444p`      | YUV 4:4:4 8-bit          |
| `yuv444p10le`  | YUV 4:4:4 10-bit         |
| `yuv444p12le`  | YUV 4:4:4 12-bit         |
| `yuv440p` etc. | YUV 4:4:0 family         |
| `gbrp`, `gbrp10le`, `gbrp12le` | Planar RGB family |
| `gray`, `gray10le`, `gray12le` | Grayscale family  |
| `nv12`, `nv16`, `nv20le`, `nv21` | Semi-planar family |
| `yuva420p`, `yuvj420p`, `yuvj422p`, `yuvj444p` | Alpha / full-range JPEG family |

Default encode filter is `resize:scaler=bicubic;format=yuv420p10le`.

## Kernel

`rescale` kernel values (vs-jetpack):

`Bicubic`, `BSpline`, `Hermite`, `Mitchell`, `Catrom`, `FFmpegBicubic`, `AdobeBicubic`, `AdobeBicubicSharper`, `AdobeBicubicSmoother`, `BicubicSharp`, `RobidouxSoft`, `Robidoux`, `RobidouxSharp`, `BicubicAuto`, `Spline16`, `Spline36`, `Spline64`, `Bilinear`, `Lanczos`, `Point`.

## Doubler

`rescale` doubler values:

| Doubler   | Models |
| --------- | ------ |
| `ArtCNN`  | `C4F32`, `C4F32_DS`, `C16F64`, `C16F64_DS`, `R16F96`, `R8F64` (default), `R8F64_DS`, `R8F64_Chroma`, `C4F16`, `C4F16_DS`, `R16F96_Chroma` |
| `Waifu2x` | `AnimeStyleArt`, `AnimeStyleArtRGB`, `Photo`, `UpConv7AnimeStyleArt`, `UpConv7Photo`, `UpResNet10`, `Cunet` (default), `SwinUnetArt` |

## Defaults

| Flag                  | Default                               |
| --------------------- | ------------------------------------- |
| `--filters`           | `resize:scaler=bicubic;format=yuv420p10le` |
| `--scd-filters`       | none                                  |
| `--tq-filters`        | none                                  |
| `--reference-filters` | `wnnm:sigma=3.0,0.0,0.0;`             |
| `--denoised-filters`  | `wnnm:sigma=6.0,0.0,0.0;`             |

## Examples

- `> condor encode --filters "resize:scaler=bilinear;width=1280;height=720;format=yuv420p;"` — Bilinear resize to 1280x720 8-bit
- `> condor encode --filters "crop:top=140;bottom=140;"` — Crop 140px top and bottom
- `> condor encode --filters "trim:start=24;end=240;"` — Trim to frames 24-240
- `> condor encode --filters "rescale:kernel=Mitchell;width=720;height=1280;doubler=ArtCNN;"` — VSJET rescale with ArtCNN

## JSON Shape

In `condor.json` (`input_filters`, `scd_input_filters`, `tq_input_filters`, `reference_filters`, `denoised_filters`) each CLI string is stored as one externally-tagged object. Exactly one key per object. `scaler` uses PascalCase (`Bicubic`), `format` uses UPPER (`YUV420P10LE`).

```json
[
    { "Crop": { "top": 140, "bottom": 140, "left": null, "right": null } },
    { "Resize": { "scaler": "Bicubic", "width": null, "height": null, "format": "YUV420P10LE" } },
    { "Trim": { "start": 24, "end": 240 } },
    { "WNNM": { "sigma": [3.0, 0.0, 0.0], "block_size": null, "block_step": null, "group_size": null, "bm_range": null, "radius": null, "ps_num": null, "ps_range": null, "residual": null, "adaptive_aggregation": null } },
    { "Bilateral": { "sigma_s": null, "sigma_r": null, "planes": null, "algorithm": null, "pbficnum": null } },
    { "FGS": { "iso": 800, "chroma_iso": null, "cy": null, "ccb": null, "ccr": null, "dynamic_seed": null } }
]
```

`Rescale` requires `kernel`, `width`, `height`, `doubler`:

```json
[
    {
        "Rescale": {
            "kernel": { "Mitchell": { "linear": false, "border_handling": "MIRROR" } },
            "width": 720,
            "height": 1280,
            "doubler": { "ArtCNN": "R8F64" }
        }
    }
]
```

`Degrain` and `ZoomDegrain` are composite denoisers with all-optional fields; omitted fields are `null`. See variant field lists in the Filter Reference table above. `BorderHandling` is `MIRROR`, `ZERO`, or `REPEAT`. `Doubler` is `{ "ArtCNN": "<model>" }` or `{ "Waifu2x": "<model>" }` with models listed in [Doubler](#doubler).
