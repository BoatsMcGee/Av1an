//! Smoke test for the documentation screenshot harness.
//!
//! Renders every TUI and help screen offscreen — through the same
//! `TuiApp::render` code the live interface uses — in both themes, and asserts
//! each rasterizes to exactly 1920x1080. This catches panics and gross layout
//! regressions whenever the interface changes. Run with:
//!
//! ```sh
//! cargo test --features screenshots --test screenshots
//! ```

#![cfg(feature = "screenshots")]

#[cfg(test)]
mod tests {
    #[test]
    fn every_screen_renders_at_1920x1080() {
        california_condor::screenshot::verify_all().expect("screenshot verification");
    }
}
