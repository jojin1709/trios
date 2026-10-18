//! The TriOS theme — design tokens and metrics (Phase 17: Display + Visual Foundation).
//!
//! Provides the centralized design tokens specified by the TriOS Design System:
//! - Colors: bg, surface, surface_hi, border, text, text_dim, accent_cyan, accent_violet, success, danger, warning
//! - Metrics: window_radius (10), button_radius (6), titlebar_height (32), dock_height (56), shadow_blur (16)
//! - Clock: UTC offset constant (IST +5:30)

use crate::graphics::Color;

pub const WINDOW_RADIUS: i32 = 10;
pub const BUTTON_RADIUS: i32 = 6;
pub const TITLEBAR_H: i32 = 32;
pub const DOCK_H: i32 = 56;
#[allow(dead_code)]
pub const SHADOW_BLUR: i32 = 16;

// Phase 18: Desktop Experience Metrics
pub const START_MENU_W: i32 = 440;
pub const START_MENU_H: i32 = 480;
pub const QUICK_SETTINGS_W: i32 = 340;
pub const QUICK_SETTINGS_H: i32 = 320;
pub const CONTEXT_MENU_W: i32 = 190;
pub const TOAST_W: i32 = 320;
pub const TOAST_H: i32 = 64;

/// Local timezone offset: IST (UTC +5:30).
pub const UTC_OFFSET_HOURS: i32 = 5;
pub const UTC_OFFSET_MINS: i32 = 30;

/// Design tokens used by the window manager, dock, and widgets.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    // Phase 17 Core Tokens
    pub bg: Color,
    pub surface: Color,
    pub surface_hi: Color,
    pub border: Color,
    pub text: Color,
    pub text_dim: Color,
    pub accent_cyan: Color,
    pub accent_violet: Color,
    pub success: Color,
    pub danger: Color,
    pub warning: Color,

    // Phase 18 Desktop Experience Tokens
    pub selection_bg: Color,
    pub card_bg: Color,
    pub card_bg_hover: Color,

    // Component mappings & aliases for widgets
    pub accent: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub window_bg: Color,
    pub titlebar_bg: Color,
    pub titlebar_bg_focused: Color,
    pub button_bg: Color,
    pub button_bg_hover: Color,
    pub button_bg_pressed: Color,
    pub close_hover: Color,
    pub taskbar_bg: Color,
    pub taskbar_active: Color,
    pub dock_bg: Color,
}

impl Theme {
    /// The default palette as `const fn` — usable in constant contexts.
    pub const fn new() -> Self {
        let bg = Color::rgb(0x0B0F1A);
        let surface = Color::rgb(0x141B2D);
        let surface_hi = Color::rgb(0x1C2540);
        let border = Color::rgb(0x283144);
        let text = Color::rgb(0xE6EAF5);
        let text_dim = Color::rgb(0x8A93AD);
        let accent_cyan = Color::rgb(0x22D3EE);
        let accent_violet = Color::rgb(0x8B5CF6);
        let success = Color::rgb(0x34D399);
        let danger = Color::rgb(0xF87171);
        let warning = Color::rgb(0xFBBF24);

        Self {
            bg,
            surface,
            surface_hi,
            border,
            text,
            text_dim,
            accent_cyan,
            accent_violet,
            success,
            danger,
            warning,

            selection_bg: Color::rgb(0x1B385C),
            card_bg: Color::rgb(0x18223A),
            card_bg_hover: Color::rgb(0x222E4C),

            accent: accent_cyan,
            text_primary: text,
            text_secondary: text_dim,
            window_bg: surface,
            titlebar_bg: surface,
            titlebar_bg_focused: surface_hi,
            button_bg: surface_hi,
            button_bg_hover: Color::rgb(0x283556),
            button_bg_pressed: surface,
            close_hover: danger,
            taskbar_bg: surface,
            taskbar_active: accent_cyan,
            dock_bg: surface,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::new()
    }
}
