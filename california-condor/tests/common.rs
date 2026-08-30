use assert_cmd::Command;

pub fn condor_bin() -> Command {
    Command::cargo_bin("condor").expect("condor binary should be built")
}

pub fn condor_cmd(working_directory: &tempfile::TempDir) -> Command {
    let mut cmd = condor_bin();
    cmd.current_dir(working_directory.path());
    cmd
}

pub fn path_str(p: &std::path::Path) -> &str {
    p.to_str().expect("path should be valid UTF-8")
}
