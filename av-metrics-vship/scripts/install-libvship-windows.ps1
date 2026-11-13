<#
.SYNOPSIS
    Downloads a prebuilt libvship for Windows, without MSYS2.

.DESCRIPTION
    Installs libvship so av-metrics-vship can load it at runtime with
    `libloading`. Only the DLL file is needed: no import library, no headers and
    no compiler. `curl.exe` ships with Windows, so this script is
    self-contained.

    The verified 5.1.2 release publishes Vulkan and NVIDIA builds, not an AMD
    archive. Downloads are installed under the name `vship.dll`:

        vulkan  libvship_VULKAN.dll  -> vship.dll
        nvidia  libvship_NVIDIA.zip  -> vship.dll   (CUDA)

    An existing local candidate is preserved without downloading; file presence
    does not prove ABI/backend/symbol or GPU compatibility. With no candidate,
    AMD 5.1.2 has no verified pin and fails before downloading or creating a
    destination. Independently audited overrides do not bypass Rust ABI admission.

    Release bundles stage the Vulkan DLL as `libvship.dll` instead.

    `vulkan` is the default: it needs only vulkan-1.dll and KERNEL32.dll, while
    `nvidia` and `amd` need the CUDA or ROCm runtime.

    FFMS2 is a separate install; Av1an links it dynamically, so it is required
    whether or not libvship is present.

.PARAMETER Destination
    Where to place `vship.dll`. Defaults to `$PWD\vship`.

.PARAMETER Version
    libvship release tag to fetch. Defaults to 5.1.2, or to `$env:VSHIP_VERSION`
    when that is set.

.PARAMETER Variant
    Which upstream build URL to select: `vulkan`, `nvidia` or `amd`. Only Vulkan
    and NVIDIA have official 5.1.2 assets and default verified download pins.

.PARAMETER ExpectedSha256
    Independently audited expected hash for a deliberate version/variant override.
    This is caller trust, not an upstream signature. Known Vulkan/NVIDIA defaults
    need no override. An existing local candidate is preserved without downloading;
    the returned Path/Downloaded fields distinguish that case from installation.

.EXAMPLE
    .\install-libvship-windows.ps1

.EXAMPLE
    .\install-libvship-windows.ps1 -Variant nvidia -Destination C:\tools\vship -Verbose
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string] $Destination = (Join-Path $PWD 'vship'),

    [Parameter()]
    [string] $Version = '5.1.2',

    [Parameter()]
    [ValidateSet('vulkan', 'nvidia', 'amd')]
    [string] $Variant = 'vulkan',

    # Explicit caller trust for a deliberately different artifact/version.
    [Parameter()]
    [string] $ExpectedSha256
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Precedence: -Version, then the tag in the environment (e.g. .github/.env), then
# the default in the param block. The leading `v` is dropped so the value matches
# the `-Version` form the download URL is built from.
if (-not $PSBoundParameters.ContainsKey('Version') -and $env:VSHIP_VERSION) {
    $Version = $env:VSHIP_VERSION.TrimStart('v')
}

# Some hosts reject requests that carry no User-Agent with 403.
$UserAgent = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)'

# Upstream names the file after the GPU it targets, so the member inside a ZIP
# has to be renamed before `dlopen` will ever find it.
$Variants = @{
    'vulkan' = @{
        Asset     = 'libvship_VULKAN.dll'
        Archive   = $false
        Member    = 'libvship_VULKAN.dll'
        # Imports vulkan-1.dll only; every GPU vendor's driver provides it.
        Runtime   = 'any working Vulkan driver'
    }
    'nvidia' = @{
        Asset     = 'libvship_NVIDIA.zip'
        Archive   = $true
        Member    = 'libvship_NVIDIA.dll'
        Runtime   = 'the NVIDIA CUDA runtime'
    }
    'amd'    = @{
        Asset     = 'libvship_AMD.zip'
        Archive   = $true
        Member    = 'libvship_AMD.dll'
        Runtime   = 'the AMD ROCm runtime (amdhip64_6.dll)'
    }
}

$BaseUrl = "https://codeberg.org/Line-fr/Vship/releases/download/v$Version/"

$Destination = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Destination)

# Do not replace a user/administrator installation or call file presence ABI proof.
# The Rust loader remains responsible for version (5,1,2), backend and symbols.
$directories = @($env:VSHIP_PLUGIN_PATH, $env:VSHIP_LIB_DIR)
if ($env:VSSCRIPT_PATH) {
    try {
        $runtime = [IO.Path]::GetDirectoryName($env:VSSCRIPT_PATH.Trim().Trim('"'))
        if ($runtime) { $directories += @([IO.Path]::Combine($runtime, 'plugins'), $runtime) }
    } catch [ArgumentException], [NotSupportedException], [IO.PathTooLongException] {
        # Malformed paths are not installation errors during read-only discovery.
    }
}
if ($env:VAPOURSYNTH_EXTRA_PLUGIN_PATH) { $directories += $env:VAPOURSYNTH_EXTRA_PLUGIN_PATH.Split(';') }
$directories += @([Environment]::SystemDirectory, [Environment]::GetFolderPath('SystemX86'))
if ($env:PATH) { $directories += $env:PATH.Split(';') }
$directories += $Destination
foreach ($directory in $directories) {
    if (-not $directory) { continue }
    try {
        $directory = $directory.Trim().Trim('"')
        if (-not $directory) { continue }
        foreach ($name in @('libvship.dll', 'vship.dll')) {
            $candidate = [IO.Path]::Combine((Get-Location -PSProvider FileSystem).ProviderPath, $directory, $name)
            if ([IO.File]::Exists($candidate)) {
                Write-Host "No package installed; preserving local candidate: $candidate"
                Write-Host "Set VSHIP_LIB_DIR to '$directory' if needed. Av1an must validate runtime version (5,1,2), backend and required symbols; file presence does not prove compatibility."
                return [pscustomobject]@{ Path = $candidate; Downloaded = $false }
            }
        }
    } catch [ArgumentException], [NotSupportedException], [IO.PathTooLongException] {
        continue
    }
}

$Sha256 = $ExpectedSha256
if (-not $Sha256 -and $Version -eq '5.1.2') {
    $Sha256 = switch ($Variant) {
        'vulkan' { '3bcfe626c9c41cb16d18be1ed4a89c58977c54761c988bfe04de0dfca0a37558' }
        'nvidia' { 'b9715b881b52764d97444f660d63398ecc754c4e335cb572b671fbc9e2807e9e' }
    }
}
if ($Sha256 -notmatch '^[0-9a-fA-F]{64}$') {
    throw "Verified package pin unavailable for Vship $Version ($Variant). No local runtime candidate found; nothing installed. Install a compatible runtime locally and set VSHIP_LIB_DIR, or independently audit the exact package and supply -ExpectedSha256."
}

function Get-Asset {
    <#
    .SYNOPSIS
        Downloads an asset with retries, returning the bytes it wrote.

    .DESCRIPTION
        Retries unconditionally: Codeberg refuses connections on some networks
        rather than returning an HTTP error.
    #>
    param(
        [Parameter(Mandatory)][string] $Uri,
        [Parameter(Mandatory)][string] $OutFile,
        [Parameter(Mandatory)][string] $Sha256
    )

    if ($Sha256 -notmatch '^[0-9a-fA-F]{64}$') { throw 'An independently verified SHA-256 is required.' }
    $partial = "$OutFile.part.$([guid]::NewGuid().ToString('N'))"
    try {
        curl.exe -sSfL --proto '=https' --proto-redir '=https' -A $UserAgent --retry 3 --retry-delay 2 -o $partial $Uri 2>$null
        if ($LASTEXITCODE -ne 0) { throw "Failed to download $Uri (curl exit $LASTEXITCODE)." }
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $partial).Hash -ne $Sha256) {
            throw "SHA-256 mismatch for $Uri; refusing installation."
        }
        $bytes = (Get-Item -LiteralPath $partial).Length
        Move-Item -LiteralPath $partial -Destination $OutFile -Force
        return $bytes
    } finally {
        if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Force }
    }
}

$selected = $Variants[$Variant]
$dllName = 'vship.dll'
$target = Join-Path $Destination $dllName

New-Item -ItemType Directory -Force -Path $Destination | Out-Null

if ($selected.Archive) {
    $stageDir = Join-Path ([IO.Path]::GetTempPath()) "vship-build-$([guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Path $stageDir | Out-Null
    try {
        $archive = Join-Path $stageDir $selected.Asset
        Write-Host "Downloading libvship $Version ($Variant)..." -ForegroundColor Cyan
        $bytes = Get-Asset -Uri "$BaseUrl$($selected.Asset)" -OutFile $archive -Sha256 $Sha256
        Write-Host ("  {0}  {1,12:N0} bytes" -f $selected.Asset, $bytes)

        Expand-Archive -Path $archive -DestinationPath $stageDir -Force

        $member = Join-Path $stageDir $selected.Member
        if (-not (Test-Path $member)) {
            throw "The archive does not contain $($selected.Member)."
        }

        # dlopen is handed the name, so the vendor suffix has to go.
        Copy-Item $member $target -Force
    } finally {
        if (Test-Path -LiteralPath $stageDir) { Remove-Item -LiteralPath $stageDir -Recurse -Force }
    }
} else {
    Write-Host "Downloading libvship $Version ($Variant)..." -ForegroundColor Cyan
    $bytes = Get-Asset -Uri "$BaseUrl$($selected.Asset)" -OutFile $target -Sha256 $Sha256
    Write-Host ("  {0}  {1,12:N0} bytes" -f $selected.Asset, $bytes)
}

if (-not (Test-Path $target)) {
    throw "vship.dll was not installed to $Destination."
}

Write-Host ''
Write-Host 'Done.' -ForegroundColor Green
Write-Host "  vship.dll : $target"
Write-Host "  version   : $Version ($Variant, needs $($selected.Runtime))"
Write-Host ''
Write-Host 'Set this environment variable to use the install:' -ForegroundColor Cyan
Write-Host ''
Write-Host "  [Environment]::SetEnvironmentVariable('VSHIP_PLUGIN_PATH', '$Destination', 'User')" -ForegroundColor Yellow
Write-Host ''
Write-Host 'That persists for future shells. For this session only, use:'
Write-Host ''
Write-Host "  `$env:VSHIP_PLUGIN_PATH = '$Destination'"
Write-Host ''
Write-Host 'FFMS2 is a separate install. Av1an links it dynamically, so it is'
Write-Host 'needed whether or not libvship is present -- see the README for how to'
Write-Host 'obtain ffms2.dll.'
Write-Host ''
Write-Host 'Verify with:'
Write-Host ''
Write-Host '  cargo test -p av-metrics-vship'
Write-Host ''
return [pscustomobject]@{ Path = $target; Downloaded = $true }
