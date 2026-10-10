//! Renders the documentation screenshots (light + dark, 1920x1080 AVIF).
//!
//! Usage:
//! ```sh
//! cargo run --release --features screenshots --bin gen_screenshots -- [OUT_DIR]
//! ```
//! `OUT_DIR` defaults to `california-condor/docs/media/tui`, which the mdBook
//! site consumes via `prefers-color-scheme` media queries.

use std::{env, path::Path, process};

fn main() {
    let out_dir = env::args()
        .nth(1)
        .unwrap_or_else(|| "california-condor/docs/media/tui".to_owned());

    match california_condor::screenshot::generate_all(Path::new(&out_dir)) {
        Ok(written) => {
            for (name, w, h) in &written {
                println!("{name}  {w}x{h}");
            }
            println!("wrote {} screenshots to {out_dir}", written.len());
        },
        Err(err) => {
            eprintln!("screenshot generation failed: {err:#}");
            process::exit(1);
        },
    }
}
