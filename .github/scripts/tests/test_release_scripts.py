"""Offline release-script regressions: python3 -m unittest discover -s .github/scripts/tests."""

import os
import json
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile


class ReleaseScriptTests(unittest.TestCase):
    def _r03_workflow(self, name: str) -> dict:
        import yaml

        path = Path(__file__).resolve().parents[2] / f"workflows/{name}.yml"
        self.assertTrue(path.is_file(), f"missing production workflow: {path.name}")
        return yaml.load(path.read_text(), Loader=yaml.BaseLoader)

    def test_r03_workflow_handoff_and_single_trusted_publisher(self) -> None:
        """An independent publisher, ungated upload, or cross-run download must fail."""
        for platform in ("linux", "windows"):
            workflow = self._r03_workflow(f"{platform}-build")
            self.assertEqual(set(workflow["on"]), {"workflow_call", "workflow_dispatch"})
            self.assertEqual(workflow["permissions"], {"contents": "read"})
            self.assertEqual(set(workflow["jobs"]), {"build"})
            self.assertNotIn("secrets.", str(workflow))
            self.assertNotIn("action-gh-release", str(workflow))
            steps = workflow["jobs"]["build"]["steps"]
            checkout = next(s for s in steps if "actions/checkout@" in s.get("uses", ""))
            self.assertEqual(checkout["with"]["ref"], "${{ github.sha }}")
            upload = next(i for i, s in enumerate(steps) if "actions/upload-artifact@" in s.get("uses", ""))
            stage = next(i for i, s in enumerate(steps) if "coordinate-release.py stage" in s.get("run", ""))
            self.assertLess(stage, upload)
            self.assertNotIn("if", steps[stage])
            self.assertNotIn("continue-on-error", steps[stage])
            self.assertNotIn("if", steps[upload])
            self.assertEqual(steps[upload]["with"], {
                "name": f"condor-{platform}-x64-${{{{ github.sha }}}}-${{{{ github.run_id }}}}-${{{{ github.run_attempt }}}}",
                "path": "dist/", "retention-days": "1", "if-no-files-found": "error",
            })
            if platform == "windows":
                gate = next(i for i, s in enumerate(steps) if "--native" in s.get("run", ""))
            else:
                gate = next(i for i, s in enumerate(steps) if s.get("name") == "Smoke-test the archive")
            self.assertLess(gate, stage)
            self.assertNotIn("if", steps[gate])
            self.assertNotIn("continue-on-error", steps[gate])

        workflow = self._r03_workflow("release")
        self.assertEqual(workflow["on"]["push"], {"branches": ["master"], "tags": ["v*"]})
        self.assertIn("workflow_dispatch", workflow["on"])
        self.assertNotIn("concurrency", workflow, "must not cancel an in-flight publisher")
        self.assertEqual(workflow["permissions"], {"contents": "read"})
        jobs = workflow["jobs"]
        self.assertEqual(set(jobs), {"linux", "windows", "release"})
        for platform in ("linux", "windows"):
            self.assertEqual(jobs[platform], {"uses": f"./.github/workflows/{platform}-build.yml"})
        release = jobs["release"]
        self.assertEqual(set(release["needs"]), {"linux", "windows"})
        self.assertEqual(release["permissions"], {"contents": "write"})
        self.assertEqual(release["concurrency"], {
            "group": "condor-release-${{ github.repository }}-${{ github.ref == 'refs/heads/master' && 'latest' || github.ref }}",
            "cancel-in-progress": "false",
        })
        steps = release["steps"]
        self.assertEqual(steps[0]["with"], {"fetch-depth": "0", "ref": "${{ github.sha }}", "persist-credentials": "false"})
        downloads = [s["with"] for s in steps if "actions/download-artifact@" in s.get("uses", "")]
        self.assertEqual(downloads, [{
            "name": f"condor-{p}-x64-${{{{ github.sha }}}}-${{{{ github.run_id }}}}-${{{{ github.run_attempt }}}}",
            "path": f"handoff/{p}",
        } for p in ("linux", "windows")])
        publishers = [s for s in steps if "coordinate-release.py publish" in s.get("run", "")]
        self.assertEqual(len(publishers), 1)
        self.assertEqual(steps[-1], publishers[0])
        self.assertEqual(publishers[0]["env"], {"GH_TOKEN": "${{ github.token }}"})
        self.assertTrue(all("GH_TOKEN" not in str(s) for s in steps[:-1]))

    def test_r03_actual_workflow_gate_denies_failure_manual_and_forks(self) -> None:
        """Evaluate the production gate, not a copied release decision table."""
        script = Path(__file__).resolve().parents[1] / "check-docker-policy.py"
        expression = runpy.run_path(str(script))["expression"]
        gate = " ".join(self._r03_workflow("release")["jobs"]["release"]["if"].split())
        cases = [
            ("rust-av/Av1an", "push", "refs/heads/master", "success", "success", True),
            ("rust-av/Av1an", "push", "refs/tags/v1.2.3", "success", "success", True),
            ("rust-av/Av1an", "push", "refs/heads/master", "failure", "success", False),
            ("rust-av/Av1an", "push", "refs/heads/master", "success", "cancelled", False),
            ("rust-av/Av1an", "push", "refs/heads/master", "skipped", "success", False),
            ("fork/Av1an", "push", "refs/heads/master", "success", "success", False),
            ("fork/Av1an", "push", "refs/tags/v1.2.3", "success", "success", False),
        ]
        cases += [("rust-av/Av1an", "workflow_dispatch", ref, "success", "success", False)
                  for ref in ("refs/heads/feature", "refs/heads/master", "refs/tags/v1.2.3")]
        cases += [("rust-av/Av1an", "push", "refs/heads/feature", "success", "success", False)]
        for repo, event, ref, linux, windows, wanted in cases:
            with self.subTest(repo=repo, event=event, ref=ref, linux=linux, windows=windows):
                context = {"github": SimpleNamespace(repository=repo, event_name=event, ref=ref),
                           "needs": SimpleNamespace(linux=SimpleNamespace(result=linux), windows=SimpleNamespace(result=windows))}
                self.assertEqual(bool(expression(gate, context)), wanted)

    def _r03_fixture(self, base: Path) -> tuple[Path, dict]:
        scripts = Path(__file__).resolve().parents[1]
        helper = scripts / "coordinate-release.py"
        self.assertTrue(helper.is_file(), "missing production handoff/publisher helper")
        repo = base / "repo"
        with patch.dict(os.environ, {"PATH": "/usr/bin:/bin"}, clear=True):
            self._r07_repo(repo)
        destination = repo / ".github/scripts"
        destination.mkdir(parents=True)
        for name in ("coordinate-release.py", "generate-changelog.sh", "linux-release-notes.md"):
            shutil.copyfile(scripts / name, destination / name)
        with patch.dict(os.environ, {"PATH": "/usr/bin:/bin"}, clear=True):
            self._r07_git(repo, "commit", "--allow-empty", "-m", "R03_OLD")
            self._r07_git(repo, "tag", "v0.5.1")
            self._r07_git(repo, "commit", "--allow-empty", "-m", "R03_NEW")
            self._r07_git(repo, "tag", "v1.2.3")
            sha = self._r07_git(repo, "rev-parse", "HEAD")
        bin_dir = base / "bin"
        bin_dir.mkdir()
        # The only remote boundary is this owned, fail-closed executable. No real gh.
        gh = bin_dir / "gh"
        gh.write_text(f"#!{sys.executable}\n" + r'''
import json, os, sys
from pathlib import Path
a = sys.argv[1:]
p = Path(os.environ["FAKE_STATE"])
s = json.loads(p.read_text())
s["calls"].append(a)
def save(): p.write_text(json.dumps(s))
def value(flag): return a[a.index(flag) + 1]
if a[0] == "api":
    if s.get("deny_api"):
        save(); print("gh: Forbidden (HTTP 403)", file=sys.stderr); sys.exit(1)
    endpoint = a[1]
    prefix = "repos/rust-av/Av1an/"
    assert endpoint.startswith(prefix), a
    endpoint = endpoint[len(prefix):]
    method = value("--method") if "--method" in a else "GET"
    if endpoint == "git/ref/heads/master" and method == "GET":
        print(json.dumps({"object": {"sha": s["master"]}}))
    elif endpoint == "commits/refs%2Ftags%2Fv1.2.3" and method == "GET":
        print(json.dumps({"sha": s["tag_commit"]}))
    elif endpoint == "git/ref/tags/latest" and method == "GET":
        if s["latest"] is None:
            save(); print("gh: Not Found (HTTP 404)", file=sys.stderr); sys.exit(1)
        print(json.dumps({"object": {"sha": s["latest"]}}))
    elif endpoint in ("git/refs/tags/latest", "git/refs") and method in ("PATCH", "POST"):
        fields = [a[i + 1] for i, v in enumerate(a[:-1]) if v in ("-f", "-F")]
        if method == "PATCH": assert "force=true" in fields, a
        else: assert "ref=refs/tags/latest" in fields, a
        s["latest"] = next(v[4:] for v in fields if v.startswith("sha="))
    elif endpoint.startswith("releases/tags/") and method == "GET":
        tag = endpoint.removeprefix("releases/tags/")
        if tag not in s["releases"]:
            save(); print("gh: Not Found (HTTP 404)", file=sys.stderr); sys.exit(1)
        print(json.dumps({"assets": [{"id": 7, "name": "condor.exe"}]}))
    elif endpoint == "releases/assets/7" and method == "DELETE":
        pass
    else: raise AssertionError(a)
elif a[:2] in (["release", "create"], ["release", "edit"]):
    assert value("--repo") == "rust-av/Av1an", a
    if a[1] == "create": assert "--verify-tag" in a, a
    s["releases"][a[2]] = {"body": Path(value("--notes-file")).read_text()}
elif a[:2] == ["release", "upload"]:
    assert value("--repo") == "rust-av/Av1an" and "--clobber" in a, a
    if s.get("fail_upload"):
        save(); sys.exit(2)
    files = a[a.index("--") + 1:]
    s["releases"][a[2]]["assets"] = {Path(f).name: Path(f).read_bytes().hex() for f in files}
else: raise AssertionError(a)
save()
''')
        gh.chmod(0o755)
        (base / "state.json").write_text(json.dumps({
            "master": sha, "tag_commit": sha, "latest": "0" * 40,
            "releases": {"latest": {}}, "calls": [],
        }))
        temp = base / "temp"
        temp.mkdir()
        # No BASH_ENV, exported functions, credentials, HOME config, or GIT_* routing.
        env = {"PATH": f"{bin_dir}:/usr/bin:/bin", "TMPDIR": str(temp),
               "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
               "GIT_CEILING_DIRECTORIES": str(base), "GH_TOKEN": "OFFLINE_R03_INERT",
               "GITHUB_SHA": sha, "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2",
               "GITHUB_REPOSITORY": "rust-av/Av1an", "GITHUB_EVENT_NAME": "push",
               "GITHUB_REF": "refs/heads/master", "FAKE_STATE": str(base / "state.json")}
        self._r03_stage(repo, env)
        return repo, env

    def _r03_stage(self, repo: Path, env: dict) -> None:
        for platform in ("linux", "windows"):
            (repo / "target").mkdir(exist_ok=True)
            schema = repo / "california-condor/configuration.schema.json"
            schema.parent.mkdir(exist_ok=True)
            schema.write_bytes(b'\xef\xbb\xbf{\r\n  "properties": {}, "type": "object"\r\n}\r\n'
                               if platform == "windows" else b'{"type":"object","properties":{}}\n')
            with zipfile.ZipFile(repo / f"target/condor-{platform}-x64.zip", "w") as archive:
                archive.writestr("configuration.schema.json", schema.read_bytes())
                archive.writestr("condor.exe" if platform == "windows" else "condor", platform + env["GITHUB_SHA"])
            steps = self._r03_workflow(f"{platform}-build")["jobs"]["build"]["steps"]
            stage = next(s["run"] for s in steps if "coordinate-release.py stage" in s.get("run", ""))
            # Windows uses the same stdlib CLI; PowerShell/native acceptance remains CI-only.
            result = subprocess.run(["bash", "--noprofile", "--norc", "-e", "-c", stage], cwd=repo, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            target = repo / f"handoff/{platform}"
            target.parent.mkdir(exist_ok=True)
            (repo / "dist").rename(target)

    def _r03_publish(self, repo: Path, env: dict) -> subprocess.CompletedProcess[str]:
        steps = self._r03_workflow("release")["jobs"]["release"]["steps"]
        command = next(s["run"] for s in steps if "coordinate-release.py publish" in s.get("run", ""))
        return subprocess.run(["bash", "--noprofile", "--norc", "-e", "-c", command], cwd=repo, env=env, capture_output=True, text=True)

    def test_r03_publication_body_assets_tag_and_serialized_stale_order(self) -> None:
        """Run production commands with real owned Git history and an inert GH boundary."""
        with tempfile.TemporaryDirectory(prefix="r03 publish ") as directory:
            base = Path(directory)
            repo, env = self._r03_fixture(base)
            state_path = base / "state.json"
            for ref in ("refs/heads/master", "refs/tags/v1.2.3"):
                with self.subTest(ref=ref):
                    env["GITHUB_REF"] = ref
                    result = self._r03_publish(repo, env)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    state = json.loads(state_path.read_text())
                    tag = "latest" if ref.endswith("master") else "v1.2.3"
                    release = state["releases"][tag]
                    self.assertIn(f"Latest build from {env['GITHUB_SHA']}\n", release["body"])
                    self.assertIn("- R03_NEW\n", release["body"])
                    notes = (repo / ".github/scripts/linux-release-notes.md").read_text()
                    self.assertIn(notes, release["body"])
                    self.assertEqual(set(release["assets"]), {"condor-linux-x64.zip", "condor-windows-x64.zip", "configuration.schema.json"})
                    for platform in ("linux", "windows"):
                        name = f"condor-{platform}-x64.zip"
                        self.assertEqual(release["assets"][name], (repo / f"handoff/{platform}/{name}").read_bytes().hex())
                    self.assertEqual(state["latest"], env["GITHUB_SHA"])
                    if ref.startswith("refs/tags/"):
                        self.assertFalse(any("git/refs" in " ".join(a) for a in state["calls"]))
                    state["calls"] = []
                    state_path.write_text(json.dumps(state))

            # A newer run publishes first, then the slow old build enters the lock.
            old_env = env.copy()
            old_env["GITHUB_REF"] = "refs/heads/master"
            (repo / "handoff").rename(repo / "old-handoff")
            with patch.dict(os.environ, env, clear=True):
                self._r07_git(repo, "commit", "--allow-empty", "-m", "R03_NEWER")
                env["GITHUB_SHA"] = self._r07_git(repo, "rev-parse", "HEAD")
            env["GITHUB_RUN_ID"] = "124"
            env["GITHUB_REF"] = "refs/heads/master"
            self._r03_stage(repo, env)
            state = json.loads(state_path.read_text())
            state["master"] = env["GITHUB_SHA"]
            state_path.write_text(json.dumps(state))
            result = self._r03_publish(repo, env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            state = json.loads(state_path.read_text())
            self.assertEqual(state["latest"], env["GITHUB_SHA"])
            self.assertIn(f"Latest build from {env['GITHUB_SHA']}\n", state["releases"]["latest"]["body"])
            for platform in ("linux", "windows"):
                name = f"condor-{platform}-x64.zip"
                self.assertEqual(state["releases"]["latest"]["assets"][name], (repo / f"handoff/{platform}/{name}").read_bytes().hex())
            before = state["releases"]
            state["calls"] = []
            state_path.write_text(json.dumps(state))
            (repo / "handoff").rename(repo / "new-handoff")
            (repo / "old-handoff").rename(repo / "handoff")
            with patch.dict(os.environ, env, clear=True):
                self._r07_git(repo, "checkout", "--detach", old_env["GITHUB_SHA"])
            result = self._r03_publish(repo, old_env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("stale", result.stdout.lower())
            state = json.loads(state_path.read_text())
            self.assertEqual(state["releases"], before)
            self.assertEqual(state["calls"], [["api", "repos/rust-av/Av1an/git/ref/heads/master"]])
            self.assertEqual(list((base / "temp").iterdir()), [])

    def test_r03_rejects_invalid_handoff_and_untrusted_runs_before_write(self) -> None:
        """Removing a provenance/schema/HEAD/policy check must expose a fake write."""
        cases = ("mixed-sha", "wrong-run", "wrong-attempt", "wrong-platform", "missing-platform",
                 "schema-mismatch", "checkout-sha", "tag-moved", "api-failure", "manual-master", "manual-tag", "manual-feature", "fork", "fork-tag")
        with tempfile.TemporaryDirectory(prefix="r03 reject ") as directory:
            for case in cases:
                with self.subTest(case=case):
                    base = Path(directory) / case
                    base.mkdir()
                    repo, env = self._r03_fixture(base)
                    manifest_path = repo / "handoff/windows/provenance.json"
                    manifest = json.loads(manifest_path.read_text())
                    key = {"mixed-sha": "sha", "wrong-run": "run_id", "wrong-attempt": "run_attempt", "wrong-platform": "platform"}.get(case)
                    if key:
                        manifest[key] = "0" * 40 if key == "sha" else "wrong"
                        manifest_path.write_text(json.dumps(manifest))
                    elif case == "missing-platform":
                        (repo / "handoff/windows/condor-windows-x64.zip").unlink()
                    elif case == "schema-mismatch":
                        (repo / "handoff/windows/configuration.schema.json").write_text('{"type":"string"}')
                        with zipfile.ZipFile(repo / "handoff/windows/condor-windows-x64.zip", "w") as archive:
                            archive.writestr("configuration.schema.json", '{"type":"string"}')
                    elif case == "checkout-sha":
                        env["GITHUB_SHA"] = "0" * 40
                    elif case in ("tag-moved", "api-failure"):
                        state_path = base / "state.json"
                        state = json.loads(state_path.read_text())
                        if case == "tag-moved":
                            env["GITHUB_REF"] = "refs/tags/v1.2.3"
                            state["tag_commit"] = "f" * 40
                        else:
                            state["deny_api"] = True
                        state_path.write_text(json.dumps(state))
                    elif case.startswith("manual"):
                        env["GITHUB_EVENT_NAME"] = "workflow_dispatch"
                        env["GITHUB_REF"] = {"manual-master": "refs/heads/master", "manual-tag": "refs/tags/v1.2.3", "manual-feature": "refs/heads/feature"}[case]
                    elif case.startswith("fork"):
                        env["GITHUB_REPOSITORY"] = "fork/Av1an"
                        if case == "fork-tag":
                            env["GITHUB_REF"] = "refs/tags/v1.2.3"
                    result = self._r03_publish(repo, env)
                    if case.startswith(("manual", "fork")):
                        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    else:
                        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                        self.assertRegex(result.stderr.lower(), "sha|run|attempt|platform|schema|archive|tag|provenance|403")
                    state = json.loads((base / "state.json").read_text())
                    self.assertEqual(state["releases"], {"latest": {}})
                    self.assertFalse(any("--method" in a or a[0] == "release" for a in state["calls"]))
                    self.assertEqual(list((base / "temp").iterdir()), [])

    def test_r03_create_latest_and_surface_partial_remote_failure(self) -> None:
        """A missing managed tag is created; failed uploads never report success."""
        with tempfile.TemporaryDirectory(prefix="r03 remote ") as directory:
            base = Path(directory)
            repo, env = self._r03_fixture(base)
            path = base / "state.json"
            state = json.loads(path.read_text())
            state.update(latest=None, releases={})
            path.write_text(json.dumps(state))
            result = self._r03_publish(repo, env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            state = json.loads(path.read_text())
            self.assertEqual(state["latest"], env["GITHUB_SHA"])
            self.assertEqual(len(state["releases"]["latest"]["assets"]), 3)
            state["fail_upload"] = True
            path.write_text(json.dumps(state))
            result = self._r03_publish(repo, env)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn("Published", result.stdout)
            self.assertEqual(list((base / "temp").iterdir()), [])

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
