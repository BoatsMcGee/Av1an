"""Run the cfg(test)-only TUI child on a real controlling Unix terminal."""

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


def check_stream(binary: str, stream: str, mode: str = "normal") -> None:
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
        with tempfile.TemporaryFile() as redirected:
            child = subprocess.Popen(
                [
                    binary,
                    "--exact",
                    {
                        "normal": "apps::tests::t01_pty_child",
                        "t03": "apps::tests::t03_pty_child",
                        "matrix": "apps::tests::t03_ownership_child",
                    }[mode],
                    "--ignored",
                    "--nocapture",
                    "--quiet",
                ],
                stdin=slave,
                stdout=slave if stream == "stdout" else redirected,
                stderr=slave if stream == "stderr" else redirected,
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


if __name__ == "__main__":
    for selected_stream in ("stderr", "stdout"):
        check_stream(
            sys.argv[1], selected_stream, sys.argv[2] if len(sys.argv) > 2 else "normal"
        )
