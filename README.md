# Av1an

![Demonstration of the California Condor TUI](./california-condor/media/demo.avif)

[![Discord server](https://discordapp.com/api/guilds/696849974230515794/embed.png)](https://discord.gg/Ar8MvJh)
[![CI tests](https://github.com/rust-av/Av1an/actions/workflows/tests.yml/badge.svg)](https://github.com/rust-av/Av1an/actions/workflows/tests.yml)
[![](https://img.shields.io/crates/v/av1an.svg)](https://crates.io/crates/av1an)
[![](https://tokei.rs/b1/github/rust-av/Av1an?category=code)](https://github.com/rust-av/Av1an)

<a href="https://www.buymeacoffee.com/master_of_zen" target="_blank"><img src="https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png" alt="Buy Me A Coffee" style="height: 60px !important;width: 217px !important;" ></a>

Av1an is a video encoding Rust library and command-line tool designed to be as fast as possible, easy to use, and extremely extensible. It supports a wide variety of encoders and tools to produce high quality videos.

## Features

* Support for Windows, Linux, and MacOS
* Docker images available
* Hyper-scalable video encoding
* Cancel and resume at any time without losing progress
* Real-time progress feedback
* Terminal User Interface (TUI)
* Modular architecture for custom workflows

### Video Filtering

Av1an is built with [FFMS2][ffms2] for simple decoding but also supports [VapourSynth][vapoursynth] for advanced filtering and analysis.

* Decode the input video with either the [FFMS2][ffms2], [BestSource][vs-bestsource], [L-SMASH-Works][vs-lsmash], or [DGDecNV][vs-dgdecnv] VapourSynth plugin
* Pass a VapourSynth Python script (`.vpy` or `.py`) as the input
* Apply custom variables to the VapourSynth script
* Output a specific track/index from the VapourSynth script
* Apply additional VapourSynth filters such as [Trim][vs-trim], [Crop][vs-crop], [Resize][vs-resize], and [Splice][vs-splice]

### Video Encoding

Av1an supports the following encoder executables:

- [aomenc][aom] - Alliance for Open Media reference AV1 encoder
- [SvtAv1EncApp][svt-av1] - Scalable Video Technology for AV1 AV1 encoder
- [rav1e][rav1e] - Rust AV1 encoder
- [avmenc][avm] - Alliance for Open Media AOM Video Model reference AV2 encoder
- [vpxenc][vpx] - WebM VP8/VP9 encoder
- [x264][x264] - x264 H.264 encoder
- [x265][x265] - x265 H.265 encoder
- [vvenc][vvenc] - Fraunhofer Versatile Video Encoder H.266 encoder
- [FFmpeg][ffmpeg] - FFmpeg

### Video Analysis

Av1an can analyze video properties and qualities to improve encoded video quality by:

* Splitting the video into contiguous scenes
* Scaling Photon Noise ISO based on the level of noise/grain in each scene
* Finding the most efficient quantizer for each scene for a given quality target
* Choosing the appropriate encoding speed for each scene

## Getting Started

### Installation

Install the Av1an CLI, California Condor, from either the [Arch AUR][aur], [crates.io][crates], or [Docker](#docker). You can also download a Windows binary from [Releases](https://github.com/rust-av/Av1an/releases) or [compile it manually](#compiling).

```bash
$ pacman -S condor # Arch Linux & Manjaro
$ cargo install condor # crates.io
$ docker pull boatsmcgee/condor:latest # Docker Hub
```

#### Windows releases

Download **`condor-windows-x64.zip`** from [Releases](https://github.com/rust-av/Av1an/releases)
and extract it into one directory. The layout matters, because Condor finds parts of
itself by relative path:

```text
condor/
├── condor.exe
├── libvmaf.dll            VMAF
├── libvship.dll           SSIMULACRA2, Butteraugli and CVVDP on the GPU
├── libgcc_s_seh-1.dll     \  runtime dependencies of libvmaf
├── libstdc++-6.dll        |
├── libwinpthread-1.dll   /
├── model/                 VMAF models, and the TransNetV2 scene detection model
└── configuration.schema.json
```

Two mistakes produce confusing failures rather than an error:

- **Leaving `model/` out or flattening it.** The VMAF models and the TransNetV2
  scene detection model must be in a `model` subdirectory beside `condor.exe`.
  Without them VMAF and the `transnetv2` scene detection method report
  themselves unavailable with no other symptom.
- **Downloading the individual files instead of the archive.** They only work when
  kept together in one directory as laid out above. The loose files are attached
  for reference and for partial updates, not as an install method.

Every DLL here is optional: each one only disables the metric it belongs to, and
`condor.exe` starts and runs without any of them. FFMS2 is linked statically into the
executable, so decoding never depends on a separate `ffms2.dll` — only the
VapourSynth plugin path and the FFVship CLI use one.

#### Linux releases

Download **`condor-linux-x64.zip`** from [Releases](https://github.com/rust-av/Av1an/releases)
and extract it into one directory — same layout as Windows, with `.so` libraries
beside the executable and the VMAF and TransNetV2 models in `model/`. The archive keeps the
executable bit on `condor`, so `unzip` followed by `./condor --version` is the whole
install.

**x86_64 only.** There is no aarch64 build: on arm64 hosts the executable fails
with `cannot execute: required file not found`, because the archive ships one
x86-64 ELF. Build from source on those machines.

Unlike Windows, the executable is **not** self-sufficient: the host provides

* **`libffms2`**, which `condor` links against directly. Without it the loader
  fails before the program starts (`error while loading shared libraries:
  libffms2.so.5`), even for `--version`:
  * Debian / Ubuntu: `apt install libffms2-5`
  * Fedora: `dnf install ffms2`
  * Arch: `pacman -S ffms2`
* **glibc 2.39 or newer** — the floor of the executable itself. Older hosts
  (Ubuntu 20.04/22.04, Debian 11/12, Rocky 9, Alpine's musl) cannot run it.
* **an encoder and a concatenator** from the list below, plus `mkvmerge` or
  `ffmpeg`.
* **VapourSynth and its plugins** only for VapourSynth inputs, for `xpsnr`
  (which has no native implementation), and as the fallback when a metric's
  native library is missing or fails. That fallback also needs the FFmpegSource
  plugin (`vs-ffms2`); FFMS2 inputs scored by `libvmaf`, `libvship` or
  `libfmetrics` need none.

The executable and the bundled metric libraries share one glibc baseline (the
Ubuntu 24.04 runner's), so the metrics load wherever `condor` runs.

Run without a terminal — a CI job, or output redirected to a file — and `condor`
skips the TUI on its own, printing JSON progress events on stdout.

For the Rust library, [Andean Condor][andean-condor], add it as a dependency to your project `Cargo.toml` with `cargo add andean-condor`.

Av1an uses several external tools for decoding, filtering, and encoding video. For a quick start, install the following:

* [Python][python-download] - Recommended for [VapourSynth][vapoursynth-download]
* [vs-jetpack][vsjetpack] - Installs [VapourSynth][vapoursynth] and a convenient collection of VapourSynth plugins and Python modules for scaling, denoising, debanding, deinterlacing, metrics, etc.
* [Vship][vship] - GPU-accelerated metrics for [SSIMULACRA 2][ssimulacra2], [butteraugli][butteraugli], and [ColorVideoVDP][cvvdp]. These run through the native `libvship` library whenever it is installed and has a usable GPU, and fall back to the VapourSynth plugin otherwise. The Windows release already ships `libvship.dll`; elsewhere see [`av-metrics-vship`](av-metrics-vship/README.md) for how to install it
* [libvmaf][libvmaf] - Required only for the `vmaf` metric. The Windows release ships `libvmaf.dll` and its models; see the [`av-metrics-vmaf` README](av-metrics-vmaf/README.md) for other platforms
* [TransNetV2][transnetv2] - The neural network behind `detect-scenes --method transnetv2`. The release ships its ONNX model in `model/` beside the executable; for source builds, run [`andean-condor/scripts/install-transnetv2-model-windows.ps1`](andean-condor/scripts/install-transnetv2-model-windows.ps1) on Windows or [`andean-condor/scripts/install-transnetv2-model-linux.sh`](andean-condor/scripts/install-transnetv2-model-linux.sh) on Linux/macOS — either downloads the model into the current directory and prints the `TRANSNETV2_MODEL_PATH` to set
* At least one of the following encoder binaries: [aomenc][aom], [SvtAv1EncApp][svt-av1], [rav1e][rav1e], [avmenc][avm], [x264][x264], [x265][x265], [vvenc][vvenc], [FFmpeg][ffmpeg]
* Either [FFmpeg][ffmpeg] or [MKVToolNix][mkvtoolnix] for concatenating the encoded scenes

> [!TIP]
> Make sure binaries like [FFmpeg][ffmpeg], [mkvmerge][mkvtoolnix], or [SVT-AV1][svt-av1] are added to your PATH.

### Usage

#### California Condor CLI

The Av1an CLI, California Condor, uses a single JSON configuration file to manage the entire encoding process. Each step can also be executed individually with its own command. You can use `--help` on each command for details on how to use them (e.g. `condor --help`, `condor detect-scenes --help`, `condor target-quality --help`). For a complete guide on using California Condor, see the [California Condor README](./california-condor/README.md) and [guide](./california-condor/docs/guide.md). For a quick start, see the example below.

*Encode a 1080p 10-bit AV1 video with Film Grain Synthesis using rav1e. Use FFMS2 to decode the input. Downscale the scene detection input to 540p to detect scenes faster. Target a SSIMULACRA 2 quality score of 85 quickly. Use mkvmerge to concatenate the output video.*

```bash
$ condor \
    --temp "./deletemelater" \ # Directory containing temporary files, primarily encoded scenes
    --logs "./deletetemelater/condor.log" \ # Log file path
    --config-file "./deletemelater/config.json" \ # JSON configuration file path
    --input "bird takeoff.mp4" \ # Input video file path
    --output "bird takeoff.mkv" \ # Output video file path
    --decoder "vs-ffms2" \ # Uses FFMS2 instead of the default BestSource to decode in the input video
    --filters "resize:scaler=bilinear;width=1920;height=1080;format=yuv420p10le" \ # Uses VapourSynth Bilinear to resize the input video to 1920x1080 and convert to YUV 4:2:0 10-bit
    --scd-filters "resize:scaler=bilinear;width=540;height=960;" \ # Uses VapourSynth Bilinear to resize the input video to 540p for faster scene change detection
    --encoder "rav1e" \ # Uses rav1e instead of the default SVT-AV1 encoder
    --params "--speed 4" \ # rav1e encoder parameters used to encode each scene
    --photon-noise 800 \ # Applies a Photon Noise film grain table with ISO strength 800 
    --target-metric "ssimulacra2" \ # Uses SSIMULACRA 2 as the target quality metric
    --target 85 \ # Targets a SSIMULACRA 2 quality score of 85 for each scene
    --target-profile "fast" \ # Uses the fast Target Quality profile instead of the default standard profile
    --concat "mkvmerge" \ # Uses mkvmerge to concatenate the encoded scenes
```

#### Andean Condor Library

The Av1an library, Andean Condor, consists of the following components: `Input`, `Output`, `Encoder`, `Scene`, and `Sequence`. These components are managed with a single `Condor` instance. For a simple example using the library, see [here](./andean-condor/examples/simple.rs). For more information on using the library, see the [Andean Condor](./andean-condor/README.md#API) API documentation.

## Developing

See [Developing and Contributing](https://rust-av.github.io/Av1an/contributing) for a guide on developing Av1an and preparing for a Pull Request.

<!-- Links -->

[crates]: https://crates.io "The Rust community’s crate registry"
[aur]: https://aur.archlinux.org "archlinux user repository"
[andean-condor]: ./andean-condor/README.md "Andean Condor - The Av1an Rust library"

[ffms2]: https://github.com/ffms/ffms2 "FFmpegSource"

[aom]: https://aomedia.googlesource.com/aom "Alliance for Open Media AV1"
[avm]: https://github.com/AOMediaCodec/avm "Alliance for Open Media AOM Video Model"
[svt-av1]: https://gitlab.com/AOMediaCodec/SVT-AV1 "Scalabe Video Technology for AV1"
[rav1e]: https://github.com/xiph/rav1e "Rust AV1 Encoder"
[vpx]: https://chromium.googlesource.com/webm/libvpx "WebM VP8/VP9"
[x264]: https://www.videolan.org/developers/x264.html "x264"
[x265]: https://www.videolan.org/developers/x265.html "x265"
[vvenc]: https://github.com/fraunhoferhhi/vvenc "Fraunhofer Versatile Video Encoder"
[ffmpeg]: https://ffmpeg.org "FFmpeg"

[ssimulacra2]: https://github.com/cloudinary/ssimulacra2 "SSIMULACRA 2 - Structural SIMilarity Unveiling Local And Compression Related Artifacts"
[butteraugli]: https://github.com/google/butteraugli "butteraugli - A tool for measuring perceived differences between images"
[cvvdp]: https://github.com/gfxdisp/colorvideovdp "ColorVideoVDP: A visible difference predictor for color images and videos"

[python-download]: https://www.python.org/downloads "Python"
[vsjetpack]: https://github.com/Jaded-Encoding-Thaumaturgy/vs-jetpack "vs-jetpack"

[vapoursynth]: https://www.vapoursynth.com "VapourSynth - A video processing framework with simplicity in mind"
[vapoursynth-download]: https://www.vapoursynth.com/doc/installation.html "Installing and Compiling"
[vs-trim]: https://www.vapoursynth.com/doc/functions/video/trim.html "Trim"
[vs-crop]: https://www.vapoursynth.com/doc/functions/video/crop_cropabs.html "Crop/CropAbs"
[vs-resize]: https://www.vapoursynth.com/doc/functions/video/resize.html "Resize"
[vs-splice]: https://www.vapoursynth.com/doc/functions/video/splice.html "Splice"

[vs-bestsource]: https://github.com/vapoursynth/bestsource "BestSource"
[vs-lsmash]: https://github.com/HomeOfAviSynthPlusEvolution/L-SMASH-Works "L-SMASH-Works"
[vs-dgdecnv]: https://www.rationalqm.us/dgdecnv/dgdecnv.html "DGDecNV - AVC/HEVC/MPG/VC1 Decoder and Frame Server"

[vszip]: https://github.com/dnjulek/vapoursynth-zip "VapourSynth Zig Image Process"
[vship]: https://codeberg.org/Line-fr/Vship "Vship : Fast Metric Computation on GPU"
[libvmaf]: https://github.com/Netflix/vmaf "libvmaf - VMAF (Video Multi-Method Assessment Fusion)"
[transnetv2]: https://huggingface.co/elya5/transnetv2 "TransNetV2 ONNX export"

[mkvtoolnix]: https://mkvtoolnix.download "MKVToolNix"
