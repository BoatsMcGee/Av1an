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
        candidates.push(PathBuf::from(r"C:\Windows\System32"));
        candidates.push(PathBuf::from(r"C:\Windows\SysWOW64"));
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
