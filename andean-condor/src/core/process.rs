//! Concrete ownership of a job child and its process group/Job Object through reap.
use crate::utils::cancellation::Cancellation;
use anyhow::{Context, Result};
use std::{
    io,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus},
    sync::{Arc, Mutex},
    time::Duration,
};

pub(crate) struct ProcessState {
    child: Child,
    armed: bool,
    killed: bool,
    status: Option<ExitStatus>,
    #[cfg(windows)]
    job: windows::Job,
}

impl ProcessState {
    pub(crate) fn kill(&mut self) -> io::Result<()> {
        if !self.armed || self.killed {
            return Ok(());
        }
        #[cfg(unix)]
        {
            let pid = i32::try_from(self.child.id()).map_err(io::Error::other)?;
            if pid <= 0 {
                return Err(io::Error::other("invalid owned process group"));
            }
            // SAFETY: Command::process_group(0) created this group. Its leader is
            // still unreaped under this lock, so the ID cannot have been reused.
            if unsafe { libc::kill(-pid, libc::SIGKILL) } != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
        }
        #[cfg(windows)]
        self.job.kill()?;
        self.killed = true;
        Ok(())
    }

    fn exited(&self) -> io::Result<bool> {
        #[cfg(unix)]
        {
            // SAFETY: waitid initializes siginfo, WNOWAIT preserves the owned
            // leader/PID until all pipe workers finish and final group cleanup runs.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: The child is unreaped and info is a valid writable siginfo.
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result != 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: Successful waitid initialized the SIGCHLD union member.
            Ok(unsafe { info.si_pid() } != 0)
        }
        #[cfg(windows)]
        {
            windows::exited(&self.child)
        }
    }
}

/// Owns kill control independently of pipe readers/writers. Drop kills the tree
/// and reaps the direct child; explicit `wait` reports OS cleanup failures.
pub struct OwnedProcess {
    state: Arc<Mutex<ProcessState>>,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}

impl OwnedProcess {
    /// An active scene may spawn another pass during Graceful, but never Forced.
    #[inline]
    pub fn spawn(command: &mut Command, cancellation: &Cancellation) -> Result<Self> {
        cancellation.check_forced()?;
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        let job = windows::Job::new()?;
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows::CREATION_FLAGS);
        }
        let mut child = command.spawn().context("Spawning owned job process")?;
        let (stdin, stdout, stderr) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let mut process = Self {
            state: Arc::new(Mutex::new(ProcessState {
                child,
                armed: true,
                killed: false,
                status: None,
                #[cfg(windows)]
                job,
            })),
            stdin,
            stdout,
            stderr,
        };
        #[cfg(windows)]
        {
            let assigned = {
                let state = process.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                state.job.assign(&state.child)
            };
            if let Err(error) = assigned {
                // Suspended and unassigned: terminate the direct child, not an empty job.
                let mut state =
                    process.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let killed = state.child.kill();
                let reaped = state.child.wait();
                state.armed = false;
                killed.context("Terminating unassigned suspended child")?;
                reaped.context("Reaping unassigned suspended child")?;
                return Err(error).context("Assigning suspended child to Job Object");
            }
        }
        #[cfg(all(test, target_os = "linux"))]
        if std::env::var("E12_CASE").as_deref() == Ok("registration") {
            // Exact race seam: the real child can run, but has not been registered.
            while !cancellation.is_forced() {
                std::thread::yield_now();
            }
        }
        if let Err(error) = cancellation.register(&process.state) {
            process.wait().context("Reaping child cancelled during spawn")?;
            return Err(error);
        }
        #[cfg(windows)]
        {
            let resumed = {
                let state = process.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                windows::resume(&state.child)
            };
            if let Err(error) = resumed {
                process.kill()?;
                process.wait()?;
                cancellation.check_forced()?;
                return Err(error).context("Resuming job-assigned child");
            }
        }
        Ok(process)
    }

    #[inline]
    pub fn id(&self) -> u32 {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).child.id()
    }

    #[inline]
    pub fn kill(&mut self) -> Result<()> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .kill()
            .context("Killing owned process tree")
    }

    /// Call after draining/joining pipe workers. No mutex spans a pending wait;
    /// cancellation retains immediate kill control even for silent children.
    #[inline]
    pub fn wait(&mut self) -> Result<ExitStatus> {
        loop {
            {
                let mut state =
                    self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(status) = state.status {
                    return Ok(status);
                }
                if state.exited().context("Checking owned child without reaping")? {
                    let killed = state.kill(); // Before reap: no stale group ID can be signalled.
                    let status = state.child.wait();
                    state.armed = false;
                    if let Ok(status) = &status {
                        state.status = Some(*status);
                    }
                    killed.context("Cleaning remaining owned descendants")?;
                    return status.context("Reaping owned child");
                }
            }
            std::thread::park_timeout(Duration::from_millis(10));
        }
    }

    #[cfg(test)]
    pub(crate) fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        let exited = {
            let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            state.status.is_some() || state.exited()?
        };
        if exited {
            self.wait().map(Some)
        } else {
            Ok(None)
        }
    }
}

impl Drop for OwnedProcess {
    #[inline]
    fn drop(&mut self) {
        let armed = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).armed;
        if armed {
            if let Err(error) = self.kill() {
                tracing::error!(%error, "Owned process Drop kill failed");
            }
            if let Err(error) = self.wait() {
                tracing::error!(%error, "Owned process Drop reap failed");
            }
        }
    }
}

/// Remove only a newly-created interrupted output, never a caller-owned sentinel.
pub(crate) struct InterruptedOutput {
    path: std::path::PathBuf,
    owned: bool,
    cancellation: Cancellation,
}

impl InterruptedOutput {
    pub(crate) fn new(path: &std::path::Path, cancellation: &Cancellation) -> Self {
        Self {
            path: path.to_owned(),
            owned: !path.exists(),
            cancellation: cancellation.clone(),
        }
    }
}

impl Drop for InterruptedOutput {
    fn drop(&mut self) {
        if self.owned && self.cancellation.is_forced() {
            match std::fs::remove_file(&self.path) {
                Ok(()) => {},
                Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                Err(error) => {
                    tracing::error!(%error, path = %self.path.display(), "Removing interrupted output failed");
                },
            }
        }
    }
}

#[cfg(windows)]
mod windows;
