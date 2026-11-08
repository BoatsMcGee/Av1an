//! Locates libvship at build time.
//!
//! libvship is opened with `dlopen` at runtime rather than linked, so this
//! script only records a *search hint* in `VSHIP_LIB_DIR`. The crate compiles
//! regardless of whether libvship is present, which keeps downstream builds
//! working on machines without a GPU; scoring reports unavailability at
//! runtime instead.
//!
//! On a working machine libvship is not a standalone install: it ships as the
//! VapourSynth plugin `<package>/vapoursynth/plugins/libvship.dll`, a sibling
//! of `vsscript.dll`. That directory is not on any loader search path, so the
//! probe below looks there explicitly, deriving it from `VSSCRIPT_PATH`.

use std::{
    env,
    path::{Path, PathBuf},
};

/// Candidate library file names for the host platform.
#[inline]
fn library_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["libvship.dll", "vship.dll"]
    } else if cfg!(target_os = "macos") {
        &["libvship.dylib", "libvship.2.dylib"]
    } else {
        &["libvship.so", "libvship.so.2"]
    }
}

/// Whether `directory` holds a libvship shared library.
#[inline]
fn contains_library(directory: &Path) -> bool {
    directory.is_dir() && library_names().iter().any(|name| directory.join(name).is_file())
}

/// Directories implied by the environment that must be tried before any
/// well-known system location.
///
/// `VSHIP_PLUGIN_PATH` is the user's established convention and is honoured
/// first; `VSHIP_LIB_DIR` is the build-hint override this crate itself records;
/// `VSSCRIPT_PATH` locates the VapourSynth runtime whose `plugins` sibling is
/// where libvship actually lives on a working machine.
fn environment_directories() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    for variable in ["VSHIP_PLUGIN_PATH", "VSHIP_LIB_DIR"] {
        if let Ok(directory) = env::var(variable)
            && !directory.is_empty()
        {
            candidates.push(PathBuf::from(directory));
        }
    }

    // `VSSCRIPT_PATH` names the VapourSynth script DLL, so the plugin directory
    // is its sibling `plugins`, not its parent.
    if let Ok(script) = env::var("VSSCRIPT_PATH")
        && !script.is_empty()
        && let Some(runtime) = Path::new(&script).parent()
    {
        candidates.push(runtime.join("plugins"));
        candidates.push(runtime.to_path_buf());
    }

    // VapourSynth's own extra-plugin variable is a `;`-separated (Windows) or
    // `:`-separated list of directories.
    if let Ok(extra) = env::var("VAPOURSYNTH_EXTRA_PLUGIN_PATH") {
        let separator = if cfg!(target_os = "windows") {
            ';'
        } else {
            ':'
        };
        candidates
            .extend(extra.split(separator).filter(|path| !path.is_empty()).map(PathBuf::from));
    }

    candidates
}

/// Well-known library directories for the host platform.
fn candidate_directories() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if cfg!(target_os = "windows") {
        candidates.extend(windows_system_directories());
    } else if cfg!(target_os = "macos") {
        for prefix in ["/opt/homebrew", "/usr/local", "/opt/local"] {
            candidates.push(PathBuf::from(prefix).join("lib"));
        }
        if let Ok(conda) = env::var("CONDA_PREFIX") {
            candidates.push(PathBuf::from(conda).join("lib"));
        }
    } else {
        candidates.push(PathBuf::from("/usr/lib"));
        candidates.push(PathBuf::from("/usr/lib64"));
        candidates.push(PathBuf::from("/usr/local/lib"));
        candidates.push(PathBuf::from("/lib"));
        candidates.push(PathBuf::from("/lib64"));
        if let Ok(conda) = env::var("CONDA_PREFIX") {
            candidates.push(PathBuf::from(conda).join("lib"));
        }
    }

    candidates
}

/// Windows' system directories, asked of the OS rather than written down.
///
/// `C:\Windows` is not a safe constant: Windows can be installed to another
/// drive or relocated, and a hardcoded path then names a directory that does
/// not exist. A build script runs on CI and on other people's machines, so this
/// matters more here than a path the loader would have resolved anyway.
#[cfg(target_os = "windows")]
fn windows_system_directories() -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    unsafe extern "system" {
        fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
        fn GetSystemWow64DirectoryW(buffer: *mut u16, size: u32) -> u32;
    }

    fn query(entry: unsafe extern "system" fn(*mut u16, u32) -> u32) -> Option<PathBuf> {
        // A return equal to the buffer size means the path was truncated, so
        // grow and ask again rather than using a partial directory.
        let mut buffer = vec![0u16; 260];
        loop {
            let capacity = buffer.len();
            // SAFETY: `buffer` is writable for `capacity` units, which is the
            // size passed alongside it.
            let written = unsafe { entry(buffer.as_mut_ptr(), capacity as u32) };
            if written == 0 {
                return None;
            }
            if (written as usize) < capacity {
                buffer.truncate(written as usize);
                break;
            }
            buffer.resize(capacity * 2, 0);
        }
        Some(PathBuf::from(std::ffi::OsString::from_wide(&buffer)))
    }

    let mut directories = Vec::with_capacity(2);
    // Native System32 first, then WOW64. A 32-bit process asking for System32
    // is redirected to SysWOW64, which is the directory it can really load
    // from.
    directories.extend(query(GetSystemDirectoryW));
    directories.extend(query(GetSystemWow64DirectoryW));
    directories.extend(
        env::var("SystemRoot")
            .ok()
            .filter(|root| !root.is_empty())
            .map(PathBuf::from)
            .map(|root| root.join("SysWOW64")),
    );
    directories.dedup();

    directories
}

/// Nothing to add off Windows.
#[cfg(not(target_os = "windows"))]
#[inline]
#[must_use]
const fn windows_system_directories() -> Vec<PathBuf> {
    Vec::new()
}

/// Locate a directory containing libvship.
fn find_libvship() -> Option<PathBuf> {
    // Environment first, then the platform probe paths. A stale override falls
    // through rather than masking a working system-wide installation.
    environment_directories()
        .into_iter()
        .chain(candidate_directories())
        .find(|directory| contains_library(directory))
}

fn main() {
    for variable in [
        "VSHIP_PLUGIN_PATH",
        "VSHIP_LIB_DIR",
        "VSSCRIPT_PATH",
        "VAPOURSYNTH_EXTRA_PLUGIN_PATH",
        "CONDA_PREFIX",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }

    match find_libvship() {
        Some(directory) => {
            println!("cargo:rustc-env=VSHIP_LIB_DIR={}", directory.display());
        },
        None => {
            // Not fatal. libvship is loaded lazily and its absence is reported
            // as unavailability at runtime.
            println!(
                "cargo:warning=libvship not found at build time; vship scoring will report as \
                 unavailable unless libvship is installed at runtime"
            );
        },
    }
}
