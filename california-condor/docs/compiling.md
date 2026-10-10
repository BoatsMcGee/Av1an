# Compiling Av1an

You can natively build Av1an on Linux and Windows. Cross-compilation is not supported.

## Compiling on Linux

To compile Av1an from source, you need the following dependencies:

- [Rust](https://www.rust-lang.org) (version 1.70.0 or higher)
- [NASM](https://www.nasm.us)
- [clang/LLVM](https://llvm.org)
- [FFmpeg](https://ffmpeg.org)
- [VapourSynth](https://www.vapoursynth.com)

On Arch Linux, you can install these dependencies by running

```sh
pacman -S --needed rust nasm clang ffmpeg vapoursynth
```

Installation instructions on other distros will vary.

After installing the dependencies, you need to clone the repository and start the build process:

```sh
git clone https://github.com/master-of-zen/Av1an && cd Av1an
cargo build --release
```

The resulting binary will be the file `./target/release/av1an`.

## Compiling on Windows

If you just want a current build of Av1an that is newer than the last official release, you can find a pre-built binary of the current `master` branch at https://github.com/master-of-zen/Av1an/releases/tag/latest.

If you want to build the binary yourself, you will need the following dependencies:

- [Microsoft Visual C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools) - this is a dependency for Rust
- [The Rust toolchain](https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe)
- [VapourSynth](https://github.com/vapoursynth/vapoursynth/releases/latest) (download the portable version; the installed version could also work)
- [NASM](https://nasm.us)
- [FFmpeg](https://github.com/GyanD/codexffmpeg/releases/download/7.0.2/ffmpeg-7.0.2-full_build-shared.7z) (thanks to [gyan](https://github.com/GyanD) for providing these builds)
- [LLVM](https://releases.llvm.org)

### FFmpeg setup:
- Extract the file `ffmpeg-7.0.2-full_build-shared.7z` to a directory.
- Create a new environment variable named `FFMPEG_DIR` and set it to the directory path where you extracted `ffmpeg-7.0.2-full_build-shared.7z`.
  - (For example, set `FFMPEG_DIR` to `C:\Users\Username\Downloads\ffmpeg-7.0.2-full_build-shared`)
	
### VapourSynth setup:
- Extract the contents of the portable VapourSynth zip file to a directory.
- Create a new environment variable named `VAPOURSYNTH_LIB_DIR` and set it to the directory path where you extracted the file, appending `\sdk\lib64` to the path.
  - (For example, set `VAPOURSYNTH_LIB_DIR` to `C:\Users\Username\Downloads\VapourSynth64-Portable-R70\sdk\lib64`)

Then, either clone the repository by running

```sh
git clone https://github.com/master-of-zen/Av1an
```

Or download and extract the [source code](https://github.com/master-of-zen/Av1an/archive/refs/heads/master.zip) manually.

Open a command prompt or PowerShell window inside the cloned repository/extracted ZIP folder and run the command `cargo build --release`. If this command executes successfully with no errors, `av1an.exe` will be in the folder `target\release`.

To use `av1an.exe`, copy all the `.dll` files from `ffmpeg-7.0.2-full_build-shared\bin` to the same directory as `av1an.exe`, and ensure that `ffmpeg.exe` is in a folder accessible via the `PATH` environment variable.

## Documentation screenshots

The [Screenshots](screenshots.md) page is generated from the live interface,
not taken by hand: every image is rendered offscreen through the same
`TuiApp::render` code the terminal uses, so it cannot drift from the real
interface. To regenerate them after changing the TUI:

```shcargo run --release --features screenshots --bin gen_screenshots
```

This writes light and dark 1920x1080 AVIFs into
`california-condor/docs/media/tui` (gitignored). The `screenshots` feature keeps
the image/font/AVIF dependencies out of the shipping binary. Pick a specific
monospace face with `CONDOR_SCREENSHOT_FONT=/path/to/font.ttf` for byte-stable
output across machines.

The same render paths are exercised as an integration test — it renders every
screen in both themes in memory and asserts the dimensions, catching panics and
layout regressions in CI:

```shcargo test --features screenshots --test screenshots
```

GitHub Actions regenerates the media on every deploy of the documentation site
(see `.github/workflows/mdbook.yml`), so the published page always matches the
interface on `master`.
