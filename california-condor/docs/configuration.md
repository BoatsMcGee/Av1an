# Configuration

Global flags and `condor.json` handling. See [guide](./guide.md) for workflow, [condor](./commands/condor.md) for full-run flags, [type reference](./types.md) for complex types, and [Configuration](./configuration/index.md) for the `condor.json` schema.

For the full `condor.json` struct reference, see [Configuration](./configuration/index.md).

## Global Flags

| Name                                              | Flag            | Type   | Default              |
| ------------------------------------------------- | --------------- | ------ | -------------------- |
| [Config File](#config-file---config-file)         | `--config-file` | Path   | `./condor.json`      |
| [Temporary](#temporary---temp)                    | `--temp`        | Path   | Input file name hash |
| [Log File](#log-file---logs)                      | `--logs`        | Path   | `./logs/condor.log`  |
| [Verbose](#verbose---verbose)                     | `--verbose`     |        |                      |
| [Version](#version--v---version)                   | `-v`, `--version` |      |                      |

## Config File `--config-file`

Path to the configuration file.

### Default

If not specified, `./condor.json` in the current directory is used.

### Examples

- `> condor --config-file ./deletemelater/config.json -i input.mp4 -o output.mkv`
- `> condor init input.mp4 output.mkv --config-file ./config.json` (via `--config-file` global on `init`)

## Temporary `--temp`

Path to the temporary directory for scenes and intermediate encodes.

### Default

If not specified, a directory named with a hash of the input file name is created in the current working directory.

### Examples

- `> condor --temp ./temp -i input.mp4 -o output.mkv`
- `> condor --temp /mnt/working_bird_folder -i input.mp4 -o output.mkv`

## Log File `--logs`

Path to the log file.

### Default

If not specified, logs to `./logs/condor.log`.

### Examples

- `> condor --logs ./deletemelater/condor.log -i input.mp4 -o output.mkv`

## Verbose `--verbose`

Enable verbose output and logging.

## Version `-v`, `--version`

Display encoder and VapourSynth installation information. With `--verbose`, prints extended details.

## condor.json

Created by [init](./commands/init.md). Contains `input`, `output`, `temp`, `input_filters`, `scd_input_filters`, `tq_input_filters`, and `condor` sequence state. Each step loads, updates, and saves it, enabling cancel and resume.

The file includes a `$schema` URL (baked via `CONDOR_SCHEMA_URL` in `build.rs`, fallback to release `configuration.schema.json`).

### External Validation

You can validate `condor.json` externally, such as in an IDE or editor with JSON Schema support, using the `$schema` URL in the file. Example: open `condor.json` in VS Code with JSON language mode; the `$schema` property enables completion and validation automatically. Schema can be regenerated with `cargo run -p california-condor --bin generate-schema`.
