//! Locates libvmaf at build time.
//!
//! Resolution order, mirroring the `vapoursynth-rs` convention:
//!
//! 1. `VMAF_LIB_DIR`, an explicit override.
//! 2. A set of well-known library directories for the host platform.
//!
//! libvmaf is opened with `dlopen` at runtime rather than linked, so this
//! script only records a *search hint* in `VMAF_LIB_DIR`. The crate compiles
//! regardless of whether libvmaf is present, which is what keeps downstream
//! builds of `andean-condor` working on machines without it; scoring reports
//! unavailability at runtime instead.

use std::{env, path::PathBuf};

/// Candidate library file names for the host platform.
#[inline]
fn library_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["vmaf.dll", "libvmaf.dll"]
    } else if cfg!(target_os = "macos") {
        &["libvmaf.dylib", "libvmaf.3.dylib"]
    } else {
        &["libvmaf.so", "libvmaf.so.3"]
    }
}

/// Whether `directory` holds a libvmaf shared library.
#[inline]
fn contains_library(directory: &std::path::Path) -> bool {
    directory.is_dir() && library_names().iter().any(|name| directory.join(name).is_file())
}

/// Locate a directory containing libvmaf.
fn find_libvmaf() -> Option<PathBuf> {
    if let Ok(directory) = env::var("VMAF_LIB_DIR") {
        let directory = PathBuf::from(directory);
        if contains_library(&directory) {
            return Some(directory);
        }
        // Fall through to the probe paths rather than failing outright, so a
        // stale override does not mask a working system-wide
        // installation.
    }

    candidate_directories()
        .into_iter()
        .find(|directory| contains_library(directory))
}

/// Well-known library directories for the host platform.
fn candidate_directories() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if cfg!(target_os = "windows") {
        if let Ok(root) = env::var("VCPKG_ROOT") {
            let root = PathBuf::from(root);
            // No vcpkg port currently supports Windows; checked for completeness.
            for triplet in ["x64-windows", "x64-windows-static"] {
                candidates.push(root.join("installed").join(triplet).join("lib"));
            }
        }
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

fn main() {
    println!("cargo:rerun-if-env-changed=VMAF_LIB_DIR");
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
    println!("cargo:rerun-if-env-changed=CONDA_PREFIX");

    match find_libvmaf() {
        Some(directory) => {
            println!("cargo:rustc-env=VMAF_LIB_DIR={}", directory.display());
        },
        None => {
            // Not fatal. The crate loads libvmaf lazily and reports unavailability
            // at runtime if it cannot be found.
            println!(
                "cargo:warning=libvmaf not found at build time; VMAF scoring will report as \
                 unavailable unless libvmaf is installed at runtime"
            );
        },
    }
}
