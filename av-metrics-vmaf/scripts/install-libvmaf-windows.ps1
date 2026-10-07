<#
.SYNOPSIS
    Downloads a prebuilt libvmaf and its VMAF models for Windows, without MSYS2.

.DESCRIPTION
    Installs libvmaf so av-metrics-vmaf can load it at runtime with `libloading`.
    Only the DLL files are needed: no MSYS2 toolchain, no import library, no
    headers and no compiler. `tar` and `curl.exe` both ship with Windows, so
    this script is self-contained.

    The four DLLs come from ordinary MSYS2 binary packages and must sit beside
    each other: `libvmaf.dll` imports the other three directly.

        libvmaf.dll          <- mingw-w64-x86_64-vmaf
        libgcc_s_seh-1.dll   <- mingw-w64-x86_64-libgcc       (GCC unwinder)
        libstdc++-6.dll      <- mingw-w64-x86_64-libstdc++   (C++ runtime)
        libwinpthread-1.dll  <- mingw-w64-x86_64-libwinpthread-git

    libvmaf's only other imports are kernel32.dll and msvcrt.dll, which every
    Windows install already provides. All nine upstream VMAF models are fetched;
    four are selectable by name through Av1an's `features` option.

.PARAMETER InstallDir
    Where to place `bin` and `model`. Defaults to `$PWD\libvmaf`.

.PARAMETER LibvmafVersion
    libvmaf version to fetch. Defaults to 3.2.1, or to `$env:VMAF_VERSION` when
    that is set.

.PARAMETER ModelsOnly
    Skip the DLL download and only fetch the models. Useful for refreshing
    models when libvmaf is already installed.

.EXAMPLE
    .\install-libvmaf-windows.ps1

.EXAMPLE
    .\install-libvmaf-windows.ps1 -InstallDir C:\tools\libvmaf -Verbose
#>
[CmdletBinding()]
param(
    [Parameter()]
    [string] $InstallDir = (Join-Path $PWD 'libvmaf'),

    [Parameter()]
    [string] $LibvmafVersion = '3.2.1',

    [Parameter()]
    [switch] $ModelsOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Precedence: -LibvmafVersion, then $env:VMAF_VERSION (e.g. .github/.env), then
# the default in the param block.
if (-not $PSBoundParameters.ContainsKey('LibvmafVersion') -and $env:VMAF_VERSION) {
    $LibvmafVersion = $env:VMAF_VERSION
}

# MSYS2's canonical host first, then mirrors carrying the same tree, tried in
# turn: the primary refuses connections on some networks.
$Repos = @(
    'https://repo.msys2.org/mingw/mingw64/',
    'https://mirrors.dotsrc.org/msys2/mingw/mingw64/',
    'https://mirror.msys2.org/mingw/mingw64/'
)

# Some mirrors reject requests that carry no User-Agent with 403.
$UserAgent = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)'

$ModelBase = "https://raw.githubusercontent.com/Netflix/vmaf/v$LibvmafVersion/model/"

# Pinned for reproducibility. Get-LatestPackage refreshes these from the live
# repository listing, so they can be re-run as MSYS2 updates its packages.
$LibvmafPackage = "mingw-w64-x86_64-vmaf-$LibvmafVersion-1-any.pkg.tar.zst"
$RuntimePackages = @(
    'mingw-w64-x86_64-libgcc-16.2.0-4-any.pkg.tar.zst',
    # The `+` is percent-encoded: MSYS2 lists the file with an encoded name, and
    # the literal character 404s.
    'mingw-w64-x86_64-libstdc%2B%2B-16.2.0-4-any.pkg.tar.zst',
    'mingw-w64-x86_64-libwinpthread-git-12.0.0.r747.g1a99f8514-1-any.pkg.tar.zst'
)

# Every model libvmaf v3.2.1 ships. The four in the first group work with the
# MSYS2 build and map to a name in av-metrics-vmaf's `VmafModel`, so they resolve
# automatically. The rest need a libvmaf built with -Denable_float=true, or, for
# `vmaf_b_v0.6.3`, one that still has the BOUND extractors.
$Models = @(
    # Selectable by name through Av1an's `features` option.
    'vmaf_v0.6.1',        # default
    'vmaf_v0.6.1neg',     # features: ["neg"]
    'vmaf_4k_v0.6.1',     # features: ["uhd"]
    'vmaf_4k_v0.6.1neg',  # `model` by path

    # Require a libvmaf built with -Denable_float=true, or an older release for
    # the BOUND models.
    'vmaf_b_v0.6.3',
    'vmaf_float_v0.6.1',
    'vmaf_float_v0.6.1neg',
    'vmaf_float_4k_v0.6.1',
    'vmaf_float_b_v0.6.3'
)

function Get-LatestPackage {
    <#
    .SYNOPSIS
        Finds a package archive in a repo index, newest first.
    #>
    param([Parameter(Mandatory)][string] $Prefix)

    $matches = [System.Collections.Generic.List[string]]::new()

    foreach ($repo in $Repos) {
        $index = curl.exe -sSfL -A $UserAgent $repo 2>$null
        if ($LASTEXITCODE -ne 0) { continue }

        foreach ($match in [regex]::Matches($index, "$Prefix-[A-Za-z0-9\.\-]+any\.pkg\.tar\.zst")) {
            if (-not $matches.Contains($match.Value)) { $matches.Add($match.Value) }
        }

        # One working index is enough.
        if ($matches.Count -gt 0) { break }
    }

    if ($matches.Count -eq 0) {
        throw "No package matching '$Prefix' was found on any mirror. Tried:`n  $($Repos -join "`n  ")"
    }

    return ($matches | Sort-Object -Descending | Select-Object -First 1)
}

function Save-File {
    <#
    .SYNOPSIS
        Downloads the first URI that responds, returning which one worked.

    .DESCRIPTION
        Mirrors that block directory listings still serve known filenames, so
        only `Get-LatestPackage` reads an index.
    #>
    param(
        [Parameter(Mandatory)][string[]] $Uri,
        [Parameter(Mandatory)][string] $OutFile
    )

    $errors = [System.Collections.Generic.List[string]]::new()

    foreach ($candidate in $Uri) {
        # -A is required by some mirrors, which reject requests without a
        # User-Agent with 403. -f makes curl fail on 4xx/5xx instead of writing
        # the error page to disk, which would otherwise fail much later with a
        # confusing parse error.
        curl.exe -sSfL -A $UserAgent --retry 2 --retry-delay 1 -o $OutFile $candidate 2>$null

        if ($LASTEXITCODE -eq 0 -and (Test-Path $OutFile) -and (Get-Item $OutFile).Length -gt 0) {
            return $candidate
        }

        $errors.Add("  $candidate -> curl exit $LASTEXITCODE")
    }

    throw "Failed to download from any source:`n$($errors -join "`n")"
}

$BinDir = Join-Path $InstallDir 'bin'
$ModelDir = Join-Path $InstallDir 'model'
$StageDir = Join-Path $InstallDir 'dl'

New-Item -ItemType Directory -Force -Path $BinDir, $ModelDir | Out-Null

if (-not $ModelsOnly) {
    New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

    $packages = @($LibvmafPackage) + $RuntimePackages
    Write-Host "Downloading $($packages.Count) packages..." -ForegroundColor Cyan

    $usedRepos = [System.Collections.Generic.HashSet[string]]::new()

    foreach ($package in $packages) {
        $archive = Join-Path $StageDir $package
        # One candidate per mirror; Save-File stops at the first that responds.
        $candidates = $Repos | ForEach-Object { "$_$package" }
        $used = Save-File -Uri $candidates -OutFile $archive
        $null = $usedRepos.Add(([uri]$used).Host)
        Write-Host ("  {0}  <- {1}" -f $package, ([uri]$used).Host)

        # Extract only the DLLs; the package also carries headers, an import
        # library, pkgconfig and vmaf.exe, none of which this crate needs.
        tar -xf $archive -C $StageDir 'mingw64/bin/*.dll' 2>$null
    }

    $staged = Join-Path $StageDir 'mingw64/bin'
    if (-not (Test-Path $staged)) {
        throw "No DLLs were extracted from any of: $($usedRepos -join ', ')"
    }

    # All four must land together: the loader resolves libvmaf.dll's imports
    # from its own directory.
    Copy-Item (Join-Path $staged '*.dll') $BinDir -Force

    $expected = @('libvmaf.dll', 'libgcc_s_seh-1.dll', 'libstdc++-6.dll', 'libwinpthread-1.dll')
    $missing = $expected | Where-Object { -not (Test-Path (Join-Path $BinDir $_)) }
    if ($missing) {
        throw "Missing after extraction: $($missing -join ', '). These are required."
    }

    Write-Host "Installed $($expected.Count) DLLs to $BinDir" -ForegroundColor Green
}

Write-Host "Downloading $($Models.Count) VMAF models..." -ForegroundColor Cyan

foreach ($model in $Models) {
    $destination = Join-Path $ModelDir "$model.json"
    $null = Save-File -Uri "$ModelBase$model.json" -OutFile $destination
    Write-Host ("  {0,-26} {1,8:N0} bytes" -f $model, (Get-Item $destination).Length)
}

# Stage space is only needed for the archives.
if (Test-Path $StageDir) { Remove-Item -Recurse -Force $StageDir }

$totalModels = Get-ChildItem $ModelDir -Filter *.json | Measure-Object Length -Sum

Write-Host ''
Write-Host 'Done.' -ForegroundColor Green
Write-Host "  DLLs   : $BinDir"
Write-Host "  Models : $ModelDir ($($totalModels.Count) files, $([math]::Round($totalModels.Sum / 1KB)) KB)"
Write-Host ''
Write-Host 'Model availability with the MSYS2 build:' -ForegroundColor Cyan
Write-Host '  usable    : vmaf_v0.6.1, vmaf_v0.6.1neg, vmaf_4k_v0.6.1, vmaf_4k_v0.6.1neg'
Write-Host '  need build: the vmaf_float_* models (-Denable_float=true)'
Write-Host '  unusable  : vmaf_b_v0.6.3 -- libvmaf 3.2.1 removed the BOUND'
Write-Host '              extractors, so this one fails on any stock build'
Write-Host ''
Write-Host 'Set these environment variables to use the install:'
Write-Host ''
Write-Host "  [Environment]::SetEnvironmentVariable('VMAF_LIB_DIR', '$BinDir', 'User')" -ForegroundColor Yellow
Write-Host "  [Environment]::SetEnvironmentVariable('VMAF_MODEL_PATH', '$ModelDir', 'User')" -ForegroundColor Yellow
Write-Host ''
Write-Host 'Those persist for future shells. For this session only, use:'
Write-Host ''
Write-Host "  `$env:VMAF_LIB_DIR = '$BinDir'"
Write-Host "  `$env:VMAF_MODEL_PATH = '$ModelDir'"
Write-Host ''
Write-Host 'Verify with:'
Write-Host ''
Write-Host '  cargo test -p av-metrics-vmaf'
Write-Host ''
Write-Host 'The `every_model_in_the_search_path_loads` test reports exactly which'
Write-Host 'models this libvmaf build accepts.'