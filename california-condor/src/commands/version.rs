use andean_condor::{
    models::encoder::{Encoder, EncoderBase},
    vapoursynth::{
        get_core,
        get_environment,
        plugins::{
            PluginFunction,
            bestsource::VideoSource,
            dgdecodenv::DGSource,
            ffms2::Source,
            lsmash::LWLibavSource,
            mvutensils::degrain::Degrain as MVUDegrain,
            vship::cvvdp::CVVDP,
            vszip::xpsnr::XPSNR,
            zoomvtools::degrain::Degrain as ZooMVDegrain,
        },
    },
};
use anyhow::Result;
use ironmark::{ParseOptions, render_ansi_terminal};

#[tracing::instrument(skip_all)]
pub fn print_version(verbose: bool) -> Result<()> {
    let version_info = match (
        option_env!("VERGEN_GIT_SHA"),
        option_env!("VERGEN_CARGO_DEBUG"),
        option_env!("VERGEN_RUSTC_SEMVER"),
        option_env!("VERGEN_RUSTC_LLVM_VERSION"),
        option_env!("VERGEN_CARGO_TARGET_TRIPLE"),
        option_env!("VERGEN_GIT_COMMIT_DATE"),
    ) {
        (
            Some(git_hash),
            Some(cargo_debug),
            Some(rustc_ver),
            Some(llvm_ver),
            Some(target_triple),
            Some(commit_date),
        ) => {
            format!(
                "{}-unstable (rev {}) ({})

* Compiler
rustc {} (LLVM {})

* Target Triple
{}

* Date Info
Commit Date:  {}",
                env!("CARGO_PKG_VERSION"),
                git_hash,
                if cargo_debug.parse::<bool>().unwrap() {
                    "Debug"
                } else {
                    "Release"
                },
                rustc_ver,
                llvm_ver,
                target_triple,
                commit_date,
            )
        },
        _ => env!("CARGO_PKG_VERSION").to_owned(),
    };

    println!("{}", version_info);

    let env = get_environment()?;
    let core = get_core(&env)?;
    let plugin_infos = vec![
        VideoSource::info(core)?,
        Source::info(core)?,
        DGSource::info(core)?,
        LWLibavSource::info(core)?,
        XPSNR::info(core)?,
        CVVDP::info(core)?,
        MVUDegrain::info(core)?,
        ZooMVDegrain::info(core)?,
    ];

    let max_width = plugin_infos
        .iter()
        .map(|plugin_info| format!("🐦 {}({})", plugin_info.name, plugin_info.id).len())
        .max()
        .unwrap_or(0)
        + 1;

    println!("\nVapourSynth Plugins Installed\n{}", "-".repeat(max_width));
    for plugin_info in plugin_infos {
        println!(
            "{} {}",
            if plugin_info.installed {
                "\x1b[0;32m✓\x1b[0m"
            } else {
                "\x1b[0;31m✗\x1b[0m"
            },
            render_ansi_terminal(
                if verbose && let Some(docs) = plugin_info.docs {
                    format!("[**{}** ({})]({})", plugin_info.name, plugin_info.id, docs)
                } else {
                    format!("**{}** ({})", plugin_info.name, plugin_info.id)
                }
                .as_str(),
                &ParseOptions::default(),
                None
            )
            .trim_end()
        );
    }

    let encoders = vec![
        (
            EncoderBase::AOM,
            Encoder::default_from_base(&EncoderBase::AOM, false),
        ),
        (
            EncoderBase::AVM,
            Encoder::default_from_base(&EncoderBase::AVM, false),
        ),
        (
            EncoderBase::SVTAV1,
            Encoder::default_from_base(&EncoderBase::SVTAV1, false),
        ),
        (
            EncoderBase::RAV1E,
            Encoder::default_from_base(&EncoderBase::RAV1E, false),
        ),
        (
            EncoderBase::VPX,
            Encoder::default_from_base(&EncoderBase::VPX, false),
        ),
        (
            EncoderBase::X264,
            Encoder::default_from_base(&EncoderBase::X264, false),
        ),
        (
            EncoderBase::X265,
            Encoder::default_from_base(&EncoderBase::X265, false),
        ),
        (
            EncoderBase::VVenC,
            Encoder::default_from_base(&EncoderBase::VVenC, false),
        ),
        (
            EncoderBase::FFmpeg,
            Encoder::default_from_base(&EncoderBase::FFmpeg, false),
        ),
    ];

    println!("\nEncoders Installed\n{}", "-".repeat(max_width));
    for (base, encoder) in encoders {
        let installed = encoder.validate().is_ok();
        println!(
            "{} {} ({}){}",
            if installed {
                "\x1b[0;32m✓\x1b[0m"
            } else {
                "\x1b[0;31m✗\x1b[0m"
            },
            render_ansi_terminal(
                &format!("**{}**", base.friendly_name()),
                &ParseOptions::default(),
                None
            )
            .trim_end(),
            encoder.executable(),
            if verbose && let Some(version) = encoder.version_text() {
                format!(
                    ": {}",
                    render_ansi_terminal(&version, &ParseOptions::default(), None).trim_end()
                )
            } else {
                String::new()
            }
        );
    }

    Ok(())
}
