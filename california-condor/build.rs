use std::error::Error;

use vergen_git2::{CargoBuilder, Emitter, Git2Builder, RustcBuilder};

/// Base URL for configuration schema release assets.
const SCHEMA_RELEASE_BASE_URL: &str =
    "https://github.com/rust-av/Av1an/releases/download";

/// Returns `true` if the `git describe` output is exactly a tag name (i.e.
/// HEAD is directly on a tag), as opposed to `<tag>-<count>-g<sha>`.
fn describe_is_exact_tag(describe: &str) -> bool {
    // `git describe` appends `-<count>-g<sha>` when HEAD is past the tag.
    // An exact tag contains no such suffix.
    !describe.rsplit_once("-g").is_some_and(|(prefix, suffix)| {
        suffix.chars().all(|c| c.is_ascii_hexdigit())
            && prefix.rsplit_once('-').is_some_and(|(_, count)| {
                !count.is_empty() && count.chars().all(|c| c.is_ascii_digit())
            })
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut git2_builder = Git2Builder::default();
    git2_builder
        .sha(true)
        .commit_date(true)
        .describe(true, false, None);
    let git2 = git2_builder.build()?;
    let cargo = CargoBuilder::default().debug(true).target_triple(true).build()?;
    let rustc = RustcBuilder::default().semver(true).llvm_version(true).build()?;

    Emitter::default()
        .add_instructions(&git2)?
        .add_instructions(&cargo)?
        .add_instructions(&rustc)?
        .emit()?;

    // Bake the configuration schema URL into the binary. Tagged full releases
    // point at their own versioned schema; everything else uses `latest`.
    let schema_tag = std::env::var("VERGEN_GIT_DESCRIBE")
        .ok()
        .filter(|describe| describe_is_exact_tag(describe))
        .unwrap_or_else(|| "latest".to_owned());
    println!(
        "cargo:rustc-env=CONDOR_SCHEMA_URL={SCHEMA_RELEASE_BASE_URL}/{schema_tag}/configuration.schema.json"
    );
    Ok(())
}
