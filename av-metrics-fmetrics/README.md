# av-metrics-fmetrics

Native CPU perceptual quality scoring (SSIMULACRA2, Butteraugli, CVVDP, IW-SSIM, MS-SSIM)
via the [fmetrics](https://github.com/halidecx/fmetrics) C API.

## Features

- SSIMULACRA2, Butteraugli, CVVDP, IW-SSIM and MS-SSIM
- Frames come from `av_decoders`, so a clip can be read from Y4M, FFMS2, FFmpeg or a
  VapourSynth script without this crate knowing which
- Loaded with `dlopen` at runtime, so the crate compiles on machines without it and
  `is_available()` reports `false`
- Scores pairs as they are decoded, so no temporary files and no distortion maps

## Usage

```rust
use av_decoders::Decoder;
use av_metrics_fmetrics::{ColorInfo, FmetricsConfig, FmetricsMetric, FmetricsScorer};

let config = FmetricsConfig::new(FmetricsMetric::Ssimulacra2);

let mut reference = Decoder::from_file("reference.mkv")?;
let mut distorted = Decoder::from_file("distorted.mkv")?;

let details = *reference.get_video_details();
let color = ColorInfo::from_details(&details)?;
let mut scorer = FmetricsScorer::new(config, color)?;

// Submit pairs as they are decoded; the caller owns frame lifetime.
let score = scorer.submit_pair(&reference_planes, &distorted_planes)?;
println!("SSIMULACRA2: {}", score.require(FmetricsMetric::Ssimulacra2)?);
```

### Choosing a metric

| Metric        | Direction        | Notes                                 |
| ------------- | ---------------- | ------------------------------------- |
| `Ssimulacra2` | higher is better | Perceptual, matches libvship's naming |
| `Butteraugli` | lower is better  | A distance, not a quality measure     |
| `Cvvdp`       | higher is better | Temporal; `cvvdp_jod` is the headline |
| `Iwssim`      | higher is better | Needs at least 16 px per dimension    |
| `Msssim`      | higher is better | Needs at least 16 px per dimension    |

`FmetricsMetric::prefers_lower_is_better()` states the direction, so pooling does
not have to be re-derived at each call site.

### CVVDP is accumulated, not per-frame

CVVDP's temporal filter is parameterised by the frame rate, and a rate of zero
accumulates error *silently* rather than failing. A rate is therefore required
and rejected up front:

```rust
# use av_metrics_fmetrics::{FmetricsConfig, FmetricsMetric};
let config = FmetricsConfig::new(FmetricsMetric::Cvvdp).with_frame_rate(24.0);
```

One context is kept for the whole run and sees every frame in order. On a scene
break — or any gap in the selection — call `reset_temporal()` before submitting
the next pair. Whether a gap is a scene break is the caller's knowledge, so the
scorer does not decide it.

### Colour

fmetrics takes **interleaved RGB**, not planar YUV, so every pair is converted
before submission. That makes the conversion part of the measurement: a wrong
matrix silently changes every score. The matrix and range therefore come from the
clip's own details rather than from a default, and `ColorInfo` exposes
`with_matrix`, `with_range` and `with_hdr` for content whose real characteristics
are known.

`av-decoders` reports no colour metadata, so the defaults are declared rather than
detected: BT.709 above 650 lines, BT.601 below, limited range throughout. That is
the same declaration made on the libvship path, which keeps the two comparable.

Bit depth maps directly: 8-bit becomes `RGB_UINT8`, and 10- or 12-bit is
up-converted to `RGB_UINT16` rather than truncated, which would discard precision
the source has. Other depths are rejected rather than silently rescaled.

## Prerequisites

- Rust 1.97 or newer
- fmetrics, built with Zig 0.16.0

fmetrics is distributed as a source library rather than a prebuilt install, and
has no shared-library target upstream, so the library is built from a branch
carrying that work.

## Installing fmetrics

Unlike libvship and libvmaf there is no prebuilt Windows binary and no Linux
package to install, so this is a source build. It needs Zig 0.16.x and Git;
0.17 is refused because it removed `b.build_root`, which the pinned `build.zig`
uses.

### Windows

```powershell
winget install --id zig.zig --version 0.16.0 --exact
.\scripts\install-fmetrics-windows.ps1
```

The script fetches a pinned commit, runs `zig build --fetch` to populate the
dependency tree, patches fcvvdp (it calls POSIX `sysconf()`, which does not
compile on Windows), builds `fmetrics.dll`, and installs it to `$PWD\fmetrics`.

`-Destination` chooses another directory; the script prints the
`SetEnvironmentVariable` line that points the crate at it.

### Linux and macOS

```sh
git clone --branch add-windows-shared-build https://github.com/BoatsMcGee/fmetrics.git
cd fmetrics
zig build --release=fast -Dshared=true
```

That produces `zig-out/lib/libfmetrics.so` or `.dylib`.

## Discovery order

At runtime the library is looked for in:

1. `FMETRICS_LIB_PATH` or `FMETRICS_LIB_DIR`
2. The directory holding the running executable
3. Platform system directories
4. Bare names, letting the platform loader's own search path apply

Staging the library beside the executable is therefore enough.

## Availability

```rust
# use av_metrics_fmetrics::is_available;
if !is_available() {
    // No fmetrics on this machine; scoring is unavailable.
}
```

Cheap to call: the library is resolved once and cached.

## Testing

```sh
cargo test -p av-metrics-fmetrics
```

Tests that need the library skip themselves when it is absent, so the suite passes
on a machine without it.

## Benchmarks

One example reports three numbers, because one cannot answer the question a
caller has:

- **end-to-end** — decode and score a pair as the scorer is actually driven.
  Comparable against another engine's end-to-end figure.
- **library only** — score pre-converted frames, so decode and conversion are
  out of the timed region. This is what says whether the library itself
  parallelises; end-to-end cannot, because decode does not.
- **conversion only** — the YUV to RGB conversion on its own, which is roughly
  half of fmetrics' per-frame cost and cannot be inferred from the other two.

```sh
# Everything, single-threaded.
cargo run --release -p av-metrics-fmetrics --example bench -- ref.mkv dist.mkv

# Through 12 threads, 200 frames per thread.
cargo run --release -p av-metrics-fmetrics --example bench -- ref.mkv dist.mkv 12 200
```

Scores are printed with the timings throughout: a speedup that changes the
numbers is not a speedup. `identical_to_serial` in the library-only section
asserts that concurrent scoring through distinct workspaces is bit-identical to a
serial run, which is the property the whole threading design rests on.

## Licence

BSD-2-Clause-Patent, matching the rest of the workspace. fmetrics itself is
Apache-2.0; redistributing its binary requires shipping its `LICENSE` and an
attribution notice.