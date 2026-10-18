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
    /// Close button hover state (clean crimson).
    pub close_hover: Color,
    /// Taskbar background.
    pub taskbar_bg: Color,
    /// Active running app indicator accent.
    pub taskbar_active: Color,
}

impl Theme {
    /// The default palette as `const fn` — usable in constant contexts.
    pub const fn new() -> Self {
        Self {
            bg: Color::rgb(0x111520),
            accent: Color::rgb(0x2563EB),
            success: Color::rgb(0x16A34A),
            text_primary: Color::rgb(0xF8FAFC),
            text_secondary: Color::rgb(0x94A3B8),
            window_bg: Color::rgb(0x1A1F2C),
            titlebar_bg: Color::rgb(0x222838),
            titlebar_bg_focused: Color::rgb(0x1E3A8A),
            button_bg: Color::rgb(0x252D3E),
            button_bg_hover: Color::rgb(0x323C52),
            button_bg_pressed: Color::rgb(0x181E2A),
            border: Color::rgb(0x333D52),
            close_hover: Color::rgb(0xE11D48),
            taskbar_bg: Color::rgb(0x151822),
            taskbar_active: Color::rgb(0x3B82F6),
        }
    }
}

impl Default for Theme {
    /// `Theme::default()` — the natural spelling at call sites.
    fn default() -> Self {
        Self::new()
    }
}
