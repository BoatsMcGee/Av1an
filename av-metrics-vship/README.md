# av-metrics-vship

Native [libvship] perceptual quality scoring — SSIMULACRA2, Butteraugli and CVVDP — with GPU acceleration.

libvship is the GPU-accelerated successor to the standalone Butteraugli and SSIMULACRA2 tools, and adds CVVDP. This crate binds its C API directly and scores frame pairs as they are decoded, so no temporary files and no distortion maps are involved.

Frames come from [`av-decoders`], so a clip can be read from Y4M, FFMS2, FFmpeg or a VapourSynth script without this crate knowing which.

## Features

- **Per-pair scores, straight from `Vship_ComputeHandler`.** Unlike the libvmaf binding, there is no sliding window and no flush: the value for a pair is final the moment the call returns.
- **A handler per worker.** A non-temporal metric creates one handler per configured thread and distributes pairs across them round-robin, so the device sees parallel work.
- **Temporal CVVDP is handled correctly.** It builds a single handler, sees pairs in index order, and calls `Vship_Reset` at an index discontinuity so a scene break is not scored as a motion event.
- **Finds libvship where you actually have it.** libvship ships as a VapourSynth plugin, not a standalone library, so discovery looks in `VSHIP_PLUGIN_PATH`, `VSHIP_LIB_DIR`, `<dir(VSSCRIPT_PATH)>/plugins` and `VAPOURSYNTH_EXTRA_PLUGIN_PATH` before falling back to the loader's own search path.
- **Optional dependency.** libvship is opened with `dlopen` at runtime, so the crate compiles on machines that do not have it and `is_available()` returns `false` rather than the build failing.
- **No `bindgen`.** The C surface is declared by hand, keeping the build free of a libclang dependency.

## Usage

```rust
use av_decoders::Decoder;
use av_metrics_vship::{PoolMethod, VideoFormat, VshipConfig, VshipMetric, VshipScorer};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
// Requires libvship and a usable GPU at runtime; see "Prerequisites" below.
let mut reference = Decoder::from_file("reference.mkv")?;
let mut distorted = Decoder::from_file("distorted.mkv")?;

let reference_format = VideoFormat::from_details(reference.get_video_details());
let distorted_format = VideoFormat::from_details(distorted.get_video_details());
let mut scorer =
    VshipScorer::new(VshipConfig::new().with_handler_threads(4), reference_format, distorted_format, None)?;

let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})?;
let pooled = VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Mean)?;
println!("SSIMULACRA2: {pooled:.4}");
# Ok(())
# }
```

`VshipScorer::new` takes the reference and the encode format separately, plus an optional `(width, height)` target that becomes libvship's `target_width` and `target_height`. The two sides need not share a format, but they must end up the same size: libvship does not resize implicitly, so a 1080p reference against a 720p encode needs the target set (see [Differing input sizes](#differing-input-sizes-need-an-explicit-target)).

The callback passed to `score_decoders` fires once per pair, which is useful for reporting progress while a long encode is being scored. Every returned [`FrameScore`] is complete at that point, so no flush follows the loop.

A scorer is `Send` but not `Sync`: one scorer owns its whole handler pool and drives it serially, so it can be moved into a worker thread but not shared across threads.

### Borrowed plane uploads

`PlaneSource<'a>` borrows storage through fallible `from_bytes`, `from_u16` or
`from_plane` adapters. `PlaneSet<'a>` remains a three-plane array; its private
geometry cannot be changed after validation. Raw callers must use unsafe
`from_raw_parts` with an **actual readable allocation length**, keeping that
allocation live throughout the borrow and synchronous native compute/upload.
No Rust byte slice is constructed over foreign uninitialized padding.

Width/height count samples; stride, origin and span count bytes. Uploads require
positive dimensions and stride, `stride >= width * bytes_per_sample`, and
`origin + (height - 1) * stride + row_bytes <= readable_len`. Inter-row padding
is uploaded; final-row trailing padding is **not** required. Bottom-up/negative
strides are rejected. Host origin/stride need not be sample-aligned.
Dimensions/stride must fit native C `int`; the HIP launch expression additionally
requires `width * height <= INT_MAX - 255` per plane.

Every plane on both sides must match its configured storage width before
submission counters, handler resets or compute change: 8-bit uses one byte,
9/10/12/14/16-bit uses two. A typed `Frame<u8>` cannot supply configured 10-bit
data. Chroma rounds up: 1919×1079 4:2:0 requires 960×540, not 959×539.
Callers tracking external frame gaps can use `submit_pair_raw_with_reset` to
request the same history reset only after both sides pass preflight; Av1an uses
this to preserve its existing discontinuity policy without resetting invalid pairs.

The prefix and misaligned-stride contracts are pinned to Vship
[`5a627933` HIP upload](https://codeberg.org/Line-fr/Vship/src/commit/5a627933274196219f782b3b74e13fad47cd6370/src/HIP/gpuColorToLinear/vshipColor.hpp),
[Vulkan upload](https://codeberg.org/Line-fr/Vship/src/commit/5a627933274196219f782b3b74e13fad47cd6370/src/Vulkan/gpuColorToLinear/vshipColor.hpp)
and [HIP sample conversion](https://codeberg.org/Line-fr/Vship/src/commit/5a627933274196219f782b3b74e13fad47cd6370/src/HIP/gpuColorToLinear/anyDepthToFloat.hpp).
Pure checks and C ABI sinks run without a GPU; they do not prove GPU upload
completion or metric accuracy. On a supported GPU host, run `just test-gpu`;
`m07_gpu_padded_uploads_and_typed_mismatch` compares padded/contiguous uploads
and fails, rather than skipping, when required mode has no working runtime.

Check availability before constructing one, if the outcome is not already known:

```rust
use av_metrics_vship::{device_name, is_available, libvship_version};

if is_available() {
    let version = libvship_version().unwrap_or("unknown version");
    let device = device_name().unwrap_or_default();
    println!("libvship {version} on {device}");
}
```

### Choosing a metric

| Metric | Direction | Temporal | Per-pair score |
| --- | --- | --- | --- |
| `VshipMetric::Ssimulacra2` | higher is better | no | yes |
| `VshipMetric::Butteraugli` | lower is better | no | yes |
| `VshipMetric::Cvvdp` | higher is better | **yes** | a running mean over everything the handler has seen |

Butteraugli reports three norms per pair — `norm_q`, `norm_3` and `norm_inf` — of which `norm_q` is the one matching the configured `q_norm`. Fields the metric does not produce are `None` rather than zero, so a zero is never mistaken for a real score.

### CVVDP is accumulated, not per-frame

CVVDP is a video metric: libvship folds each frame into a single accumulator and
returns the running mean over everything it has seen so far. Upstream is explicit
that **only the last score should be used**, and every frame must still be
computed. Consequences for callers:

- **Do not average the per-frame values.** Averaging a sequence of prefix means
  over-weights the earliest frames. On a 756-frame clip the mean of the running
  means came out at 8.1455 where the final value is 9.0475.
- **Take the last value.** `PoolMethod` has no `Last` variant, so read
  `scores.last()` directly rather than reaching for `Max`, which is wrong for
  every CVVDP clip whose score drifts upward.
- **`with_fps` is required.** The temporal filter is parameterised by the frame
  rate, so a zero rate does not merely omit information — it drifts. On the same
  756-frame 1080p clip the score is 8.1346 at the default `0.0` and 9.0475 at the
  real 23.976 fps, a gap that widens with clip length because the error
  accumulates per frame.
- **`reset_temporal` clears the frame history, not the score.** Per the C header,
  `Vship_Reset` *"empties temporal filter history (but not score accumulation)"*, so
  the running mean continues across a scene break. `reset_score` clears the
  accumulator as well, and is what a caller needs if each scene's score is to cover
  only that scene.

Measured against the VapourSynth plugin on the same clip and the same libvship
build, per-frame CVVDP agrees exactly once `fps` is supplied.

### Pooling

libvship reports a score per pair, so reducing a clip to one number is this crate's job. `VshipScorer::pool` picks the values the metric produces and applies a `PoolMethod`:

| Method | Result |
| --- | --- |
| `PoolMethod::Mean` | Arithmetic mean over the frames that produced a value. |
| `PoolMethod::Min` | The lowest value. |
| `PoolMethod::Max` | The highest value. |

`Min` and `Max` are literal extremes rather than best and worst; which one is preferable follows `VshipMetric::higher_is_better`. Pooling a list with no values in it fails with `VshipError::InvalidConfiguration`. `Mean` is the right choice for SSIMULACRA2 and Butteraugli, and the wrong one for CVVDP, whose per-frame values are a running mean.

### Configuration

| Builder | Default | Effect |
| --- | --- | --- |
| `with_metric` | `Ssimulacra2` | Which metric to compute. |
| `with_gpu_id` | auto | Device index. Unset by default, so a discrete GPU is chosen at run time; see [Device selection](#device-selection). |
| `with_handler_threads` | `4` | Handlers in the pool. Clamped to at least one; ignored for CVVDP, which always uses one. |
| `with_q_norm` | `2` | Butteraugli's norm exponent. Must be positive. |
| `with_intensity_multiplier` | `203.0` | Butteraugli's peak display brightness in nits. Must be positive and finite. |
| `with_display_model` | `standard_fhd` | CVVDP's display model key. Must be non-empty. Fails on an interior NUL. |
| `with_display_model_json` | empty | Path to a JSON display-configuration override, or empty for none. Passed to libvship verbatim. |
| `with_resize_to_display` | `false` | Whether CVVDP scales both inputs to the model's display resolution. |
| `with_fps` | `0.0` | Frame rate handed to CVVDP's temporal model. **Set this for CVVDP**: it is not a neutral default, and a zero rate accumulates error over the clip (see [CVVDP](#cvvdp-is-accumulated-not-per-frame)). Negative is rejected. |
| `with_reset_on_discontinuity` | `true` | Whether CVVDP clears its temporal history at a discontinuity in frame index. |
| `with_disable_temporal` | `false` | The inverse of the above, in the VapourSynth plugin's spelling. Note that upstream `disableTemporal` also disables score accumulation, which `Vship_InitCVVDP_1` has no field for; this only controls whether history is cleared at a cut. For per-frame scores, read each pair rather than pooling. |

The defaults match what libvship's own CLI does for an ordinary encode. The default handler count is [`DEFAULT_HANDLER_THREADS`], which is 4 — the same as the VapourSynth plugin's own default, so the two paths agree when nothing is configured. A `threads` setting maps directly onto `with_handler_threads`.

`with_display_model` and `with_display_model_json` return `Result`, because both strings cross into C and an interior NUL cannot be represented there. `VshipConfig::validate` checks the remaining constraints without touching libvship, so an unusable configuration is reported before a device is claimed.

### Colour

`VideoFormat` describes each side separately, and `colorspace_from_details` maps an `av_decoders::VideoDetails` onto libvship's `Vship_Colorspace_t`:

- **Bit depth** becomes `Vship_Sample_t`. libvship's values are non-contiguous — 8, 9, 10, 12, 14 and 16 bits are enum values 2, 3, 5, 7, 9 and 11 — so every variant is declared with an explicit discriminant and nothing is converted sequentially. A depth with no enum value (11 or 13, say) is reported as `UnsupportedFormat` rather than guessed.
- **Chroma sampling** becomes `Vship_ChromaSubsample_t`: 4:2:0 is `{1, 1}`, 4:2:2 is `{1, 0}` and 4:4:4 is `{0, 0}`. Monochrome has no chroma planes at all, so it is rejected.
- **Color metadata** is preserved independently in each `VideoFormat`. Explicit full/limited range, supported matrices, chroma locations, transfer functions (including native PQ/HLG) and primaries override defaults. Values without a native equivalent return `UnsupportedFormat` before device discovery; reserved values never become BT.709 silently. BT.2020 SDR transfers map to native BT.709 as in libvship's FFMS2 adapter.
- Only absent/`Unspecified` values use defaults: limited range, left-sited chroma, BT.709 transfer/primaries, and BT.709 matrix above 650 lines (BT.601 otherwise). The family remains planar YUV; RGB input is rejected.
- **Av1an's `resolution`** becomes `target_width` and `target_height`. An absent target resolution is libvship's `-1` "do not scale" sentinel, which is why no plane is resized here — libvship scales each input internally.

`VideoFormat::colorspace` performs the same mapping from a `VideoFormat` alone, for callers that have geometry rather than a decoder.

av-decoders 0.12 populates metadata through FFMS2/FFmpeg. For VapourSynth, get the
backend with `get_vapoursynth_impl()`, then call its `get_extended_video_details()`
to read the first output frame's properties; ordinary decoder details omit them. Y4M color properties are
unknown. Mastering-display/content-light-level metadata and per-frame metadata
changes are not applied by this binding. `VideoFormat` adds the five optional
decoder color fields; struct-literal callers must supply them (or use `from_details`).
Unrecognized raw numeric metadata may already be collapsed to `None` by the
decoder; this API cannot distinguish that from an absent property.

### Device selection

Leaving `with_gpu_id` unset does not mean device 0. A machine with more than one
GPU usually pairs a discrete part with an integrated one, and the discrete part
is the one that can run these metrics at a useful rate, so libvship's
`integrated` flag is used to prefer it:

- **Discrete first.** The highest-indexed device reporting `integrated == 0` wins,
  which on a typical laptop or desktop is the only discrete GPU.
- **Integrated only as a last resort.** If every device reports integrated — an
  iGPU-only machine, or a desktop with no discrete part — the last device
  enumerated is used, so scoring still works.
- **Cached.** The choice is resolved once and kept in a `OnceLock`. A machine's
  GPU configuration cannot change while the process runs, so re-enumerating on
  every request would be pure waste.
- **Never used when `integrated` is unavailable.** A backend that does not fill
  the field reports zero, which reads as discrete, so such a build simply keeps
  the first device.

`VshipConfig::gpu_id` reports what was configured (`GPU_UNSET` when nothing was),
`has_explicit_gpu_id` distinguishes the two cases, and `gpu_id_or_default`
returns the resolved index. `VshipScorer::gpu_id` reports the device a scorer was
actually built for. The module-level `default_gpu_id()` exposes the same choice
on its own, as an `Option` that is `None` when libvship reports no devices.

### Differing input sizes need an explicit target

libvship performs **no geometry-equality check** between the two sides. It converts
each per its own colorspace and then fails `Vship_InitHandler` with
`DifferingInputType` (status 4) if the two results differ in size:

```text
Vship received 2 videos with different properties.
(Advice) verify that they have the same width, height and length
```

Scoring a 1080p reference against a 720p encode therefore requires naming the
common size explicitly, through either `target_width`/`target_height` on the
encode or on the source. FFVship avoids this by always telling libvship to scale
the encode to the source, which is the behaviour to copy; passing no target at
all (`-1`, the "do not scale" sentinel) leaves both inputs at their native sizes
and fails unless they already match.

## Native or VapourSynth plugin

Av1an ships both paths for SSIMULACRA2, Butteraugli and CVVDP and picks between them at runtime. The C API is preferred wherever `is_available()` is true, because it scores pairs as they are decoded rather than through a filter graph. The decision is `native_supported`, and it falls back to the plugin branch when the metric is one libvship does not implement, when no libvship or no usable device is found, or when native scoring fails at any point. Every fallback is logged rather than propagated, so behaviour on a machine without libvship is unchanged.

VMAF and XPSNR are unaffected: VMAF is served by the separate libvmaf binding and XPSNR by the `vszip` plugin, neither of which libvship provides.

## Prerequisites

libvship is **not** vendored, and it has no CPU fallback in the builds that are published — it is a GPU metric. Install it separately:

- [libvship](https://codeberg.org/Line-fr/Vship) 5.1.2 or later.
- A GPU and the driver matching the libvship build you installed: a Vulkan driver for the `vulkan` build, the NVIDIA CUDA runtime for `nvidia`, or the AMD ROCm runtime for `amd`.

> [!NOTE]
> FFMS2 is unrelated to libvship, which neither needs nor loads it. Av1an links FFMS2
> statically, so decoding never depends on a separate `ffms2.dll`; only the
> VapourSynth plugin path and the FFVship CLI use one, and neither is part of this
> crate.

## Installing libvship

### Windows — no building required

libvship cannot be built with MSVC. Fortunately a prebuilt DLL is published for each backend, so nothing needs compiling. This crate loads it **at runtime** with `libloading`, so only the `.dll` is needed — no import library, no headers, no compiler.

[`scripts/install-libvship-windows.ps1`](scripts/install-libvship-windows.ps1) downloads and stages one:

```powershell
.\scripts\install-libvship-windows.ps1
```

Each release publishes three builds. The crate passes the file to `dlopen`, so whichever variant is chosen is installed as `vship.dll`:

| Variant | Upstream asset | Runtime needed |
| --- | --- | --- |
| `vulkan` (default) | `libvship_VULKAN.dll` | any working Vulkan driver |
| `nvidia` | `libvship_NVIDIA.zip` → `libvship_NVIDIA.dll` | the NVIDIA CUDA runtime |
| `amd` | `libvship_AMD.zip` → `libvship_AMD.dll` | the AMD ROCm runtime (`amdhip64_6.dll`) |

`vulkan` is the default because its only imports are `vulkan-1.dll` and `KERNEL32.dll`: it runs against any working NVIDIA, AMD or Intel Vulkan driver and needs no vendor redistributable. `nvidia` and `amd` do require the CUDA or ROCm runtime respectively.

Select a variant with `-Variant`, and install elsewhere with `-Destination`.

Setting the environment variable is left to you, as a script that mutates the machine's environment is not something to run by surprise. Either scope it to the session:

```powershell
$env:VSHIP_PLUGIN_PATH = "$PWD\vship"
```

or persist it for future shells:

```powershell
[Environment]::SetEnvironmentVariable('VSHIP_PLUGIN_PATH', "$PWD\vship", 'User')
```

Then confirm it works:

```powershell
cargo test -p av-metrics-vship
```

### Already using the VapourSynth plugin

If libvship is installed as a VapourSynth plugin, nothing else is needed. The plugin sits at `<vs package>/vapoursynth/plugins/libvship.dll`, a sibling of `vsscript.dll` and on no loader search path, which is why the environment-derived directories below are probed. A working VapourSynth install therefore implies a loadable C API, and the C API path is preferred over the plugin wherever both are usable.

### Linux and macOS

There is no distribution-agnostic libvship build to install. Upstream publishes release assets for Windows only, so on Linux the C API must come from a source build:

```sh
./scripts/install-libvship-linux.sh
```

`-Backend` selects `vulkan` (default), `cuda` or `amd`. Vulkan is the default
because its only imports are `libvulkan.so.1` and libc/libstdc++/libm/libgcc_s,
so one build runs against any NVIDIA, AMD or Intel driver; the other two need the
CUDA and ROCm runtimes. `-Destination` chooses the install directory and `-Quiet`
suppresses the printed `export` line.

Vulkan initialization is lazy in libvship 5.1.2 and later, so the library can
be loaded on a machine without a Vulkan driver and report itself unavailable.

Set `VSHIP_LIB_DIR` or `VSHIP_PLUGIN_PATH` at build time so `build.rs` records the directory it finds, and at run time so `dlopen` finds the same one. Without either, the crate still compiles cleanly but reports itself unavailable, and the VapourSynth plugin path is used instead.


## Discovery order

libvship is on no loader search path, so `dlopen` alone would never find it. Candidates are tried most specific first:

1. `VSHIP_PLUGIN_PATH` — the user's explicit override.
2. `VSHIP_LIB_DIR` — the build-hint override `build.rs` records when it finds a library at compile time.
3. `<dir(VSSCRIPT_PATH)>/plugins`, then `<dir(VSSCRIPT_PATH)>` — the VapourSynth runtime that owns the plugin, and its own directory.
4. Every entry of `VAPOURSYNTH_EXTRA_PLUGIN_PATH` — VapourSynth's own extra-plugin path list, `;`-separated on Windows and `:`-separated elsewhere.
5. The platform system directories (`C:\Windows\System32`, `C:\Windows\SysWOW64`).
6. The bare file names `libvship.dll`, `vship.dll` (or `.dylib` / `.so` elsewhere), so the OS loader's own search path applies as a last resort.

Each directory is tried under both names, because an installation may use either. `build.rs` performs the same probe at compile time purely to record the `VSHIP_LIB_DIR` hint, and never fails the build. `ffi::library_candidates` returns the full list in this order, which is what the discovery tests assert against.

## Availability

`is_available()` needs more than a successful `dlopen`: `Vship_GetVersion` must answer and `Vship_GPUFullCheck` must pass on a device. Since libvship 5.1.2, a missing Vulkan driver is reported through the C API instead of aborting during load. Missing runtime libraries for other backends can still prevent loading. Any unavailable library or device returns `false`, so callers can use the VapourSynth plugin path instead.

The result is cached, so the call is cheap. `VshipScorer::backend()` reports which backend the loaded build targets, and `device_name()` names the device.

## Testing

```powershell
cargo test -p av-metrics-vship
```

Everything that does not need libvship always runs: struct-layout assertions against the `VshipAPI.h` and `VshipColor.h` declarations, colourspace-mapping unit tests, and discovery tests that manipulate the environment variables.

Tests that need libvship are **skipped cleanly** when `is_available()` is false. CI has no GPU and no libvship binary, so the skip is the expected outcome there rather than a failure.

All test input is generated in memory as a y4m byte stream and fed to `av-decoders`' always-available Y4M path. No media files are committed, and the suite runs anywhere, including environments without FFMS2 or FFmpeg installed.

## Benchmarks

```powershell
cargo bench -p av-metrics-vship
```

Criterion benchmarks cover throughput at several resolutions, scaling with the handler count, cost per metric, and decoding alone for comparison. They are deliberately **not** run in CI: Criterion adds minutes per run and its output is noisy on shared runners, so enforcing a regression threshold would produce flaky failures. The scoring benchmarks skip themselves when libvship is unavailable.

## Licence

BSD-2-Clause-Patent, matching the rest of Av1an.

[libvship]: https://codeberg.org/Line-fr/Vship
[`av-decoders`]: https://github.com/rust-av/av-decoders
[`DEFAULT_HANDLER_THREADS`]: https://docs.rs/av-metrics-vship/latest/av_metrics_vship/constant.DEFAULT_HANDLER_THREADS.html
[`FrameScore`]: https://docs.rs/av-metrics-vship/latest/av_metrics_vship/struct.FrameScore.html
