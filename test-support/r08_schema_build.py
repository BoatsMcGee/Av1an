#!/usr/bin/env python3
"""Offline acceptance check: python3 test-support/r08_schema_build.py.

Run outside Cargo tests: one owned source/Git fixture builds the real consumers,
then reuses its compiled build script for the description/repository tables.
Requires the normal Condor build/runtime prerequisites, including VapourSynth.
"""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = "https://github.com/rust-av/Av1an/releases/download"
ASSET = "/configuration.schema.json"


def run(args: list[str], cwd: Path, env: dict[str, str]) -> str:
    """Keep subprocess diagnostics on failure, including Cargo's JSON output."""
    result = subprocess.run(args, cwd=cwd, env=env, text=True, capture_output=True)
    if result.stderr:
        print(result.stderr, end="", flush=True)
    if result.returncode:
        print(result.stdout, end="", flush=True)
        raise subprocess.CalledProcessError(result.returncode, args)
    return result.stdout


def check(fixture: Path) -> None:
    """Catch missing emission, permissive tag selection and stale fork URLs."""
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("VERGEN_", "GIT_"))
        and key not in ("GITHUB_REPOSITORY", "SOURCE_DATE_EPOCH")
    }
    env.update(
        CARGO_TARGET_DIR=str(ROOT / "target"),
        CARGO_NET_OFFLINE="true",
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL=os.devnull,
    )
    package = fixture / "california-condor"
    package.mkdir()
    for name in ("Cargo.toml", "build.rs"):
        shutil.copy2(ROOT / "california-condor" / name, package / name)
    shutil.copytree(ROOT / "california-condor/src", package / "src")
    shutil.copy2(ROOT / "Cargo.lock", fixture / "Cargo.lock")
    workspace = (ROOT / "Cargo.toml").read_text().split("[workspace.dependencies]", 1)[1]
    workspace = workspace.replace('path = "./andean-condor"',
                                  f'path = "{ROOT / "andean-condor"}"')
    (fixture / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["california-condor"]\nresolver = "3"\n'
        + "[workspace.dependencies]" + workspace
    )
    (fixture / ".gitignore").write_text("/Cargo.lock\n/source.vpy\n/work/\n")
    (package / "src/bin/r08-config-check.rs").write_text('''
use california_condor::configuration::Configuration;
use std::path::Path;
fn main() -> anyhow::Result<()> {
    let config = Configuration::new(
        Path::new("source.vpy"), Path::new("work/out.mkv"),
        Some(Path::new("work")), None, None,
    )?;
    println!("{}", serde_json::to_string(&config)?);
    Ok(())
}
''')
    (fixture / "source.vpy").write_text(
        "import vapoursynth as vs\n"
        "vs.core.std.BlankClip(width=16, height=16, length=1, "
        "format=vs.YUV420P8).set_output()\n"
    )
    (fixture / "work").mkdir()

    def git(*args: str) -> str:
        return run(["git", "-c", "user.name=R08 fixture", "-c",
                    "user.email=r08@example.invalid", "-c", "commit.gpgsign=false",
                    *args], fixture, env).strip()

    git("init", "-b", "fixture")
    git("add", ".")
    git("commit", "-m", "owned schema fixture")
    git("tag", "v1.2.3")

    def build(repository: str | None, expected: str) -> tuple[Path, Path]:
        build_env = dict(env)
        if repository is not None:
            build_env["GITHUB_REPOSITORY"] = repository
        messages = run([
            "cargo", "build", "--offline", "--profile", "ci", "-p",
            "california-condor", "--bin", "generate-schema", "--bin",
            "r08-config-check", "--message-format=json",
        ], fixture, build_env)
        rows = [json.loads(line) for line in messages.splitlines()]
        emitted = next(row for row in rows if row["reason"] == "build-script-executed"
                       and "california-condor" in row["package_id"])
        values = dict(emitted["env"])
        print(f"emission: {values['VERGEN_GIT_DESCRIBE']} -> "
              f"{values['CONDOR_SCHEMA_URL']}", flush=True)
        consumer_urls = []
        for binary, field in (("generate-schema", "$id"),
                              ("r08-config-check", "$schema")):
            artifact = next(row["executable"] for row in rows
                            if row["reason"] == "compiler-artifact"
                            and row["target"]["name"] == binary)
            output = json.loads(run([artifact], fixture, build_env))
            consumer_urls.append(output[field])
            print(f"consumer {binary} {field}: {output[field]}", flush=True)
        assert values["CONDOR_SCHEMA_URL"] == expected, values
        assert consumer_urls == [expected, expected], (consumer_urls, expected)
        script = next(row["filenames"][0] for row in rows
                      if row["reason"] == "compiler-artifact"
                      and row["target"]["kind"] == ["custom-build"]
                      and "california-condor" in row["package_id"])
        return Path(script), Path(emitted["out_dir"])

    script, out_dir = build(None, UPSTREAM + "/v1.2.3" + ASSET)
    host = run(["rustc", "-vV"], fixture, env).split("host: ")[1].splitlines()[0]

    def emission(describe: str | None = None, repository: str | None = None) -> dict[str, str]:
        script_env = dict(env, CARGO_MANIFEST_DIR=str(package), OUT_DIR=str(out_dir),
                          DEBUG="true", TARGET=host, OPT_LEVEL="3", PROFILE="ci")
        if describe is not None:
            script_env["VERGEN_GIT_DESCRIBE"] = describe
        if repository is not None:
            script_env["GITHUB_REPOSITORY"] = repository
        output = run([str(script)], package, script_env)
        assert "cargo:rerun-if-env-changed=GITHUB_REPOSITORY\n" in output
        values = dict(line.removeprefix("cargo:rustc-env=").split("=", 1)
                      for line in output.splitlines() if line.startswith("cargo:rustc-env="))
        for metadata in ("VERGEN_GIT_SHA", "VERGEN_GIT_COMMIT_DATE",
                         "VERGEN_CARGO_DEBUG", "VERGEN_CARGO_TARGET_TRIPLE",
                         "VERGEN_RUSTC_SEMVER", "VERGEN_RUSTC_LLVM_VERSION"):
            assert values[metadata], (metadata, output)
        return values

    for describe, selected in [
        ("v1.2.3", "v1.2.3"), ("v0.0.0", "v0.0.0"),
        ("v1.2.3-rc.1+build.07", "v1.2.3-rc.1%2Bbuild.07"),
        ("v1.2.3+windows-x64", "v1.2.3%2Bwindows-x64"),
        *[(value, "latest") for value in (
            "", "latest", "release", "deadbee", "VERGEN_IDEMPOTENT_OUTPUT",
            "v1.2", "1.2.3", "v01.2.3", "v1.02.3", "v1.2.03", "v1.2.3-01",
            "v1.2.3-", "v1.2.3+", "v1.2.3-rc..1", "v1.2.3+bad_thing",
            "v1.2.3-2-gdeadbee", "v1.2.3-dirty", "v1.2.3-rc.1-dirty",
            "v1.2.3#fragment", "v1.2.3/path", "v1.2.3\ncargo:warning=injected",
        )],
    ]:
        actual = emission(describe)["CONDOR_SCHEMA_URL"]
        assert actual == UPSTREAM + "/" + selected + ASSET, (describe, actual)
    print("description table: 25 passed", flush=True)

    for repository in (None, "", " ", "owner", "/repo", "owner/", "a/b/c",
                       "a/..", "../b", "a/b#fragment", "a/b?query", "a/b\ncargo:x=y"):
        actual = emission("v1.2.3", repository)["CONDOR_SCHEMA_URL"]
        assert actual == UPSTREAM + "/v1.2.3" + ASSET, (repository, actual)
    fork = "https://github.com/BoatsMcGee/Av1an/releases/download/v1.2.3" + ASSET
    assert emission("v1.2.3", " BoatsMcGee/Av1an ")["CONDOR_SCHEMA_URL"] == fork
    print("repository table: 13 passed", flush=True)

    # No source touch: only the repository environment changes between builds.
    build("BoatsMcGee/Av1an", fork)
    build("invalid", UPSTREAM + "/v1.2.3" + ASSET)
    git("tag", "-d", "v1.2.3")
    assert emission()["CONDOR_SCHEMA_URL"] == UPSTREAM + "/latest" + ASSET
    build("invalid", UPSTREAM + "/latest" + ASSET)
    git("tag", "latest")
    build("invalid", UPSTREAM + "/latest" + ASSET)
    git("tag", "v1.2.3")
    for tag in ("latest", "v1.2.3"):
        assert git("cat-file", "-t", f"refs/tags/{tag}") == "commit"
    print("H1: lightweight latest + v1.2.3 at HEAD; only version tag added", flush=True)
    build("invalid",
          "https://github.com/rust-av/Av1an/releases/download/v1.2.3/configuration.schema.json")
    git("tag", "-d", "latest")
    build("invalid", UPSTREAM + "/v1.2.3" + ASSET)
    git("pack-refs", "--all")
    build("invalid", UPSTREAM + "/v1.2.3" + ASSET)
    git("tag", "-d", "v1.2.3")
    build("invalid", UPSTREAM + "/latest" + ASSET)
    git("tag", "v1.2.3")
    git("tag", "latest")
    (package / "build.rs").write_text((package / "build.rs").read_text() + "\n")
    assert emission()["CONDOR_SCHEMA_URL"] == UPSTREAM + "/latest" + ASSET
    git("add", ".")
    git("commit", "-m", "past tag")
    assert emission()["CONDOR_SCHEMA_URL"] == UPSTREAM + "/latest" + ASSET
    git("tag", "-f", "latest")
    print("H1: latest moved to an unversioned commit; keep latest schema URL", flush=True)
    assert emission()["VERGEN_GIT_DESCRIBE"].startswith("v1.2.3-1-g")
    build("invalid",
          "https://github.com/rust-av/Av1an/releases/download/latest/configuration.schema.json")
    print("owned Git: untagged, dirty, commit-after-tag passed", flush=True)
    print("R08 schema build checks passed", flush=True)


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="r08-schema-", dir="/tmp/opencode") as temp:
        check(Path(temp))
