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
    io::{self, Cursor, Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use anyhow::{Context, Result, bail};
use crossbeam_channel::Receiver;

/// Tail of FFmpeg's stderr retained for failure messages.
const STDERR_TAIL_LIMIT: usize = 8 * 1024;
/// Read size for FFmpeg's stdout. Chunk boundaries are irrelevant: the
/// encoder's stdin thread concatenates chunks into one byte stream.
const STDOUT_READ_SIZE: usize = 64 * 1024;

/// Handle to a running FFmpeg filter subprocess bridging a frame producer to
/// a frame consumer.
///
/// Keep this handle with the consumer and call [`Self::check`] when it exits,
/// not after joining a potentially blocked producer. Dropping it also stops
/// and joins the stage. Only the owned FFmpeg child is stopped, not descendants.
pub struct FfmpegFilter {
    receiver: Option<Receiver<Cursor<Vec<u8>>>>,
    failure: Arc<Mutex<Option<String>>>,
    child: Child,
    stop: Option<crossbeam_channel::Sender<()>>,
    input_thread: Option<JoinHandle<io::Result<()>>>,
    output_thread: Option<JoinHandle<io::Result<()>>>,
    stderr_thread: Option<JoinHandle<Result<Vec<u8>>>>,
    output_eof: Arc<AtomicBool>,
    completed: bool,
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
        let mut stdin = child.stdin.take().expect("FFmpeg should have STDIN");
        let mut stdout = child.stdout.take().expect("FFmpeg should have STDOUT");
        let stderr = child.stderr.take().expect("FFmpeg should have STDERR");

        let (output_tx, output_rx) = crossbeam_channel::bounded::<Cursor<Vec<u8>>>(output_bound);

        // Disconnect wakes both relays even before the first input/output.
        // Killing the owned child also releases blocked pipe I/O.
        let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(0);
        let input_stop = stop_rx.clone();
        let input_thread = thread::spawn(move || {
            loop {
                crossbeam_channel::select_biased! {
                    recv(input_stop) -> _ => return Ok(()),
                    recv(input) -> chunk => match chunk {
                        Ok(chunk) => stdin.write_all(chunk.get_ref())?,
                        Err(_) => return Ok(()),
                    },
                }
            }
        });

        // FFmpeg can block on a full stderr pipe, so drain it concurrently.
        let stderr_thread = spawn_stderr_drain(stderr);
        let output_eof = Arc::new(AtomicBool::new(false));
        let relay_eof = Arc::clone(&output_eof);
        let output_thread = thread::spawn(move || {
            let mut buffer = vec![0_u8; STDOUT_READ_SIZE];
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) => {
                        // Publish before dropping the sender: consumer EOF
                        // distinguishes natural completion from early abort.
                        relay_eof.store(true, Ordering::Release);
                        return Ok(());
                    },
                    Ok(read) => crossbeam_channel::select_biased! {
                        recv(stop_rx) -> _ => return Ok(()),
                        send(output_tx, Cursor::new(buffer[..read].to_vec())) -> sent => {
                            if sent.is_err() { return Ok(()); }
                        },
                    },
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => {},
                    Err(err) => return Err(err),
                }
            }
        });

        Ok(Self {
            receiver: Some(output_rx),
            failure,
            child,
            stop: Some(stop_tx),
            input_thread: Some(input_thread),
            output_thread: Some(output_thread),
            stderr_thread: Some(stderr_thread),
            output_eof,
            completed: false,
        })
    }

    /// Transfers the sole output receiver to the consumer, once. The first
    /// chunk carries FFmpeg's regenerated Y4M header. A second take is an error.
    #[inline]
    pub fn take_receiver(&mut self) -> Result<Receiver<Cursor<Vec<u8>>>> {
        self.receiver.take().context("FFmpeg filter output receiver was already taken")
    }

    /// Finishes after the consumer exits, stopping remaining input and joining
    /// every relay. Early consumer exit intentionally aborts FFmpeg; natural
    /// failure retains its status and bounded stderr tail.
    #[inline]
    pub fn check(&mut self) -> Result<()> {
        if !self.completed {
            self.finish();
        }
        if let Some(message) = self.is_failed() {
            bail!(message);
        }
        Ok(())
    }

    /// The failure message, if FFmpeg has failed.
    #[inline]
    pub fn is_failed(&self) -> Option<String> {
        self.failure.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn finish(&mut self) {
        self.receiver.take();
        self.stop.take();
        let mut errors = Vec::new();
        // A disconnected consumer may already have caused FFmpeg's SIGPIPE
        // before cleanup runs. Exit status alone cannot identify a real failure.
        let aborted = !self.output_eof.load(Ordering::Acquire);
        match self.child.try_wait() {
            Ok(Some(_)) => {},
            status => {
                if let Err(err) = status {
                    errors.push(format!("checking FFmpeg status failed: {err}"));
                }
                if aborted && let Err(err) = self.child.kill() {
                    errors.push(format!("stopping FFmpeg failed: {err}"));
                }
            },
        }
        // This stage alone owns/reaps Child. No mutex or completion worker can
        // prevent stop/kill or race take/drop against a blocking wait.
        match self.child.wait() {
            Ok(status) if !status.success() && !aborted => {
                errors.insert(0, format!("FFmpeg exited with {status}"));
            },
            Ok(_) => {},
            Err(err) => errors.push(format!("waiting on FFmpeg failed: {err}")),
        }
        if let Some(worker) = self.input_thread.take() {
            match worker.join() {
                Ok(Ok(())) => {},
                Ok(Err(err)) if aborted && err.kind() == io::ErrorKind::BrokenPipe => {},
                Ok(Err(err)) => errors.push(format!("writing FFmpeg input failed: {err}")),
                Err(_) => errors.push("FFmpeg input forwarder panicked".to_owned()),
            }
        }
        if let Some(worker) = self.output_thread.take() {
            match worker.join() {
                Ok(Ok(())) => {},
                Ok(Err(err)) => errors.push(format!("reading FFmpeg output failed: {err}")),
                Err(_) => errors.push("FFmpeg output relay panicked".to_owned()),
            }
        }
        let tail = match self.stderr_thread.take().map(JoinHandle::join) {
            Some(Ok(Ok(tail))) => tail,
            Some(Ok(Err(err))) => {
                errors.push(format!("reading FFmpeg stderr failed: {err:#}"));
                Vec::new()
            },
            Some(Err(_)) => {
                errors.push("FFmpeg stderr drain panicked".to_owned());
                Vec::new()
            },
            None => Vec::new(),
        };
        if !errors.is_empty() {
            record_failure(
                &self.failure,
                format!("{}{}", errors.join("; "), format_stderr(&tail)),
            );
        }
        self.completed = true;
    }
}

impl Drop for FfmpegFilter {
    #[inline]
    fn drop(&mut self) {
        if !self.completed
            && let Err(err) = self.check()
        {
            tracing::warn!(%err, "FFmpeg filter cleanup failed");
        }
    }
}

/// Drains stderr into a bounded tail, keeping only the last
/// [`STDERR_TAIL_LIMIT`] bytes so a noisy filter cannot grow memory without
/// bound.
fn spawn_stderr_drain(mut stderr: impl Read + Send + 'static) -> JoinHandle<Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut tail: Vec<u8> = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {},
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("FFmpeg stderr tail{}", format_stderr(&tail)));
                },
                Ok(read) => {
                    tail.extend_from_slice(&buffer[..read]);
                    if tail.len() > STDERR_TAIL_LIMIT {
                        let excess = tail.len() - STDERR_TAIL_LIMIT;
                        tail.drain(..excess);
                    }
                },
            }
        }
        Ok(tail)
    })
}

/// Records `message` as the stage's failure, overwriting any earlier one.
fn record_failure(failure: &Arc<Mutex<Option<String>>>, message: String) {
    *failure.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(message);
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
#[cfg(target_os = "linux")]
mod e10_tests;

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
        let mut stage =
            FfmpegFilter::spawn(input_rx, 2, graph, failure).expect("FfmpegFilter should spawn");
        let output_rx = stage.take_receiver().expect("sole receiver");

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
