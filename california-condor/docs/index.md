# Av1an Documentation

Av1an is a video encoding library and command-line tool built for speed, ease of use, and extensibility.
It splits a video into scenes, encodes them in parallel, and stitches the result back together.
Optionally target a level of quality, analyze video quality, and get feedback through a terminal UI.

The command-line tool is **California Condor** (`condor`) which is built on top of the Rust library, **Andean Condor**.

## Where to start

| Section | What it covers |
| ------- | -------------- |
| [Guide](guide.md) | The end-to-end workflow and how the pieces fit together |
| [Commands](commands/condor.md) | Every `condor` subcommand and its flags |
| [Configuration](configuration.md) | The `condor.json` file: global flags and validation |
| [Configuration Reference](configuration/index.md) | The full `condor.json` schema, field by field |
| [Types](types.md) | Decoders, encoders, filters, and other complex types |
| [Encoders](encoders/aomenc.md) | Per-encoder tuning notes |

## Install

Install from a package manager, crates.io, or Docker, or grab a release binary:

```bash
pacman -S condor        # Arch Linux & Manjaro
cargo install condor    # crates.io
docker pull boatsmcgee/condor:latest
```

See [Compiling](compiling.md) to build from source and [Docker](docker.md) for the
container image.

## Library

Use Av1an as a Rust library by adding Andean Condor to your project:

```bash
cargo add andean-condor
```
