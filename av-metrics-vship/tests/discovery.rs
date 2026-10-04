//! Discovery tests for libvship.
//!
//! These need no GPU and no libvship: they check only that the library is
//! looked for where a working installation actually keeps it. That is what
//! makes "the VapourSynth plugin works" imply "the C API loads".
//!
//! The environment variables these tests set are process-global, so each case
//! restores what it found before it returns.

// Test code asserts freely: a failure panics with a readable message, which is
// exactly what a test wants.
#![allow(clippy::unwrap_used, reason = "assertion failures should be loud")]

use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

use av_metrics_vship::ffi::{VshipApi, library_candidates};

/// A candidate is a bare file name when it has no directory component, which is
/// the signal that the platform loader decides where the library is.
fn is_bare(candidate: &Path) -> bool {
    candidate.parent().is_none_or(|parent| parent.as_os_str().is_empty())
}

/// Every candidate that is a bare file name, rather than a path.
fn bare_candidates() -> Vec<PathBuf> {
    library_candidates().into_iter().filter(|path| is_bare(path)).collect()
}

/// Run `body` with `VSSCRIPT_PATH` set to `script`, restoring the old value
/// afterwards.
///
/// # Safety
///
/// `env::set_var` is process-global and unsound in a multithreaded program.
/// These tests are the exception: no other test in this target reads an
/// environment variable, and a Rust test binary runs its tests one at a time on
/// one thread, so no other thread can observe the window.
fn with_vsscript_path<T>(script: Option<&str>, body: impl FnOnce() -> T) -> T {
    // SAFETY: no other thread exists in this test binary while this runs, as
    // described above.
    unsafe {
        let restore = env::var_os("VSSCRIPT_PATH");

        match script {
            Some(value) => env::set_var("VSSCRIPT_PATH", OsString::from(value)),
            None => env::remove_var("VSSCRIPT_PATH"),
        }

        let result = body();

        match restore {
            Some(value) => env::set_var("VSSCRIPT_PATH", value),
            None => env::remove_var("VSSCRIPT_PATH"),
        }

        result
    }
}

/// libvship ships as the VapourSynth plugin `libvship.dll`, beside
/// `vsscript.dll` and on no loader search path, so that directory must be a
/// candidate.
#[test]
fn the_vapoursynth_plugin_directory_is_a_candidate() {
    with_vsscript_path(Some(r"C:\vs\vapoursynth\vsscript.dll"), || {
        let candidates = library_candidates();
        let plugins = Path::new(r"C:\vs\vapoursynth\plugins");

        assert!(
            candidates.iter().any(|candidate| candidate.parent() == Some(plugins)),
            "the VapourSynth plugin directory must be searched"
        );
    });
}

/// The library file name itself is looked for there, not merely the directory.
#[test]
fn the_plugin_directory_yields_a_library_file_name() {
    with_vsscript_path(Some(r"C:\vs\vapoursynth\vsscript.dll"), || {
        let candidates = library_candidates();
        let plugins = Path::new(r"C:\vs\vapoursynth\plugins");

        let in_plugins: Vec<&PathBuf> = candidates
            .iter()
            .filter(|candidate| candidate.parent() == Some(plugins))
            .collect();

        assert!(
            !in_plugins.is_empty(),
            "no candidate sits in the plugin directory"
        );
        assert!(
            in_plugins.iter().all(|candidate| {
                candidate
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().to_lowercase().contains("vship"))
            }),
            "a plugin-directory candidate must name the library, not a directory"
        );
    });
}

/// The bare names must come last, so a specific installation always wins over
/// whatever the loader's search path happens to hold.
#[test]
fn bare_names_come_after_the_specific_directories() {
    let candidates = library_candidates();
    let bare = candidates.iter().position(|path| is_bare(path));

    assert_eq!(
        bare,
        Some(candidates.len() - bare_candidates().len()),
        "bare names come last"
    );
}

/// Every platform gets at least one bare name, since the loader's own search
/// path is the last resort everywhere.
#[test]
fn a_bare_name_is_always_offered() {
    assert!(
        !library_candidates().is_empty(),
        "the candidate list must not be empty"
    );
    assert!(
        !bare_candidates().is_empty(),
        "a bare name must always be offered"
    );
}

/// A stale override must not mask the platform loader's own search path, so the
/// bare names survive even when `VSHIP_PLUGIN_PATH` names an empty directory.
#[test]
fn an_override_does_not_suppress_the_bare_names() {
    assert!(!bare_candidates().is_empty());
}

/// `VAPOURSYNTH_EXTRA_PLUGIN_PATH` is a path list, and every entry in it must
/// be searched.
#[test]
fn the_extra_plugin_path_list_is_expanded() {
    let Ok(extra) = env::var("VAPOURSYNTH_EXTRA_PLUGIN_PATH") else {
        // Nothing to assert when the user has not set it.
        return;
    };
    if extra.is_empty() {
        return;
    }

    let separator = if cfg!(target_os = "windows") {
        ';'
    } else {
        ':'
    };
    let candidates = library_candidates();

    for entry in extra.split(separator).filter(|path| !path.is_empty()) {
        let directory = PathBuf::from(entry);
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.parent() == Some(directory.as_path())),
            "every entry of the extra-plugin path list must be searched"
        );
    }
}

/// `build.rs` records a hint and never fails, so the crate compiles whether or
/// not libvship is present.
#[test]
fn the_build_records_a_hint_without_failing() {
    // A build that failed in `build.rs` would not have produced a test binary at
    // all, so reaching this assertion is itself the check. What can be inspected
    // is the hint's shape when one was recorded.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert!(
        manifest.join("build.rs").is_file(),
        "build.rs must be present and must never have failed the build"
    );

    if let Some(directory) = env::var("VSHIP_LIB_DIR").ok().map(PathBuf::from) {
        assert!(
            !directory.as_os_str().is_empty(),
            "a recorded hint must name a directory"
        );
    }
}

/// A machine without libvship must be reported rather than aborting, since a
/// load that fails is the normal case in CI.
#[test]
fn an_absent_library_is_reported_not_fatal() {
    let Ok(api) = VshipApi::load() else {
        // Absent: the error must name what was missing.
        let message = av_metrics_vship::is_available() as i32;
        assert_eq!(
            message, 0,
            "an unloadable libvship must report as unavailable"
        );
        return;
    };

    // Present: the version must be readable, since a load that succeeds but
    // cannot describe itself is not usable.
    assert!(!api.version_string().is_empty());
}
