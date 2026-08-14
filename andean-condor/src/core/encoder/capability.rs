use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{LazyLock, RwLock},
};

use crate::models::encoder::EncoderBase;

/// Capabilities that need to be checked per encoder binary
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncoderCapability {
    /// SVT-AV1 support for fractional/quarter-step CRF values (e.g., `--crf
    /// 25.25`)
    SvtAv1QuarterStepCrf,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BinaryKey {
    encoder_base: EncoderBase,
    executable:   PathBuf,
}

/// Thread-safe global cache mapping `(BinaryKey, EncoderCapability)` to a
/// support flag.
static CAPABILITY_CACHE: LazyLock<RwLock<HashMap<(BinaryKey, EncoderCapability), bool>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Generate a single Y4M frame stream (420jpeg, `width`x`height`) into
/// `writer` for testing encoder capabilities.
fn generate_y4m_stream<W: Write>(mut writer: W, width: u32, height: u32) -> std::io::Result<()> {
    writeln!(writer, "YUV4MPEG2 W{width} H{height} F30:1 Ip A0:0 C420p10")?;
    writeln!(writer, "FRAME")?;
    let y_size = (width * height) as usize;
    let uv_size = ((width / 2) * (height / 2)) as usize;
    let frame = vec![0u8; y_size + 2 * uv_size];
    writer.write_all(&frame)?;
    writer.flush()?;
    Ok(())
}

/// Probe encoder binary for the given capability
///
/// Returns `false` on any failure
fn probe_capability(
    encoder_base: EncoderBase,
    executable: &Path,
    capability: EncoderCapability,
) -> bool {
    match capability {
        EncoderCapability::SvtAv1QuarterStepCrf => {
            if encoder_base != EncoderBase::SVTAV1 {
                return false;
            }
            svt_av1_supports_quarter_step_crf(executable)
        },
    }
}

fn svt_av1_supports_quarter_step_crf(executable: &Path) -> bool {
    let Ok(mut child) = Command::new(executable)
        .args([
            "-i",
            "stdin",
            "--crf",
            "50.75",
            "-b",
            crate::core::encoder::NULL_OUTPUT,
            "--progress",
            "0",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };

    if let Some(stdin) = child.stdin.take() {
        // Writing may fail if the encoder rejects arguments before reading input;
        // that's fine — we only care about the exit status.
        let _ = generate_y4m_stream(stdin, 320, 240);
    }

    child.wait().is_ok_and(|status| status.success())
}

/// Checks if encoder supports capability and caches the result
pub(crate) fn check_capability(
    encoder_base: EncoderBase,
    executable: &Path,
    capability: EncoderCapability,
) -> bool {
    let key = BinaryKey {
        encoder_base,
        executable: executable.to_path_buf(),
    };

    // Read result from cache
    if let Ok(cache) = CAPABILITY_CACHE.read()
        && let Some(&supported) = cache.get(&(key.clone(), capability))
    {
        return supported;
    }

    // Write result to cache
    let supported = probe_capability(encoder_base, &key.executable, capability);

    if let Ok(mut cache) = CAPABILITY_CACHE.write() {
        cache.insert((key, capability), supported);
    }

    supported
}
