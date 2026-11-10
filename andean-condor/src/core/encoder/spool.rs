//! One owned job-local Y4M file, reused sequentially without retaining future passes.

use std::{
    fs::File,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
    thread,
};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, bounded};

const CHUNK_BYTES: usize = 64 * 1024;
const WINDOW: usize = 2;

pub(super) fn create(parent: &Path, output: &Path) -> Result<File> {
    tempfile::tempfile_in(parent).with_context(|| {
        format!(
            "Creating Y4M replay spool for {} under {}",
            output.display(),
            parent.display()
        )
    })
}

pub(super) fn encode<T>(
    mut file: impl Read + Write + Seek + Send,
    frames: Receiver<Cursor<Vec<u8>>>,
    output: &Path,
    total: u8,
    mut encode: impl FnMut(u8, Receiver<Cursor<Vec<u8>>>) -> Result<T>,
) -> Result<T> {
    for frame in frames {
        // COMPAT: The stream writer concatenates entire messages, ignoring cursor position.
        file.write_all(frame.get_ref()).with_context(|| {
            format!(
                "Writing Y4M replay spool for {} (check free disk space)",
                output.display()
            )
        })?;
        #[cfg(all(test, target_os = "linux"))]
        super::e09_tests::observe_capture();
    }
    file.flush().with_context(|| {
        format!(
            "Flushing Y4M replay spool for {} (check free disk space)",
            output.display()
        )
    })?;
    #[cfg(all(test, target_os = "linux"))]
    super::e09_tests::observe_flush();

    let mut result = replay(&mut file, output, |rx| encode(1, rx))?;
    for pass in 2..=total {
        result = replay(&mut file, output, |rx| encode(pass, rx))?;
    }
    Ok(result)
}

pub(super) fn replay<T>(
    file: &mut (impl Read + Seek + Send),
    output: &Path,
    encode: impl FnOnce(Receiver<Cursor<Vec<u8>>>) -> Result<T>,
) -> Result<T> {
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("Seeking Y4M replay spool for {}", output.display()))?;
    let (tx, rx) = bounded(WINDOW);
    thread::scope(|scope| {
        let producer = scope.spawn(move || feed(file, &tx, output));
        // The encoder owns the only receiver. Even spawn failure drops it, unblocking send.
        let result = encode(rx);
        let feed = producer
            .join()
            .map_err(|_| anyhow::anyhow!("Y4M replay worker panicked for {}", output.display()));
        // Preserve the original encoder failure and its raw diagnostics, not SendError.
        let result = result?;
        feed??;
        Ok(result)
    })
}

fn feed(file: &mut impl Read, tx: &Sender<Cursor<Vec<u8>>>, output: &Path) -> Result<()> {
    loop {
        // ponytail: two queued + one writer + one producer chunk = at most 256 KiB.
        let mut bytes = vec![0; CHUNK_BYTES];
        let count = loop {
            match file.read(&mut bytes) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {},
                result => break result,
            }
        }
        .with_context(|| format!("Reading Y4M replay spool for {}", output.display()))?;
        if count == 0 {
            return Ok(());
        }
        bytes.truncate(count);
        tx.send(Cursor::new(bytes))
            .with_context(|| format!("Sending Y4M replay spool for {}", output.display()))?;
    }
}
