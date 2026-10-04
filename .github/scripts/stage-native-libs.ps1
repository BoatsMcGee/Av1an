<#
.SYNOPSIS
    Stages the prebuilt native libraries a Windows release needs.

.DESCRIPTION
    Condor.exe opens libvmaf and libvship with `dlopen`, so the libraries it does
    use have to be beside the executable before packaging:

      libvmaf.dll          dlopened by av-metrics-vmaf, so absence only costs VMAF.
      libgcc_s_seh-1.dll   \
      libstdc++-6.dll       |  imported by libvmaf.dll itself. All three are hard
      libwinpthread-1.dll  /   imports: libvmaf.dll fails to load without any of
                               them, verified against the MSYS2 package rather
                               than inferred.
      libvship.dll         dlopened by av-metrics-vship; absent costs the native
                           vship metrics, which fall back to the VapourSynth plugin.

    FFMS2 is not among them: it is linked into the executable, so decoding never
    needs a separate ffms2.dll. Only the VapourSynth plugin path and the FFVship
    CLI use one, and neither is part of this release.

    libvship's Vulkan build is the default because its only imports are
    vulkan-1.dll and KERNEL32.dll: it loads against any working NVIDIA, AMD or
    Intel Vulkan driver and needs no vendor redistributable.

.PARAMETER StageDir
    Directory the release is assembled in. Defaults to `target\release`.

.PARAMETER SkipVmaf
    Skip the libvmaf stage. Only useful when testing the vship stage alone.

.PARAMETER SkipModels
    Skip fetching the VMAF model JSON files.
#>
[CmdletBinding()]
param(
    [Parameter()]
    [string] $StageDir = 'target\release',

    [Parameter()]
    [switch] $SkipVmaf,

    [Parameter()]
    [switch] $SkipModels
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Some mirrors reject requests without a User-Agent with 403.
$UserAgent = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)'

# Pinned for reproducibility. MSYS2 retains old package files for a long time,
# but nothing guarantees it forever, so these are re-checkable rather than
# resolved at run time.
$VmafVersion = '3.2.1'
$Msys2Packages = @(
    "mingw-w64-x86_64-vmaf-$VmafVersion-1-any.pkg.tar.zst",
    'mingw-w64-x86_64-libgcc-16.2.0-4-any.pkg.tar.zst',
    'mingw-w64-x86_64-libstdc%2B%2B-16.2.0-4-any.pkg.tar.zst',
    'mingw-w64-x86_64-libwinpthread-git-12.0.0.r747.g1a99f8514-1-any.pkg.tar.zst'
)

# repo.msys2.org refuses connections on some networks, where curl reports
# `(7) Failed to connect` rather than an HTTP error, so the failure is
# indistinguishable from the host being down. Every mirror is tried in turn.
$Msys2Repos = @(
    'https://repo.msys2.org/mingw/mingw64/',
    'https://mirrors.dotsrc.org/msys2/mingw/mingw64/',
    'https://mirror.msys2.org/mingw/mingw64/'
)

$VshipUrl = 'https://codeberg.org/Line-fr/Vship/releases/download/v5.1.1/libvship_VULKAN.dll'

# Every model libvmaf ships. Only the first four work with the MSYS2 build, which
# is compiled without -Denable_float=true and against a libvmaf that removed the
# BOUND extractors. The rest are staged anyway -- under a megabyte in total -- so a
# user pairing this release with a source-built libvmaf has the full set.
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

function Test-DllLoadable {
    <#
    .SYNOPSIS
        Tries to load a DLL, returning the Win32 error on failure.

    .DESCRIPTION
        Presence is not integrity: a truncated download satisfies a file check
        and then fails to load. LoadLibrary is the only honest test.
    #>
    param([Parameter(Mandatory)][string] $Path)

    Add-Type -Namespace Native -Name DllProbe -MemberDefinition @'
        [System.Runtime.InteropServices.DllImport("kernel32.dll", SetLastError = true,
            CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
        private static extern System.IntPtr LoadLibrary(string path);

        [System.Runtime.InteropServices.DllImport("kernel32.dll")]
        private static extern bool FreeLibrary(System.IntPtr handle);

        public static string TryLoad(string path) {
            System.IntPtr handle = LoadLibrary(path);
            if (handle == System.IntPtr.Zero) {
                return "Win32 error " + System.Runtime.InteropServices.Marshal.GetLastWin32Error();
            }
            FreeLibrary(handle);
            return null;
        }
'@

    return [Native.DllProbe]::TryLoad((Resolve-Path $Path).Path)
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
                curl.exe -sSfL -A $UserAgent --retry 2 --retry-delay 1 -o $archive "$repo$package" 2>$null
                if ($LASTEXITCODE -eq 0 -and (Test-Path $archive) -and (Get-Item $archive).Length -gt 0) {
                    Write-Host "    $package <- $repo"
                    $failures.Clear()
                    break
                }
                $failures.Add("$repo (curl exit $LASTEXITCODE)")
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

# A runner has no Vulkan ICD, so this is expected to fail there and is reported
# rather than treated as fatal. A missing file above is still fatal.
$failure = Test-DllLoadable -Path $vshipStaging
if ($failure) {
    Write-Warning "libvship.dll did not load ($failure). Expected without a Vulkan ICD; the VapourSynth plugin still provides the metrics."
} else {
    Write-Host '    libvship.dll loads' -ForegroundColor DarkGray
}

if (-not $SkipModels) {
    Write-Step 'Fetching VMAF models'

    $modelDir = Join-Path $StageDir 'model'
    New-Item -ItemType Directory -Force -Path $modelDir | Out-Null

    foreach ($model in $VmafModels) {
        Invoke-Download -Uri "$VmafModelBase$model.json" -OutFile (Join-Path $modelDir "$model.json")
    }
}

Write-Step 'Verifying the staged release'

Assert-Staged -Names @(
    'libvmaf.dll'
    'libgcc_s_seh-1.dll'
    'libstdc++-6.dll'
    'libwinpthread-1.dll'
    'libvship.dll'
) -Directory $StageDir

Write-Host 'All native dependencies staged.' -ForegroundColor Green
