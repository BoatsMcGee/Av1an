<#
.SYNOPSIS
    Builds fmetrics from source and installs the shared library for Windows.

.DESCRIPTION
    Installs fmetrics so av-metrics-fmetrics can load it at runtime with
    `libloading`. Unlike libvship and libvmaf, fmetrics publishes no prebuilt
    Windows binaries and nothing installable from a package mirror, so this
    fetches a pinned commit and compiles it with Zig.

    That makes this the one install script here that needs a toolchain: Zig
    0.16.x and Git. Zig 0.17 is refused because it removed `b.build_root`, which
    the pinned `build.zig` still uses.

    Built from BoatsMcGee/fmetrics, which adds what upstream lacks: a
    shared-library target to load at all, and a patch making fcvvdp compile on
    Windows, where it calls POSIX-only `sysconf()`.

    The library is installed as `fmetrics.dll`, the first name `library_names`
    offers on Windows.

.PARAMETER Destination
    Where to place `fmetrics.dll`. Defaults to `$PWD\fmetrics`.

.PARAMETER Commit
    The commit to build. Defaults to the pinned revision of the branch below.

.PARAMETER ZigPath
    `zig` to look up on `PATH`, or a path to `zig.exe`.

.EXAMPLE
    .\install-fmetrics-windows.ps1

.EXAMPLE
    .\install-fmetrics-windows.ps1 -Destination C:\tools\fmetrics -Verbose
#>
[CmdletBinding()]
param(
    [Parameter()]
    [string] $Destination = (Join-Path $PWD 'fmetrics'),

    [Parameter()]
    [string] $Commit = 'ae87c8e5607063f0dcc8241b52cb143f6c9ad4ac',

    [Parameter()]
    [string] $ZigPath = 'zig'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Defaults, overridable from the environment so the release pipeline can pass
# its pins without editing this script. It stays standalone: with nothing set,
# these literals are what a direct run uses.
$Repository   = if ($env:FMETRICS_REPO)          { $env:FMETRICS_REPO }          else { 'https://github.com/BoatsMcGee/fmetrics.git' }
$PatchRef     = if ($env:FMETRICS_BRANCH)        { $env:FMETRICS_BRANCH }        else { 'add-windows-shared-build' }
$UpstreamBase = if ($env:FMETRICS_UPSTREAM_BASE) { $env:FMETRICS_UPSTREAM_BASE } else { 'e8bf3cfe06fb78f51864804b2f7003667d4c5a71' }
$RequiredZigMajor = '0.16'

if (-not $PSBoundParameters.ContainsKey('Commit') -and $env:FMETRICS_COMMIT) {
    $Commit = $env:FMETRICS_COMMIT
}

$dllName = 'fmetrics.dll'
$target = Join-Path $Destination $dllName

function Write-Step([string] $Message) {
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Resolve-Zig {
    <#
    .SYNOPSIS
        Locates zig.exe and checks its major version.

    .DESCRIPTION
        `-ZigPath` may be a bare name to look up on PATH, or a path to zig.exe.
        The major version is checked because 0.17 removed `b.build_root`, which
        the pinned `build.zig` uses.
    #>
    $command = Get-Command $ZigPath -ErrorAction SilentlyContinue
    if ($command) {
        $exe = $command.Source
    } elseif (Test-Path $ZigPath) {
        $exe = (Resolve-Path $ZigPath).Path
    } else {
        throw "zig not found at '$ZigPath'. Install Zig $RequiredZigMajor.x from https://ziglang.org/download/, or pass -ZigPath with the path to zig.exe."
    }

    $version = (& $exe version) 2>$null
    if (-not $version) {
        throw "could not run '$exe version'."
    }
    if (-not $version.StartsWith("$RequiredZigMajor.")) {
        throw "zig $version found, but this needs $RequiredZigMajor.x: 0.17 removed b.build_root, which the pinned build.zig uses."
    }

    Write-Host "    zig $version" -ForegroundColor DarkGray
    return $exe
}

# --- toolchain ---

$zig = Resolve-Zig

if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    throw 'git not found; it is needed to fetch a pinned commit.'
}

# Staged under the system temp directory rather than beside the destination, so a
# checkout never lands in a directory that is about to be packaged.
$work = Join-Path ([IO.Path]::GetTempPath()) "fmetrics-build-$PID"

try {
    Write-Step "Fetching fmetrics $PatchRef"
    & git clone --quiet --branch $PatchRef $Repository $work
    if ($LASTEXITCODE -ne 0) { throw 'git clone failed.' }

    # The branch head is what carries the shared-library target and the
    # dependency patch, so it is built rather than the upstream commit. The pin
    # is checked anyway, so a later push cannot silently change what is built.
    $head = (& git -C $work rev-parse HEAD).Trim()
    if ($head -ne $Commit) {
        throw "$PatchRef is at $head, not the expected $Commit. Rebase the branch, or pass -Commit to build a different one deliberately."
    }

    & git -C $work merge-base --is-ancestor $UpstreamBase $Commit
    if ($LASTEXITCODE -ne 0) {
        throw "$Commit is not built on the upstream $UpstreamBase. Rebase before reinstalling."
    }

    # --- dependency patch ---

    # fcvvdp is fetched by Zig into zig-pkg/, outside the repository tree, and
    # calls POSIX-only sysconf() on Windows. Zig extracts dependencies without
    # their .git directory, so the patch is applied from the repository root with
    # --directory.
    $patch = "$work/patches/0001-windows-portability.patch"
    if (-not (Test-Path $patch)) {
        throw "the $PatchRef checkout has no patches/0001-windows-portability.patch; the shared-library work moved."
    }

    Write-Step 'Fetching dependencies'
    Push-Location $work
    try {
        # `--fetch` populates zig-pkg/ without compiling, so fcvvdp can be
        # patched before it is built.
        & $zig build --fetch
        if ($LASTEXITCODE -ne 0) { throw 'fetching the Zig dependencies failed.' }
    } finally {
        Pop-Location
    }

        $dep = Get-ChildItem (Join-Path $work 'zig-pkg') -Directory -Filter 'fcvvdp-*' |
            Select-Object -First 1
        if (-not $dep) { throw 'zig-pkg/fcvvdp-* was not populated.' }

        Write-Step 'Patching fcvvdp for Windows'
        & git -C $work apply --directory="zig-pkg/$($dep.Name)" $patch
        if ($LASTEXITCODE -ne 0) { throw 'applying the fcvvdp patch failed.' }

    # --- build ---

    # `-Dshared=true` is the fork's addition; without it Zig installs only a
    # static library, which `dlopen` cannot use.
    Write-Step 'Building fmetrics.dll'
    Push-Location $work
    try {
        & $zig build --release=fast -Dshared=true
        if ($LASTEXITCODE -ne 0) { throw 'the fmetrics build failed.' }
    } finally {
        Pop-Location
    }

    $built = Join-Path $work 'zig-out\bin\fmetrics.dll'
    if (-not (Test-Path $built)) {
        throw "the build succeeded but $built does not exist; the output layout may have changed."
    }

    # --- install ---

    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    Copy-Item $built $target -Force
} finally {
    # The checkout is several hundred megabytes with its dependency tree.
    if (Test-Path $work) { Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue }
}

$size = [math]::Round((Get-Item $target).Length / 1KB)

Write-Host ''
Write-Host 'Done.' -ForegroundColor Green
Write-Host "  fmetrics.dll : $target ($size KB)"
Write-Host "  commit       : $Commit"
Write-Host ''
Write-Host 'Set this environment variable to use the install:' -ForegroundColor Cyan
Write-Host ''
Write-Host "  [Environment]::SetEnvironmentVariable('FMETRICS_LIB_DIR', '$Destination', 'User')" -ForegroundColor Yellow
Write-Host ''
Write-Host 'That persists for future shells. For this session only, use:'
Write-Host ''
Write-Host "  `$env:FMETRICS_LIB_DIR = '$Destination'"
Write-Host ''
Write-Host 'Not needed when the DLL sits beside condor.exe, which is where a'
Write-Host 'release stages it; the crate searches there on its own.'
Write-Host ''
Write-Host 'Verify with:'
Write-Host ''
Write-Host '  cargo test -p av-metrics-fmetrics'
Write-Host ''
Write-Host 'Those tests skip themselves when the library is absent, so a pass'
Write-Host 'alone does not prove it loaded.'
Write-Host ''
