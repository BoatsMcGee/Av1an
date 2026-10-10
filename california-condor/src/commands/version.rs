use andean_condor::{
    metrics::{MetricLibraryInfo, libraries},
    models::encoder::{Encoder, EncoderBase},
    vapoursynth::{
        get_api,
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
use std::fmt::Write as _;

/// The coloured installed/absent marker used by every section.
///
/// Shared so the three tables cannot drift apart in how they report a
/// negative, and so the width calculation can ignore the escape codes.
#[inline]
fn marker(available: bool) -> &'static str {
    if available {
        "\x1b[0;32m✓\x1b[0m"
    } else {
        "\x1b[0;31m✗\x1b[0m"
    }
}

/// Render markdown through the same pipeline every section uses.
#[inline]
fn render(markdown: &str) -> String {
    render_ansi_terminal(markdown, &ParseOptions::default(), None)
        .trim_end()
        .to_owned()
}

/// Prints the `--version` report to stdout.
#[tracing::instrument(skip_all)]
pub fn print_version(verbose: bool) -> Result<()> {
    print!("{}", render_version(verbose)?);
    Ok(())
}

/// Builds the `--version` report as a string.
///
/// [`print_version`] prints this verbatim; the screenshot harness captures the
/// same string, so the documentation image cannot drift from the real output.
#[tracing::instrument(skip_all)]
pub fn render_version(verbose: bool) -> Result<String> {
    let mut out = String::new();
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

    let _ = writeln!(out, "{version_info}");

    // VapourSynth is optional: Environment::new() panics when the library
    // is missing, so check the API first.
    let environment = match get_api() {
        Ok(_) => Some(get_environment()?),
        Err(_) => None,
    };
    let core = environment.as_ref().map(get_core).transpose()?;
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

    // Reported alongside the VapourSynth plugins above rather than folded into
    // them, because the two are independent: libvship ships as the `libvship`
    // plugin, so the plugin row can be ✓ while the native C API is still
    // unloadable, and it is that second fact which decides whether scoring
    // runs on the GPU or falls back to the plugin.
    let library_infos = libraries();

    // Measured across all three tables so the rules and rows below share one
    // width, rather than each table picking its own.
    let max_width = plugin_infos
        .iter()
        .map(|plugin_info| format!("🐦 {}({})", plugin_info.name, plugin_info.id).len())
        .chain(encoders.iter().map(|(base, encoder)| {
            format!("{} ({})", base.friendly_name(), encoder.executable()).len()
        }))
        .chain(library_infos.iter().map(|info| info.name.len()))
        .max()
        .unwrap_or(0)
        + 1;

    let _ = writeln!(out, "\nVapourSynth Plugins Installed\n{}", "-".repeat(max_width));
    for plugin_info in plugin_infos {
        let _ = writeln!(out,
            "{} {}",
            marker(plugin_info.installed),
            render(&if verbose && let Some(docs) = plugin_info.docs {
                format!("[**{}** ({})]({})", plugin_info.name, plugin_info.id, docs)
            } else {
                format!("**{}** ({})", plugin_info.name, plugin_info.id)
            })
        );
    }

    let _ = writeln!(out, "\nEncoders Installed\n{}", "-".repeat(max_width));
    for (base, encoder) in encoders {
        let installed = encoder.validate().is_ok();
        let _ = writeln!(out,
            "{} {} ({}){}",
            marker(installed),
            render(&format!("**{}**", base.friendly_name())),
            encoder.executable(),
            if verbose && let Some(version) = encoder.version_text() {
                format!(": {}", render(&version))
            } else {
                String::new()
            }
        );
    }

    let _ = writeln!(out, "\nQuality Metric Libraries\n{}", "-".repeat(max_width));
    for info in library_infos {
        // The details are diagnostics rather than state, so an absent library
        // still renders: it just has no version or detail suffix to show.
        let suffix = if verbose && let Some(version) = info.version.as_deref() {
            let details = info
                .details
                .iter()
                .map(|(key, value)| format!("{key}: {value}"))
                .collect::<Vec<_>>()
                .join(", ");

            if details.is_empty() {
                format!(": {}", render(version))
            } else {
                format!(": {} ({details})", render(version))
            }
        } else {
            String::new()
        };

        let _ = writeln!(
            out,
            "{} {}{suffix}",
            marker(info.available),
            render(&format!("**{}**", info.name))
        );

        // Listed under the library that owns them, and only when asked for:
        // a one-GPU machine gains nothing from a table with one row, and the
        // default is already named on the line above.
        if verbose && !info.devices.is_empty() {
            write_devices(&mut out, &info);
        }
    }

    Ok(out)
}

/// Render one library's compute devices, marking the default.
///
/// The `gpuId` column leads because it is the actionable value: it is what a
/// user copies into a configuration to override the default selection.
fn write_devices(out: &mut String, info: &MetricLibraryInfo) {
    let rows: Vec<String> = info
        .devices
        .iter()
        .map(|device| {
            let mut facts = vec![device.name.clone()];
            if let Some(vram) = device.vram_bytes {
                facts.push(format!("{} VRAM", format_vram(vram)));
            }
            if device.integrated {
                facts.push("integrated".to_owned());
            }

            let selected = if device.is_default { " (default)" } else { "" };

            format!("[{}] {}{selected}", device.id, facts.join(", "))
        })
        .collect();

    let width = rows.iter().map(String::len).max().unwrap_or(0) + 1;

    let _ = writeln!(out, "  Compute Devices");
    let _ = writeln!(out, "  {}", "-".repeat(width));
    for row in &rows {
        let _ = writeln!(out, "  {}", render(row));
    }
}

/// Render a VRAM size in GiB, the unit GPU capacities are quoted in.
///
/// Whole-numbered only: a device's usable VRAM is not meaningfully precise to
/// the megabyte, so a fractional value here would imply accuracy the number
/// does not have.
fn format_vram(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    let gib = bytes as f64 / GIB;

    if gib >= 10.0 {
        format!("{gib:.0} GiB")
    } else {
        format!("{gib:.1} GiB")
    }
}

#[cfg(test)]
mod tests {
    use super::format_vram;

    /// Capacities are quoted in whole GiB once they are large enough that the
    /// decimal is noise, and to one decimal below that so a 6 GiB part does not
    /// read as "6" and a 3.5 GiB one does not read as "4".
    #[test]
    fn vram_is_quoted_in_gib() {
        assert_eq!(format_vram(1024 * 1024 * 1024), "1.0 GiB");
        assert_eq!(
            format_vram(3 * 1024 * 1024 * 1024 + 512 * 1024 * 1024),
            "3.5 GiB"
        );
        assert_eq!(format_vram(6 * 1024 * 1024 * 1024), "6.0 GiB");
        assert_eq!(format_vram(24 * 1024 * 1024 * 1024), "24 GiB");
        assert_eq!(format_vram(0), "0.0 GiB");
    }
}
