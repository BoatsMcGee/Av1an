//! Stable Windows APIs: create suspended, assign kill-on-close job, then resume.
use std::{
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::Child,
};
use windows_sys::Win32::{
    Foundation::{INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
        Threading::{
            CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED, OpenThread, ResumeThread,
            THREAD_SUSPEND_RESUME, WaitForSingleObject,
        },
    },
};

pub(super) const CREATION_FLAGS: u32 = CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP;
pub(super) struct Job(OwnedHandle);
impl Job {
    pub(super) fn new() -> io::Result<Self> {
        // SAFETY: No name/security descriptor; returned handle is uniquely owned.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(unsafe { OwnedHandle::from_raw_handle(raw) });
        // SAFETY: Zero initializes this POD Win32 structure; size matches its type.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
    pub(super) fn assign(&self, child: &Child) -> io::Result<()> {
        #[cfg(test)]
        if tests::injected("assign", child) {
            return Err(io::Error::other("injected assignment failure"));
        }
        // SAFETY: Both handles are live; child was created suspended and cannot spawn.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub(super) fn kill(&self) -> io::Result<()> {
        // SAFETY: This private job contains only this owned child and its descendants.
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

pub(super) fn exited(child: &Child) -> io::Result<bool> {
    // SAFETY: Child keeps its process handle alive; zero timeout never blocks.
    match unsafe { WaitForSingleObject(child.as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}

pub(super) fn resume(child: &Child) -> io::Result<()> {
    #[cfg(test)]
    if tests::injected("resume", child) {
        return Err(io::Error::other("injected resume failure"));
    }
    // RATIONALE: Stable Child doesn't expose the primary thread handle. ToolHelp
    // enumerates threads; only the suspended, still-owned child's PID is selected.
    // LINK: learn.microsoft.com/windows/win32/toolhelp/traversing-the-thread-list
    // SAFETY: Snapshot APIs take integer flags and initialize THREADENTRY32.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    if unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } == 0 {
        return Err(io::Error::last_os_error());
    }
    loop {
        if entry.th32OwnerProcessID == child.id() {
            // SAFETY: Only this owned suspended process's thread is opened. It
            // cannot have exited/reused its PID while the caller holds its lock.
            let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
            let previous = unsafe { ResumeThread(thread.as_raw_handle()) };
            return match previous {
                1 => Ok(()),
                u32::MAX => Err(io::Error::last_os_error()),
                _ => Err(io::Error::other("Unexpected primary thread suspend count")),
            };
        }
        if unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } == 0 {
            return Err(io::Error::other(
                "Suspended child's primary thread not found",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{core::process::OwnedProcess, utils::cancellation::Cancellation};
    use std::{
        cell::{Cell, RefCell},
        os::windows::{io::BorrowedHandle, process::CommandExt},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    use windows_sys::Win32::System::{
        JobObjects::IsProcessInJob,
        Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE},
    };

    thread_local! {
        static FAILURE: Cell<Option<&'static str>> = const { Cell::new(None) };
        static FAILED_CHILD: RefCell<Option<OwnedHandle>> = const { RefCell::new(None) };
    }

    pub(super) fn injected(stage: &'static str, child: &Child) -> bool {
        if FAILURE.get() != Some(stage) {
            return false;
        }
        FAILURE.set(None);
        // SAFETY: Child owns this handle throughout the immediate duplication.
        let handle = unsafe { BorrowedHandle::borrow_raw(child.as_raw_handle()) }
            .try_clone_to_owned()
            .expect("duplicate child handle");
        FAILED_CHILD.with(|saved| *saved.borrow_mut() = Some(handle));
        true
    }

    fn command(root: &std::path::Path) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("test binary"));
        command
            .args(["--exact", "core::process::windows::tests::e12_windows_fixture", "--nocapture"])
            .env("E12_WINDOWS_ROOT", root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    fn descendant(root: &std::path::Path) -> OwnedHandle {
        let deadline = Instant::now() + Duration::from_secs(4);
        let pid = loop {
            if let Ok(pid) = std::fs::read_to_string(root.join("grandchild")) {
                break pid.parse::<u32>().expect("PID");
            }
            assert!(
                Instant::now() < deadline,
                "native child handshake timed out"
            );
            std::thread::yield_now();
        };
        // SAFETY: Open only the PID published by this private fixture, for wait/query only.
        let raw = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        assert!(!raw.is_null());
        unsafe { OwnedHandle::from_raw_handle(raw) }
    }

    #[test]
    fn e12_windows_fixture() {
        let Some(root) = std::env::var_os("E12_WINDOWS_ROOT") else {
            return;
        };
        if std::env::var_os("E12_WINDOWS_LEAF").is_some() {
            std::thread::sleep(Duration::from_secs(8));
        } else {
            let mut command = command(std::path::Path::new(&root));
            command.env("E12_WINDOWS_LEAF", "1");
            let mut child = command.spawn().expect("grandchild inside inherited job");
            std::fs::write(
                std::path::Path::new(&root).join("grandchild"),
                child.id().to_string(),
            )
            .expect("PID handshake");
            child.wait().expect("finite emergency lifetime");
        }
    }

    #[test]
    fn e12_windows_suspended_assignment_and_forced_tree() {
        let root = tempfile::tempdir().expect("fixture");
        let token = Cancellation::default();
        let mut process = OwnedProcess::spawn(&mut command(root.path()), &token)
            .expect("suspended assignment/resume");
        let grandchild = descendant(root.path());
        let mut inside = 0;
        let state = process.state.lock().expect("state");
        // SAFETY: Query the known descendant and this owned job, with a valid BOOL output.
        assert_ne!(
            unsafe {
                IsProcessInJob(
                    grandchild.as_raw_handle(),
                    state.job.0.as_raw_handle(),
                    &mut inside,
                )
            },
            0
        );
        assert_ne!(inside, 0, "descendant cannot run outside the assigned job");
        drop(state);
        token.request().expect("graceful");
        assert!(!exited(&process.state.lock().expect("state").child).expect("still running"));
        token.request().expect("force");
        process.wait().expect("reap direct child");
        // SAFETY: Wait on the already-opened fixture descendant only; bounded failure gate.
        assert_eq!(
            unsafe { WaitForSingleObject(grandchild.as_raw_handle(), 1000) },
            WAIT_OBJECT_0
        );
    }

    #[test]
    fn e12_windows_assignment_and_resume_failure_reap_suspended_child() {
        for stage in ["assign", "resume"] {
            let root = tempfile::tempdir().expect("fixture");
            FAILURE.set(Some(stage));
            assert!(
                OwnedProcess::spawn(&mut command(root.path()), &Cancellation::default()).is_err()
            );
            let handle =
                FAILED_CHILD.with(|saved| saved.borrow_mut().take()).expect("real failed child");
            // SAFETY: The duplicate remains valid after the spawning owner returned its error.
            assert_eq!(
                unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) },
                WAIT_OBJECT_0
            );
            assert!(
                !root.path().join("grandchild").exists(),
                "failed suspended child must never execute"
            );
        }
    }

    #[test]
    fn e12_windows_job_close_kills_descendants() {
        let root = tempfile::tempdir().expect("fixture");
        let job = Job::new().expect("kill-on-close job");
        let mut child = command(root.path())
            .creation_flags(CREATION_FLAGS)
            .spawn()
            .expect("suspended child");
        job.assign(&child).expect("assign before resume");
        resume(&child).expect("resume");
        let grandchild = descendant(root.path());
        drop(job);
        // SAFETY: Wait only on this fixture's process handles, with a bounded timeout.
        assert_eq!(
            unsafe { WaitForSingleObject(child.as_raw_handle(), 1000) },
            WAIT_OBJECT_0
        );
        assert_eq!(
            unsafe { WaitForSingleObject(grandchild.as_raw_handle(), 1000) },
            WAIT_OBJECT_0
        );
        child.wait().expect("reap direct child");
    }
}
