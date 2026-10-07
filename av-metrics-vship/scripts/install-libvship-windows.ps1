<#
.SYNOPSIS
    Downloads a prebuilt libvship for Windows, without MSYS2.

.DESCRIPTION
    Installs libvship so av-metrics-vship can load it at runtime with
    `libloading`. Only the DLL file is needed: no import library, no headers and
    no compiler. `curl.exe` ships with Windows, so this script is
    self-contained.

    Each release publishes three builds. The crate passes the file to `dlopen`,
    so whichever variant is chosen is installed under the name `vship.dll`:

        vulkan  libvship_VULKAN.dll  -> libvship_VULKAN.dll
        nvidia  libvship_NVIDIA.zip  -> libvship_NVIDIA.dll   (CUDA)
        amd     libvship_AMD.zip     -> libvship_AMD.dll      (ROCm)

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
    Which upstream build to install: `vulkan`, `nvidia` or `amd`.

.EXAMPLE
    .\install-libvship-windows.ps1

.EXAMPLE
    .\install-libvship-windows.ps1 -Variant amd -Destination C:\tools\vship -Verbose
#>

[CmdletBinding()]
param(
    [Parameter()]
    [string] $Destination = (Join-Path $PWD 'vship'),

    [Parameter()]
    [string] $Version = '5.1.2',

    [Parameter()]
    [ValidateSet('vulkan', 'nvidia', 'amd')]
    [string] $Variant = 'vulkan'
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

# Nothing upstream is smaller than 1.1 MB, so half a megabyte is the floor for a
# complete download.
$MinimumBytes = 500KB

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
        [Parameter(Mandatory)][string] $OutFile
    )

    curl.exe -sSfL -A $UserAgent --retry 3 --retry-delay 2 -o $OutFile $Uri 2>$null

    if ($LASTEXITCODE -ne 0) {
        throw "Failed to download $Uri (curl exit $LASTEXITCODE)."
    }
    if (-not (Test-Path $OutFile)) {
        throw "curl reported success but $OutFile does not exist."
    }

    $bytes = (Get-Item $OutFile).Length
    if ($bytes -lt $MinimumBytes) {
        Remove-Item $OutFile -Force -ErrorAction SilentlyContinue
        throw "$Uri produced $bytes bytes, below the $MinimumBytes byte floor. Refusing to stage a partial download."
    }

    return $bytes
}

$selected = $Variants[$Variant]
$stageDir = Join-Path $Destination 'dl'
$archive = Join-Path $stageDir $selected.Asset
$dllName = 'vship.dll'
$target = Join-Path $Destination $dllName

New-Item -ItemType Directory -Force -Path $Destination | Out-Null

if ($selected.Archive) {
    New-Item -ItemType Directory -Force -Path $stageDir | Out-Null

    Write-Host "Downloading libvship $Version ($Variant)..." -ForegroundColor Cyan
    $bytes = Get-Asset -Uri "$BaseUrl$($selected.Asset)" -OutFile $archive
    Write-Host ("  {0}  {1,12:N0} bytes" -f $selected.Asset, $bytes)

    Expand-Archive -Path $archive -DestinationPath $stageDir -Force

    $member = Join-Path $stageDir $selected.Member
    if (-not (Test-Path $member)) {
        throw "The archive does not contain $($selected.Member)."
    }

    # dlopen is handed the name, so the vendor suffix has to go.
    Copy-Item $member $target -Force
} else {
    Write-Host "Downloading libvship $Version ($Variant)..." -ForegroundColor Cyan
    $bytes = Get-Asset -Uri "$BaseUrl$($selected.Asset)" -OutFile $target
    Write-Host ("  {0}  {1,12:N0} bytes" -f $selected.Asset, $bytes)
}

if (-not (Test-Path $target)) {
    throw "vship.dll was not installed to $Destination."
}

# Stage space is only needed for the archives.
if (Test-Path $stageDir) { Remove-Item -Recurse -Force $stageDir }

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