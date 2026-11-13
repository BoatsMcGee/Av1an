"""Offline release-script regressions: python3 -m unittest discover -s .github/scripts/tests."""

from pathlib import Path
import re
import runpy
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile


class ReleaseScriptTests(unittest.TestCase):
    def test_r02_msvc_redist_and_dynamic_ffms_contract(self) -> None:
        """Require selected VS app-local CRT staging, not a host CRT or static FFMS claim."""
        root = Path(__file__).resolve().parents[3]
        stage = (root / ".github/scripts/stage-native-libs.ps1").read_text()
        package = (root / ".github/scripts/package-release.ps1").read_text()
        with self.subTest(contract="VS redist source"):
            self.assertIn("vswhere.exe", stage)
            self.assertIn("Microsoft.VCRedistVersion.default.txt", stage)
            self.assertIn("Microsoft.VC14*.CRT", stage)
            self.assertIn("VC\\Redist\\MSVC", stage)
            self.assertIn("Get-ChildItem -LiteralPath $crtDir -File -Filter '*.dll'", stage)
            self.assertIn("Copy-Item -LiteralPath $runtime.FullName", stage)
            self.assertIn("$runtime.Length -eq 0", stage)
        for name in ("vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"):
            with self.subTest(runtime=name):
                self.assertIn(f"'{name}'", stage)
                self.assertIn(f"'{name}'", package)
        for path in ("california-condor/docs/compiling.md", "av-metrics-vship/README.md",
                     ".github/scripts/stage-native-libs.sh"):
            with self.subTest(path=path):
                text = (root / path).read_text()
                self.assertNotRegex(text, r"(?is)FFMS2.{0,100}static|links FFMS2\s+statically")
                self.assertNotIn("FFMS2 and VapourSynth are linked into the executable", text)

    def test_r02_staging_packaging_and_native_gate_are_connected(self) -> None:
        """Dropping FFMS2, build DLLs, or the pre-upload native gate breaks this contract."""
        import yaml

        scripts = Path(__file__).resolve().parents[1]
        stage = (scripts / "stage-native-libs.ps1").read_text()
        package = (scripts / "package-release.ps1").read_text()
        contents = re.search(r"\$Contents = @\((.*?)\n\)", package, re.S)[1]
        self.assertIn("'ffms2.dll'", contents)
        self.assertIn("$env:FFMS_LIB_DIR", stage)
        self.assertIn("'ffms2.dll'", stage)
        self.assertIn(".Length -eq 0", stage)
        self.assertRegex(package, r"Get-ChildItem .*?-Filter ['\"]\*\.dll['\"]")
        self.assertIn("verify-windows-bundle.py", package)
        self.assertNotIn('"*$leaf"', package)
        workflow = yaml.safe_load((scripts.parent / "workflows/windows-build.yml").read_text())
        steps = workflow["jobs"]["build"]["steps"]
        packaging = next(i for i, s in enumerate(steps) if "package-release.ps1" in s.get("run", ""))
        gate = next((i for i, s in enumerate(steps) if "verify-windows-bundle.py" in s.get("run", "") and "--native" in s["run"]), None)
        self.assertIsNotNone(gate, "release needs a native clean-extracted Windows gate")
        self.assertGreater(gate, packaging)
        self.assertNotIn("if", steps[gate])
        self.assertFalse(steps[gate].get("continue-on-error", False))
        for i, step in enumerate(steps):
            if "action-gh-release" in step.get("uses", ""):
                self.assertGreater(i, gate)

    def test_r02_archive_rejects_missing_empty_and_wrong_relative_entries(self) -> None:
        """Leaf aliases and absent runtime DLLs must not pass production validation."""
        helper = Path(__file__).resolve().parents[1] / "verify-windows-bundle.py"
        self.assertTrue(helper.is_file(), "missing production archive verifier")
        verify = runpy.run_path(str(helper))["verify_archive"]
        entries = {
            "condor.exe": b"exe", "ffms2.dll": b"dll", "libvmaf.dll": b"dll",
            "libvship.dll": b"dll", "fmetrics.dll": b"dll",
            "libgcc_s_seh-1.dll": b"dll", "libstdc++-6.dll": b"dll",
            "libwinpthread-1.dll": b"dll", "model/vmaf_v0.6.1.json": b"{}",
            "model/transnetv2.onnx": b"model", "onnxruntime.dll": b"ort",
            "DirectML.dll": b"directml", "model/nested/example.json": b"{}",
            "vcruntime140.dll": b"crt", "vcruntime140_1.dll": b"crt",
            "msvcp140.dll": b"cpp", "msvcp140_2.dll": b"companion",
        }
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "bundle.zip"
            for change in (None, "missing", "wrong-layout", "wrong-dll-layout", "empty", "missing-ort", "missing-directml",
                           "vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"):
                with self.subTest(change=change):
                    files = entries.copy()
                    if change == "missing":
                        del files["ffms2.dll"]
                    elif change == "wrong-layout":
                        files["wrong/vmaf_v0.6.1.json"] = files.pop("model/vmaf_v0.6.1.json")
                    elif change == "wrong-dll-layout":
                        files["wrong/ffms2.dll"] = files.pop("ffms2.dll")
                    elif change == "empty":
                        files["ffms2.dll"] = b""
                    elif change == "missing-ort":
                        del files["onnxruntime.dll"]
                    elif change == "missing-directml":
                        del files["DirectML.dll"]
                    elif change in ("vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"):
                        del files[change]
                    with zipfile.ZipFile(archive, "w") as bundle:
                        for name, content in files.items():
                            bundle.writestr(name, content)
                    if change is None:
                        verify(archive, ["onnxruntime.dll", "DirectML.dll", "model/nested/example.json"])
                    else:
                        with self.assertRaises(ValueError):
                            verify(archive, ["onnxruntime.dll"])

    def test_r02_real_import_policy_fails_closed(self) -> None:
        """Missing ORT/DirectML/CRT imports fail; Vulkan is driver-only for libvship."""
        helper = Path(__file__).resolve().parents[1] / "verify-windows-bundle.py"
        self.assertTrue(helper.is_file(), "missing production import verifier")
        module = runpy.run_path(str(helper))
        output = """File: condor.exe
Format: COFF-x86-64
Import {
  Name: ffms2.dll
  Symbol: FFMS_Init (0)
}
DelayImport {
  Name: onnxruntime.dll
  Symbol: OrtGetApiBase (0)
}
"""
        self.assertEqual(module["import_names"](output), ["ffms2.dll", "onnxruntime.dll"])
        with self.assertRaises(ValueError):
            module["import_names"]("unrecognized tool output")
        imports = {"condor.exe": ["ffms2.dll", "onnxruntime.dll", "KERNEL32.dll", "MSVCP140.dll", "VCRUNTIME140_1.dll"],
                   "onnxruntime.dll": ["DirectML.dll"], "DirectML.dll": ["VCRUNTIME140.dll"],
                   "ffms2.dll": ["WS2_32.dll", "ole32.dll", "USER32.dll", "bcrypt.dll",
                                 "CRYPT32.dll", "KERNEL32.dll", "ADVAPI32.dll"],
                   "libvship.dll": ["vulkan-1.dll", "KERNEL32.dll"]}
        check = module["check_imports"]
        bundled = {"condor.exe", "ffms2.dll", "onnxruntime.dll", "DirectML.dll", "vcruntime140.dll", "libvship.dll", "msvcp140.dll", "vcruntime140_1.dll"}
        check(imports, bundled)
        for missing in ("ffms2.dll", "onnxruntime.dll", "DirectML.dll", "vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"):
            with self.subTest(missing=missing), self.assertRaisesRegex(ValueError, "[Mm]issing"):
                check(imports, bundled - {missing})
        for dependency in ("unknown.dll", "vulkan-1.dll", "avcodec.dll"):
            with self.subTest(dependency=dependency), self.assertRaises(ValueError):
                check({"condor.exe": [dependency]}, {"condor.exe"})
        try:
            check({"condor.exe": ["pdh.dll"]}, {"condor.exe"})
        except ValueError as error:
            self.fail(f"Windows inbox PDH must be accepted: {error}")

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
