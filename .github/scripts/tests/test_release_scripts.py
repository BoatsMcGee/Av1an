"""Offline release-script regressions: python3 -m unittest discover -s .github/scripts/tests."""

import os
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile


class ReleaseScriptTests(unittest.TestCase):
    def _r07_git(self, repo: Path, *args: str) -> str:
        # SECURITY: Hook/wrapper Git routing must never redirect fixture writes.
        return subprocess.run(
            ["git", *args], cwd=repo, check=True, capture_output=True, text=True,
            env={**{key: value for key, value in os.environ.items()
                    if not key.startswith("GIT_")}, "GIT_CONFIG_NOSYSTEM": "1",
                 "GIT_CONFIG_GLOBAL": os.devnull,
                 "GIT_CEILING_DIRECTORIES": str(repo.parent.resolve()),
                 "GIT_AUTHOR_DATE": "2000-01-01T00:00:00+0000",
                 "GIT_COMMITTER_DATE": "2000-01-01T00:00:00+0000"},
        ).stdout.strip()

    def _r07_repo(self, repo: Path) -> None:
        repo.mkdir()
        self._r07_git(repo, "init", "--initial-branch=main")
        self._r07_git(repo, "config", "user.name", "Release Fixture")
        self._r07_git(repo, "config", "user.email", "fixture@example.invalid")
        self._r07_git(repo, "config", "commit.gpgsign", "false")
        self._r07_git(repo, "config", "tag.gpgsign", "false")

    def _r07_changelog(
        self, repo: Path, output: Path, *args: str,
    ) -> subprocess.CompletedProcess[str]:
        script = Path(__file__).resolve().parents[1] / "generate-changelog.sh"
        return subprocess.run(
            ["bash", str(script), *args], cwd=repo, capture_output=True, text=True,
            env={**{key: value for key, value in os.environ.items()
                    if not key.startswith("GIT_")}, "GITHUB_SHA": "OFFLINE_R07_SHA",
                 "GITHUB_OUTPUT": str(output), "GIT_CONFIG_NOSYSTEM": "1",
                 "GIT_CONFIG_GLOBAL": os.devnull,
                 "GIT_CEILING_DIRECTORIES": str(repo.parent.resolve())},
        )

    def test_r07_helpers_isolate_git_environment_and_parent_discovery(self) -> None:
        """Inherited routing must not write to a containing repo or leak its history."""
        with tempfile.TemporaryDirectory(prefix="r07 isolation ") as directory:
            base = Path(directory)
            env = {key: os.environ[key] for key in ("PATH", "SYSTEMROOT")
                   if key in os.environ}
            env["TMPDIR"] = str(base)
            # SECURITY: Never route the unsafe RED helpers to the actual project.
            with patch.dict(os.environ, env, clear=True):
                outer = base / "outer repo"
                self._r07_repo(outer)
                (outer / "sentinel").write_text("preserve outer index and worktree\n")
                self._r07_git(outer, "add", "--", "sentinel")
                self._r07_git(outer, "commit", "-m", "R07_OUTER")
                self._r07_git(outer, "tag", "v9.0.0")
                self._r07_git(outer, "config", "user.name", "Outer Sentinel")

                def outer_state() -> dict[str, bytes]:
                    git_dir = outer / ".git"
                    paths = [git_dir / name for name in
                             ("HEAD", "config", "index", "packed-refs")]
                    paths.extend((git_dir / "refs").rglob("*"))
                    return {str(path.relative_to(git_dir)): path.read_bytes()
                            for path in paths if path.is_file()}

                before = outer_state()
                inner = outer / "inner repo"
                output = base / "inner output"
                polluted = {
                    "GIT_DIR": str(outer / ".git"),
                    "GIT_COMMON_DIR": str(outer / ".git"),
                    "GIT_WORK_TREE": str(outer),
                    "GIT_INDEX_FILE": str(outer / ".git/index"),
                    "GIT_OBJECT_DIRECTORY": str(outer / ".git/objects"),
                    "GIT_ALTERNATE_OBJECT_DIRECTORIES": str(outer / ".git/objects"),
                    "GIT_CONFIG": str(outer / ".git/config"),
                    "GIT_CONFIG_COUNT": "1",
                    "GIT_CONFIG_KEY_0": "user.name",
                    "GIT_CONFIG_VALUE_0": "Injected Outer Identity",
                    "GIT_AUTHOR_NAME": "Injected Outer Author",
                    "GIT_CEILING_DIRECTORIES": str(base),
                }
                with patch.dict(os.environ, polluted):
                    self._r07_repo(inner)
                    self._r07_git(inner, "commit", "--allow-empty", "-m", "R07_INNER")
                    self._r07_git(inner, "tag", "v0.5.1")
                    result = self._r07_changelog(inner, output)

                self.assertEqual(outer_state(), before, "outer HEAD/config/index/refs changed")
                self.assertTrue((inner / ".git").is_dir())
                self.assertEqual(self._r07_git(inner, "log", "-1", "--format=%s"), "R07_INNER")
                self.assertEqual(self._r07_git(inner, "tag", "--list"), "v0.5.1")
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                body = output.read_text()
                self.assertEqual(
                    [line[2:] for line in body.splitlines() if line.startswith("- ")],
                    ["R07_INNER"],
                )
                self.assertIn(": R07_INNER (Release Fixture)\n", body)

                not_git = outer / "not git"
                not_git.mkdir()
                with self.assertRaises(subprocess.CalledProcessError):
                    self._r07_git(not_git, "rev-parse", "--show-toplevel")
                output.write_text("existing=true\n")
                result = self._r07_changelog(not_git, output)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(output.read_text(), "existing=true\n")
                self.assertEqual(outer_state(), before)

    def test_r07_previous_reachable_version_excludes_head_aliases_and_latest(self) -> None:
        """Current, moving, non-version, or numerically largest tags are not baselines."""
        root = ("commit", "--allow-empty", "-m", "R07_ROOT")
        change = ("commit", "--allow-empty", "-m", "R07_CHANGE")
        head = ("commit", "--allow-empty", "-m", "R07_HEAD")
        previous = ("tag", "-a", "v0.5.1", "-m", "previous release")
        cases = (
            ("firsttag", [root, change, ("tag", "v0.5.1")],
             ["R07_CHANGE", "R07_ROOT"], "the beginning of history"),
            ("secondtag", [root, previous, change, ("tag", "v0.5.2")],
             ["R07_CHANGE"], "v0.5.1"),
            ("masteraftertag", [root, previous, change, head],
             ["R07_HEAD", "R07_CHANGE"], "v0.5.1"),
            ("stale-latest-ancestor", [root, previous, change, ("tag", "latest"), head],
             ["R07_HEAD", "R07_CHANGE"], "v0.5.1"),
            ("moving-latest-head", [root, previous, change, ("tag", "latest")],
             ["R07_CHANGE"], "v0.5.1"),
            ("multiple-head-aliases", [root, previous, change,
             ("tag", "v0.5.2"), ("tag", "-a", "v0.5.3", "-m", "alias"),
             ("tag", "latest")], ["R07_CHANGE"], "v0.5.1"),
            ("untagged-first-history", [root, change],
             ["R07_CHANGE", "R07_ROOT"], "the beginning of history"),
            ("ignore-nonrelease-tags", [root, previous, change,
             ("tag", "build-2026"), ("tag", "v9.9.9-preview"),
             ("tag", "v8.8.8junk"), ("tag", "v7.7.7.7"), head],
             ["R07_HEAD", "R07_CHANGE"], "v0.5.1"),
            ("nearest-not-largest", [root, ("tag", "v9.0.0"), change, previous, head],
             ["R07_HEAD"], "v0.5.1"),
        )
        with tempfile.TemporaryDirectory(prefix="r07 git ") as directory:
            base = Path(directory)
            for name, commands, expected, baseline in cases:
                with self.subTest(history=name):
                    repo = base / name
                    self._r07_repo(repo)
                    for command in commands:
                        self._r07_git(repo, *command)
                    output = base / f"{name} output"

                    result = self._r07_changelog(repo, output)

                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    body = output.read_text()
                    self.assertEqual(
                        [line[2:] for line in body.splitlines() if line.startswith("- ")],
                        expected,
                    )
                    self.assertIn(f"Changes since {baseline}:\n", body)
                    self.assertNotIn("No changes", body)

    def test_r07_merge_uses_release_on_second_parent_not_unreachable_tag(self) -> None:
        """Walking only HEAD's first parent loses the reachable side-branch release."""
        with tempfile.TemporaryDirectory(prefix="r07 merge ") as directory:
            base = Path(directory)
            repo = base / "repo"
            self._r07_repo(repo)
            for command in (
                ("commit", "--allow-empty", "-m", "R07_ROOT"),
                ("branch", "unreachable"),
                ("checkout", "-b", "side"),
                ("commit", "--allow-empty", "-m", "R07_PREVIOUS"),
                ("tag", "v0.5.1"),
                ("commit", "--allow-empty", "-m", "R07_SIDE"),
                ("checkout", "main"),
                ("commit", "--allow-empty", "-m", "R07_MAIN"),
                ("merge", "--no-ff", "side", "-m", "R07_MERGE"),
                ("tag", "v0.5.2"),
                ("checkout", "unreachable"),
                ("commit", "--allow-empty", "-m", "R07_UNREACHABLE"),
                ("tag", "v99.0.0"),
                ("checkout", "main"),
            ):
                self._r07_git(repo, *command)
            output = base / "output"

            result = self._r07_changelog(repo, output)

            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            body = output.read_text()
            self.assertCountEqual(
                [line[2:] for line in body.splitlines() if line.startswith("- ")],
                ["R07_MERGE", "R07_MAIN", "R07_SIDE"],
            )
            self.assertIn("Changes since v0.5.1:\n", body)

    def test_r07_body_protocol_notes_and_first_ten_commit_boundary(self) -> None:
        """Keep every subject exactly once, correct overflow counts, and spaced paths."""
        with tempfile.TemporaryDirectory(prefix="r07 body ") as directory:
            base = Path(directory)
            for count in (1, 10, 11, 12):
                with self.subTest(commits=count):
                    repo = base / f"repo {count}"
                    self._r07_repo(repo)
                    self._r07_git(repo, "commit", "--allow-empty", "-m", "R07_BASE")
                    self._r07_git(repo, "tag", "v0.5.1")
                    subjects = [f"R07 change {i:02d} `quotes` & spaces" for i in range(count)]
                    for subject in subjects:
                        self._r07_git(repo, "commit", "--allow-empty", "-m", subject)
                    notes = base / "release notes with spaces.md"
                    notes.write_text("Offline release notes\nverbatim, no final newline")
                    output = base / f"output {count}"
                    output.write_text("existing=true\n")

                    result = self._r07_changelog(repo, output, str(notes))

                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    body = output.read_text()
                    self.assertTrue(body.startswith(
                        "existing=true\nbody<<CHANGELOG_EOF\nLatest build from OFFLINE_R07_SHA\n"
                    ))
                    self.assertIn(f": {subjects[-1]} (Release Fixture)\n", body)
                    self.assertEqual(
                        [line[2:] for line in body.splitlines() if line.startswith("- ")],
                        subjects[::-1],
                    )
                    inline = body.split("<details>")[0]
                    self.assertEqual(inline.count("\n- R07 change"), min(count, 10))
                    if count > 10:
                        self.assertIn(f"<summary>... and {count - 10} more commits</summary>", body)
                        self.assertEqual(body.count("</details>"), 1)
                    else:
                        self.assertNotIn("<details>", body)
                    self.assertTrue(body.endswith(f"\n---\n\n{notes.read_text()}\nCHANGELOG_EOF\n"))

    def test_r07_shallow_and_git_errors_fail_before_output_mutation(self) -> None:
        """Shallow roots must not masquerade as first releases, nor Git errors as success."""
        with tempfile.TemporaryDirectory(prefix="r07 errors ") as directory:
            base = Path(directory)
            source = base / "source"
            self._r07_repo(source)
            self._r07_git(source, "commit", "--allow-empty", "-m", "R07_ROOT")
            self._r07_git(source, "tag", "v0.5.1")
            self._r07_git(source, "commit", "--allow-empty", "-m", "R07_CHANGE")
            self._r07_git(source, "clone", "--depth=1", source.as_uri(), str(base / "shallow"))
            self._r07_repo(base / "empty")
            (base / "not-git").mkdir()
            for name in ("shallow", "empty", "not-git"):
                with self.subTest(history=name):
                    output = base / f"{name} output"
                    output.write_text("existing=true\n")

                    result = self._r07_changelog(base / name, output)

                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertEqual(output.read_text(), "existing=true\n")
                    self.assertNotIn("No changes", result.stdout + result.stderr)
                    if name == "shallow":
                        self.assertIn("shallow", result.stderr.lower())
                        self.assertIn("fetch-depth: 0", result.stderr)

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
