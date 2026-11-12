"""Offline release-script regressions: python3 -m unittest discover -s .github/scripts/tests."""

from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


class ReleaseScriptTests(unittest.TestCase):
    def test_r01_run_step_uses_selected_tmpdir_and_bash(self) -> None:
        """Catch host-specific paths without changing any host files or environment."""
        bash = shutil.which("bash")
        self.assertIsNotNone(bash, "Bash must be installed to run this regression")
        checker = Path(__file__).resolve().parents[1] / "check-docker-policy.py"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            temp = root / "temp"
            temp.mkdir()
            bin_dir = root / "bin"
            bin_dir.mkdir()
            (bin_dir / "bash").symlink_to(bash)
            # A fresh interpreter isolates tempfile's cached default and the dummy env.
            result = subprocess.run(
                [sys.executable, "-B", "-c", r'''
import os
from pathlib import Path
import runpy
import shutil
import sys

checker = runpy.run_path(sys.argv[1])
output, logs = checker["run_step"]({
    "env": {"TOKEN": checker["TOKEN"]},
    "run": """
test "${HOST_ONLY+x}" = ""
test "$TOKEN" = "OFFLINE_TOKEN_DO_NOT_PRINT"
printf 'ready=true\\n' >> "$GITHUB_OUTPUT"
printf '%s\\n' "$GITHUB_OUTPUT" "$BASH"
""",
}, {})
assert output == "ready=true\n", output
output_path, bash_path = logs.splitlines()
temp = Path(os.environ["TMPDIR"])
assert Path(output_path).parent.parent == temp, "run_step ignored selected TMPDIR"
assert bash_path == shutil.which("bash"), "run_step ignored Bash on PATH"
assert list(temp.iterdir()) == [], "run_step left temporary files behind"
assert checker["TOKEN"] not in output + logs, "dummy credential leaked"
''', str(checker)],
                env={"TMPDIR": str(temp), "PATH": str(bin_dir),
                     "HOST_ONLY": "OFFLINE_SENTINEL"},
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
