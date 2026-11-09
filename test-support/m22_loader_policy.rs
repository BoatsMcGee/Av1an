use super::*;

// These checks exercise the production Windows policy on native host paths;
// they do not establish Windows DLL search behavior on Linux.
#[test]
fn m22_qualified_candidates_select_sibling_search() {
    let root = tempfile::tempdir().expect("policy directory");
    let absolute = root.path().join("marker.dll");
    assert_eq!(
        windows_load_policy(&absolute).expect("absolute policy"),
        (absolute, true)
    );
    let (relative, siblings) = windows_load_policy(Path::new("./runtime/marker.dll"))
        .expect("intentional relative policy");
    assert!(
        relative.is_absolute(),
        "qualified relative paths must not be searched as bare names"
    );
    assert!(
        siblings,
        "qualified candidates must search their sibling dependencies"
    );
    assert_eq!(
        windows_load_policy(Path::new("marker.dll")).expect("bare policy"),
        (PathBuf::from("marker.dll"), false),
        "bare names must use safe defaults without DLL_LOAD_DIR"
    );
    assert!(
        windows_load_policy(Path::new("")).is_err(),
        "empty paths must not select CWD"
    );
}

#[test]
fn m22_explicit_directories_are_validated_without_losing_unicode() {
    let root = tempfile::tempdir().expect("directory policy fixture");
    let unicode = root.path().join("日本語-é");
    std::fs::create_dir_all(&unicode).expect("Unicode directory");
    assert_eq!(
        windows_directory_override(unicode.as_os_str()).expect("Unicode override"),
        Some(unicode)
    );
    let relative = windows_directory_override(std::ffi::OsStr::new("."))
        .expect("explicit current directory")
        .expect("nonempty selection");
    assert!(
        relative.is_absolute(),
        "an explicit relative directory is intentional, but must be qualified"
    );
    assert!(
        windows_directory_override(root.path().join("missing").as_os_str()).is_err(),
        "missing directories must not become candidates"
    );
    let file = root.path().join("not-a-directory.dll");
    std::fs::write(&file, b"not a DLL").expect("file override fixture");
    assert!(
        windows_directory_override(file.as_os_str()).is_err(),
        "DLL filenames are not directory overrides"
    );
    assert_eq!(
        windows_directory_override(std::ffi::OsStr::new("")).expect("empty absent override"),
        None
    );
}

#[cfg(target_os = "windows")]
#[test]
fn m22_windows_drive_and_unc_paths_are_qualified() {
    for value in [
        r"C:\trusted\marker.dll",
        r"\\server\share\marker.dll",
        r"\runtime\marker.dll",
        r"C:runtime\marker.dll",
    ] {
        let (path, siblings) =
            windows_load_policy(Path::new(value)).expect("Windows path qualification");
        assert!(
            path.is_absolute() && siblings,
            "qualified Windows path lost full-path sibling policy: {path:?}"
        );
    }
}
