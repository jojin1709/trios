//! GUI widgets — the reusable pieces windows are built from (Phase 3,
//! extended for Phase 4 by [`TextInput`]).
//!
//! ### Design: plain data + draw + pointer handlers
//!
//! Every widget is an ordinary struct (no trait objects, no vtables —
//! `no_std` hobby kernels pay for dyn dispatch in complexity, not speed):
//!
//! * **draw** — paint yourself into the backbuffer at an offset (the
//!   window's position; widgets store *window-local* coordinates so they
//!   move with the window for free);
//! * **pointer handlers** — react to mouse state. The window manager
//!   calls them with local coordinates:
//!   `pointer_down` → returns `true` if the widget *captured* the pointer
//!   (the press started on it), `pointer_move` → hover/drag feedback,
//!   `pointer_up` → returns an [`WidgetOutcome`] if something happened.
//!
//! The capture model is the key idea: after a press, the WM routes all
//! motion/release to the capturing widget until the button comes back up.
//! That's what makes a slider keep tracking even when the cursor slips
//! off its thumb.

use alloc::string::String;

use super::theme::Theme;
use super::wm::Rect;
use crate::graphics::{Color, Graphics, font::GLYPH_W};
use crate::input::{KeyCode, KeyEvent};

/// What a widget reports when the pointer comes back up on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WidgetOutcome {
    /// A button was pressed and released inside its rect.
    Clicked,
    /// A slider finished a drag (or was clicked) at this value.
    ValueChanged(f32),
}

// ---------------------------------------------------------------------------
// Label
// ---------------------------------------------------------------------------

/// A line of static (or externally updated) text.
#[derive(Debug, Clone)]
pub struct Label {
    pub text: String,
    /// Window-local position of the text's top-left cell.
    pub x: i32,
    pub y: i32,
    pub color: Color,
    /// font8x8 pixel-duplication scale (1 = 8 px tall).
    pub scale: u32,
}

impl Label {
    pub fn new(text: impl Into<String>, x: i32, y: i32, color: Color) -> Self {
        Self {
            text: text.into(),
            x,
            y,
            color,
            scale: 1,
        }
    }

    pub fn draw(&self, g: &mut Graphics, ox: i32, oy: i32) {
        g.draw_text(
            ox + self.x,
            oy + self.y,
            &self.text,
            self.color,
            None,
            self.scale,
        );
    }
}

// ---------------------------------------------------------------------------
// Button
// ---------------------------------------------------------------------------

/// Visual state of a [`Button`] — drives which palette entry it paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Idle,
    Hover,
    /// Mouse held down on it (set on press, cleared on release/leave).
    Pressed,
}

/// A clickable button: filled rect + centered label.
#[derive(Debug, Clone)]
pub struct Button {
    /// Window-local rect (hit target and drawing area are the same).
    pub rect: Rect,
    pub label: String,
    pub state: ButtonState,
}

impl Button {
    pub fn new(rect: Rect, label: impl Into<String>) -> Self {
        Self {
            rect,
            label: label.into(),
            state: ButtonState::Idle,
        }
    }

    pub fn draw(&self, g: &mut Graphics, ox: i32, oy: i32, theme: &Theme) {
        let r = self.rect;
        let face = match self.state {
            ButtonState::Idle => theme.button_bg,
            ButtonState::Hover => theme.button_bg_hover,
            ButtonState::Pressed => theme.button_bg_pressed,
        };
        g.fill_rect(ox + r.x, oy + r.y, r.w as u32, r.h as u32, face);
        // Accent outline when the cursor is on it — cheap affordance.
        let outline = if self.state == ButtonState::Hover {
            theme.accent
        } else {
            theme.border
        };
        g.draw_rect(ox + r.x, oy + r.y, r.w as u32, r.h as u32, outline);

        // Centered label (integer math; scale 1 = 8 px cells).
        let tw = Graphics::text_width(&self.label, 1);
        let th = Graphics::text_height(1);
        g.draw_text(
            ox + r.x + (r.w - tw) / 2,
            oy + r.y + (r.h - th) / 2,
            &self.label,
            theme.text_primary,
            None,
            1,
        );
    }

    /// Press started on the button? Capture it and show `Pressed`.
    pub fn pointer_down(&mut self, x: i32, y: i32) -> bool {
        if self.rect.contains(x, y) {
            self.state = ButtonState::Pressed;
            true
        } else {
            false
        }
    }

    /// `captured` = this button owns the current press (drag feedback
    /// while held); otherwise this is a plain hover update.
    pub fn pointer_move(&mut self, x: i32, y: i32, captured: bool) {
        let inside = self.rect.contains(x, y);
        self.state = match (captured, inside) {
            (true, true) => ButtonState::Pressed,
            (true, false) => ButtonState::Idle, // left the button while holding
            (false, true) => ButtonState::Hover,
            (false, false) => ButtonState::Idle,
        };
    }

    /// Press released. Called by the WM only when this button captured
    /// the press, so "fire" is simply "are we still inside?".
    pub fn pointer_up(&mut self, x: i32, y: i32) -> Option<WidgetOutcome> {
        let inside = self.rect.contains(x, y);
        self.state = if inside {
            ButtonState::Hover
        } else {
            ButtonState::Idle
        };
        inside.then_some(WidgetOutcome::Clicked)
    }
}

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

/// A flat container: background fill + optional 1-px border. Non-
/// interactive — purely visual grouping (it never captures the pointer).
#[derive(Debug, Clone)]
pub struct Panel {
    pub rect: Rect,
    pub bg: Color,
    pub border: Option<Color>,
}

impl Panel {
    pub fn new(rect: Rect, bg: Color, border: Option<Color>) -> Self {
        Self { rect, bg, border }
    }

    pub fn draw(&self, g: &mut Graphics, ox: i32, oy: i32) {
        let r = self.rect;
        g.fill_rect(ox + r.x, oy + r.y, r.w as u32, r.h as u32, self.bg);
        if let Some(color) = self.border {
            g.draw_rect(ox + r.x, oy + r.y, r.w as u32, r.h as u32, color);
        }
    }
}

// ---------------------------------------------------------------------------
// ProgressBar
// ---------------------------------------------------------------------------

/// A horizontal value bar: dark track + accent fill for `value` of it.
/// The *owner* (main loop) drives `value` — e.g. from tick animations.
#[derive(Debug, Clone)]
pub struct ProgressBar {
    pub rect: Rect,
    /// 0.0 ..= 1.0 (clamped on draw; garbage input can't corrupt pixels).
    pub value: f32,
}

impl ProgressBar {
    pub fn new(rect: Rect, value: f32) -> Self {
        Self {
            rect,
            value: value.clamp(0.0, 1.0),
        }
    }

    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(0.0, 1.0);
    }

    pub fn draw(&self, g: &mut Graphics, ox: i32, oy: i32, theme: &Theme) {
        let r = self.rect;
        // Track.
        g.fill_rect(ox + r.x, oy + r.y, r.w as u32, r.h as u32, theme.button_bg);
        g.draw_rect(ox + r.x, oy + r.y, r.w as u32, r.h as u32, theme.border);
        // Fill (f32 -> i32 truncation is fine for a bar).
        let inner = r.w - 2; // don't paint over the border
        if inner > 0 {
            let filled = (inner as f32 * self.value.clamp(0.0, 1.0)) as i32;
            if filled > 0 {
                g.fill_rect(
                    ox + r.x + 1,
                    oy + r.y + 1,
                    filled as u32,
                    (r.h - 2).max(0) as u32,
                    theme.accent,
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Slider
// ---------------------------------------------------------------------------

/// Thumb width in pixels (the value maps to `rect.w - THUMB_W` of travel).
const THUMB_W: i32 = 10;

/// A draggable horizontal slider. The *drag* is where this widget earns
/// its keep: once captured, motion keeps updating `value` even outside
/// the rect (see `pointer_move`), which is what feels right in a real UI.
#[derive(Debug, Clone)]
pub struct Slider {
    pub rect: Rect,
    /// 0.0 ..= 1.0.
    pub value: f32,
    /// True while this slider owns the active pointer press.
    pub dragging: bool,
}

impl Slider {
    pub fn new(rect: Rect, value: f32) -> Self {
        Self {
            rect,
            value: value.clamp(0.0, 1.0),
            dragging: false,
        }
    }

    /// Maps a local x coordinate to a value in `0.0..=1.0` (the thumb
    /// travels over `rect.w - THUMB_W` of track).
    fn value_from_x(&self, x: i32) -> f32 {
        let travel = (self.rect.w - THUMB_W).max(1);
        ((x - self.rect.x) as f32 / travel as f32).clamp(0.0, 1.0)
    }

    /// Thumb's local x for the current value.
    fn thumb_x(&self) -> i32 {
        let travel = (self.rect.w - THUMB_W).max(1);
        self.rect.x + (travel as f32 * self.value) as i32
    }

    pub fn draw(&self, g: &mut Graphics, ox: i32, oy: i32, theme: &Theme) {
        let r = self.rect;
        let cy = oy + r.y + r.h / 2;

        // Track: 4 px bar centered vertically.
        g.fill_rect(ox + r.x, cy - 2, r.w as u32, 4, theme.button_bg);
        // Accent fill from the left edge to the thumb's center.
        let fill = (self.thumb_x() + THUMB_W / 2) - r.x;
        if fill > 0 {
            g.fill_rect(ox + r.x, cy - 2, fill as u32, 4, theme.accent);
        }
        // Thumb: bright while dragging (feedback), bordered otherwise.
        let thumb_color = if self.dragging {
            theme.success
        } else {
            theme.text_primary
        };
        let tx = ox + self.thumb_x();
        g.fill_rect(tx, oy + r.y, THUMB_W as u32, r.h as u32, thumb_color);
        g.draw_rect(tx, oy + r.y, THUMB_W as u32, r.h as u32, theme.border);
    }

    pub fn pointer_down(&mut self, x: i32, y: i32) -> bool {
        if !self.rect.contains(x, y) {
            return false;
        }
        self.dragging = true;
        // Clicking the track should jump the thumb there (Windows-style).
        self.value = self.value_from_x(x);
        true
    }

    pub fn pointer_move(&mut self, x: i32, _y: i32, captured: bool) {
        if captured && self.dragging {
            self.value = self.value_from_x(x);
        }
    }

    pub fn pointer_up(&mut self, _x: i32, _y: i32) -> Option<WidgetOutcome> {
        if !self.dragging {
            return None;
        }
        self.dragging = false;
        Some(WidgetOutcome::ValueChanged(self.value))
    }
}

// ---------------------------------------------------------------------------
// TextInput
// ---------------------------------------------------------------------------

/// Horizontal padding between the field's border and its text (px).
const TEXT_PAD: i32 = 4;

/// A single-line editable text field — the Phase 4 building block for
/// the Terminal's command line and the File Explorer's filename prompt.
///
/// ### Model
///
/// Plain data like every other widget: `value` (the text), `cursor` (a
/// **byte** index into `value` — the keyboard driver only produces
/// ASCII, so byte and char indices coincide and no UTF-8 boundary math
/// is needed), `focused` (draws the caret; its owner is the window
/// manager's focus routing in `wm.rs`, not this widget), and
/// `placeholder` (grey hint shown while the value is empty).
///
/// [`TextInput::handle_key`] returns `true` when the key *changed* the
/// value or cursor — that's the WM's "was consumed" signal. Enter is
/// deliberately **not** consumed: submitting a line is the owner's
/// decision (Terminal: run it; Explorer: confirm the name).
#[derive(Debug, Clone)]
pub struct TextInput {
    pub rect: Rect,
    pub value: String,
    /// Byte index into `value` (ASCII-only input ⇒ also a char index).
    pub cursor: usize,
    /// Draw the caret? Mirrors "this field has keyboard focus". The WM
    /// keeps it in sync (`sync_text_focus` in `wm.rs`) so a field in an
    /// unfocused window never shows a caret.
    pub focused: bool,
    pub placeholder: String,
}

impl TextInput {
    pub fn new(rect: Rect, placeholder: impl Into<String>) -> Self {
        Self {
            rect,
            value: String::new(),
            cursor: 0,
            focused: false,
            placeholder: placeholder.into(),
        }
    }

    /// How many character cells fit between the paddings (scale 1 = 8 px
    /// cells), never less than one so tiny rects degrade gracefully.
    fn visible_chars(&self) -> usize {
        let inner = (self.rect.w - TEXT_PAD * 2).max(GLYPH_W as i32) as usize;
        inner / GLYPH_W as usize
    }

    /// How many leading characters to skip so the *cursor* stays inside
    /// the visible window — the simplest horizontal scroll that works:
    /// keep the caret on screen, clip whatever runs off the left edge.
    /// Shared by `draw` and click-to-position so both agree on what
    /// "visible" means.
    fn hscroll(&self) -> usize {
        let visible = self.visible_chars();
        self.cursor.saturating_sub(visible.saturating_sub(1))
    }

    pub fn draw(&self, g: &mut Graphics, ox: i32, oy: i32, theme: &Theme) {
        let r = self.rect;
        // Field face + focus-reactive border (accent = "typing goes
        // here" — same affordance language as the focused titlebar).
        g.fill_rect(ox + r.x, oy + r.y, r.w as u32, r.h as u32, theme.button_bg);
        let border = if self.focused {
            theme.accent
        } else {
            theme.border
        };
        g.draw_rect(ox + r.x, oy + r.y, r.w as u32, r.h as u32, border);

        let ty = oy + r.y + (r.h - Graphics::text_height(1)) / 2;
        if self.value.is_empty() {
            // Placeholder in muted text. Never scrolls (an empty field
            // has nothing to scroll, and hints are short by design).
            g.draw_text(
                ox + r.x + TEXT_PAD,
                ty,
                &self.placeholder,
                theme.text_secondary,
                None,
                1,
            );
        } else {
            // Visible slice, drawn glyph-by-glyph like the window
            // titlebar — no per-frame allocation, clip by column.
            let scroll = self.hscroll();
            let limit = self.visible_chars();
            let x_max = ox + r.x + r.w - TEXT_PAD;
            let mut cx = ox + r.x + TEXT_PAD;
            for ch in self.value.chars().skip(scroll).take(limit) {
                if cx + GLYPH_W as i32 > x_max {
                    break;
                }
                g.draw_char(cx, ty, ch, theme.text_primary, None, 1);
                cx += GLYPH_W as i32;
            }
        }

        // Caret: a 1 px vertical rule at the cursor column when focused.
        // `hscroll` guarantees `cursor >= scroll`, so the subtraction is
        // safe; at the end of the text the caret sits in the trailing
        // padding, which is exactly where a real field draws it.
        if self.focused {
            let scroll = self.hscroll();
            let caret_x = ox + r.x + TEXT_PAD + ((self.cursor - scroll) as i32) * GLYPH_W as i32;
            g.draw_line(
                caret_x,
                oy + r.y + 3,
                caret_x,
                oy + r.y + r.h - 4,
                theme.text_primary,
            );
        }
    }

    /// Handles one key event. Returns `true` if the key *changed* the
    /// value or cursor (i.e. was consumed). Key releases, Enter, Esc,
    /// Tab, arrows-with-nothing-to-do and modifier-only keys return
    /// `false` — the owner (window / main loop) decides what they mean.
    pub fn handle_key(&mut self, ev: KeyEvent) -> bool {
        if !ev.pressed {
            // Releases never edit text (the driver reports both edges).
            return false;
        }
        // Defensive clamp: an owner that poked `cursor` directly still
        // can't make the indexing below go out of bounds.
        let len = self.value.len();
        if self.cursor > len {
            self.cursor = len;
        }
        match ev.code {
            KeyCode::Char(c) => {
                // ASCII-only input (see `event.rs`) ⇒ `cursor` is always
                // on a char boundary, so the infallible byte insert is
                // safe here by construction.
                self.value.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                true
            }
            KeyCode::Backspace => {
                if self.cursor == 0 {
                    return false;
                }
                self.value.remove(self.cursor - 1);
                self.cursor -= 1;
                true
            }
            KeyCode::Delete => {
                if self.cursor >= self.value.len() {
                    return false;
                }
                self.value.remove(self.cursor);
                true
            }
            KeyCode::ArrowLeft => {
                if self.cursor == 0 {
                    return false;
                }
                self.cursor -= 1;
                true
            }
            KeyCode::ArrowRight => {
                if self.cursor >= self.value.len() {
                    return false;
                }
                self.cursor += 1;
                true
            }
            KeyCode::Home => {
                let changed = self.cursor != 0;
                self.cursor = 0;
                changed
            }
            KeyCode::End => {
                let len = self.value.len();
                let changed = self.cursor != len;
                self.cursor = len;
                changed
            }
            // Enter/Esc/Tab/Up/Down/Ctrl/... : not ours. Esc especially —
            // `wm.rs` keeps it as the window-close key, and a text field
            // that swallowed Esc would trap the user.
            _ => false,
        }
    }

    /// Press inside the field: place the caret at the clicked column
    /// (honouring the horizontal scroll). Keyboard *focus* itself is the
    /// WM's call — it sets `focused` via `sync_text_focus` right after
    /// routing the press here.
    pub fn pointer_down(&mut self, x: i32, y: i32) -> bool {
        if !self.rect.contains(x, y) {
            return false;
        }
        let col = (x - self.rect.x - TEXT_PAD).max(0) / GLYPH_W as i32;
        self.cursor = (self.hscroll() + col as usize).min(self.value.len());
        true
    }

    /// Nothing drag-sensitive about a text field (kept for enum parity
    /// with the other widgets).
    pub fn pointer_move(&mut self, _x: i32, _y: i32, _captured: bool) {}

    /// A completed click produces no [`WidgetOutcome`] — typing *is* the
    /// interaction; the press already placed the caret.
    pub fn pointer_up(&mut self, _x: i32, _y: i32) -> Option<WidgetOutcome> {
        None
    }
}

// ---------------------------------------------------------------------------
// The widget enum — what a window actually contains
// ---------------------------------------------------------------------------

/// One node of a window's widget list. An enum (not `Box<dyn Widget>`)
/// keeps the tree allocation-free per node and lets matches stay exhaustive
/// as the set grows in Phase 4.
#[derive(Debug, Clone)]
pub enum Widget {
    Label(Label),
    Button(Button),
    Panel(Panel),
    ProgressBar(ProgressBar),
    Slider(Slider),
    /// Phase 4 (RS-002): single-line text field. The WM routes keys to
    /// it when the window's `focused_widget` points here (see `wm.rs`).
    TextInput(TextInput),
}

impl Widget {
    // -- terse constructors (main.rs builds demo windows with these) ------

    pub fn label(text: impl Into<String>, x: i32, y: i32, color: Color) -> Self {
        Widget::Label(Label::new(text, x, y, color))
    }

    pub fn button(x: i32, y: i32, w: i32, h: i32, label: impl Into<String>) -> Self {
        Widget::Button(Button::new(Rect::new(x, y, w, h), label))
    }

    pub fn panel(x: i32, y: i32, w: i32, h: i32, bg: Color, border: Option<Color>) -> Self {
        Widget::Panel(Panel::new(Rect::new(x, y, w, h), bg, border))
    }

    pub fn progress(x: i32, y: i32, w: i32, h: i32, value: f32) -> Self {
        Widget::ProgressBar(ProgressBar::new(Rect::new(x, y, w, h), value))
    }

    pub fn slider(x: i32, y: i32, w: i32, h: i32, value: f32) -> Self {
        Widget::Slider(Slider::new(Rect::new(x, y, w, h), value))
    }

    pub fn text_input(x: i32, y: i32, w: i32, h: i32, placeholder: impl Into<String>) -> Self {
        Widget::TextInput(TextInput::new(Rect::new(x, y, w, h), placeholder))
    }

    // -- drawing ----------------------------------------------------------

    pub fn draw(&self, g: &mut Graphics, ox: i32, oy: i32, theme: &Theme) {
        match self {
            Widget::Label(l) => l.draw(g, ox, oy),
            Widget::Button(b) => b.draw(g, ox, oy, theme),
            Widget::Panel(p) => p.draw(g, ox, oy),
            Widget::ProgressBar(p) => p.draw(g, ox, oy, theme),
            Widget::Slider(s) => s.draw(g, ox, oy, theme),
            Widget::TextInput(t) => t.draw(g, ox, oy, theme),
        }
    }

    /// Hit-test: can this widget take a pointer press at local (x, y)?
    /// Labels and panels are decoration → never interactive.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        match self {
            Widget::Button(b) => b.rect.contains(x, y),
            Widget::Slider(s) => s.rect.contains(x, y),
            Widget::TextInput(t) => t.rect.contains(x, y),
            Widget::Label(_) | Widget::Panel(_) | Widget::ProgressBar(_) => false,
        }
    }

    // -- pointer routing (called by the window manager) -------------------

    pub fn pointer_down(&mut self, x: i32, y: i32) -> bool {
        match self {
            Widget::Button(b) => b.pointer_down(x, y),
            Widget::Slider(s) => s.pointer_down(x, y),
            Widget::TextInput(t) => t.pointer_down(x, y),
            // Non-interactive widgets never capture a press.
            Widget::Label(_) | Widget::Panel(_) | Widget::ProgressBar(_) => false,
        }
    }

    pub fn pointer_move(&mut self, x: i32, y: i32, captured: bool) {
        match self {
            Widget::Button(b) => b.pointer_move(x, y, captured),
            Widget::Slider(s) => s.pointer_move(x, y, captured),
            Widget::TextInput(t) => t.pointer_move(x, y, captured),
            Widget::Label(_) | Widget::Panel(_) | Widget::ProgressBar(_) => {}
        }
    }

    pub fn pointer_up(&mut self, x: i32, y: i32) -> Option<WidgetOutcome> {
        match self {
            Widget::Button(b) => b.pointer_up(x, y),
            Widget::Slider(s) => s.pointer_up(x, y),
            Widget::TextInput(t) => t.pointer_up(x, y),
            Widget::Label(_) | Widget::Panel(_) | Widget::ProgressBar(_) => None,
        }
    }
}
