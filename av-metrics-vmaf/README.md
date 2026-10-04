# av-metrics-vmaf

Native VMAF scoring via [libvmaf], with CUDA acceleration when available.

VMAF (Video Multi-Method Assessment Fusion) is Netflix's perceptual quality metric.
This crate binds libvmaf directly and scores frame pairs as they are decoded, so no
temporary files or JSON logs are involved.

Frames come from [`av-decoders`], so a clip can be read from Y4M, FFMS2, FFmpeg or a
VapourSynth script without this crate knowing which.

## Features

- **Per-frame scores**, streamed as frames are decoded. Unlike the stock VapourSynth
  VMAF plugin, nothing is written to disk and no result is available only at the end.
- **CUDA when it works, CPU otherwise.** The backend is chosen by actually
  constructing a scoring context, then reported via [`VmafScorer::backend`], so a
  silent fallback can never be mistaken for GPU acceleration.
- **Optional dependency.** libvmaf is opened with `dlopen` at runtime, so the crate
  compiles on machines that do not have it. Scoring reports
  [`VmafError::LibraryNotFound`] rather than failing the build.
- **No `bindgen`.** The C surface is declared by hand, keeping the build free of a
  libclang dependency.

## Usage

```rust
use av_decoders::Decoder;
use av_metrics_vmaf::{PoolMethod, VmafConfig, VmafScorer, VideoFormat};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
// Requires libvmaf at runtime; see "Building libvmaf" below.
let mut reference = Decoder::from_file("reference.mkv")?;
let mut distorted = Decoder::from_file("distorted.mkv")?;

let details = *reference.get_video_details();
let format = VideoFormat {
    width: details.width as u32,
    height: details.height as u32,
    bit_depth: details.bit_depth as u32,
    chroma_sampling: details.chroma_sampling,
};

let mut scorer = VmafScorer::new(VmafConfig::new().with_threads(4), format)?;
println!("scoring on the {} backend", scorer.backend());

let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})?;
println!("VMAF: {:.3}", VmafScorer::pool(&scores, PoolMethod::Mean)?);
# Ok(())
# }
```

Scores are also emitted through the callback passed to
[`score_decoders`][`VmafScorer::score_decoders`], which is useful for reporting
progress while a long encode is being scored.

## Cargo features

| Feature | Default | Effect |
| --- | --- | --- |
| `cuda` | off | Reserved for enabling CUDA symbol resolution. CUDA still requires libvmaf itself to be built with `-Denable_cuda=true`; otherwise the CPU backend is used regardless. |

## Prerequisites

libvmaf is **not** vendored. Install it separately:

- [libvmaf](https://github.com/Netflix/vmaf) 3.2.0 or later.
- A C toolchain and, for the build, Meson and Ninja.
- **CUDA only:** the NVIDIA CUDA Toolkit (tested upstream with CUDA >= 11) plus
  [`nv-codec-headers`](https://github.com/FFmpeg/nv-codec-headers).

## Building libvmaf

libvmaf's own prerequisites are Python >= 3.6, Meson >= 0.56.1, Ninja >= 1.7.1,
NASM >= 2.13.02 (x86 only) and `xxd`.

### Arch Linux

The easiest path — a binary package is available:

```bash
pacman -S vmaf
```

This installs `libvmaf.so` to `/usr/lib`, which the build script finds
automatically. Note that Arch's package is built **without** CUDA.

### Debian / Ubuntu

`libvmaf-dev` exists only in Debian unstable. On stable, build from source:

```bash
sudo apt-get install -y build-essential meson ninja-build nasm python3-pip xxd
meson setup build --buildtype release
meson install -C build
```

### macOS

```bash
brew install meson ninja nasm
meson setup build --buildtype release
meson install -C build
```

### From source, any platform

```bash
git clone --branch v3.2.1 --depth 1 https://github.com/Netflix/vmaf.git
cd vmaf
meson setup libvmaf libvmaf/build --buildtype release
meson install -C libvmaf/build
```

Useful Meson options:

| Option | Effect |
| --- | --- |
| `-Denable_cuda=true` | Enable CUDA. Requires `nvcc` and `nv-codec-headers`. |
| `-Denable_avx512=true` | Faster SIMD on supported CPUs. Requires NASM >= 2.14. |
| `-Denable_float=true` | Compile the floating-point feature extractors. |
| `-Denable_nvtx=true` | Emit NVTX ranges for Nsight profiling. |
| `-Denable_tools=false` | Skip the `vmaf` CLI, leaving just the library. |
| `-Denable_docs=false`, `-Denable_tests=false` | Trim the build further. |
| `-Denable_asm=false` | Drop the NASM CPUID assembly. |

### Windows — no building required

**libvmaf cannot be built with MSVC.** vcpkg's port declares
`"supports": "!windows"`, and that exclusion is accurate: `src/feature/integer_vif.h`
defines `__builtin_clz` under `#ifdef _MSC_VER`, which MSVC rejects, and several
headers `#include <pthread.h>`, which the MSVC CRT does not provide.

Fortunately **a prebuilt `libvmaf.dll` is published**, so nothing needs compiling.
Pick whichever route suits you:

- **[Manual install](#windows-manual-install-no-msys2)** — download three `.tar.zst`
  packages and extract them with the `tar` that ships in Windows. No MSYS2, no
  pacman, no compiler, ~518 KB.
- **MSYS2** — `pacman -S mingw-w64-x86_64-vmaf`, which installs
  `mingw64/bin/libvmaf.dll` along with headers, an import library and `pkgconfig`.
  Also available for the `ucrt64`, `clang64` and `clangarm64` environments.

Then point `VMAF_LIB_DIR` at the directory containing `libvmaf.dll`.

> [!IMPORTANT]
> Every prebuilt Windows DLL is compiled **without CUDA**, so Windows gets CPU-only
> VMAF. libvmaf's CUDA build needs `nvcc` and `nv-codec-headers`, which are not part
> of the MinGW package.

### Windows manual install (no MSYS2)

This crate loads libvmaf **at runtime** with `libloading`, so you need only the
`.dll` files — not MSYS2's toolchain, not the import library, and not a compiler.
Three DLLs are enough, and all three come from MSYS2's ordinary binary packages:

| File | Provided by | Why it is needed |
| --- | --- | --- |
| `libvmaf.dll` | `mingw-w64-x86_64-vmaf` | the library itself |
| `libgcc_s_seh-1.dll` | `mingw-w64-x86_64-libgcc` | GCC unwinder, imported by `libvmaf.dll` |
| `libwinpthread-1.dll` | `mingw-w64-x86_64-libwinpthread-git` | pthreads, imported by `libvmaf.dll` |

The last two are transitive dependencies of `libvmaf.dll` itself — its only other
imports are `kernel32.dll` and `msvcrt.dll`, which every Windows install already has.

`tar` and `curl` are both in the base Windows install, so nothing else is required.
[`scripts/install-libvmaf-windows.ps1`](scripts/install-libvmaf-windows.ps1) does
the whole thing:

```powershell
.\scripts\install-libvmaf-windows.ps1
```

By default it installs into `.\libvmaf`, downloading three packages (~518 KB) and
all nine upstream models (~550 KB), then prints the environment variables to set.
It cleans up after itself. Useful switches:

| Switch | Effect |
| --- | --- |
| `-InstallDir <path>` | Install somewhere other than `.\libvmaf` |
| `-LibvmafVersion <ver>` | Fetch a different libvmaf release, default `3.2.1` |
| `-ModelsOnly` | Refresh the models without re-downloading the DLLs |

Setting the variables is left to you, as a script that mutates the machine's
environment is not something to run by surprise. Either scope it to the session:

```powershell
$env:VMAF_LIB_DIR    = "$PWD\libvmaf\bin"
$env:VMAF_MODEL_PATH = "$PWD\libvmaf\model"
```

or persist it for future shells:

```powershell
[Environment]::SetEnvironmentVariable('VMAF_LIB_DIR',    "$PWD\libvmaf\bin",   'User')
[Environment]::SetEnvironmentVariable('VMAF_MODEL_PATH', "$PWD\libvmaf\model", 'User')
```

Then confirm it works:

```powershell
cargo test -p av-metrics-vmaf
```

`every_model_in_the_search_path_loads` reports which models the installed libvmaf
actually accepts, so a build that cannot use some of them is visible immediately
rather than at scoring time.

#### Which models work

All nine are downloaded, but only four work with the MSYS2 build:

| Model | Usable | How to select |
| --- | --- | --- |
| `vmaf_v0.6.1` | yes | default |
| `vmaf_v0.6.1neg` | yes | `features: ["neg"]` |
| `vmaf_4k_v0.6.1` | yes | `features: ["uhd"]` |
| `vmaf_float_v0.6.1` | **no** | needs `-Denable_float=true` |
| `vmaf_float_v0.6.1neg` | **no** | needs `-Denable_float=true` |
| `vmaf_float_4k_v0.6.1` | **no** | needs `-Denable_float=true` |
| `vmaf_b_v0.6.3` | **no** | not supported by libvmaf 3.2.1 |
| `vmaf_float_b_v0.6.3` | **no** | needs `-Denable_float=true` |

That is a property of the binary, not of the models. libvmaf's meson options
default `enable_float` to **false**, and there is no `enable_bound` option at all
in 3.2.1 — the BOUND extractors were removed, which is why `vmaf_b_v0.6.3` fails
with `could not read model from path` even though the file is present and valid.

The MSYS2 package is therefore built with neither `VMAF_feature_float_adm` nor
`VMAF_feature_bound_adm`, and libvmaf rejects the affected models with `EINVAL`
when it tries to register their extractors:

```console
> dumpbin /exports libvmaf.dll | findstr adm
VMAF_feature_adm2_score      VMAF_feature_adm_scale0    ... present
VMAF_feature_float_adm      VMAF_feature_bound_adm    ... absent
```

The four working models are all reachable by name through `VmafModel`, so
`features: ["neg"]`, `features: ["uhd"]` and their combination
`features: ["uhd", "neg"]` (which selects `vmaf_4k_v0.6.1neg`) resolve on a build
with no built-in models. `features: ["weighted"]` maps to `vmaf_b_v0.6.3` and
**will fail on every stock libvmaf build**, Windows or Linux — use
`features: ["uhd"]`, or pass a model path explicitly, instead.

> [!TIP]
> The script pins package versions for reproducibility. If the canonical host is
> unreachable, it falls back to the `mirrors.dotsrc.org` and `mirror.msys2.org`
> mirrors, which serve byte-identical packages. You will see which one answered:
>
> ```console
>   mingw-w64-x86_64-vmaf-3.2.1-1-any.pkg.tar.zst  <- mirrors.dotsrc.org
> ```
>
> To see what MSYS2 currently offers, list the repository index directly. Note
> that some mirrors refuse directory listings and answer only `403`, so this
> needs the canonical host or one that permits it:
>
> ```powershell
> (curl.exe -s -A "Mozilla/5.0" https://repo.msys2.org/mingw/mingw64/) -match "mingw-w64-x86_64-libgcc-[^"]+\.pkg\.tar\.zst" |
>   Select-Object -Last 1
> ```

> [!NOTE]
> `vmaf.exe`, the import library in `mingw64/lib/` and the headers are all optional:
> `vmaf.exe` is only useful for cross-checking scores by hand, and this crate never
> needs the headers because it declares the C ABI itself.

Verified on a machine with no MSYS2 installed: the full `correctness` suite passes,
including the check that pins our scores to libvmaf's own CLI and to FFmpeg's
`libvmaf` filter.

### Models

**Not every libvmaf build includes the VMAF models.** Where they are missing,
`vmaf_model_load("vmaf_v0.6.1")` fails with `EINVAL` and no scoring is possible.
Arch's `vmaf` package and MSYS2's `mingw-w64-*-vmaf` are both built this way.

This crate handles that automatically: when a stock model fails to load from the
library, it looks for the same model as a `<version>.json` file and retries. The
search order is

1. `VMAF_MODEL_PATH`, if set — an explicit override, so it wins outright
2. platform defaults: `/usr/share/model` (Arch), `/usr/share/vmaf/model`
   (Debian, Fedora), `/usr/local/share/model` and `/usr/local/share/vmaf/model`
   on Linux; `/opt/homebrew/share/vmaf/model` and `/usr/local/share/vmaf/model`
   on macOS; `%MSYS2_ROOT%\mingw64\share\vmaf\model` and the `ucrt64` equivalent
   on Windows
3. `model/` beside the running executable — this is what a release layout uses
4. `model/`, then `share/vmaf/model/`, beside the directory in `VMAF_LIB_DIR`

System paths are searched **before** the executable-adjacent ones, so a stray
`vmaf_v0.6.1.json` in a shared or downloads-adjacent binary directory cannot
silently override the distribution's model. `VMAF_MODEL_PATH` is the deliberate
exception, since setting it is an explicit choice.

So for a release that ships `condor.exe`, `libvmaf.dll` and `model/`, no
configuration is needed. Otherwise point `VMAF_MODEL_PATH` at the directory holding
`vmaf_v0.6.1.json`, or pass the file explicitly with `VmafConfig::with_model_path`.

The stock model is a single ~19 KB JSON file from the [libvmaf repository][models].
A model only works if the libvmaf build has the matching feature extractors — see
[which models work](#which-models-work) for why five of the nine are rejected by
the MSYS2 build.

> [!TIP]
> If you hit `ModelNotFound`, the error lists every directory that was searched,
> which makes a mistyped `VMAF_MODEL_PATH` obvious.

> [!NOTE]
> Identical clips do **not** score 100. libvmaf's motion and temporal extractors
> compare consecutive frames, so frame 0 has no predecessor and scores lower — 97.43
> on the reference clip — while every later frame scores exactly 100. The pooled
> mean of an identical pair is therefore around 99.96. Verified against both
> libvmaf's own CLI and FFmpeg's `libvmaf` filter, which agree to six decimals.

If you would rather compile it yourself, MSYS2 with MinGW-w64 is the configuration
upstream documents and tests:

```bash
meson setup libvmaf libvmaf/build --buildtype release --default-library shared
meson install -C libvmaf/build
```

## Configuration

The build script searches for libvmaf in this order:

1. **`VMAF_LIB_DIR`** — a directory containing the library. Set this when libvmaf is
   somewhere non-standard, or when cross-compiling.
2. Well-known system locations: `/usr/lib`, `/usr/lib64`, `/usr/local/lib`, `/lib`,
   `/lib64` and `$CONDA_PREFIX/lib` on Linux; `/opt/homebrew/lib`, `/usr/local/lib`,
   `/opt/local/lib` and `$CONDA_PREFIX/lib` on macOS.

```bash
export VMAF_LIB_DIR=/opt/libvmaf/lib
cargo build --release
```

`VMAF_LIB_DIR` is also honoured at runtime, so a binary can be pointed at a libvmaf
installed after the fact. If nothing is found at build time, the crate still compiles
and prints a warning; scoring then reports unavailability at runtime.

### Frame selection

Score every frame you care about and let this crate return one score per submitted
pair, positionally. Both `score_frames` and `score_decoders` preserve position: if
libvmaf fails to produce a score for a frame, that is reported as
[`VmafError::MissingScore`] rather than skipped, because dropping an entry would
shift every later score into the wrong slot.

`VmafConfig::validate` rejects `n_subsample > 1`. libvmaf applies that as a gate on
the **spatial** extractors only:

```c
if (!(fex->flags & (VMAF_FEATURE_EXTRACTOR_TEMPORAL | VMAF_FEATURE_EXTRACTOR_PREV_REF)))
    if ((n_subsample > 1) && (index % n_subsample)) continue;
```

The temporal and motion extractors still run on every frame, so the frames in between
end up with only some of their features and report no score at all. To score fewer
frames, submit every *n*-th pair instead:

```rust
// Score every third frame by selecting every third pair.
let every_third: Vec<_> = pairs.iter().step_by(3).cloned().collect();
let scores = scorer.score_frames(&every_third)?;
```

Not every libvmaf build embeds the stock VMAF models. When yours does not, the model
is looked up on disk in this order:

1. **`VMAF_MODEL_PATH`** — an explicit override.
2. System locations: `/usr/share/model` (Arch), `/usr/share/vmaf/model` (Debian,
   Fedora), `/usr/local/share/model` and `/usr/local/share/vmaf/model` on Linux;
   `/opt/homebrew/share/vmaf/model` and `/usr/local/share/vmaf/model` on macOS;
   `%MSYS2_ROOT%\mingw64\share\vmaf\model` and the `ucrt64` equivalent on Windows.
3. `model/` beside the running executable, which is the release layout.
4. `model/` and `share/vmaf/model/` next to `VMAF_LIB_DIR`.

System paths are searched before the executable-adjacent ones, so a stray
`vmaf_v0.6.1.json` in a shared or downloads-adjacent binary directory cannot silently
override the distribution's model. `VMAF_MODEL_PATH` is the deliberate exception,
since setting it is an explicit choice.

Arch's `vmaf` package installs **all nine** models to `/usr/share/model`, which is
what the Docker image relies on, so Linux needs no download at all.

## Platform support

| Platform | libvmaf availability | VMAF support |
| --- | --- | --- |
| Linux (Arch) | `vmaf` package, ships all nine models | CPU |
| Linux (Debian/Ubuntu) | `libvmaf-dev` (unstable) or source | CPU |
| Linux with CUDA-enabled libvmaf | source, `-Denable_cuda=true` | CPU, with CUDA attempted |
| macOS | `brew` or source | CPU |
| Windows | prebuilt DLL from MSYS2 packages, extracted manually | CPU |

GPU acceleration is available **only on Linux with a CUDA-enabled libvmaf**. See
`BackendPreference` to require a specific backend or to skip the CUDA attempt.

Which of the nine models a build can use depends on its feature extractors: the
`float` and BOUND models need options that no packaged build enables. See
[which models work](#which-models-work).

## Testing and benchmarking

**No media files are required.** All test content is generated in memory at runtime
by `tests/common`, as a deterministic y4m stream fed through `av-decoders`'
always-available Y4M path. Content comes from a fixed-seed generator, so runs are
byte-identical across platforms and machines.

```bash
cargo test -p av-metrics-vmaf
```

Tests that require libvmaf skip themselves when it is unavailable, so the suite is
green in any environment.

Not every libvmaf build embeds the stock VMAF models. When yours does not, point
`VMAF_TEST_MODEL` at a model JSON (for example `model/vmaf_v0.6.1.json` from the
libvmaf repository) and the scoring tests will use it:

```bash
VMAF_TEST_MODEL=/path/to/vmaf_v0.6.1.json cargo test -p av-metrics-vmaf
```

Benchmarks exist to guide local optimisation work:

```bash
cargo bench -p av-metrics-vmaf
```

They are deliberately **not** run in CI: Criterion adds minutes per run and its
output is noisy on shared runners, so enforcing a regression threshold would produce
flaky failures.

## License

BSD-2-Clause-Patent, matching [libvmaf]. Any binary redistributing libvmaf must also
carry its license text.

[libvmaf]: https://github.com/Netflix/vmaf
[models]: https://github.com/Netflix/vmaf/tree/v3.2.1/model "libvmaf model files"
[`av-decoders`]: https://github.com/rust-av/av-decoders
[`VmafScorer::backend`]: https://docs.rs/av-metrics-vmaf/latest/av_metrics_vmaf/struct.VmafScorer.html#method.backend
[`VmafScorer::score_decoders`]: https://docs.rs/av-metrics-vmaf/latest/av_metrics_vmaf/struct.VmafScorer.html#method.score_decoders
[`VmafError::LibraryNotFound`]: https://docs.rs/av-metrics-vmaf/latest/av_metrics_vmaf/enum.VmafError.html
[`VmafError::MissingScore`]: https://docs.rs/av-metrics-vmaf/latest/av_metrics_vmaf/enum.VmafError.html
