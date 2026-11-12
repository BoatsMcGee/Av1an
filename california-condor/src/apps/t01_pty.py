"""Run the cfg(test)-only TUI child on a real controlling Unix terminal."""

import ctypes
import errno
import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path


def check_stream(
    binary: str, stream: str, mode: str = "normal", root: str = "",
    cleanup_errors: list[str] | None = None,
) -> None:
    errors = cleanup_errors if cleanup_errors is not None else []
    master, slave = pty.openpty()
    try:
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 6, 80, 0, 0))
        original_mode = termios.tcgetattr(slave)

        def controlling_terminal() -> None:
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
            assert os.tcgetpgrp(slave) == os.getpgrp()

        env = os.environ.copy()
        env.pop("CONDOR_TEST_MODE", None)
        env["T01_PTY_STREAM"] = stream
        env["TERM"] = "xterm-256color"
        panic_case = mode.removeprefix("t04-") if mode.startswith("t04-") else None
        if panic_case:
            env["T04_CASE"] = panic_case
            env["T04_ROOT"] = root
        with (
            tempfile.TemporaryFile() as redirected,
            tempfile.TemporaryFile() as headless_stderr,
        ):
            child = subprocess.Popen(
                [
                    binary,
                    "--exact",
                    ("apps::tests::t04_startup_child" if panic_case == "startup"
                     else "apps::tests::t04_cli_child" if panic_case.startswith("cli-")
                     else "apps::tests::t04_panic_child")
                    if panic_case
                    else {
                        "normal": "apps::tests::t01_pty_child",
                        "t03": "apps::tests::t03_pty_child",
                        "matrix": "apps::tests::t03_ownership_child",
                    }[mode],
                    "--include-ignored",
                    "--nocapture",
                    "--quiet",
                ],
                stdin=slave,
                stdout=slave if stream == "stdout" else redirected,
                stderr=(
                    slave if stream == "stderr"
                    else headless_stderr if stream == "headless" else redirected
                ),
                env=env,
                preexec_fn=controlling_terminal,
            )
            terminal_output = bytearray()
            answered_queries = 0
            deadline = time.monotonic() + 30
            try:
                while child.poll() is None:
                    if time.monotonic() >= deadline:
                        raise AssertionError(f"{stream}: TUI child exceeded 30s watchdog")
                    if select.select([master], [], [], 0.05)[0]:
                        terminal_output.extend(os.read(master, 65536))
                    # Answer even a misrouted query so RED detects stream pollution,
                    # rather than merely timing out in crossterm's cursor reader.
                    redirected_output = os.pread(redirected.fileno(), 1048576, 0)
                    queries = terminal_output.count(b"\x1b[6n")
                    queries += redirected_output.count(b"\x1b[6n")
                    while answered_queries < queries:
                        os.write(master, b"\x1b[1;1R")
                        answered_queries += 1
                # Close our slave after child exit so EOF, not a zero-time poll,
                # marks the end of all terminal output.
                restored_mode = termios.tcgetattr(slave)
                os.close(slave)
                slave = -1
                while True:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                        break
                    if not chunk:
                        break
                    terminal_output.extend(chunk)
            finally:
                try:
                    if child.poll() is None:
                        child.kill()  # Popen still owns this unreaped child, pinning its PID.
                except OSError as error:
                    errors.append(f"child kill: {error}")
                try:
                    child.wait(timeout=5)
                except (OSError, subprocess.TimeoutExpired) as error:
                    errors.append(f"child reap: {error}")

            redirected.seek(0)
            redirected_output = redirected.read()
            terminal_output = bytes(terminal_output)
            evidence = (stream, child.returncode, terminal_output, redirected_output)
            if panic_case:
                headless_stderr.seek(0)
                diagnostic = (
                    headless_stderr.read() if stream == "headless"
                    else terminal_output if stream == "stderr" else redirected_output
                )
                data = terminal_output if stream == "stdout" else redirected_output
                assert child.returncode != 0, evidence
                cli = panic_case.startswith("cli-")
                payload_case = (
                    "main" if panic_case == "subsequent"
                    else "ui" if panic_case in ("cli-ui", "cli-mixed")
                    else "worker" if panic_case == "cli-aggregate"
                    else panic_case.removeprefix("cli-")
                )
                payload = f"T04-{payload_case}-payload".encode()
                if panic_case == "cli-error":
                    assert b"T04 primary context" in diagnostic, evidence
                    assert b"T04-nonpanic-error" in diagnostic, evidence
                    assert b"panicked" not in diagnostic, evidence
                else:
                    assert diagnostic.count(payload) == 1, (payload, diagnostic, evidence)
                if panic_case == "cli-unreported":
                    assert b"diagnostic above" not in diagnostic, evidence
                assert payload not in data, evidence
                sentinel = b"T04_BEGIN\nsource-output-sentinel\nT04_END\n"
                assert sentinel in data.replace(b"\r\n", b"\n"), evidence
                assert restored_mode == original_mode, evidence
                assert Path(root, "unwound").read_bytes() == b"owned child reaped", evidence
                if cli:
                    assert Path(root, "library-error").read_bytes() == b"original library error retained"
                    if panic_case == "cli-mixed":
                        assert b"T04 primary context" in diagnostic, evidence
                        assert b"T04-nonpanic-error" in diagnostic, evidence
                    if panic_case == "cli-aggregate":
                        for preserved in (b"T04-nonpanic-error", b"codec-stdout-sentinel", b"codec-stderr-sentinel"):
                            assert preserved in diagnostic, evidence
                if panic_case == "startup":
                    assert b"T04_STARTUP_RETIRED_RAW_OFF" in data, evidence
                    assert b"\x1b" not in terminal_output, evidence
                elif stream == "headless":
                    assert b"\x1b" not in diagnostic + data + terminal_output, evidence
                else:
                    shows = terminal_output.count(b"\x1b[?25h")
                    assert shows == (2 if panic_case == "subsequent" else 1), evidence
                    assert terminal_output.count(b"\x1b[?1049l") == shows, evidence
                    for segment in terminal_output.split(b"\x1b[?25h")[1:]:
                        assert b"\x1b" not in segment.split(b"\x1b[?1049h")[0], evidence
                    if stream == "stderr" and panic_case not in ("cli-error", "cli-unreported"):
                        leave = terminal_output.index(b"\x1b[?1049l")
                        show = terminal_output.index(b"\x1b[?25h")
                        assert leave < show < diagnostic.index(payload), evidence
                    assert b"\x1b" not in redirected_output, evidence
                if panic_case == "subsequent":
                    assert diagnostic.count(b"T04_PRIOR:") == 3, evidence
                    assert diagnostic.count(b"T04-second-payload") == 1, evidence
                    assert diagnostic.count(b"T04-third-payload") == 1, evidence
                    assert b"T04_PRIOR_COUNT=3" in diagnostic, evidence
                print(
                    f"{stream}/{panic_case}: diagnostic once, failed outcome, "
                    "owned unwind, terminal retired"
                )
                return
            assert child.returncode == 0, evidence
            harness_output = (
                terminal_output if stream == "stdout" else redirected_output
            )
            assert b"running 1 test" in harness_output, evidence
            assert b"1 passed; 0 failed" in harness_output, evidence
            assert b"\x1b" not in redirected_output, evidence
            if mode == "matrix":
                assert b"T03_MATRIX_PASSED" in harness_output, evidence
                assert harness_output.count(b"T03_CASE_") == 15, evidence
                assert restored_mode == original_mode, evidence
                print(f"{stream}: 15 ownership cases passed; termios restored")
                return
            if mode == "t03":
                assert b"T03_ERROR_RETURNED_CANCEL_TRUE" in harness_output, evidence
            elif stream == "stderr":
                payload = redirected_output.split(b"T01_BEGIN\n", 1)[1]
                assert payload.split(b"T01_END\n", 1)[0] == b"payload\n", evidence
            enter = terminal_output.index(b"\x1b[?1049h")
            leave = terminal_output.index(b"\x1b[?1049l")
            show = terminal_output.index(b"\x1b[?25h", leave)
            if mode == "t03":
                assert enter < leave < show, evidence
                for command in (b"\x1b[?1049h", b"\x1b[?1049l", b"\x1b[?25h"):
                    assert terminal_output.count(command) == 1, evidence
                assert b"\x1b[?25l" not in terminal_output[show:], evidence
            else:
                render = terminal_output.index(b"T01-final-render", leave)
                assert enter < leave < render < show, evidence
                assert terminal_output[leave + 8 :].startswith(b"\r\n" * 6), evidence
            assert restored_mode == original_mode, evidence
            actions = (
                "error+cancel/enter/leave/show"
                if mode == "t03"
                else "enter/leave/final-render/show"
            )
            print(
                f"{stream}: 1 child passed; "
                f"{actions} on PTY; "
                f"redirected stream clean; {answered_queries} cursor queries; "
                "terminal mode restored"
            )
    finally:
        for fd in (master, slave):
            if fd >= 0:
                try:
                    os.close(fd)
                except OSError as error:
                    errors.append(f"PTY close: {error}")
        if cleanup_errors is None and errors:
            raise AssertionError("; ".join(errors))


def fixture_identity(pid: int) -> tuple[int, int]:
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
    return int(fields[19]), int(fields[2])  # start ticks and process group


def fixture_pidfd(pid: int, identity: tuple[int, int]) -> int:
    """Bind a kernel handle before checking the recorded lifetime; never kill by PID."""
    fd = os.pidfd_open(pid)
    try:
        if fixture_identity(pid) != identity:
            raise ValueError(f"fixture PID {pid} identity changed; refusing cleanup")
        return fd
    except BaseException:
        os.close(fd)
        raise


def cleanup_fixtures(records: list[str], errors: list[str]) -> None:
    for record in records:
        fd = None
        try:
            pid, start, group = map(int, record.split())
            try:
                fd = fixture_pidfd(pid, (start, group))
            except ProcessLookupError:
                continue  # An already-reaped lifetime needs no further cleanup.
            except FileNotFoundError:
                continue  # It exited between opening the handle and checking identity.
            if not select.select([fd], [], [], 5)[0]:
                errors.append(f"owned PID {pid} alive after child outcome")
                signal.pidfd_send_signal(fd, signal.SIGKILL)
            if not select.select([fd], [], [], 5)[0]:
                errors.append(f"fixture PID {pid} did not exit after kill")
            deadline = time.monotonic() + 5
            while True:
                try:
                    reaped = os.waitid(os.P_PIDFD, fd, os.WEXITED | os.WNOHANG)
                except ChildProcessError:
                    break  # OwnedProcess already reaped the leader.
                if reaped:
                    break
                if time.monotonic() >= deadline:
                    errors.append(f"fixture PID {pid} not reaped")
                    break
                time.sleep(0.01)
        except (OSError, ValueError) as error:
            errors.append(str(error))
        finally:
            if fd is not None:
                try:
                    os.close(fd)
                except OSError as error:
                    errors.append(f"fixture handle close: {error}")


def check_fixture_safety() -> None:
    # A stale first record must neither signal its PID nor skip the next cleanup.
    children = []
    try:
        for _ in range(2):
            children.append(subprocess.Popen(["sleep", "300"]))
        first, second = children
        start, group = fixture_identity(first.pid)
        next_start, next_group = fixture_identity(second.pid)
        errors = []
        cleanup_fixtures(
            [f"{first.pid} {start + 1} {group}", f"{second.pid} {next_start} {next_group}"],
            errors,
        )
        assert first.poll() is None, "stale identity signaled a live process"
        assert second.poll() is not None, "first error skipped later fixture cleanup"
        assert any("identity changed" in error for error in errors), errors
        assert any("alive after child outcome" in error for error in errors), errors
        print("fixture safety: stale identity refused; later owned cleanup attempted and reaped")
    finally:
        errors = []
        for child in children:
            try:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired) as error:
                errors.append(str(error))
        if errors:
            raise AssertionError("; ".join(errors))


def check_panics(binary: str) -> None:
    # Local to this test driver: adopt/reap fixture descendants even on RED.
    libc = ctypes.CDLL(None, use_errno=True)
    assert libc.prctl(36, 1, 0, 0, 0) == 0  # PR_SET_CHILD_SUBREAPER
    check_fixture_safety()
    failures = []
    for stream in ("stderr", "stdout", "headless"):
        cases = (
            ("ui", "worker", "progress", "main", "subsequent", "startup", "cli-ui", "cli-worker", "cli-mixed", "cli-error", "cli-aggregate", "cli-unreported")
            if stream != "headless" else ("main", "progress", "cli-worker", "cli-error", "cli-aggregate", "cli-unreported")
        )
        for case in cases:
            with tempfile.TemporaryDirectory(prefix="t04-") as root:
                Path(root, "source").write_bytes(b"source-sentinel")
                Path(root, "output").write_bytes(b"output-sentinel")
                cleanup_errors = []
                try:
                    check_stream(binary, stream, f"t04-{case}", root, cleanup_errors)
                except (AssertionError, OSError, subprocess.TimeoutExpired) as error:
                    failures.append(f"{stream}/{case}: {error}")
                finally:
                    # Also clean real descendants after a RED timeout/failed assertion.
                    pids_path = Path(root, "pids")
                    try:
                        if pids_path.exists():
                            cleanup_fixtures(pids_path.read_text().splitlines(), cleanup_errors)
                    except OSError as error:
                        cleanup_errors.append(str(error))
                    for name in ("source", "output"):
                        try:
                            if Path(root, name).read_bytes() != f"{name}-sentinel".encode():
                                cleanup_errors.append(f"{name} sentinel changed")
                        except OSError as error:
                            cleanup_errors.append(str(error))
                    failures.extend(f"{stream}/{case}: {error}" for error in cleanup_errors)
    assert not failures, "\n".join(failures)


if __name__ == "__main__" and len(sys.argv) > 2 and sys.argv[2] == "t04":
    check_panics(sys.argv[1])
elif __name__ == "__main__":
    for selected_stream in ("stderr", "stdout"):
        check_stream(
            sys.argv[1], selected_stream, sys.argv[2] if len(sys.argv) > 2 else "normal"
        )
