//! Read-only decoder-backed eligibility checks, independent of encoder progress.
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
};

use anyhow::{Context, Result, bail, ensure};

use crate::{
    core::{encoder::pipes::join_worker, process::OwnedProcess},
    utils::cancellation::Cancellation,
};

/// Publication must not replace caller-owned files (including dangling links).
/// Directories remain ordinary rename errors; a regular file cannot replace one.
pub(crate) fn protect_existing(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(metadata) => ensure!(
            metadata.is_dir(),
            "Refusing to overwrite existing chunk {}; file preserved. Recompute in a new job or move it aside first",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Inspecting publication target {}", path.display()));
        },
    }
    Ok(())
}

/// Count policy never disables the regular-file/nonempty requirement. The caller
/// must establish successful encoder status (or a saved completed record) first.
pub(crate) fn validate(
    path: &Path,
    expected: usize,
    count: bool,
    token: &Cancellation,
) -> Result<u64> {
    let checked = || -> Result<u64> {
        token.check_forced()?;
        let metadata = path.metadata().context("Reading encoded artifact metadata")?;
        ensure!(
            metadata.is_file() && metadata.len() > 0,
            "encoded artifact is not a nonempty regular file"
        );
        if count {
            probe(path, expected, token)?;
        }
        token.check_forced()?;
        Ok(metadata.len())
    };
    checked().with_context(|| format!("Validating encoded artifact {}", path.display()))
}

// Keep draining after the capture ceiling, so malformed/noisy probes cannot
// exhaust memory or block the other pipe. Overflow is an error, never evidence.
fn capture(mut pipe: impl Read) -> Result<Vec<u8>> {
    const LIMIT: usize = 16 * 1024;
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    let mut overflow = false;
    loop {
        let read = match pipe.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if read == 0 {
            break;
        }
        let kept = read.min(LIMIT - bytes.len());
        bytes.extend_from_slice(&buffer[..kept]);
        overflow |= kept != read;
    }
    ensure!(!overflow, "ffprobe output exceeded {LIMIT} bytes");
    Ok(bytes)
}

fn probe(path: &Path, expected: usize, token: &Cancellation) -> Result<()> {
    let executable = which::which("ffprobe").context("Frame-count validation requires ffprobe on PATH; install/provide ffprobe with a decoder for this codec")?;
    // Absolute filesystem paths cannot be interpreted as URL/protocol inputs.
    let path = path.canonicalize().context("Resolving encoded artifact path")?;
    let mut command = Command::new(executable);
    command
        .args([
            "-v",
            "error",
            "-threads",
            "1",
            "-err_detect",
            "explode",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "json",
            "-i",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = OwnedProcess::spawn(&mut command, token)
        .context("Starting ffprobe frame-count validation")?;
    let stdout = child.stdout.take().context("ffprobe stdout unavailable")?;
    let stderr = child.stderr.take().context("ffprobe stderr unavailable")?;
    let stdout = thread::Builder::new()
        .name("chunk-count".into())
        .spawn(move || capture(stdout))
        .context("Starting ffprobe stdout drain")?;
    let stderr = match thread::Builder::new()
        .name("chunk-diagnostics".into())
        .spawn(move || capture(stderr))
    {
        Ok(stderr) => stderr,
        Err(error) => {
            let killed = child.kill();
            let reaped = child.wait();
            let drained = join_worker(
                stdout,
                "Joining ffprobe stdout after stderr worker setup failed",
            );
            token.check_forced()?;
            killed?;
            reaped?;
            drained?;
            return Err(error).context("Starting ffprobe stderr drain");
        },
    };
    // Wait retains kill control and cleans remaining descendants before joining
    // readers; every result is collected before any error can escape.
    let status = child.wait();
    if status.is_err() {
        let _ = child.kill();
    }
    let stdout = join_worker(stdout, "Reading ffprobe frame count");
    let stderr = join_worker(stderr, "Reading ffprobe diagnostics");
    token.check_forced()?;
    let status = status?;
    let stdout = stdout?;
    let stderr = stderr?;
    ensure!(
        status.success() && stderr.is_empty(),
        "ffprobe frame-count/decode validation failed ({status}); verify codec decoder support and recompute the chunk: {}",
        String::from_utf8_lossy(&stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&stdout).context("Malformed ffprobe frame-count JSON")?;
    let streams = json
        .get("streams")
        .and_then(serde_json::Value::as_array)
        .context("ffprobe returned no video streams")?;
    ensure!(
        streams.len() == 1,
        "ffprobe returned no unique video frame count"
    );
    let frames = streams[0]
        .get("nb_read_frames")
        .and_then(serde_json::Value::as_str)
        .and_then(|count| count.parse::<usize>().ok())
        .context(
            "ffprobe returned a missing/invalid decoded frame count; verify decoder support",
        )?;
    if frames != expected {
        bail!(
            "decoded frame count {frames} does not match expected {expected}; recompute the chunk, or explicitly disable validate_frame_count only for intentional frame-changing filters"
        );
    }
    Ok(())
}
