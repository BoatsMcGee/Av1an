<#
.SYNOPSIS
    Packs the staged release into a single zip with the layout the binaries need.

.DESCRIPTION
    The release assets form a directory, not a pile of loose files, because the
    runtime resolves some of them by relative path:

      * VMAF models must sit in `model/` beside the executable. av-metrics-vmaf
        searches `<exe dir>/model`, so a flat layout silently fails to find them
        and VMAF is reported unavailable with no other symptom.
      * The native libraries must sit beside the executable, because libvmaf.dll
        imports its MinGW runtime DLLs and neither it nor libvship.dll can be
        found anywhere else. fmetrics.dll has no dependencies of its own but is
        still found by relative path. FFMS2 needs nothing here: it is linked
        into the executable.

.PARAMETER StageDir
    Directory holding the staged release. Defaults to `target\release`.

.PARAMETER OutputPath
    Where to write the archive. Defaults to `target\condor-windows-x64.zip`.

.PARAMETER SkipSchema
    Leave `configuration.schema.json` out of the archive.
#>
[CmdletBinding()]
param(
    [Parameter()]
    [string] $StageDir = 'target\release',

    [Parameter()]
    [string] $OutputPath = 'target\condor-windows-x64.zip',

    [Parameter()]
    [string] $SchemaPath = 'california-condor/configuration.schema.json',

    [Parameter()]
    [switch] $SkipSchema
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Everything the runtime loads by relative path. `model/` is a directory rather
# than a file, which is the whole reason this archive exists.
$Contents = @(
    'condor.exe'
    'libvmaf.dll'
    'libvship.dll'
    'fmetrics.dll'
    'libgcc_s_seh-1.dll'
    'libstdc++-6.dll'
    'libwinpthread-1.dll'
    'model'
)

function Write-Step {
    param([Parameter(Mandatory)][string] $Message)
    Write-Host "==> $Message" -ForegroundColor Cyan
}

Write-Step 'Checking the staged release'

$missing = $Contents | Where-Object { -not (Test-Path (Join-Path $StageDir $_)) }
if ($missing) {
    throw "Missing from $StageDir : $($missing -join ', ')"
}

$schema = $null
if (-not $SkipSchema -and (Test-Path $SchemaPath)) {
    $schema = (Resolve-Path $SchemaPath).Path
    Write-Host "    including $SchemaPath"
}

$models = Get-ChildItem (Join-Path $StageDir 'model') -Filter *.json
Write-Host "    $($Contents.Count) entries plus $($models.Count) model files"

Write-Step "Writing $OutputPath"

New-Item -ItemType Directory -Force -Path (Split-Path $OutputPath -Parent) | Out-Null
if (Test-Path $OutputPath) { Remove-Item -Force $OutputPath }

# Written entry by entry so every entry lands at the archive root.
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Add-ZipFile {
    <#
    .SYNOPSIS
        Copies one file into the open archive under a given entry name.

    .DESCRIPTION
        Uses the stream API, which Windows PowerShell 5.1 supports.
    #>
    param(
        [Parameter(Mandatory)][string] $Source,
        [Parameter(Mandatory)][string] $EntryName,
        [Parameter(Mandatory)][System.IO.Compression.ZipArchive] $Archive
    )

    $entry = $Archive.CreateEntry($EntryName, [System.IO.Compression.CompressionLevel]::Optimal)
    $target = $entry.Open()
    $input = [System.IO.File]::OpenRead($Source)
    try {
        $input.CopyTo($target)
    } finally {
        $input.Dispose()
        $target.Dispose()
    }
}

$archive = [System.IO.Compression.ZipFile]::Open($OutputPath, 'Create')
try {
    foreach ($entry in $Contents) {
        $source = Join-Path $StageDir $entry
        if (Test-Path $source -PathType Container) {
            foreach ($file in Get-ChildItem $source -Recurse -File) {
                Add-ZipFile -Source $file.FullName -EntryName "$entry/$($file.Name)" -Archive $archive
            }
        } else {
            Add-ZipFile -Source $source -EntryName $entry -Archive $archive
        }
    }

    if ($schema) {
        # Sits at the archive root, where `condor init` looks for it.
        Add-ZipFile -Source $schema -EntryName (Split-Path $schema -Leaf) -Archive $archive
    }
} finally {
    $archive.Dispose()
}

Write-Step 'Verifying the archive'

Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead((Resolve-Path $OutputPath).Path)
try {
    $names = $zip.Entries | ForEach-Object { $_.FullName }
} finally { $zip.Dispose() }

# A model outside model/ would resolve to nothing at runtime, so the layout is
# asserted rather than assumed.
$flatModels = $names | Where-Object { $_ -match '^vmaf_.*\.json$' }
if ($flatModels) {
    throw "Models landed at the archive root, where they cannot be found: $($flatModels -join ', ')"
}

$expected = @(
    'condor.exe'
    'libvmaf.dll'
    'libvship.dll'
    'fmetrics.dll'
    'model/vmaf_v0.6.1.json'
)
foreach ($name in $expected) {
    $leaf = Split-Path $name -Leaf
    if (-not ($names | Where-Object { $_ -like "*$leaf" })) {
        throw "The archive is missing $name. It contains: $($names -join ', ')"
    }
}

$size = [math]::Round((Get-Item $OutputPath).Length / 1MB, 2)
Write-Host "    $($names.Count) entries, $size MB" -ForegroundColor DarkGray
Write-Host "Archive ready: $OutputPath" -ForegroundColor Green
