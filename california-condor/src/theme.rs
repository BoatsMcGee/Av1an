//! The TUI color palette.
//!
//! Condor's screens historically hardcoded their colors (`Color::DarkGray`,
//! `Color::Blue`, …). That made a light theme impossible and left no single
//! place to reason about the palette. [`Theme`] owns every color the interface
//! uses, named by *role* rather than by value, and ships a light and a dark
//! variant.
//!
//! The active theme is a **render context**: it is installed once at the edge
//! (the event loop, or the screenshot harness) and read by the render code via
//! [`Theme::current`]. A scoped guard restores the previous theme on drop, so
//! capture code can flip themes without leaking state. When nothing is
//! installed the default is dark, which is byte-for-byte the historical look.

use std::cell::RefCell;

use ratatui::style::Color;

/// Which palette variant is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
}

impl ThemeMode {
    /// Parse a mode from user input (`light`/`dark`/`auto`). `auto` and any
    /// unrecognized value fall back to `dark`.
    pub fn parse(value: Option<&str>) -> ThemeMode {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("light") => ThemeMode::Light,
            _ => ThemeMode::Dark,
        }
    }
}

/// Every color role the interface uses. Values are named by purpose so a theme
/// is a complete, self-contained palette.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub mode:         ThemeMode,
    /// Canvas fill behind every screen.
    pub background:   Color,
    /// Default text where no explicit style is set.
    pub foreground:   Color,
    /// Primary accent: progress bars and highlighted borders.
    pub main:         Color,
    /// Secondary accent (quantizer scatter, informational).
    pub accent_blue:  Color,
    /// Tertiary accent (score scatter, success).
    pub accent_green: Color,
    /// Block borders.
    pub border:       Color,
    /// De-emphasized / secondary text.
    pub dim:          Color,
}

impl Theme {
    /// The historical palette — the default when no theme is installed.
    pub const fn dark() -> Theme {
        Theme {
            mode:         ThemeMode::Dark,
            background:   Color::Rgb(18, 18, 22),
            foreground:   Color::Rgb(214, 214, 218),
            main:         Color::Rgb(128, 128, 128),
            accent_blue:  Color::Rgb(90, 140, 240),
            accent_green: Color::Rgb(110, 200, 120),
            border:       Color::Rgb(90, 90, 96),
            dim:          Color::Rgb(128, 128, 128),
        }
    }

    /// A light palette mirroring the dark roles with contrast tuned for a
    /// bright canvas.
    pub const fn light() -> Theme {
        Theme {
            mode:         ThemeMode::Light,
            background:   Color::Rgb(246, 246, 247),
            foreground:   Color::Rgb(28, 28, 32),
            main:         Color::Rgb(110, 110, 118),
            accent_blue:  Color::Rgb(30, 90, 200),
            accent_green: Color::Rgb(20, 140, 70),
            border:       Color::Rgb(160, 160, 168),
            dim:          Color::Rgb(130, 130, 138),
        }
    }

    pub fn for_mode(mode: ThemeMode) -> Theme {
        match mode {
            ThemeMode::Light => Theme::light(),
            ThemeMode::Dark => Theme::dark(),
        }
    }
}

thread_local! {
    static CURRENT: RefCell<Theme> = const { RefCell::new(Theme::dark()) };
}

/// Restores the previously installed theme when dropped.
pub struct ThemeGuard(Option<Theme>);

impl Drop for ThemeGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.0 {
            CURRENT.with(|current| *current.borrow_mut() = previous);
        }
    }
}

impl Theme {
    /// Install `theme` as the active render context, returning a guard that
    /// restores the previous theme on drop.
    pub fn install(theme: Theme) -> ThemeGuard {
        let previous = CURRENT.with(|current| current.replace(theme));
        ThemeGuard(Some(previous))
    }

    /// Install the theme for `mode`, restoring the previous theme on drop.
    pub fn set_mode(mode: ThemeMode) -> ThemeGuard {
        Theme::install(Theme::for_mode(mode))
    }

    /// The active theme. Defaults to dark when none is installed.
    pub fn current() -> Theme {
        CURRENT.with(|current| *current.borrow())
    }
}
