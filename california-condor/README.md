# California Condor

![Demonstration of the California Condor TUI](./media/demo.avif)

[![Discord server](https://discordapp.com/api/guilds/696849974230515794/embed.png)](https://discord.gg/Ar8MvJh)
[![CI tests](https://github.com/rust-av/Av1an/actions/workflows/tests.yml/badge.svg)](https://github.com/rust-av/Av1an/actions/workflows/tests.yml)
[![](https://img.shields.io/crates/v/av1an.svg)](https://crates.io/crates/av1an)

<a href="https://www.buymeacoffee.com/master_of_zen" target="_blank"><img src="https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png" alt="Buy Me A Coffee" style="height: 60px !important;width: 217px !important;" ></a>

California Condor is the command-line interface and Terminal User Interface (TUI) for Av1an. The `condor` binary manages all processes including encoding with a single JSON configuration file. It wraps Av1an's library, Andean Condor.

## Features

* Cross-platform CLI and TUI for Windows, Linux, and MacOS
* Chunked parallel encoding pipeline: scene detection, noise detection, benchmarking, target quality, encoding, concatenation, quality check
* Single JSON configuration file with schema validation
* Cancel and resume at any time with progress saved in the JSON configuration file
* Real-time TUI progress reporting for long-running sequences
* Real-time progress reporting to STDOUT when piping (external script support)
* Per-step commands for individual execution
* VapourSynth filtering with per-stage overrides for encode, scene detection, and target quality inputs
* Supported decoders: `bestsource`, `vs-ffms2`, `lsmash`, `dgdecnv`, `ffms2`
* Supported encoders: `aom`, `rav1e`, `vpx`, `svt-av1`, `avm`, `x264`, `x265`, `vvenc`, `ffmpeg`
* Supported concatenation methods: `mkvmerge`, `ffmpeg`, `ivf`
* Supported quality metrics: `ssimulacra2`, `butteraugli`, `butteraugli-3`, `xpsnr`, `cvvdp`

> [!NOTE]
> Per-command captures (`media/detect-scenes.avif`, `media/detect-noise.avif`, `media/benchmark.avif`, `media/target-quality.avif`, `media/encode.avif`, `media/concatenate.avif`, `media/quality-check.avif`) will replace the placeholders above when available. Non-TUI commands (`init`, `scale-noise`, `optimize-bitrate`, `scale-speed`, `clean`) have no gallery entry.

## Getting Started

For a complete walkthrough, see the [guide](./docs/guide.md). It covers the end-to-end workflow, the JSON configuration file, temporary and log file layout, and when each command launches a TUI.

### Installation

Install `condor` from the [Arch AUR][aur], [crates.io][crates], or [Docker](#docker). A Windows binary is available from [Releases](https://github.com/rust-av/Av1an/releases).

```bash
$ pacman -S condor # Arch Linux & Manjaro
$ cargo install condor # crates.io
$ docker pull boatsmcgee/condor:latest # Docker Hub
```

External tools required for decoding, filtering, encoding, metrics, and concatenation:

* [Python][python-download] - Recommended for [VapourSynth][vapoursynth-download]
* [vs-jetpack][vsjetpack] - Installs [VapourSynth][vapoursynth] and plugins for scaling, denoising, debanding, deinterlacing, etc.
* [Vship][vship] - GPU-accelerated metrics for [SSIMULACRA 2][ssimulacra2], [butteraugli][butteraugli], and [ColorVideoVDP][cvvdp]
* At least one encoder: [aomenc][aom], [SvtAv1EncApp][svt-av1], [rav1e][rav1e], [avmenc][avm], [vpxenc][vpx], [x264][x264], [x265][x265], [vvenc][vvenc], [FFmpeg][ffmpeg]
* Either [FFmpeg][ffmpeg] or [MKVToolNix][mkvtoolnix] for concatenating encoded scenes

> [!TIP]
> Ensure binaries such as [FFmpeg][ffmpeg], [mkvmerge][mkvtoolnix], or [SVT-AV1][svt-av1] are on `PATH`.

### Usage

Run the full pipeline with no subcommand, or run steps individually. Use `--help` on each command for details (e.g. `condor --help`, `condor detect-scenes --help`, `condor target-quality --help`, `condor encode --help`).

```bash
$ condor init "input.mp4" "output.mkv"
$ condor detect-scenes
$ condor encode
$ condor concatenate
```

*Encode a 1080p 10-bit AV1 video with Film Grain Synthesis using rav1e. Use FFMS2 to decode. Downscale scene detection input to 540p for faster detection. Target SSIMULACRA 2 score 85 with the fast profile. Concatenate with mkvmerge.*

```bash
$ condor \
    --temp "./deletemelater" \
    --logs "./deletemelater/condor.log" \
    --config-file "./deletemelater/config.json" \
    --input "bird takeoff.mp4" \
    --output "bird takeoff.mkv" \
    --decoder "vs-ffms2" \
    --filters "resize:scaler=bilinear;width=1920;height=1080;format=yuv420p10le" \
    --scd-filters "resize:scaler=bilinear;width=540;height=960;" \
    --encoder "rav1e" \
    --params "--speed 4" \
    --photon-noise 800 \
    --target-metric "ssimulacra2" \
    --target 85 \
    --target-profile "fast" \
    --concat "mkvmerge"
```

See [condor](./docs/commands/condor.md) for all full-run flags, [configuration](./docs/configuration.md) for global flags and `condor.json` validation, [types](./docs/types.md) for Decoder, Encoder, Filters, and other complex types, and the [guide](./docs/guide.md) for step ordering.

## Gallery

![[condor](./media/condor.avif)](./media/condor-small.avif) | ![detect-scenes](./media/detect-scenes-small.avif) |
:-: | :-: |
`condor` | `detect-scenes` |
![detect-noise](./media/detect-noise-small.avif) | ![benchmark](./media/benchmark-small.avif) |
`detect-noise` | `benchmark` |
![target-quality](./media/target-quality-small.avif) | ![encode](./media/encode-small.avif) |
`target-quality` | `encode` |
![concatenate](./media/concatenate-small.avif) | ![quality-check](./media/quality-check-small.avif) |
`concatenate` | `quality-check` |

<!-- Links -->

[crates]: https://crates.io "The Rust community's crate registry"
[aur]: https://aur.archlinux.org "archlinux user repository"

[aom]: https://aomedia.googlesource.com/aom "Alliance for Open Media AV1"
[avm]: https://github.com/AOMediaCodec/avm "Alliance for Open Media AOM Video Model"
[svt-av1]: https://gitlab.com/AOMediaCodec/SVT-AV1 "Scalable Video Technology for AV1"
[rav1e]: https://github.com/xiph/rav1e "Rust AV1 Encoder"
[vpx]: https://chromium.googlesource.com/webm/libvpx "WebM VP8/VP9"
[x264]: https://www.videolan.org/developers/x264.html "x264"
[x265]: https://www.videolan.org/developers/x265.html "x265"
[vvenc]: https://github.com/fraunhoferhhi/vvenc "Fraunhofer Versatile Video Encoder"
[ffmpeg]: https://ffmpeg.org "FFmpeg"

[python-download]: https://www.python.org/downloads "Python"
[vsjetpack]: https://github.com/Jaded-Encoding-Thaumaturgy/vs-jetpack "vs-jetpack"

[ssimulacra2]: https://github.com/cloudinary/ssimulacra2 "SSIMULACRA 2 - Structural SIMilarity Unveiling Local And Compression Related Artifacts"
[butteraugli]: https://github.com/google/butteraugli "butteraugli - A tool for measuring perceived differences between images"
[cvvdp]: https://github.com/gfxdisp/colorvideovdp "ColorVideoVDP: A visible difference predictor for color images and videos"

[vapoursynth]: https://www.vapoursynth.com "VapourSynth - A video processing framework with simplicity in mind"
[vapoursynth-download]: https://www.vapoursynth.com/doc/installation.html "Installing and Compiling"

[vszip]: https://github.com/dnjulek/vapoursynth-zip "VapourSynth Zig Image Process"
[vship]: https://codeberg.org/Line-fr/Vship "Vship : Fast Metric Computation on GPU"

[mkvtoolnix]: https://mkvtoolnix.download "MKVToolNix"
