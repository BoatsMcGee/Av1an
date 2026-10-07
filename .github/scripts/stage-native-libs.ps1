<#
.SYNOPSIS
    Stages the native libraries a Windows release needs.

.DESCRIPTION
    Condor.exe opens libvmaf, libvship and fmetrics with `dlopen`, so the ones it
    uses have to be beside the executable before packaging:

      libvmaf.dll          dlopened by av-metrics-vmaf; absence only costs VMAF.
      libgcc_s_seh-1.dll   \
      libstdc++-6.dll       |  imported by libvmaf.dll, which fails to load
      libwinpthread-1.dll  /   without all three.
      libvship.dll         dlopened by av-metrics-vship; absent costs the native
                           vship metrics, which fall back to the VapourSynth
                           plugin. The Vulkan build imports only vulkan-1.dll and
                           KERNEL32.dll, so it needs no vendor runtime.
      fmetrics.dll         dlopened by av-metrics-fmetrics; absent costs the CPU
                           fallback for SSIMULACRA2, Butteraugli and CVVDP on a
                           machine with no usable GPU.

    fmetrics has no prebuilt Windows release, so the crate's own install script
    compiles it from a pinned commit, which needs Zig on the runner.

    FFMS2 is linked into the executable, so decoding never loads an ffms2.dll.

.PARAMETER StageDir
    Directory the release is assembled in. Defaults to `target\release`.

.PARAMETER SkipVmaf
    Skip the libvmaf stage. Only useful when testing another stage alone.

.PARAMETER SkipModels
    Skip fetching the VMAF model JSON files.

.PARAMETER SkipFmetrics
    Skip building fmetrics. The stage then has no CPU fallback, and only the
    VapourSynth plugin path can score SSIMULACRA2, Butteraugli and CVVDP.
#>
[CmdletBinding()]
param(
    [Parameter()]
    [string] $StageDir = 'target\release',

    [Parameter()]
    [switch] $SkipVmaf,

    [Parameter()]
    [switch] $SkipModels,

    [Parameter()]
    [switch] $SkipFmetrics
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# The Docker image, the Linux staging script and the install scripts all read
# this same file, so every entry point builds the same pins. Any value may also
# be overridden from the environment.
$CicdEnv = @{}
foreach ($line in Get-Content (Join-Path $PSScriptRoot '..\.env')) {
    if ($line -match '^\s*#' -or $line -notmatch '\S') { continue }
    if ($line -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.+?)\s*$') {
        $CicdEnv[$Matches[1]] = $Matches[2]
    }
}

function Get-Pin([string] $Name) {
    <#
    .SYNOPSIS
        Returns one value from .github/.env, failing if it is absent.
    #>
    if (-not $CicdEnv.ContainsKey($Name)) {
        throw "Missing $Name in .github/.env."
    }
    return $CicdEnv[$Name]
}

# Some mirrors reject requests without a User-Agent with 403.
$UserAgent = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)'

# Pinned so every run stages the same bytes.
$VmafVersion = Get-Pin 'VMAF_VERSION'
$Msys2Packages = @(
    "mingw-w64-x86_64-vmaf-$VmafVersion-1-any.pkg.tar.zst",
    'mingw-w64-x86_64-libgcc-16.2.0-4-any.pkg.tar.zst',
    'mingw-w64-x86_64-libstdc%2B%2B-16.2.0-4-any.pkg.tar.zst',
    'mingw-w64-x86_64-libwinpthread-git-12.0.0.r747.g1a99f8514-1-any.pkg.tar.zst'
)

# Tried in turn: the primary mirror refuses connections on some networks.
$Msys2Repos = @(
    'https://repo.msys2.org/mingw/mingw64/',
    'https://mirrors.dotsrc.org/msys2/mingw/mingw64/',
    'https://mirror.msys2.org/mingw/mingw64/'
)

$VshipVersion = Get-Pin 'VSHIP_VERSION'
$VshipUrl = "https://codeberg.org/Line-fr/Vship/releases/download/$VshipVersion/libvship_VULKAN.dll"

# Every model libvmaf ships. The first four are what the MSYS2 build accepts; the
# rest need a libvmaf built with -Denable_float=true or with the BOUND extractors.
$VmafModels = @(
    'vmaf_v0.6.1', 'vmaf_v0.6.1neg', 'vmaf_4k_v0.6.1', 'vmaf_4k_v0.6.1neg',
    'vmaf_b_v0.6.3', 'vmaf_float_v0.6.1', 'vmaf_float_v0.6.1neg',
    'vmaf_float_4k_v0.6.1', 'vmaf_float_b_v0.6.3'
)
$VmafModelBase = "https://raw.githubusercontent.com/Netflix/vmaf/v$VmafVersion/model/"

function Write-Step {
    param([Parameter(Mandatory)][string] $Message)
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Invoke-Download {
    <#
    .SYNOPSIS
        Downloads one URI, failing loudly with the curl exit code.
    #>
    param(
        [Parameter(Mandatory)][string] $Uri,
        [Parameter(Mandatory)][string] $OutFile,
        [Parameter()] [int] $Retries = 3
    )

    curl.exe -sSfL -A $UserAgent --retry $Retries --retry-delay 2 -o $OutFile $Uri 2>$null

    if ($LASTEXITCODE -ne 0) {
        throw "Failed to download $Uri (curl exit $LASTEXITCODE)."
    }

    if (-not (Test-Path $OutFile) -or (Get-Item $OutFile).Length -eq 0) {
        throw "The download from $Uri produced no file."
    }

    $size = (Get-Item $OutFile).Length
    Write-Host ("    {0}  {1:N0} bytes" -f (Split-Path $OutFile -Leaf), $size)
}

function Assert-Staged {
    <#
    .SYNOPSIS
        Fails when any named file is absent from the stage directory.
    #>
    param(
        [Parameter(Mandatory)][string[]] $Names,
        [Parameter(Mandatory)][string] $Directory
    )

    $missing = $Names | Where-Object { -not (Test-Path (Join-Path $Directory $_)) }
    if ($missing) {
        throw "Missing after staging: $($missing -join ', ')"
    }

    Write-Host "    all $($Names.Count) required files present" -ForegroundColor DarkGray
}

New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

if (-not $SkipVmaf) {
    Write-Step 'Fetching libvmaf and its runtime dependencies'

    $vmafStage = Join-Path $StageDir 'vmaf-stage'
    New-Item -ItemType Directory -Force -Path $vmafStage, (Join-Path $StageDir 'model') | Out-Null

    try {
        foreach ($package in $Msys2Packages) {
            $archive = Join-Path $vmafStage $package
            $failures = [System.Collections.Generic.List[string]]::new()

            foreach ($repo in $Msys2Repos) {
                try {
                    Invoke-Download -Uri "$repo$package" -OutFile $archive -Retries 2
                    Write-Host "    $package <- $repo"
                    $failures.Clear()
                    break
                } catch {
                    $failures.Add($_.Exception.Message)
                }
            }

            if ($failures.Count -gt 0) {
                throw "Could not fetch $package from any MSYS2 mirror:`n  $($failures -join "`n  ")"
            }

            # Extract only the DLLs; the package also carries headers, an import
            # library and pkgconfig, none of which the runtime needs.
            tar -xf $archive -C $vmafStage 'mingw64/bin/*.dll'
            if ($LASTEXITCODE -ne 0) { throw "tar failed to extract $package." }
        }

        Copy-Item (Join-Path $vmafStage 'mingw64/bin/*.dll') $StageDir -Force

        Assert-Staged -Names @(
            'libvmaf.dll'
            'libgcc_s_seh-1.dll'
            'libstdc++-6.dll'
            'libwinpthread-1.dll'
        ) -Directory $StageDir
    } finally {
        # Staging space is only needed while the archives are in flight.
        if (Test-Path $vmafStage) { Remove-Item -Recurse -Force $vmafStage }
    }
}

Write-Step 'Fetching libvship (Vulkan build)'

$vshipStaging = Join-Path $StageDir 'libvship.dll'
Invoke-Download -Uri $VshipUrl -OutFile $vshipStaging

if (-not $SkipModels) {
    Write-Step 'Fetching VMAF models'

    $modelDir = Join-Path $StageDir 'model'
    New-Item -ItemType Directory -Force -Path $modelDir | Out-Null

    foreach ($model in $VmafModels) {
        Invoke-Download -Uri "$VmafModelBase$model.json" -OutFile (Join-Path $modelDir "$model.json")
    }
}

if (-not $SkipFmetrics) {
    # The crate's own install script does the work, so there is one implementation
    # of the build rather than two that can drift apart.
    Write-Step 'Building fmetrics from source'

    # Hand the pins over in the environment; the install script keeps working
    # standalone when they are absent.
    $env:FMETRICS_COMMIT        = Get-Pin 'FMETRICS_COMMIT'
    $env:FMETRICS_REPO          = Get-Pin 'FMETRICS_REPO'
    $env:FMETRICS_BRANCH        = Get-Pin 'FMETRICS_BRANCH'
    $env:FMETRICS_UPSTREAM_BASE = Get-Pin 'FMETRICS_UPSTREAM_BASE'

    & ./av-metrics-fmetrics/scripts/install-fmetrics-windows.ps1 -Destination $StageDir
    if ($LASTEXITCODE -ne 0) {
        throw 'the fmetrics build failed.'
    }
}

Write-Step 'Verifying the staged release'

# Only what this run was asked to produce: the Skip switches exist so one stage can
# be exercised alone, and asserting on a skipped library made them always throw.
$required = @('libvship.dll')
if (-not $SkipVmaf) {
    $required += @('libvmaf.dll', 'libgcc_s_seh-1.dll', 'libstdc++-6.dll', 'libwinpthread-1.dll')
}
if (-not $SkipFmetrics) {
    $required += 'fmetrics.dll'
}

Assert-Staged -Names $required -Directory $StageDir

Write-Host 'All native dependencies staged.' -ForegroundColor Green
