//! The TriOS theme — every color the GUI uses, in one table (Phase 3).
//!
//! Why centralize? Because "make unfocused windows obviously unfocused"
//! or "darken the palette" must be a *one-line* edit, not a hunt through
//! widget code. Widgets receive `&Theme` on every draw and pick the
//! colors they need; nothing hardcodes a hex value downstream.
//!
//! The palette continues Phase 2's "GitHub dark" direction: deep
//! blue-black canvas, one confident blue accent, green for success.

use crate::graphics::Color;

/// Colors used by the window manager and widgets.
///
/// Construct with [`Theme::default`] (or `Theme::new` in const contexts).
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Desktop background (behind all windows).
    pub bg: Color,
    /// Accent blue — focused titlebars, progress fill, highlights.
    pub accent: Color,
    /// Success green — e.g. a slider thumb being dragged.
    pub success: Color,
    /// Primary text (titles, label bodies).
    pub text_primary: Color,
    /// Secondary/muted text (hints, counters).
    pub text_secondary: Color,
    /// Window body background.
    pub window_bg: Color,
    /// Titlebar of an *unfocused* window (deliberately darker — the
    /// "which window has focus?" cue at a glance).
    pub titlebar_bg: Color,
    /// Titlebar of the focused window (accent blue).
    pub titlebar_bg_focused: Color,
    /// Button face, resting.
    pub button_bg: Color,
    /// Button face, cursor over it.
    pub button_bg_hover: Color,
    /// Button face, mouse held down on it.
    pub button_bg_pressed: Color,
    /// 1-px borders, window outlines, separators.
    pub border: Color,
}

impl Theme {
    /// The default palette as `const fn` — usable in constant contexts.
    pub const fn new() -> Self {
        Self {
            bg: Color::rgb(0x0D1117),
            accent: Color::rgb(0x1F6FEB),
            success: Color::rgb(0x3FB950),
            text_primary: Color::rgb(0xE6EDF3),
            text_secondary: Color::rgb(0x8B949E),
            window_bg: Color::rgb(0x161B22),
            titlebar_bg: Color::rgb(0x21262D),
            titlebar_bg_focused: Color::rgb(0x1F6FEB),
            button_bg: Color::rgb(0x21262D),
            button_bg_hover: Color::rgb(0x30363D),
            button_bg_pressed: Color::rgb(0x0D1117),
            border: Color::rgb(0x30363D),
        }
    }
}

impl Default for Theme {
    /// `Theme::default()` — the natural spelling at call sites.
    fn default() -> Self {
        Self::new()
    }
}
