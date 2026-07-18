use ironmark::{render_ansi_terminal, ParseOptions};

pub fn process_command_tree(cmd: clap::Command) -> clap::Command {
    cmd.mut_args(|mut arg| {
        if let Some(long_help) = arg.get_long_help() {
            let pure_ansi =
                render_ansi_terminal(&long_help.to_string(), &ParseOptions::default(), None);
            arg = arg.long_help(pure_ansi);
        }
        if let Some(short_help) = arg.get_help() {
            let pure_ansi =
                render_ansi_terminal(&short_help.to_string(), &ParseOptions::default(), None);
            arg = arg.help(pure_ansi);
        }

        arg
    })
    .mut_subcommands(process_command_tree)
}

/// Extracts the first line of long help text at compile time.
const fn short_help(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            let (first_line, _) = bytes.split_at(i);
            // SAFETY: `\n` is ASCII, so splitting at it is always a valid UTF-8 boundary.
            return unsafe { core::str::from_utf8_unchecked(first_line) };
        }
        i += 1;
    }
    s
}

pub const HELP_CONFIG: &str = r#"Path to the configuration file.

Defaults to `./condor.json` in the current directory.
"#;
pub const HELP_CONFIG_SHORT: &str = short_help(HELP_CONFIG);

pub const HELP_LOGS: &str = r#"Path to the log file.

Defaults to `./logs/condor.log` in the current directory.
"#;
pub const HELP_LOGS_SHORT: &str = short_help(HELP_LOGS);

/// Input template
macro_rules! input_template {
    ($text:literal) => {
        concat!(
            r#"Path to the input file "#,
            $text,
            r#".

Can be a video or VapourSynth script (`.py` or `.vpy`).

Examples:
- `./video.mp4`
- `/Videos/secret_bird_folder/takeoff.mkv`
- `./script.vpy`
- `/vapoursynth_scripts/downscale_template.py`
"#
        )
    };
}

pub const HELP_INPUT: &str = input_template!("to encode");
pub const HELP_INPUT_SHORT: &str = short_help(HELP_INPUT);

pub const HELP_SCD_INPUT: &str = input_template!("to detect scenes");
pub const HELP_SCD_INPUT_SHORT: &str = short_help(HELP_SCD_INPUT);

pub const HELP_ND_INPUT: &str = input_template!("to detect noise");
pub const HELP_ND_INPUT_SHORT: &str = short_help(HELP_ND_INPUT);

pub const HELP_TQ_INPUT: &str = input_template!("to encode and measure quality in Target Quality");
pub const HELP_TQ_INPUT_SHORT: &str = short_help(HELP_TQ_INPUT);

pub const HELP_OUTPUT: &str = r#"Path to the output video file.

File extension must be supported by the specified Concatenation method (*--concat*).

Examples:
- `./output.mp4`
- `/Videos/bird_folder/takeoff.mkv`
- `/mnt/encoded_clips/flock.ivf`
"#;
pub const HELP_OUTPUT_SHORT: &str = short_help(HELP_OUTPUT);

pub const HELP_TEMP: &str = r#"Path to the temporary directory to use.

If not specified, the temporary directory is created in the current working directory with a hash of the input file name as the directory name.

Examples:
- `./temp`
- `/mnt/working_bird_folder`
- `./2fe7da9`
"#;
pub const HELP_TEMP_SHORT: &str = short_help(HELP_TEMP);

/// Decoder Template
macro_rules! decoder_template {
    ($name:expr) => {
        concat!(
            r#"Method used for decoding the "#,
            $name,
            r#" video.

Methods besides **ffms2** require external VapourSynth plugins to be installed.

Defaults to **bestsource**.
"#
        )
    };
}

pub const HELP_DECODER: &str = decoder_template!("input");
pub const HELP_DECODER_SHORT: &str = short_help(HELP_DECODER);

pub const HELP_SCD_DECODER: &str = decoder_template!("Scene Detector input");
pub const HELP_SCD_DECODER_SHORT: &str = short_help(HELP_SCD_DECODER);

pub const HELP_TQ_DECODER: &str = decoder_template!("Target Quality input");
pub const HELP_TQ_DECODER_SHORT: &str = short_help(HELP_TQ_DECODER);

/// VapourSynth arguments template
macro_rules! vs_args_template {
    ($name:literal) => {
        concat!(
            r#"VapourSynth/Python arguments to pass to the "#,
            $name,
            r#" script environment.

Examples:
- `--vs-args "message=fluffy kittens" --vs-args "head=empty"`
- `--vs-args "denoiser=primary" --vs-args "downscaler=bicubic"`
"#
        )
    };
}

pub const HELP_VS_ARGS: &str = vs_args_template!("input");
pub const HELP_VS_ARGS_SHORT: &str = short_help(HELP_VS_ARGS);

pub const HELP_SCD_VS_ARGS: &str = vs_args_template!("Scene Detector input");
pub const HELP_SCD_VS_ARGS_SHORT: &str = short_help(HELP_SCD_VS_ARGS);

pub const HELP_TQ_VS_ARGS: &str = vs_args_template!("Target Quality input");
pub const HELP_TQ_VS_ARGS_SHORT: &str = short_help(HELP_TQ_VS_ARGS);

pub const HELP_SCD_METHOD: &str = r#"Method used for detecting scenes.

Defaults to **standard**.
"#;
pub const HELP_SCD_METHOD_SHORT: &str = short_help(HELP_SCD_METHOD);

pub const HELP_MIN_SCENE_SECONDS: &str = r#"Minimum scene duration in seconds.

Defaults to `1`.
"#;
pub const HELP_MIN_SCENE_SECONDS_SHORT: &str = short_help(HELP_MIN_SCENE_SECONDS);

pub const HELP_MAX_SCENE_SECONDS: &str = r#"Maximum scene duration in seconds.

Defaults to `10`.
"#;
pub const HELP_MAX_SCENE_SECONDS_SHORT: &str = short_help(HELP_MAX_SCENE_SECONDS);

pub const HELP_ENCODER: &str = r#"Video encoder to use.

Defaults to **svt-av1**.
"#;
pub const HELP_ENCODER_SHORT: &str = short_help(HELP_ENCODER);

pub const HELP_PASSES: &str = r#"Number of encoder passes.

Defaults to `2` for **aom** and **vpx** encoders, otherwise `1`.

Since **aom** and **vpx** benefit from two-pass mode even with constant quality mode (unlike other encoders in which two-pass mode is used for more accurate VBR rate control), two-pass mode is used by default for these encoders.
"#;
pub const HELP_PASSES_SHORT: &str = short_help(HELP_PASSES);

/// Encoder parameters examples
macro_rules! params_examples {
    ($param_name:literal) => {
        concat!(
            "\n",
            "- `",
            $param_name,
            r#" "--preset 2 --crf 24 --aq-mode 0"`
"#,
            "- `",
            $param_name,
            r#" "--cpu-used=3 --cq-level=30 --tune=ssim"`
"#,
            "- `",
            $param_name,
            r#" "--crf 18 --preset slow --tune film"`
"#,
        )
    };
}

pub const HELP_PARAMS: &str = concat!(
    r#"Parameters for the video encoder (*--encoder*).

These parameters are passed directly to the encoder binary. These parameters will be merged with Condor's default set of encoder parameters.

Examples:"#,
    params_examples!("--params")
);
pub const HELP_PARAMS_SHORT: &str = short_help(HELP_PARAMS);

pub const HELP_TQ_PARAMS: &str = concat!(
    r#"Parameters for the video encoder (*--encoder*) used in Target Quality.

These parameters are passed directly to the encoder binary. These parameters will be merged with Condor's default set of encoder parameters and psychovisual parameters will be omitted.

If not specified, the encoder parameters (*--params*) will be used.

Examples:"#,
    params_examples!("--tq-params")
);
pub const HELP_TQ_PARAMS_SHORT: &str = short_help(HELP_TQ_PARAMS);

pub const HELP_PHOTON_NOISE: &str = r#"Generate and apply a photon noise table using Film Grain Synthesis with the specified ISO strength.

Do not specify this option when using the internal encoder Film Grain Synthesis (e.g. `--film-grain` in **svt-av1**). Only compatible with **aom**, **svt-av1**,  **rav1e**, and **avm** encoders (*--encoder*).

A minimum noise ISO strength of `200` is recommended for reducing gradient banding.
"#;
pub const HELP_PHOTON_NOISE_SHORT: &str = short_help(HELP_PHOTON_NOISE);

pub const HELP_CHROMA_NOISE: &str = r#"Apply chroma noise of the specified ISO strength to the photon noise table using Film Grain Synthesis.

Do not specify this option when using the internal encoder Film Grain Synthesis (e.g. `--film-grain` in **svt-av1**). Only compatible with **aom**, **svt-av1**,  **rav1e**, and **avm** encoders (*--encoder*).
"#;
pub const HELP_CHROMA_NOISE_SHORT: &str = short_help(HELP_CHROMA_NOISE);

/// Example VapourSynth filters
macro_rules! example_filters {
    () => {
        r#"
- `resize:scaler=bilinear;width=1280;height=720;format=yuv420p;` -  Uses Bilinear scaler to resize input to 1280x720 in YUV420P (YUV 4:2:0 8-bit) format.
- `crop:top=140;bottom=140;` - Crop the top and bottom 140 pixels out of the input.
- `trim:start=24;end=240;` - Remove the first second and last 10 seconds from the 24FPS input.
- `rescale:kernel=Mitchell;width=720;height=1280;doubler=ArtCNN;` - Rescale 720p native input with Mitchell and upscale with ArtCNN.
"#
    };
}

/// Available VapourSynth filters
macro_rules! available_filters {
    () => {
        r#"
- `resize:scaler?;width?;height?;format?;`: Resize input to the specified resolution, format, and/or scaler.
- `crop:top?;bottom?;left?;right?;`: Crop input to the specified region in pixels.
- `trim:start?;end?;`: Trim input to the specified start and end frames.
- `rescale:kernel;width;height;doubler;`: Rescale input with the specified VSJET kernel, width, height, and doubler. Requires vs-jetpack and vodesfunc to be installed.
- `wnnm:sigma?;block_size?;block_step?;group_size?;bm_range?;radius?;ps_num?;ps_range?;residual?;adaptive_aggregation?;`: Apply WNNM denoising to the input. Requires vszip to be installed.
- `bilateral:sigma_s?;sigma_r?;planes?;algorithm?;pbficnum?;`: Apply bilateral filtering to the input. Requires vszip to be installed.
"#
    };
}

pub const HELP_FILTERS: &str = concat!(
    r#"VapourSynth filters to apply to the input.

Defaults to `resize:scaler=bicubic;format=yuv420p10le` (YUV 4:2:0 10-bit).

Available filters:"#,
    available_filters!(),
    r#"Example filters:"#,
    example_filters!(),
);
pub const HELP_FILTERS_SHORT: &str = short_help(HELP_FILTERS);

pub const HELP_SCD_FILTERS: &str = concat!(
    r#"VapourSynth filters to apply to the Scene Detector input.

Available filters:"#,
    available_filters!(),
    r#"Example filters:
"#,
    example_filters!(),
);
pub const HELP_SCD_FILTERS_SHORT: &str = short_help(HELP_SCD_FILTERS);

pub const HELP_REFERENCE_FILTERS: &str = concat!(
    r#"VapourSynth filters to apply to the Reference VideoNode.

Defaults to `wnnm:sigma=3.0,0.0,0.0;`

Available filters:"#,
    available_filters!(),
    r#"Example filters:
"#,
    example_filters!(),
);
pub const HELP_REFERENCE_FILTERS_SHORT: &str = short_help(HELP_REFERENCE_FILTERS);

pub const HELP_DENOISED_FILTERS: &str = concat!(
    r#"VapourSynth filters to apply to the Denoised VideoNode.

Defaults to `wnnm:sigma=6.0,0.0,0.0;`

Available filters:"#,
    available_filters!(),
    r#"Example filters:
"#,
    example_filters!(),
);
pub const HELP_DENOISED_FILTERS_SHORT: &str = short_help(HELP_DENOISED_FILTERS);

pub const HELP_TQ_FILTERS: &str = concat!(
    r#"VapourSynth filters to apply to the Target Quality input.

Available filters:"#,
    available_filters!(),
    r#"Example filters:
"#,
    example_filters!(),
);
pub const HELP_TQ_FILTERS_SHORT: &str = short_help(HELP_TQ_FILTERS);

pub const HELP_CONCAT: &str = r#"Method used for concatenating the encoded chunks into the output file.

Defaults to **mkvmerge**.
"#;
pub const HELP_CONCAT_SHORT: &str = short_help(HELP_CONCAT);

pub const HELP_WORKERS: &str = r#"The amount of encoder processes to use at once.

If not specified, Benchmarker is used to determine the optimal number of workers.
"#;
pub const HELP_WORKERS_SHORT: &str = short_help(HELP_WORKERS);

pub const HELP_TARGET_METRIC: &str = r#"The quality metric used for Target Quality.

Defaults to **ssimulacra2**.
"#;
pub const HELP_TARGET_METRIC_SHORT: &str = short_help(HELP_TARGET_METRIC);

pub const HELP_TARGET: &str = r#"The quality metric score that Target Quality will aim for.
"#;
pub const HELP_TARGET_SHORT: &str = short_help(HELP_TARGET);

pub const HELP_MINIMUM_QUANTIZER: &str = r#"The lowest quantizer for Target Quality to try when searching for the optimal quantizer.

Default depends on the specified encoder (*--encoder*).
"#;
pub const HELP_MINIMUM_QUANTIZER_SHORT: &str = short_help(HELP_MINIMUM_QUANTIZER);

pub const HELP_MAXIMUM_QUANTIZER: &str = r#"The highest quantizer for Target Quality to try when searching for the optimal quantizer.

Default depends on the specified encoder (*--encoder*).
"#;
pub const HELP_MAXIMUM_QUANTIZER_SHORT: &str = short_help(HELP_MAXIMUM_QUANTIZER);

pub const HELP_TARGET_QUALITY_PROFILE: &str = r#"The preset profile to choose the Target Quality Probe Strategy and Statistic.

Defaults to **standard**.
"#;
pub const HELP_TARGET_QUALITY_PROFILE_SHORT: &str = short_help(HELP_TARGET_QUALITY_PROFILE);
