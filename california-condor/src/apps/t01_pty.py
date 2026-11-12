"""Run the cfg(test)-only TUI child on a real controlling Unix terminal."""

import ctypes
import errno
import fcntl
import os
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path


def check_stream(binary: str, stream: str, mode: str = "normal", root: str = "") -> None:
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
                    "apps::tests::t04_panic_child"
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
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=5)

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
                payload_case = "main" if panic_case == "subsequent" else panic_case
                payload = f"T04-{payload_case}-payload".encode()
                assert diagnostic.count(payload) == 1, (payload, diagnostic, evidence)
                assert payload not in data, evidence
                sentinel = b"T04_BEGIN\nsource-output-sentinel\nT04_END\n"
                assert sentinel in data.replace(b"\r\n", b"\n"), evidence
                assert Path(root, "unwound").read_bytes() == b"owned child reaped", evidence
                assert restored_mode == original_mode, evidence
                if stream == "headless":
                    assert b"\x1b" not in diagnostic + data + terminal_output, evidence
                else:
                    shows = terminal_output.count(b"\x1b[?25h")
                    assert shows == (2 if panic_case == "subsequent" else 1), evidence
                    assert terminal_output.count(b"\x1b[?1049l") == shows, evidence
                    for segment in terminal_output.split(b"\x1b[?25h")[1:]:
                        assert b"\x1b" not in segment.split(b"\x1b[?1049h")[0], evidence
                    if stream == "stderr":
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
        os.close(master)
        if slave >= 0:
            os.close(slave)


def process_running(path: Path) -> bool:
    try:
        return path.read_text().split(")", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False


def check_panics(binary: str) -> None:
    # Local to this test driver: adopt/reap fixture descendants even on RED.
    libc = ctypes.CDLL(None, use_errno=True)
    assert libc.prctl(36, 1, 0, 0, 0) == 0  # PR_SET_CHILD_SUBREAPER
    failures = []
    for stream in ("stderr", "stdout", "headless"):
        cases = (
            ("ui", "worker", "progress", "main", "subsequent")
            if stream != "headless" else ("main", "progress")
        )
        for case in cases:
            with tempfile.TemporaryDirectory(prefix="t04-") as root:
                Path(root, "source").write_bytes(b"source-sentinel")
                Path(root, "output").write_bytes(b"output-sentinel")
                try:
                    check_stream(binary, stream, f"t04-{case}", root)
                except (AssertionError, OSError) as error:
                    failures.append(f"{stream}/{case}: {error}")
                finally:
                    # Also clean real descendants after a RED timeout/failed assertion.
                    pids_path = Path(root, "pids")
                    if pids_path.exists():
                        for pid in pids_path.read_text().split():
                            path = Path(f"/proc/{pid}/stat")
                            deadline = time.monotonic() + 5
                            while process_running(path):
                                if time.monotonic() >= deadline:
                                    failures.append(
                                        f"{stream}/{case}: owned PID {pid} alive after child outcome"
                                    )
                                    os.kill(int(pid), 9)
                                    break
                                time.sleep(0.01)
                            deadline = time.monotonic() + 5
                            while True:
                                try:
                                    reaped, _ = os.waitpid(int(pid), os.WNOHANG)
                                except ChildProcessError:
                                    break  # Leaders were reaped by OwnedProcess.
                                if reaped:
                                    break
                                assert time.monotonic() < deadline, f"fixture PID {pid} not reaped"
                                time.sleep(0.01)
                    assert Path(root, "source").read_bytes() == b"source-sentinel"
                    assert Path(root, "output").read_bytes() == b"output-sentinel"
    assert not failures, "\n".join(failures)


if __name__ == "__main__" and len(sys.argv) > 2 and sys.argv[2] == "t04":
    check_panics(sys.argv[1])
elif __name__ == "__main__":
    for selected_stream in ("stderr", "stdout"):
        check_stream(
            sys.argv[1], selected_stream, sys.argv[2] if len(sys.argv) > 2 else "normal"
        )
