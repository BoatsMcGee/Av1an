//! Local admission shutdown; active finite encoders still finish and are joined.
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::utils::semaphore::Semaphore;
use anyhow::{Result, anyhow};

pub(super) struct Stop {
    pub(super) stopped: AtomicBool,
    pub(super) workers: Arc<Semaphore>,
    sender: Mutex<Option<crossbeam_channel::Sender<()>>>,
    pub(super) receiver: crossbeam_channel::Receiver<()>,
}

impl Stop {
    pub(super) fn new(workers: usize) -> Self {
        let (sender, receiver) = crossbeam_channel::bounded(0);
        Self {
            stopped: AtomicBool::new(false),
            workers: Arc::new(Semaphore::new(workers)),
            sender: Mutex::new(Some(sender)),
            receiver,
        }
    }

    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.workers.close();
        // Disconnect broadcasts to every idle worker and job handoff selector.
        self.sender.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        #[cfg(all(test, target_os = "linux"))]
        super::e11_tests::stopped();
    }

    pub(super) fn stopped(&self, cancelled: &AtomicBool) -> bool {
        self.stopped.load(Ordering::SeqCst) || cancelled.load(Ordering::Relaxed)
    }

    /// The guard stops admission before outer permits/receivers are dropped.
    pub(super) fn guard(&self) -> StopOnFailure<'_> {
        StopOnFailure {
            stop: self,
            successful: false,
        }
    }

    pub(super) fn catch<T>(&self, context: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.catch_panic(context, f).map_err(|err| err.context(context.to_owned()))
    }

    /// Scope boundaries already contain contextual worker errors; preserve
    /// them while converting scheduling/spawn panics after shutdown and joins.
    pub(super) fn catch_panic<T>(&self, context: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let result = catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|panic| {
            let message = panic
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("non-string panic");
            Err(anyhow!("{context} panicked: {message}"))
        });
        if result.is_err() {
            self.stop();
        }
        result
    }
}

pub(super) struct StopOnFailure<'a> {
    stop: &'a Stop,
    successful: bool,
}

impl StopOnFailure<'_> {
    pub(super) fn success(&mut self) {
        self.successful = true;
    }
}

impl Drop for StopOnFailure<'_> {
    fn drop(&mut self) {
        if !self.successful {
            self.stop.stop();
        }
    }
}

/// Keep the original typed error; secondary failures remain visible in logs.
pub(super) fn retain_error(first: &mut Option<anyhow::Error>, next: anyhow::Error) {
    if first.is_some() {
        tracing::error!("Secondary parallel encoder failure: {next:#}");
    } else {
        *first = Some(next);
    }
}

pub(super) struct TempOutput {
    pub(super) path: PathBuf,
    owned: bool,
}

impl TempOutput {
    pub(super) fn new(path: PathBuf) -> Self {
        let owned = !path.exists();
        Self { path, owned }
    }
}

impl Drop for TempOutput {
    fn drop(&mut self) {
        if self.owned {
            match std::fs::remove_file(&self.path) {
                Ok(()) => {},
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {},
                Err(err) => tracing::error!("Could not remove {}: {err}", self.path.display()),
            }
        }
    }
}
