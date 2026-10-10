//! Streams Y4M frame data through an FFmpeg filtergraph subprocess.
//!
//! FFmpeg reads `yuv4mpegpipe` on stdin and writes the filtered stream on
//! stdout, regenerating the Y4M header (resolution, pixel format, frame
//! rate), so downstream encoders consume FFmpeg's header rather than the
//! input's.
//!
//! Backpressure keeps window-streaming bounded: a slow encoder blocks the
//! bounded output channel, which propagates back to the producer's window.
//! Memory stays O(window) per worker plus FFmpeg's own buffers.

use std::{
    io::{Cursor, Read, Write},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

use anyhow::{Result, bail};
use crossbeam_channel::{Receiver, Sender};

/// Tail of FFmpeg's stderr retained for failure messages.
const STDERR_TAIL_LIMIT: usize = 8 * 1024;
/// Read size for FFmpeg's stdout. Chunk boundaries are irrelevant: the
/// encoder's stdin thread concatenates chunks into one byte stream.
const STDOUT_READ_SIZE: usize = 64 * 1024;

/// Handle to a running FFmpeg filter subprocess bridging a frame producer to
/// a frame consumer.
pub struct FfmpegFilter {
    receiver: Receiver<Cursor<Vec<u8>>>,
    failure:  Arc<Mutex<Option<String>>>,
}

impl FfmpegFilter {
    /// Spawns `ffmpeg` applying `graph` to the Y4M stream arriving on
    /// `input`, forwarding the filtered stream through the returned handle's
    /// receiver.
    ///
    /// `failure` is shared with the caller so producers can report FFmpeg
    /// errors that would otherwise surface as opaque channel disconnects. It
    /// is filled by [`FfmpegFilter::check`] once FFmpeg exits.
    #[inline]
    pub fn spawn(
        input: Receiver<Cursor<Vec<u8>>>,
        output_bound: usize,
        graph: &str,
        failure: Arc<Mutex<Option<String>>>,
    ) -> Result<Self> {
        let ffmpeg = which::which("ffmpeg").map_err(|err| {
            anyhow::anyhow!(
                "FFmpeg is required to apply the filtergraph but was not found in PATH: {err}"
            )
        })?;
        let mut child = Command::new(ffmpeg)
            .arg("-hide_banner")
            .arg("-v")
            .arg("error")
            .arg("-nostats")
            .arg("-i")
            .arg("pipe:0")
            .arg("-vf")
            .arg(graph)
            .arg("-fps_mode")
            .arg("passthrough")
            // Y4M is nominally 8-bit only; `-strict -1` lets the muxer write
            // high-bit-depth tags (C420p10, ...) that encoders accept.
            .arg("-strict")
            .arg("-1")
            .arg("-f")
            .arg("yuv4mpegpipe")
            .arg("pipe:1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("FFmpeg should have STDIN");
        let stdout = child.stdout.take().expect("FFmpeg should have STDOUT");
        let stderr = child.stderr.take().expect("FFmpeg should have STDERR");

        let (output_tx, output_rx) = crossbeam_channel::bounded::<Cursor<Vec<u8>>>(output_bound);

        // Dropping `stdin` at the end of this thread gives FFmpeg its end of
        // stream.
        thread::spawn(move || {
            let mut stdin = stdin;
            for chunk in input {
                if stdin.write_all(chunk.get_ref()).is_err() {
                    break;
                }
            }
        });

        // FFmpeg can block on a full stderr pipe, so drain it concurrently.
        let stderr_thread = spawn_stderr_drain(stderr);
        let relay_failure = Arc::clone(&failure);
        thread::spawn(move || {
            relay_stdout(child, stdout, &output_tx, stderr_thread, &relay_failure);
        });

        Ok(Self {
            receiver: output_rx,
            failure,
        })
    }

    /// The filtered Y4M stream. The first chunk carries FFmpeg's regenerated
    /// Y4M header.
    #[inline]
    pub fn receiver(&self) -> Receiver<Cursor<Vec<u8>>> {
        self.receiver.clone()
    }

    /// Whether FFmpeg failed. Reports `Err` with FFmpeg's exit status and
    /// stderr tail once the subprocess has exited; always safe to call after
    /// the consumer observed end of stream.
    #[inline]
    pub fn check(&self) -> Result<()> {
        if let Some(message) = self.is_failed() {
            bail!(message);
        }
        Ok(())
    }

    /// The failure message, if FFmpeg has failed.
    #[inline]
    pub fn is_failed(&self) -> Option<String> {
        self.failure
            .lock()
            .expect("ffmpeg filter failure mutex should acquire lock")
            .clone()
    }
}

/// Drains FFmpeg's stdout into `output_tx`, then reaps FFmpeg.
///
/// The failure record is written before the sender drops, so a consumer that
/// observed end of stream is guaranteed to see any failure via
/// [`FfmpegFilter::check`]. If the consumer disappears first, FFmpeg is killed
/// instead of reported.
fn relay_stdout(
    mut child: Child,
    mut stdout: impl Read,
    output_tx: &Sender<Cursor<Vec<u8>>>,
    stderr_thread: JoinHandle<Vec<u8>>,
    failure: &Arc<Mutex<Option<String>>>,
) {
    let mut buffer = vec![0_u8; STDOUT_READ_SIZE];
    loop {
        match stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if output_tx.send(Cursor::new(buffer[..read].to_vec())).is_err() {
                    // The consumer is gone; stop FFmpeg rather than report a
                    // failure it never had a chance to surface.
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stderr_thread.join();
                    return;
                }
            },
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                let tail = stderr_thread.join().unwrap_or_default();
                record_failure(
                    failure,
                    format!(
                        "reading FFmpeg output failed: {err}{}",
                        format_stderr(&tail)
                    ),
                );
                return;
            },
        }
    }

    let status = match child.wait() {
        Ok(status) => status,
        Err(err) => {
            let tail = stderr_thread.join().unwrap_or_default();
            record_failure(
                failure,
                format!("waiting on FFmpeg failed: {err}{}", format_stderr(&tail)),
            );
            return;
        },
    };
    let tail = stderr_thread.join().unwrap_or_default();
    if !status.success() {
        record_failure(
            failure,
            format!("FFmpeg exited with {status}{}", format_stderr(&tail)),
        );
    }
}

/// Drains stderr into a bounded tail, keeping only the last
/// [`STDERR_TAIL_LIMIT`] bytes so a noisy filter cannot grow memory without
/// bound.
fn spawn_stderr_drain(mut stderr: impl Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut tail: Vec<u8> = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    tail.extend_from_slice(&buffer[..read]);
                    if tail.len() > STDERR_TAIL_LIMIT {
                        let excess = tail.len() - STDERR_TAIL_LIMIT;
                        tail.drain(..excess);
                    }
                },
            }
        }
        tail
    })
}

/// Records `message` as the stage's failure, overwriting any earlier one.
fn record_failure(failure: &Arc<Mutex<Option<String>>>, message: String) {
    *failure.lock().expect("ffmpeg filter failure mutex should acquire lock") = Some(message);
}

/// Formats a stderr tail for appending to a failure message. Returns an empty
/// string when there is nothing to show.
fn format_stderr(tail: &[u8]) -> String {
    let tail = String::from_utf8_lossy(tail);
    let tail = tail.trim();
    if tail.is_empty() {
        String::new()
    } else {
        format!(":\n{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal Y4M stream: header plus `frames` solid-black 8-bit 4:2:0
    /// frames at 8x8.
    fn y4m_stream(frames: usize) -> Vec<u8> {
        const HEADER: &[u8] = b"YUV4MPEG2 W8 H8 F1:1 Ip A1:1 C420mpeg2\n";
        const FRAME_HEADER: &[u8] = b"FRAME\n";
        let mut stream = HEADER.to_vec();
        for _ in 0..frames {
            stream.extend_from_slice(FRAME_HEADER);
            stream.extend_from_slice(&[16_u8; 8 * 8]); // Y
            stream.extend_from_slice(&[128_u8; 4 * 4]); // U
            stream.extend_from_slice(&[128_u8; 4 * 4]); // V
        }
        stream
    }

    /// Feeds `stream` through `graph` and returns all output bytes plus the
    /// stage's `check()` result.
    fn run_filter(graph: &str, stream: Vec<u8>) -> (Vec<u8>, Result<()>) {
        let (input_tx, input_rx) = crossbeam_channel::bounded(2);
        let failure = Arc::new(Mutex::new(None));
        let stage =
            FfmpegFilter::spawn(input_rx, 2, graph, failure).expect("FfmpegFilter should spawn");
        let output_rx = stage.receiver();

        let producer = thread::spawn(move || {
            // Feed one chunk per frame to mirror window-streamed producers.
            let split = stream
                .windows(6)
                .position(|window| window == b"FRAME\n")
                .expect("stream should have frames");
            input_tx.send(Cursor::new(stream[..split].to_vec())).ok();
            let mut rest = &stream[split..];
            while !rest.is_empty() {
                let end = rest[6..]
                    .windows(6)
                    .position(|window| window == b"FRAME\n")
                    .map_or(rest.len(), |offset| offset + 6);
                input_tx.send(Cursor::new(rest[..end].to_vec())).ok();
                rest = &rest[end..];
            }
        });

        let mut output = Vec::new();
        for chunk in output_rx {
            output.extend_from_slice(chunk.get_ref());
        }
        producer.join().expect("producer thread should join");
        (output, stage.check())
    }

    #[test]
    fn passes_frames_through_unchanged() {
        if which::which("ffmpeg").is_err() {
            eprintln!("skipping passes_frames_through_unchanged: ffmpeg not in PATH");
            return;
        }
        let frames = 8;
        let (output, result) = run_filter("null", y4m_stream(frames));
        result.expect("the null filter should not fail");
        let output = String::from_utf8_lossy(&output);
        assert!(
            output.starts_with("YUV4MPEG2"),
            "output starts with a Y4M header"
        );
        let frame_count = output.matches("FRAME\n").count();
        assert_eq!(frame_count, frames, "passthrough preserves the frame count");
    }

    #[test]
    fn crop_filter_updates_dimensions() {
        if which::which("ffmpeg").is_err() {
            eprintln!("skipping crop_filter_updates_dimensions: ffmpeg not in PATH");
            return;
        }
        let (output, result) = run_filter("crop=6:6", y4m_stream(8));
        result.expect("the crop filter should not fail");
        let output = String::from_utf8_lossy(&output);
        assert!(
            output.starts_with("YUV4MPEG2"),
            "output starts with a Y4M header"
        );
        assert!(
            output.contains("W6 H6"),
            "FFmpeg regenerates the header with the cropped dimensions"
        );
    }

    #[test]
    fn reports_invalid_filter_failure() {
        if which::which("ffmpeg").is_err() {
            eprintln!("skipping reports_invalid_filter_failure: ffmpeg not in PATH");
            return;
        }
        let (output, result) = run_filter("definitely_not_a_filter", y4m_stream(8));
        let message = result.expect_err("an invalid filter should fail the stage");
        assert!(output.is_empty(), "a failed filter produces no frames");
        let message = message.to_string();
        assert!(
            message.contains("definitely_not_a_filter"),
            "failure message includes FFmpeg's stderr: {message}"
        );
    }

    #[test]
    fn check_reports_missing_ffmpeg() {
        let (input_tx, input_rx) = crossbeam_channel::bounded(2);
        drop(input_tx);
        let result = FfmpegFilter::spawn(input_rx, 2, "null", Arc::new(Mutex::new(None)));
        match result {
            Ok(stage) => drop(stage),
            Err(err) => assert!(
                err.to_string().contains("not found in PATH"),
                "spawn error names the PATH lookup: {err}"
            ),
        }
    }
}
