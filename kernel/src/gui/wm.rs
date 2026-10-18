//! Window manager — windows, z-order, chrome, drag/resize, hit-testing
//! and composition (Phase 3).
//!
//! ### Responsibilities
//!
//! [`WindowManager`] owns every [`Window`] plus all *interaction state*:
//! where the cursor is, which button went down last, and what the current
//! press is doing ([`Drag`]: moving a window, resizing it, or driving a
//! widget). That state machine is the subtle part of any WM, so it lives
//! in one place with the event order spelled out in [`handle_mouse`].
//! Keyboard events (Phase 4) route to the focused window, and inside it
//! to its `focused_widget` — a text field — before the window-level
//! Esc-closes shortcut.
//!
//! ### Coordinates
//!
//! * **Global** — pixels on the screen (the cursor, window rects).
//! * **Window-local** — relative to a window's top-left corner (widget
//!   rects, titlebar/close/grip hit zones). Widgets store local coords,
//!   so moving a window never re-lays-out its content: drawing adds the
//!   window origin, hit-testing subtracts it.
//!
//! ### Z-order invariant
//!
//! `self.windows` is kept **sorted ascending by `z`** (re-sorted in
//! [`focus`], append-only elsewhere). Composition is therefore a plain
//! forward iteration (bottom → top) and hit-testing a reverse one
//! (top → first hit wins) — no per-pixel tricks, just ordering.
//!
//! ### The cursor draws last
//!
//! [`compose`] paints desktop → windows → hint bar → taskbar → **cursor**,
//! always.
//! The cursor is decoration *over* the scene, never part of it: windows
//! under it are composited untouched, and no save/restore buffer is
//! needed because every frame is rebuilt from scratch before `flush`.
//! The hint bar and the taskbar sit between windows and cursor for the
//! same reason the cursor does: they are OS chrome painted on top every
//! frame, not windows (they can't be focused or dragged, and only the
//! taskbar's three buttons are hit-tested — see [`taskbar_click`]).
//!
//! [`taskbar_click`]: WindowManager::taskbar_click

use alloc::string::String;
use alloc::vec::Vec;

use super::icons::Icon;
use super::theme::Theme;
use super::widgets::{Widget, WidgetOutcome};
use crate::graphics::{Color, Graphics};
use crate::input::{self, mouse};

#[allow(unused_imports)]
pub use super::theme::{
    BUTTON_RADIUS, CONTEXT_MENU_W, DOCK_H, QUICK_SETTINGS_H, QUICK_SETTINGS_W, SHADOW_BLUR,
    START_MENU_H, START_MENU_W, TITLEBAR_H, TOAST_H, TOAST_W, UTC_OFFSET_HOURS, UTC_OFFSET_MINS,
    WINDOW_RADIUS,
};

/// Context menu item (Phase 18).
#[derive(Debug, Clone)]
pub struct ContextMenuItem {
    pub id: u32,
    pub label: String,
    pub shortcut: Option<String>,
    pub is_separator: bool,
    pub danger: bool,
}

impl ContextMenuItem {
    pub fn action(id: u32, label: impl Into<String>, shortcut: Option<&str>) -> Self {
        Self {
            id,
            label: label.into(),
            shortcut: shortcut.map(String::from),
            is_separator: false,
            danger: false,
        }
    }

    pub fn separator() -> Self {
        Self {
            id: 0,
            label: String::new(),
            shortcut: None,
            is_separator: true,
            danger: false,
        }
    }
}

/// Context menu popup state (Phase 18).
#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub rect: Rect,
    pub items: Vec<ContextMenuItem>,
    pub hovered: Option<usize>,
}

impl ContextMenu {
    pub fn new(x: i32, y: i32, w: i32, items: Vec<ContextMenuItem>) -> Self {
        let mut total_h = 10;
        for item in &items {
            total_h += if item.is_separator { 6 } else { 24 };
        }
        Self {
            rect: Rect::new(x, y, w, total_h),
            items,
            hovered: None,
        }
    }
}

/// Start Menu state (Phase 18).
#[derive(Debug, Clone)]
pub struct StartMenuState {
    pub open: bool,
    pub search_query: String,
    pub selected_pinned: usize,
    pub power_menu_open: bool,
    pub power_menu_selected: usize,
}

impl Default for StartMenuState {
    fn default() -> Self {
        Self {
            open: false,
            search_query: String::new(),
            selected_pinned: 0,
            power_menu_open: false,
            power_menu_selected: 0,
        }
    }
}

/// Quick Settings tray panel state (Phase 18).
#[derive(Debug, Clone)]
pub struct QuickSettingsState {
    pub open: bool,
    pub network_on: bool,
    pub dnd_on: bool,
    pub night_light_on: bool,
    pub volume: u8,
    pub brightness: u8,
}

impl Default for QuickSettingsState {
    fn default() -> Self {
        Self {
            open: false,
            network_on: true,
            dnd_on: false,
            night_light_on: false,
            volume: 75,
            brightness: 90,
        }
    }
}

/// Alt+Tab window switcher state (Phase 18).
#[derive(Debug, Clone, Default)]
pub struct AltTabState {
    pub open: bool,
    pub selected: usize,
}

/// Window control button size (minimize, maximize, close) in titlebar.
pub const CLOSE_SIZE: i32 = 20;
/// Bottom-right resize-grip hit zone (square).
pub const RESIZE_GRIP: i32 = 14;
/// Smallest window we allow (a window you can't use is a bug magnet).
pub const MIN_W: i32 = 120;
/// Smallest window height (fits titlebar + a little content).
pub const MIN_H: i32 = 70;

// ---------------------------------------------------------------------------
// Rect
// ---------------------------------------------------------------------------

/// An axis-aligned rectangle — the currency of all layout and hit tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Is the point inside? Right/bottom edges are *exclusive* — the
    /// standard half-open interval, so adjacent rects never double-hit.
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Do the two rects overlap at all? (Touching edges don't count.)
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

/// One top-level window and its widget tree (draw order = vec order).
#[derive(Debug)]
pub struct Window {
    pub id: u64,
    pub title: String,
    /// Global position/size — moved by drag, resized by the grip.
    pub rect: Rect,
    /// Painter's z: higher = drawn later = on top. See module docs.
    pub z: usize,
    /// Exactly one window has this set; drives titlebar/border colors.
    pub focused: bool,
    /// Widget list in *window-local* coordinates.
    pub widgets: Vec<Widget>,
    /// Index into `widgets` of the widget receiving keys (Phase 4: a
    /// `TextInput`). Independent of `focused`: a background window keeps
    /// its field selection for when it regains focus, but only the
    /// focused window ever *receives* keys (see
    /// [`WindowManager::handle_key`]).
    pub focused_widget: Option<usize>,
    /// Set when the user hit X (or Esc) — [`WindowManager::reap`] closes it.
    pub close_requested: bool,
    /// Whether window is minimized to the dock.
    pub minimized: bool,
    /// Whether window is maximized to desktop work area.
    pub maximized: bool,
    /// Restored rectangle before maximize.
    pub saved_rect: Rect,
}

impl Window {
    /// Local titlebar zone (drag-to-move target).
    pub fn titlebar_rect(&self) -> Rect {
        Rect::new(0, 0, self.rect.w, TITLEBAR_H)
    }

    /// Local close-button zone (top-right of titlebar).
    pub fn close_rect(&self) -> Rect {
        Rect::new(
            self.rect.w - CLOSE_SIZE - 6,
            (TITLEBAR_H - CLOSE_SIZE) / 2,
            CLOSE_SIZE,
            CLOSE_SIZE,
        )
    }

    /// Local maximize/restore button zone.
    pub fn maximize_rect(&self) -> Rect {
        Rect::new(
            self.rect.w - CLOSE_SIZE * 2 - 10,
            (TITLEBAR_H - CLOSE_SIZE) / 2,
            CLOSE_SIZE,
            CLOSE_SIZE,
        )
    }

    /// Local minimize button zone.
    pub fn minimize_rect(&self) -> Rect {
        Rect::new(
            self.rect.w - CLOSE_SIZE * 3 - 14,
            (TITLEBAR_H - CLOSE_SIZE) / 2,
            CLOSE_SIZE,
            CLOSE_SIZE,
        )
    }

    /// Local bottom-right resize zone.
    pub fn grip_rect(&self) -> Rect {
        Rect::new(
            self.rect.w - RESIZE_GRIP,
            self.rect.h - RESIZE_GRIP,
            RESIZE_GRIP,
            RESIZE_GRIP,
        )
    }

    /// Keyboard routing target (see [`WindowManager::handle_key`]).
    ///
    /// A focused [`TextInput`](super::widgets::TextInput) (index in
    /// [`Self::focused_widget`]) gets first refusal: if it changed the
    /// value or caret, the key is consumed and done. Esc is the one key
    /// every window still sees — the text field never consumes it (see
    /// `TextInput::handle_key`), so "Esc closes" keeps working even while
    /// you're typing, exactly like a real WM.
    pub fn handle_key(&mut self, ev: input::KeyEvent) {
        if let Some(i) = self.focused_widget
            && let Some(Widget::TextInput(t)) = self.widgets.get_mut(i)
            && t.handle_key(ev)
        {
            return;
        }
        if ev.pressed && ev.code == input::KeyCode::Esc {
            self.close_requested = true;
        }
    }
}

/// Mirrors a window's keyboard-focus state onto its text fields: only a
/// `TextInput` at `focused_widget` inside the *focused* window draws a
/// caret. Called everywhere `focused` or `focused_widget` changes — one
/// tiny loop over a handful of widgets, so brute-force syncing beats
/// bookkeeping flags that can silently drift apart.
fn sync_text_focus(win: &mut Window) {
    for (i, wg) in win.widgets.iter_mut().enumerate() {
        if let Widget::TextInput(t) = wg {
            t.focused = win.focused && win.focused_widget == Some(i);
        }
    }
}

/// What a finished pointer interaction reported to the application.
#[derive(Debug, Clone, Copy)]
pub struct WidgetAction {
    /// Which window owns the widget (ids are stable across moves).
    pub window_id: u64,
    /// Index into that window's `widgets` vec (creation order).
    pub widget_index: usize,
    pub outcome: WidgetOutcome,
}

// ---------------------------------------------------------------------------
// Window manager
// ---------------------------------------------------------------------------

/// Which window interaction the current press has become.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    /// Nothing pressed (or the press hit bare desktop).
    None,
    /// Titlebar held: window follows the cursor minus the grab offset
    /// (so the window doesn't jump its corner to the cursor).
    Move { id: u64, off_x: i32, off_y: i32 },
    /// Resize grip held: right/bottom edges follow the cursor.
    Resize { id: u64 },
    /// A widget captured the press — all motion is routed to it until
    /// release (this is what slider dragging *is*).
    Widget { id: u64, index: usize },
}

/// Zones of a window, decided once per press (topmost hit wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Close,
    Maximize,
    Minimize,
    Grip,
    Titlebar,
    Body,
}

/// The windows behind the taskbar's three buttons, in **button order**
/// (left to right: Files, Editor, Terminal).
///
/// A plain named struct rather than a bare `(u64, u64, u64)`: the whole
/// point of this type is that [`taskbar_click`]'s `match` on the button
/// index can't silently swap two ids — the field names say which app each
/// slot is, at both the write end (`main.rs`) and the read end.
///
/// [`taskbar_click`]: WindowManager::taskbar_click
#[derive(Debug, Clone, Copy)]
struct TaskbarTargets {
    files: u64,
    editor: u64,
    terminal: u64,
    browser: u64,
    sysmon: u64,
    mines: u64,
    settings: u64,
}

/// The compositor + interaction state machine.
pub struct WindowManager {
    /// Windows, **sorted ascending by z** (invariant — see module docs).
    pub windows: Vec<Window>,
    /// The one window receiving focus styling (and keyboard events).
    focused_id: Option<u64>,
    /// Next window id (stable for a window's lifetime).
    next_id: u64,
    /// Monotonic z counter: each focus/create takes the next value.
    next_z: usize,
    /// Screen bounds for cursor clamping and drag limits.
    screen_w: i32,
    screen_h: i32,
    /// WM's ordered cursor view — advanced per event while draining the
    /// queue (see `input::mouse` docs: it mirrors the driver's absolute
    /// position, but must lag *with* the events being processed).
    cursor: (i32, i32),
    /// Previous event's left-button state (to spot press/release edges).
    prev_buttons: input::MouseButtons,
    /// The active interaction, if any.
    drag: Drag,
    /// Which windows the dock app buttons focus.
    taskbar_targets: Option<TaskbarTargets>,
    /// Cached procedural wallpaper backbuffer for 60+ FPS blits.
    wallpaper: Vec<u8>,
    // Phase 18 Desktop Experience State
    pub selected_desktop_icon: Option<usize>,
    pub last_desktop_click: Option<(usize, u64)>,
    pub tick_count: u64,
    pub context_menu: Option<ContextMenu>,
    pub start_menu: StartMenuState,
    pub quick_settings: QuickSettingsState,
    pub alt_tab: AltTabState,
    // Phase 19 Lock Screen State
    pub is_locked: bool,
}

impl WindowManager {
    /// Creates an empty desktop over a `width x height` screen. The
    /// cursor starts wherever the mouse driver left it (centred by
    /// `mouse::set_bounds` during boot) so the two views agree.
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            windows: Vec::new(),
            focused_id: None,
            next_id: 0,
            next_z: 0,
            screen_w: width.max(1),
            screen_h: height.max(1),
            cursor: mouse::position(),
            prev_buttons: input::MouseButtons::default(),
            drag: Drag::None,
            taskbar_targets: None,
            wallpaper: Vec::new(),
            selected_desktop_icon: None,
            last_desktop_click: None,
            tick_count: 0,
            context_menu: None,
            start_menu: StartMenuState::default(),
            quick_settings: QuickSettingsState::default(),
            alt_tab: AltTabState::default(),
            is_locked: false,
        }
    }

    // -- window lifecycle --------------------------------------------------

    /// Creates a window (bottom of the stack until [`focus`]ed) and
    /// returns its stable id. Size is clamped to sane minimums.
    pub fn create_window(
        &mut self,
        title: impl Into<String>,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let title: String = title.into();
        let rect = Rect::new(x, y, w.max(MIN_W), h.max(MIN_H));

        // Sanity check for programmatic callers (a window entirely off
        // the desktop is unclickable — future taskbar code will care).
        let screen = Rect::new(0, 0, self.screen_w, self.screen_h);
        if !rect.intersects(&screen) {
            serial_println!(
                "[wm     ] warning: window '{title}' starts outside the screen ({rect:?})"
            );
        }

        let z = self.take_z();
        self.windows.push(Window {
            id,
            title,
            rect,
            z,
            focused: false,
            widgets: Vec::new(),
            focused_widget: None,
            close_requested: false,
            minimized: false,
            maximized: false,
            saved_rect: rect,
        });
        id
    }

    /// Brings `id` to the top of the stack and makes it the sole focused
    /// window. (Every window you click gets raised — standard behaviour.)
    /// Each window keeps its `focused_widget` across focus changes; only
    /// the caret *drawing* follows (via [`sync_text_focus`]), so clicking
    /// away and back restores the field selection.
    ///
    /// Unknown ids are a **no-op**. Two of this cycle's callers resolve
    /// ids that are only valid *sometimes*: the taskbar buttons and the
    /// `Ctrl+1/2/3` hotkeys target the shell's app windows, which the
    /// user can dismiss with Esc or X at any moment. Writing
    /// `focused_id = <dead id>` would point the keyboard at a window that
    /// is no longer drawn (and make `Apps::owns_focus` claim keys for it),
    /// so the honest answer is to keep the current focus untouched.
    ///
    /// Those two callers (and any path that must leave the window *ready
    /// to type in*) want [`focus_and_type`] instead — plain [`focus`]
    /// alone is what caused the dropped-key bug described there.
    pub fn focus(&mut self, id: u64) {
        if !self.windows.iter().any(|w| w.id == id) {
            return;
        }
        let z = self.take_z();
        for w in &mut self.windows {
            w.focused = w.id == id;
            sync_text_focus(w);
        }
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.z = z;
        }
        self.focused_id = Some(id);
        // Re-establish the ascending-z invariant (create appends the max,
        // focus can jump any window to the top).
        self.windows.sort_by_key(|w| w.z);
    }

    /// Focuses `id` **and** arms its first text field for typing — the
    /// keyboard half of "click a text field" (the *mouse* half is the
    /// `Zone::Body` arm of [`handle_mouse`], which writes
    /// `focused_widget` from the widget under the cursor).
    ///
    /// ### Why this exists (the dropped-key bug it prevents)
    ///
    /// [`focus`] deliberately leaves `focused_widget` alone, so *before*
    /// this method existed the mouse press path was the **only** thing
    /// that ever armed a field. Any focus that arrived some other way —
    /// the `Ctrl+1/2/3` hotkey in `main.rs`, a taskbar button — left
    /// `focused_widget == None`, hence `has_text_focus() == false`,
    /// hence [`Window::handle_key`] forwarded the key to *nobody*: the
    /// printable fell through to the Esc check, failed it, and vanished
    /// **without a log line**. On the target VirtualBox the PS/2 mouse is
    /// dead (`[ps2] mouse irq count = 0` forever), so no click could
    /// ever repair that state — focusing a window without arming its
    /// field made the whole shell type-blind. Focus and "ready to type"
    /// must be one step for keyboard-driven focus, or the keyboard goes
    /// quiet exactly when it is the only input device that works.
    ///
    /// The scan picks the **first** `TextInput` in creation order (the
    /// widget vec *is* the address space — see `apps`), which is each
    /// shell window's primary field: the Terminal prompt
    /// (`terminal::TERM_INPUT`), the Editor's filename box
    /// (`editor::ED_PATH`) and the Explorer's "new file name" prompt
    /// (`explorer::EX_INPUT`). A window with no text field leaves
    /// `focused_widget` as `None`, and an unknown id is a no-op — the
    /// same contract as [`focus`].
    ///
    /// ### Explorer Backspace (verified by reading, not assumed)
    ///
    /// Arming the Explorer's prompt does **not** make `Backspace` fire
    /// twice: `Explorer::on_key` guards its "up one dir" with
    /// `KeyCode::Backspace if !wm.has_text_focus()`, and `main.rs` runs
    /// `apps.on_key` *before* `wm.handle_key` — so the guard always sees
    /// the armed state and exactly one handler acts per press (armed =>
    /// the caret deletes a character, the guard suppresses `go_up`;
    /// unarmed => `go_up`, no field receives the key). Arrow-key row
    /// navigation and Enter opening a row keep working for the same
    /// reason: `TextInput::handle_key` consumes neither (it only takes
    /// Char/Backspace/Delete/Left/Right/Home/End — see `widgets.rs`).
    ///
    /// Called from the `Ctrl+1/2/3` hotkey in `main.rs` and the taskbar
    /// press in [`handle_mouse`].
    pub fn focus_and_type(&mut self, id: u64) {
        self.focus(id);
        // Unknown id: `focus` was a no-op, so there is nothing to arm
        // (the `find` below misses too — kept as the honest guard for
        // "window exists" rather than re-deriving `focus`'s check).
        let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else {
            return;
        };
        w.focused_widget = w
            .widgets
            .iter()
            .position(|wg| matches!(wg, Widget::TextInput(_)));
        sync_text_focus(w);
    }

    /// Removes a window by id (logged).
    pub fn close_window(&mut self, id: u64) {
        if let Some(i) = self.windows.iter().position(|w| w.id == id) {
            let w = self.windows.remove(i);
            if self.focused_id == Some(id) {
                self.focused_id = None;
            }
            serial_println!("[gui    ] window '{}' closed", w.title);
        }
    }

    /// Closes every window whose close button (or Esc) was hit.
    /// Called by the main loop once per event batch.
    pub fn reap(&mut self) {
        let pending: Vec<u64> = self
            .windows
            .iter()
            .filter(|w| w.close_requested)
            .map(|w| w.id)
            .collect();
        for id in pending {
            self.close_window(id);
        }
    }

    fn take_z(&mut self) -> usize {
        self.next_z += 1;
        self.next_z
    }

    // -- state helpers used by main.rs -------------------------------------

    /// Replaces a window label's text (a no-op if the window or slot is
    /// gone — live windows come and go, static indices must stay safe).
    pub fn set_label(&mut self, window_id: u64, index: usize, text: String) {
        if let Some(w) = self.window_mut(window_id)
            && let Some(Widget::Label(l)) = w.widgets.get_mut(index)
        {
            l.text = text;
        }
    }

    /// Replaces a button's label text.
    pub fn set_button_label(&mut self, window_id: u64, index: usize, label: String) {
        if let Some(w) = self.window_mut(window_id)
            && let Some(Widget::Button(b)) = w.widgets.get_mut(index)
        {
            b.label = label;
        }
    }

    /// Sets a progress bar's value (0.0..=1.0).
    pub fn set_progress(&mut self, window_id: u64, index: usize, value: f32) {
        if let Some(w) = self.window_mut(window_id)
            && let Some(Widget::ProgressBar(p)) = w.widgets.get_mut(index)
        {
            p.set_value(value);
        }
    }

    /// Mutable access to a window (append widgets, read state).
    pub fn window_mut(&mut self, id: u64) -> Option<&mut Window> {
        let i = self.windows.iter().position(|w| w.id == id)?;
        self.windows.get_mut(i)
    }

    /// Gives keyboard focus to a widget inside a window (Phase 4: a
    /// freshly opened dialog wants its text field live on appear).
    /// Bounds-checked — a stale index is a no-op, never a panic.
    // Mouse clicks set `focused_widget` inside `handle_mouse`; this is
    // the *programmatic* path for next cycle's Phase 4 apps (open a
    // dialog, focus its field), which is why the binary crate's
    // dead-code lint is silenced here — same reasoning as `fs::mod`.
    #[allow(dead_code)]
    pub fn focus_widget(&mut self, window_id: u64, index: usize) {
        if let Some(w) = self.window_mut(window_id)
            && index < w.widgets.len()
        {
            w.focused_widget = Some(index);
            sync_text_focus(w);
        }
    }

    /// Clears a window's focused widget (the demo text field treats
    /// Enter as "commit and release"; the next key falls back to
    /// window-level handling).
    pub fn clear_widget_focus(&mut self, window_id: u64) {
        if let Some(w) = self.window_mut(window_id) {
            w.focused_widget = None;
            sync_text_focus(w);
        }
    }

    /// The id of the currently focused window, if any.
    ///
    /// The Phase 4 shell (`apps::Apps::owns_focus`) asks this before it
    /// takes a key press: ids are stable for a window's lifetime, so the
    /// answer is just the bookkeeping [`focus`] writes — no lookup, no
    /// allocation. `None` means nothing is focused (every window closed
    /// or none created yet).
    ///
    /// [`focus`]: WindowManager::focus
    pub fn focused_window(&self) -> Option<u64> {
        self.focused_id
    }

    /// Does the *focused* window currently route keys to a text field?
    /// The main loop gates the keyboard-cursor fallback on this, so
    /// arrow keys move the caret (not the mouse) while typing.
    pub fn has_text_focus(&self) -> bool {
        let Some(id) = self.focused_id else {
            return false;
        };
        let Some(w) = self.windows.iter().find(|w| w.id == id) else {
            return false;
        };
        matches!(
            w.focused_widget.and_then(|i| w.widgets.get(i)),
            Some(Widget::TextInput(_))
        )
    }

    /// Reads a text field's current value (`None` if the window/widget is
    /// gone or isn't a `TextInput`) — lets the main loop diff before/after
    /// a key without reaching into widget internals.
    pub fn text_value(&self, window_id: u64, index: usize) -> Option<String> {
        self.windows
            .iter()
            .find(|w| w.id == window_id)
            .and_then(|w| w.widgets.get(index))
            .and_then(|wg| match wg {
                Widget::TextInput(t) => Some(t.value.clone()),
                _ => None,
            })
    }

    /// Reads the value of the **armed** text field — the focused window's
    /// `focused_widget`, when that widget is a `TextInput` — or `None`
    /// when no field is armed.
    ///
    /// This is [`text_value`] without the caller having to know *which*
    /// widget is armed: the main loop snapshots it around
    /// [`Window::handle_key`] and logs `[gui    ] text field: "..."`
    /// when the value changes — the serial line that proves a keystroke
    /// actually travelled scancode → `KeyEvent` → focused `TextInput`.
    /// Before this existed only the Playground demo field could be
    /// verified that way (by hard-coded index), which is precisely how
    /// shell fields went silently dead — see [`focus_and_type`].
    pub fn focused_text_value(&self) -> Option<String> {
        let id = self.focused_id?;
        let w = self.windows.iter().find(|w| w.id == id)?;
        match w.widgets.get(w.focused_widget?) {
            Some(Widget::TextInput(t)) => Some(t.value.clone()),
            _ => None,
        }
    }

    // -- taskbar -----------------------------------------------------------

    /// Wires the three taskbar buttons to their windows, **in button
    /// order**: Files, Editor, Terminal (left to right).
    ///
    /// Why the indirection: the taskbar lives in this WM (it owns the
    /// screen chrome), but which window is "Files" is the shell's
    /// business, and the ids only exist after `main.rs` builds the apps.
    /// The WM would otherwise have to *search windows by title* — string
    /// matching that breaks the moment a title is edited or a second
    /// "Terminal" is opened. Handing over three ids costs nothing and
    /// keeps the dependency pointing one way (main knows the WM; the WM
    /// knows nothing about `apps`).
    ///
    /// Called exactly once from `desktop_run` right after
    /// [`crate::apps::Apps::new`]. Until then the buttons draw but
    /// [`taskbar_click`] answers `None`.
    ///
    /// [`taskbar_click`]: Self::taskbar_click
    pub fn set_taskbar_targets(&mut self, files: u64, editor: u64, terminal: u64, browser: u64, sysmon: u64, mines: u64, settings: u64) {
        self.taskbar_targets = Some(TaskbarTargets {
            files,
            editor,
            terminal,
            browser,
            sysmon,
            mines,
            settings,
        });
    }

    /// Which taskbar button sits at `(x, y)`? The window id to focus, or
    /// `None` — point off every button, *or* targets never wired (see
    /// [`set_taskbar_targets`]), *or* the button's window no longer
    /// exists (the user can Esc/X an app away; focusing a dead id would
    /// poison `focused_id`, see [`focus`], so the click simply does
    /// nothing).
    /// Bounding rectangle of the floating bottom dock.
    pub fn dock_rect(&self) -> Rect {
        let dock_w = 590.min(self.screen_w - 16);
        let dock_h = DOCK_H;
        let dock_x = ((self.screen_w - dock_w) / 2).max(8);
        let dock_y = self.screen_h - dock_h - 12;
        Rect::new(dock_x, dock_y, dock_w, dock_h)
    }

    /// Global rect for the Start button inside the dock.
    pub fn dock_start_rect(&self) -> Rect {
        let d = self.dock_rect();
        Rect::new(d.x + 8, d.y + (d.h - 40) / 2, 40, 40)
    }

    /// Global rects for the 7 application buttons inside the dock.
    pub fn dock_button_rects(&self) -> [Rect; 7] {
        let d = self.dock_rect();
        let icon_x0 = d.x + 8 + 40 + 10;
        let mut rects = [Rect::new(0, 0, 40, 40); 7];
        for (i, r) in rects.iter_mut().enumerate() {
            r.x = icon_x0 + (i as i32 * 46);
            r.y = d.y + (d.h - 40) / 2;
            r.w = 40;
            r.h = 40;
        }
        rects
    }

    /// Global rect for the system tray inside the dock.
    pub fn dock_tray_rect(&self) -> Rect {
        let d = self.dock_rect();
        let buttons = self.dock_button_rects();
        let sep2_x = buttons[6].right() + 6;
        Rect::new(sep2_x + 6, d.y + 4, (d.right() - sep2_x - 12).max(40), d.h - 8)
    }

    /// Global rect for the Start Menu panel.
    pub fn start_menu_rect(&self) -> Rect {
        let w = START_MENU_W.min(self.screen_w - 24);
        let h = START_MENU_H.min(self.screen_h - DOCK_H - 24);
        let x = ((self.screen_w - w) / 2).max(12);
        let y = (self.dock_rect().y - h - 10).max(12);
        Rect::new(x, y, w, h)
    }

    /// Global rect for the Quick Settings panel.
    pub fn quick_settings_rect(&self) -> Rect {
        let w = QUICK_SETTINGS_W.min(self.screen_w - 24);
        let h = QUICK_SETTINGS_H.min(self.screen_h - DOCK_H - 24);
        let x = (self.dock_rect().right() - w).clamp(12, (self.screen_w - w - 12).max(12));
        let y = (self.dock_rect().y - h - 10).max(12);
        Rect::new(x, y, w, h)
    }

    /// Rects for all 9 desktop shortcut icons.
    pub fn desktop_icon_rects(&self) -> Vec<Rect> {
        let mut rects = Vec::new();
        let col_w = 76;
        let row_h = 76;
        let max_rows = ((self.screen_h - DOCK_H - 30) / row_h).max(4);
        for i in 0..DESKTOP_ITEMS.len() {
            let col = (i as i32) / max_rows;
            let row = (i as i32) % max_rows;
            let x = 16 + col * (col_w + 10);
            let y = 16 + row * row_h;
            rects.push(Rect::new(x, y, col_w, row_h));
        }
        rects
    }

    /// Launches or un-minimizes the application corresponding to the icon.
    pub fn launch_app_by_icon(&mut self, icon: Icon) {
        let Some(targets) = self.taskbar_targets else { return; };
        let id = match icon {
            Icon::ThisPc | Icon::SysMon => targets.sysmon,
            Icon::Files | Icon::Folder => targets.files,
            Icon::Editor | Icon::FileTxt | Icon::FileRs => targets.editor,
            Icon::Terminal | Icon::FileElf => targets.terminal,
            Icon::Browser => targets.browser,
            Icon::Mines => targets.mines,
            Icon::Settings => targets.settings,
            Icon::Trash => {
                crate::gui::notify("Recycle Bin", "Trash is empty", crate::gui::NotificationKind::Info);
                return;
            }
        };
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.minimized = false;
        }
        self.focus_and_type(id);
    }

    /// Filters pinned apps according to the active search query.
    pub fn filtered_pinned_apps(&self) -> Vec<(usize, Icon, &'static str)> {
        let q = self.start_menu.search_query.trim();
        PINNED_APPS
            .iter()
            .enumerate()
            .filter(|(_, (_, name))| contains_ignore_ascii_case(name, q))
            .map(|(orig_idx, &(icon, name))| (orig_idx, icon, name))
            .collect()
    }

    /// Global key shortcuts (Win/Super, Alt+Tab, Start Menu & Context Menu navigation).
    pub fn handle_global_key(&mut self, ev: input::KeyEvent) -> bool {
        // Lock screen shortcut (Ctrl+L)
        if (ev.code == input::KeyCode::Char('l') || ev.code == input::KeyCode::Char('L')) && ev.modifiers.ctrl && ev.pressed {
            self.is_locked = true;
            self.start_menu.open = false;
            self.context_menu = None;
            self.quick_settings.open = false;
            crate::gui::notify("Desktop Locked", "Press Enter or Click to Unlock", crate::gui::NotificationKind::Info);
            return true;
        }

        if self.is_locked {
            if ev.pressed {
                self.is_locked = false;
                crate::gui::notify("Session Unlocked", "Welcome back, Jojin", crate::gui::NotificationKind::Success);
            }
            return true;
        }

        // 1. Super/Win key toggles Start Menu
        if ev.code == input::KeyCode::Super && ev.pressed {
            self.start_menu.open = !self.start_menu.open;
            if self.start_menu.open {
                self.context_menu = None;
                self.quick_settings.open = false;
                self.start_menu.search_query.clear();
                self.start_menu.selected_pinned = 0;
                self.start_menu.power_menu_open = false;
            }
            return true;
        }

        // 2. Alt+Tab switcher
        if ev.modifiers.alt && ev.code == input::KeyCode::Tab && ev.pressed {
            let win_count = self.windows.iter().filter(|w| !w.minimized).count();
            if win_count > 0 {
                self.alt_tab.open = true;
                self.alt_tab.selected = (self.alt_tab.selected + 1) % win_count;
            }
            return true;
        }
        if ev.code == input::KeyCode::Alt && !ev.pressed && self.alt_tab.open {
            let non_min: Vec<u64> = self.windows.iter().filter(|w| !w.minimized).map(|w| w.id).collect();
            if let Some(&id) = non_min.get(self.alt_tab.selected) {
                self.focus_and_type(id);
            }
            self.alt_tab.open = false;
            return true;
        }
        if self.alt_tab.open {
            if ev.code == input::KeyCode::Esc && ev.pressed {
                self.alt_tab.open = false;
            }
            return true;
        }

        // 3. Context Menu dismiss
        if self.context_menu.is_some() && ev.code == input::KeyCode::Esc && ev.pressed {
            self.context_menu = None;
            return true;
        }

        // 4. Quick Settings dismiss
        if self.quick_settings.open && ev.code == input::KeyCode::Esc && ev.pressed {
            self.quick_settings.open = false;
            return true;
        }

        // 5. Start Menu navigation & search
        if self.start_menu.open && ev.pressed {
            if ev.code == input::KeyCode::Esc {
                if self.start_menu.power_menu_open {
                    self.start_menu.power_menu_open = false;
                } else {
                    self.start_menu.open = false;
                }
                return true;
            }

            if self.start_menu.power_menu_open {
                match ev.code {
                    input::KeyCode::ArrowUp => {
                        self.start_menu.power_menu_selected = self.start_menu.power_menu_selected.saturating_sub(1);
                        return true;
                    }
                    input::KeyCode::ArrowDown => {
                        self.start_menu.power_menu_selected = (self.start_menu.power_menu_selected + 1).min(2);
                        return true;
                    }
                    input::KeyCode::Enter => {
                        match self.start_menu.power_menu_selected {
                            0 => {
                                crate::gui::notify("Power", "System sleeping...", crate::gui::NotificationKind::Info);
                                self.start_menu.power_menu_open = false;
                                self.start_menu.open = false;
                            }
                            1 => crate::acpi::reboot(),
                            2 => crate::acpi::poweroff(),
                            _ => {}
                        }
                        return true;
                    }
                    _ => {}
                }
            }

            let filtered = self.filtered_pinned_apps();
            let count = filtered.len();

            match ev.code {
                input::KeyCode::ArrowLeft => {
                    if count > 0 {
                        self.start_menu.selected_pinned = self.start_menu.selected_pinned.saturating_sub(1);
                    }
                    return true;
                }
                input::KeyCode::ArrowRight => {
                    if count > 0 {
                        self.start_menu.selected_pinned = (self.start_menu.selected_pinned + 1).min(count - 1);
                    }
                    return true;
                }
                input::KeyCode::ArrowUp => {
                    if self.start_menu.selected_pinned >= 4 {
                        self.start_menu.selected_pinned -= 4;
                    }
                    return true;
                }
                input::KeyCode::ArrowDown => {
                    if count > 0 && self.start_menu.selected_pinned + 4 < count {
                        self.start_menu.selected_pinned += 4;
                    }
                    return true;
                }
                input::KeyCode::Enter => {
                    if let Some(&(_, icon, _)) = filtered.get(self.start_menu.selected_pinned) {
                        self.launch_app_by_icon(icon);
                        self.start_menu.open = false;
                    }
                    return true;
                }
                input::KeyCode::Backspace => {
                    self.start_menu.search_query.pop();
                    self.start_menu.selected_pinned = 0;
                    return true;
                }
                input::KeyCode::Char(c) if !ev.modifiers.ctrl && !ev.modifiers.alt => {
                    if self.start_menu.search_query.len() < 24 {
                        self.start_menu.search_query.push(c);
                        self.start_menu.selected_pinned = 0;
                    }
                    return true;
                }
                _ => {}
            }
        }

        false
    }

    /// Executes a context menu action by its numerical id.
    pub fn execute_context_action(&mut self, action_id: u32) {
        match action_id {
            1 => crate::gui::notify("Desktop", "New folder created", crate::gui::NotificationKind::Success),
            2 => {
                self.wallpaper.clear();
                crate::gui::notify("Desktop", "Desktop refreshed", crate::gui::NotificationKind::Info);
            }
            3 => self.launch_app_by_icon(Icon::SysMon),
            4 => self.launch_app_by_icon(Icon::Terminal),
            101 => crate::gui::notify("Files", "Opening selected item...", crate::gui::NotificationKind::Info),
            102 => crate::gui::notify("Files", "Renaming selected item...", crate::gui::NotificationKind::Info),
            103 => crate::gui::notify("Files", "Item copied to clipboard", crate::gui::NotificationKind::Info),
            104 => crate::gui::notify("Files", "Item moved to Trash", crate::gui::NotificationKind::Warning),
            105 => crate::gui::notify("Properties", "TriOS RAMFS Object (Read/Write)", crate::gui::NotificationKind::Info),
            201 => crate::gui::notify("Editor", "Cut text to clipboard", crate::gui::NotificationKind::Info),
            202 => crate::gui::notify("Editor", "Copied text to clipboard", crate::gui::NotificationKind::Info),
            203 => crate::gui::notify("Editor", "Pasted text from clipboard", crate::gui::NotificationKind::Info),
            204 => crate::gui::notify("Editor", "Selected all text", crate::gui::NotificationKind::Info),
            _ => {}
        }
    }

    /// Handles a click on the floating dock.
    pub fn taskbar_click(&mut self, x: i32, y: i32) -> Option<u64> {
        if self.dock_start_rect().contains(x, y) {
            self.start_menu.open = !self.start_menu.open;
            if self.start_menu.open {
                self.quick_settings.open = false;
                self.context_menu = None;
                self.start_menu.search_query.clear();
                self.start_menu.selected_pinned = 0;
                self.start_menu.power_menu_open = false;
            }
            return None;
        }
        if self.dock_tray_rect().contains(x, y) {
            self.quick_settings.open = !self.quick_settings.open;
            if self.quick_settings.open {
                self.start_menu.open = false;
                self.context_menu = None;
            }
            return None;
        }
        let targets = self.taskbar_targets?;
        let idx = self.dock_button_rects()
            .iter()
            .position(|r| r.contains(x, y))?;
        let id = match idx {
            0 => targets.files,
            1 => targets.editor,
            2 => targets.terminal,
            3 => targets.browser,
            4 => targets.sysmon,
            5 => targets.mines,
            _ => targets.settings,
        };
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            if w.minimized {
                w.minimized = false;
            } else if w.focused {
                w.minimized = true;
                return None;
            }
        }
        self.windows.iter().any(|w| w.id == id).then_some(id)
    }


    /// The dock's hit test zone.
    fn taskbar_rect(&self) -> Rect {
        self.dock_rect()
    }

    // -- input -------------------------------------------------------------

    /// Topmost non-minimized window containing the point.
    fn window_index_at(&self, x: i32, y: i32) -> Option<usize> {
        self.windows
            .iter()
            .rposition(|w| !w.minimized && w.rect.contains(x, y))
    }

    /// Handles clicks inside the Start Menu panel.
    pub fn handle_start_menu_click(&mut self, cx: i32, cy: i32, sm_r: Rect) {
        // 1. Power button check
        let pwr_r = Rect::new(sm_r.right() - 52, sm_r.y + sm_r.h - 44, 36, 34);
        if pwr_r.contains(cx, cy) {
            self.start_menu.power_menu_open = !self.start_menu.power_menu_open;
            return;
        }

        // 2. Power popup menu items check
        if self.start_menu.power_menu_open {
            let pm_r = Rect::new(sm_r.right() - 166, sm_r.y + sm_r.h - 174, 150, 114);
            if pm_r.contains(cx, cy) {
                let row = ((cy - pm_r.y) / 28).clamp(0, 3);
                match row {
                    0 => {
                        self.is_locked = true;
                        self.start_menu.power_menu_open = false;
                        self.start_menu.open = false;
                    }
                    1 => {
                        crate::gui::notify("Power", "System sleep (preview)", crate::gui::NotificationKind::Info);
                        self.start_menu.power_menu_open = false;
                        self.start_menu.open = false;
                    }
                    2 => crate::acpi::reboot(),
                    3 => crate::acpi::poweroff(),
                    _ => {}
                }
                return;
            } else {
                self.start_menu.power_menu_open = false;
            }
        }

        // 3. Search bar click (resets search query)
        let sb_r = Rect::new(sm_r.x + 20, sm_r.y + 16, sm_r.w - 40, 36);
        if sb_r.contains(cx, cy) {
            self.start_menu.search_query.clear();
            return;
        }

        // 4. Pinned 4x3 grid items
        let filtered = self.filtered_pinned_apps();
        let grid_x0 = sm_r.x + 20;
        let grid_y0 = sm_r.y + 88;
        let cell_w = (sm_r.w - 40) / 4;
        let cell_h = 70;

        for (i, &(_, icon, _)) in filtered.iter().enumerate() {
            if i >= 12 { break; }
            let col = (i as i32) % 4;
            let row = (i as i32) / 4;
            let cell_r = Rect::new(grid_x0 + col * cell_w, grid_y0 + row * cell_h, cell_w, cell_h);
            if cell_r.contains(cx, cy) {
                self.launch_app_by_icon(icon);
                self.start_menu.open = false;
                return;
            }
        }

        // 5. Recent items click
        let rec_y = sm_r.y + 336;
        if cy >= rec_y && cy < rec_y + 44 {
            self.launch_app_by_icon(Icon::Editor);
            self.start_menu.open = false;
        }
    }

    /// Handles clicks inside the Quick Settings tray panel.
    pub fn handle_quick_settings_click(&mut self, cx: i32, cy: i32, qs_r: Rect) {
        let tile_y = qs_r.y + 50;
        let tile_w = (qs_r.w - 40 - 16) / 3;
        let tile_h = 56;

        // Tile 1: Network
        let t1 = Rect::new(qs_r.x + 20, tile_y, tile_w, tile_h);
        if t1.contains(cx, cy) {
            self.quick_settings.network_on = !self.quick_settings.network_on;
            if self.quick_settings.network_on {
                crate::gui::notify("Network", "Connected to local network", crate::gui::NotificationKind::Success);
            } else {
                crate::gui::notify("Network", "Network interface disabled", crate::gui::NotificationKind::Warning);
            }
            return;
        }

        // Tile 2: DND
        let t2 = Rect::new(qs_r.x + 20 + tile_w + 8, tile_y, tile_w, tile_h);
        if t2.contains(cx, cy) {
            self.quick_settings.dnd_on = !self.quick_settings.dnd_on;
            let status = if self.quick_settings.dnd_on { "Do Not Disturb active" } else { "Do Not Disturb off" };
            crate::gui::notify("Focus", status, crate::gui::NotificationKind::Info);
            return;
        }

        // Tile 3: Night Light
        let t3 = Rect::new(qs_r.x + 20 + (tile_w + 8) * 2, tile_y, tile_w, tile_h);
        if t3.contains(cx, cy) {
            self.quick_settings.night_light_on = !self.quick_settings.night_light_on;
            let status = if self.quick_settings.night_light_on { "Night Light warm tint on" } else { "Night Light off" };
            crate::gui::notify("Display", status, crate::gui::NotificationKind::Info);
            return;
        }

        // Volume Slider track
        let track_w = qs_r.w - 40;
        let vol_track = Rect::new(qs_r.x + 20, qs_r.y + 136, track_w, 24);
        if vol_track.contains(cx, cy) {
            let v = ((cx - (qs_r.x + 20)) * 100 / track_w).clamp(0, 100) as u8;
            self.quick_settings.volume = v;
            return;
        }

        // Brightness Slider track
        let bri_track = Rect::new(qs_r.x + 20, qs_r.y + 186, track_w, 24);
        if bri_track.contains(cx, cy) {
            let b = ((cx - (qs_r.x + 20)) * 100 / track_w).clamp(10, 100) as u8;
            self.quick_settings.brightness = b;
        }
    }

    /// Feeds one mouse event through the interaction state machine.
    pub fn handle_mouse(&mut self, me: input::MouseEvent) -> Option<WidgetAction> {
        // 1. cursor, clamped to the screen.
        let max_x = (self.screen_w - 1).max(0);
        let max_y = (self.screen_h - 1).max(0);
        self.cursor.0 = (self.cursor.0 + me.dx).clamp(0, max_x);
        self.cursor.1 = (self.cursor.1 + me.dy).clamp(0, max_y);
        let (cx, cy) = self.cursor;

        let pressed = me.buttons.left && !self.prev_buttons.left;
        let released = !me.buttons.left && self.prev_buttons.left;
        let right_pressed = me.buttons.right && !self.prev_buttons.right;
        self.prev_buttons = me.buttons;

        if self.is_locked {
            if pressed {
                self.is_locked = false;
                crate::gui::notify("Session Unlocked", "Welcome back, Jojin", crate::gui::NotificationKind::Success);
            }
            return None;
        }

        // Context menu hover update
        if let Some(ref mut menu) = self.context_menu {
            if menu.rect.contains(cx, cy) {
                let mut cur_y = menu.rect.y + 5;
                let mut hovered_item = None;
                for (i, item) in menu.items.iter().enumerate() {
                    let item_h = if item.is_separator { 6 } else { 24 };
                    if !item.is_separator && cy >= cur_y && cy < cur_y + item_h {
                        hovered_item = Some(i);
                        break;
                    }
                    cur_y += item_h;
                }
                menu.hovered = hovered_item;
            } else {
                menu.hovered = None;
            }
        }

        // Right-click context menu creation
        if right_pressed {
            if let Some(idx) = self.window_index_at(cx, cy) {
                let win = &self.windows[idx];
                if win.title.contains("File") {
                    self.context_menu = Some(ContextMenu::new(
                        cx.min(self.screen_w - CONTEXT_MENU_W - 8),
                        cy.min(self.screen_h - 160),
                        CONTEXT_MENU_W,
                        alloc::vec![
                            ContextMenuItem::action(101, "Open", Some("Enter")),
                            ContextMenuItem::action(102, "Rename", Some("F2")),
                            ContextMenuItem::action(103, "Copy", Some("Ctrl+C")),
                            ContextMenuItem::action(104, "Delete", Some("Del")),
                            ContextMenuItem::separator(),
                            ContextMenuItem::action(105, "Properties", Some("Alt+Enter")),
                        ],
                    ));
                } else if win.title.contains("Editor") {
                    self.context_menu = Some(ContextMenu::new(
                        cx.min(self.screen_w - CONTEXT_MENU_W - 8),
                        cy.min(self.screen_h - 140),
                        CONTEXT_MENU_W,
                        alloc::vec![
                            ContextMenuItem::action(201, "Cut", Some("Ctrl+X")),
                            ContextMenuItem::action(202, "Copy", Some("Ctrl+C")),
                            ContextMenuItem::action(203, "Paste", Some("Ctrl+V")),
                            ContextMenuItem::separator(),
                            ContextMenuItem::action(204, "Select All", Some("Ctrl+A")),
                        ],
                    ));
                }
            } else if !self.taskbar_rect().contains(cx, cy) {
                self.context_menu = Some(ContextMenu::new(
                    cx.min(self.screen_w - CONTEXT_MENU_W - 8),
                    cy.min(self.screen_h - 130),
                    CONTEXT_MENU_W,
                    alloc::vec![
                        ContextMenuItem::action(1, "New Folder", None),
                        ContextMenuItem::action(2, "Refresh", Some("F5")),
                        ContextMenuItem::separator(),
                        ContextMenuItem::action(3, "Display Settings", None),
                        ContextMenuItem::action(4, "Open Terminal here", None),
                    ],
                ));
            }
        }

        // 2. press edge: choose window + zone.
        if pressed {
            self.drag = Drag::None;

            // Context Menu item click check
            if let Some(menu) = self.context_menu.take() {
                if menu.rect.contains(cx, cy) {
                    let mut cur_y = menu.rect.y + 5;
                    for item in &menu.items {
                        let item_h = if item.is_separator { 6 } else { 24 };
                        if !item.is_separator && cy >= cur_y && cy < cur_y + item_h {
                            self.execute_context_action(item.id);
                            return None;
                        }
                        cur_y += item_h;
                    }
                    return None;
                }
            }

            // Start Menu panel click check
            if self.start_menu.open {
                let sm_r = self.start_menu_rect();
                if sm_r.contains(cx, cy) {
                    self.handle_start_menu_click(cx, cy, sm_r);
                    return None;
                } else if !self.dock_start_rect().contains(cx, cy) {
                    self.start_menu.open = false;
                    self.start_menu.power_menu_open = false;
                }
            }

            // Quick Settings panel click check
            if self.quick_settings.open {
                let qs_r = self.quick_settings_rect();
                if qs_r.contains(cx, cy) {
                    self.handle_quick_settings_click(cx, cy, qs_r);
                    return None;
                } else if !self.dock_tray_rect().contains(cx, cy) {
                    self.quick_settings.open = false;
                }
            }

            if self.taskbar_rect().contains(cx, cy) {
                if let Some(id) = self.taskbar_click(cx, cy) {
                    self.focus_and_type(id);
                }
            } else if let Some(idx) = self.window_index_at(cx, cy) {
                let (id, zone, widget_target) = {
                    let win = &self.windows[idx];
                    let lx = cx - win.rect.x;
                    let ly = cy - win.rect.y;
                    let zone = if win.close_rect().contains(lx, ly) {
                        Zone::Close
                    } else if win.maximize_rect().contains(lx, ly) {
                        Zone::Maximize
                    } else if win.minimize_rect().contains(lx, ly) {
                        Zone::Minimize
                    } else if win.grip_rect().contains(lx, ly) {
                        Zone::Grip
                    } else if win.titlebar_rect().contains(lx, ly) {
                        Zone::Titlebar
                    } else {
                        Zone::Body
                    };
                    let target = if zone == Zone::Body {
                        win.widgets
                            .iter()
                            .enumerate()
                            .rev()
                            .find(|(_, wg)| wg.contains(lx, ly))
                            .map(|(i, _)| i)
                    } else {
                        None
                    };
                    (win.id, zone, target)
                };

                self.focus(id);

                if let Some(idx) = self.windows.iter().position(|w| w.id == id) {
                    let (rect_x, rect_y) = {
                        let w = &self.windows[idx].rect;
                        (w.x, w.y)
                    };
                    let (lx, ly) = (cx - rect_x, cy - rect_y);
                    match zone {
                        Zone::Close => self.windows[idx].close_requested = true,
                        Zone::Minimize => self.windows[idx].minimized = true,
                        Zone::Maximize => {
                            let w = &mut self.windows[idx];
                            if w.maximized {
                                w.rect = w.saved_rect;
                                w.maximized = false;
                            } else {
                                w.saved_rect = w.rect;
                                w.rect = Rect::new(
                                    12,
                                    12,
                                    (self.screen_w - 24).max(MIN_W),
                                    (self.screen_h - DOCK_H - 28).max(MIN_H),
                                );
                                w.maximized = true;
                            }
                        }
                        Zone::Grip => self.drag = Drag::Resize { id },
                        Zone::Titlebar => {
                            self.drag = Drag::Move {
                                id,
                                off_x: lx,
                                off_y: ly,
                            }
                        }
                        Zone::Body => {
                            let text_focus = match widget_target {
                                Some(wi)
                                    if matches!(
                                        self.windows[idx].widgets.get(wi),
                                        Some(Widget::TextInput(_))
                                    ) =>
                                {
                                    Some(wi)
                                }
                                _ => None,
                            };
                            self.windows[idx].focused_widget = text_focus;
                            sync_text_focus(&mut self.windows[idx]);
                            if let Some(wi) = widget_target
                                && self.windows[idx].widgets[wi].pointer_down(lx, ly)
                            {
                                self.drag = Drag::Widget { id, index: wi };
                            }
                        }
                    }
                }
            } else {
                let icons = self.desktop_icon_rects();
                if let Some(idx) = icons.iter().position(|r| r.contains(cx, cy)) {
                    let now = self.tick_count;
                    let is_double = if let Some((last_idx, last_tick)) = self.last_desktop_click {
                        last_idx == idx && now.saturating_sub(last_tick) <= 60
                    } else {
                        false
                    };
                    self.selected_desktop_icon = Some(idx);
                    self.last_desktop_click = Some((idx, now));
                    if is_double {
                        if let Some(item) = DESKTOP_ITEMS.get(idx) {
                            self.launch_app_by_icon(item.0);
                        }
                    }
                } else {
                    self.selected_desktop_icon = None;
                }
            }
        }

        // 3. release edge: finish the capturing widget's interaction.
        let mut action = None;
        if released {
            if let Drag::Widget { id, index } = self.drag {
                if let Some(idx) = self.windows.iter().position(|w| w.id == id) {
                    let w = &mut self.windows[idx];
                    let (lx, ly) = (cx - w.rect.x, cy - w.rect.y);
                    if let Some(outcome) = w
                        .widgets
                        .get_mut(index)
                        .and_then(|wg| wg.pointer_up(lx, ly))
                    {
                        action = Some(WidgetAction {
                            window_id: id,
                            widget_index: index,
                            outcome,
                        });
                    }
                }
            }
            self.drag = Drag::None;
        }

        // 4. still holding: apply window geometry drags.
        if me.buttons.left {
            match self.drag {
                Drag::Move { id, off_x, off_y } => {
                    if let Some(idx) = self.windows.iter().position(|w| w.id == id) {
                        let r = &mut self.windows[idx].rect;
                        let min_x = 60 - r.w;
                        let max_x = self.screen_w - 60;
                        let max_y = (self.screen_h - TITLEBAR_H).max(0);
                        r.x = (cx - off_x).clamp(min_x.min(max_x), max_x);
                        r.y = (cy - off_y).clamp(0, max_y);
                    }
                }
                Drag::Resize { id } => {
                    if let Some(idx) = self.windows.iter().position(|w| w.id == id) {
                        let r = &mut self.windows[idx].rect;
                        let max_w = (self.screen_w - r.x).max(MIN_W);
                        let max_h = (self.screen_h - r.y).max(MIN_H);
                        r.w = (cx - r.x).clamp(MIN_W, max_w);
                        r.h = (cy - r.y).clamp(MIN_H, max_h);
                    }
                }
                Drag::Widget { .. } | Drag::None => {}
            }
        }

        // 5. widget hover/drag routing.
        let captured = match self.drag {
            Drag::Widget { id, index } => Some((id, index)),
            _ => None,
        };
        for win in &mut self.windows {
            if win.minimized {
                continue;
            }
            let (lx, ly) = (cx - win.rect.x, cy - win.rect.y);
            for (i, wg) in win.widgets.iter_mut().enumerate() {
                let is_captured = captured == Some((win.id, i));
                wg.pointer_move(lx, ly, is_captured);
            }
        }

        action
    }

    /// Routes a key event to the focused window (Phase 3: Esc closes it).
    pub fn handle_key(&mut self, ev: input::KeyEvent) {
        if let Some(id) = self.focused_id
            && let Some(idx) = self.windows.iter().position(|w| w.id == id)
        {
            self.windows[idx].handle_key(ev);
        }
    }

    // -- composition -------------------------------------------------------

    /// Ensures the procedural wallpaper is cached into `self.wallpaper`.
    /// Rendered once onto the backbuffer at boot / resolution change, then blitted
    /// with zero CPU gradient overhead for 60+ FPS performance.
    pub fn ensure_wallpaper(&mut self, g: &mut Graphics, theme: &Theme) {
        if self.wallpaper.len() == g.back.len() {
            return;
        }
        let w = g.width() as i32;
        let h = g.height() as i32;

        // 1. Dark vertical gradient: #0B0F1A to #101A33
        g.fill_vertical_gradient(0, 0, w as u32, h as u32, theme.bg, Color::new(0x10, 0x1A, 0x33));

        // 2. Subtle dot-grid every 24px (#162038)
        let dot_color = Color::new(0x16, 0x20, 0x38);
        for y in (12..h).step_by(24) {
            for x in (12..w).step_by(24) {
                g.set_pixel(x, y, dot_color);
            }
        }

        // 3. Large faint TriOS logo on the right of center (~12% opacity)
        let cx = (w * 66) / 100;
        let cy = h / 2;
        Self::draw_faint_trios_emblem(g, cx, cy, 140, theme);

        self.wallpaper = g.back.clone();
    }

    fn draw_faint_trios_emblem(g: &mut Graphics, cx: i32, cy: i32, size: i32, _theme: &Theme) {
        let faint_cyan = Color::new(0x12, 0x28, 0x40);
        let faint_violet = Color::new(0x1E, 0x1C, 0x44);

        for r in (size - 32..=size).step_by(2) {
            let p1 = (cx, cy - r);
            let p2 = (cx - (r * 866) / 1000, cy + r / 2);
            let p3 = (cx + (r * 866) / 1000, cy + r / 2);
            g.draw_line(p1.0, p1.1, p2.0, p2.1, faint_cyan);
            g.draw_line(p2.0, p2.1, p3.0, p3.1, faint_violet);
            g.draw_line(p3.0, p3.1, p1.0, p1.1, faint_cyan);
        }

        let inner_r = size / 2;
        for r in (inner_r - 18..=inner_r).step_by(2) {
            let p1 = (cx, cy + r);
            let p2 = (cx - (r * 866) / 1000, cy - r / 2);
            let p3 = (cx + (r * 866) / 1000, cy - r / 2);
            let faint_mid = Color::new(0x19, 0x22, 0x44);
            g.draw_line(p1.0, p1.1, p2.0, p2.1, faint_mid);
            g.draw_line(p2.0, p2.1, p3.0, p3.1, faint_mid);
            g.draw_line(p3.0, p3.1, p1.0, p1.1, faint_mid);
        }
    }

    /// Paints the whole desktop into the backbuffer: wallpaper, desktop icons,
    /// non-minimized windows with modern glass chrome, floating dock, Start Menu,
    /// Quick Settings, Context Menu, Notifications, Alt+Tab overlay, and the software cursor.
    pub fn compose(&mut self, g: &mut Graphics, theme: &Theme) {
        self.ensure_wallpaper(g, theme);
        g.blit_full(&self.wallpaper);

        if self.is_locked {
            self.draw_lock_screen(g, theme);
            draw_cursor(g, self.cursor);
            return;
        }

        self.draw_desktop_icons(g, theme);

        for win in &self.windows {
            if win.minimized {
                continue;
            }
            Self::draw_window(g, theme, win, self.cursor);
        }

        self.draw_dock(g, theme);

        if self.start_menu.open {
            self.draw_start_menu(g, theme);
        }

        if self.quick_settings.open {
            self.draw_quick_settings(g, theme);
        }

        if let Some(ref menu) = self.context_menu {
            Self::draw_context_menu(g, theme, menu);
        }

        if !self.quick_settings.dnd_on {
            Self::draw_notifications(g, theme, self.screen_w);
        }

        if self.alt_tab.open {
            self.draw_alt_tab(g, theme);
        }

        if self.quick_settings.night_light_on {
            // Warm night light filter: softly attenuates harsh blue tones
            for chunk in g.back.chunks_exact_mut(4) {
                chunk[0] = (chunk[0] as u16 * 210 / 256) as u8;
                chunk[1] = (chunk[1] as u16 * 242 / 256) as u8;
                chunk[2] = (chunk[2] as u16 * 255 / 256).min(255) as u8;
            }
        }

        // Real screen brightness dimming: scale pixel luminance by brightness level (20%..=100%)
        let brightness = self.quick_settings.brightness.clamp(20, 100) as u16;
        if brightness < 100 {
            for chunk in g.back.chunks_exact_mut(4) {
                chunk[0] = (chunk[0] as u16 * brightness / 100) as u8;
                chunk[1] = (chunk[1] as u16 * brightness / 100) as u8;
                chunk[2] = (chunk[2] as u16 * brightness / 100) as u8;
            }
        }

        draw_cursor(g, self.cursor);
    }

    /// Draws clickable desktop shortcut icons on the left side of the workspace.
    fn draw_desktop_icons(&self, g: &mut Graphics, theme: &Theme) {
        let icons = self.desktop_icon_rects();
        let (cx, cy) = self.cursor;

        for (i, (&(icon, label), r)) in DESKTOP_ITEMS.iter().zip(icons.iter()).enumerate() {
            let is_hover = r.contains(cx, cy);
            let is_sel = self.selected_desktop_icon == Some(i);

            if is_sel {
                g.fill_rounded_rect(r.x, r.y, r.w as u32, r.h as u32, BUTTON_RADIUS, theme.selection_bg);
                g.draw_rounded_rect(r.x, r.y, r.w as u32, r.h as u32, BUTTON_RADIUS, theme.accent_cyan);
            } else if is_hover {
                g.fill_rounded_rect(r.x, r.y, r.w as u32, r.h as u32, BUTTON_RADIUS, theme.card_bg_hover);
                g.draw_rounded_rect(r.x, r.y, r.w as u32, r.h as u32, BUTTON_RADIUS, theme.border);
            }

            let ix = r.x + (r.w - 48) / 2;
            let iy = r.y + 4;
            icon.draw(g, ix, iy, 48, theme);

            let tw = Graphics::text_width(label, 1);
            let tx = r.x + (r.w - tw) / 2;
            let ty = r.y + 56;
            g.draw_text(tx + 1, ty + 1, label, Color::new(0x05, 0x08, 0x0E), None, 1);
            g.draw_text(tx, ty, label, theme.text, None, 1);
        }
    }

    /// Draws one window: elevation shadow, rounded body + titlebar, widgets,
    /// three window controls (minimize, maximize/restore, close), resize grip, and border.
    fn draw_window(g: &mut Graphics, theme: &Theme, win: &Window, cursor: (i32, i32)) {
        let r = win.rect;

        // Elevation shadow around window
        if win.focused {
            g.fill_rounded_rect(
                r.x - 3,
                r.y - 1,
                (r.w + 6) as u32,
                (r.h + 8) as u32,
                WINDOW_RADIUS + 3,
                Color::new(0x04, 0x07, 0x10),
            );
            g.fill_rounded_rect(
                r.x - 1,
                r.y,
                (r.w + 2) as u32,
                (r.h + 5) as u32,
                WINDOW_RADIUS + 1,
                Color::new(0x06, 0x0B, 0x18),
            );
        } else {
            g.fill_rounded_rect(
                r.x - 1,
                r.y,
                (r.w + 2) as u32,
                (r.h + 4) as u32,
                WINDOW_RADIUS + 1,
                Color::new(0x04, 0x06, 0x0E),
            );
        }

        // Window background
        g.fill_rounded_rect(r.x, r.y, r.w as u32, r.h as u32, WINDOW_RADIUS, theme.surface);

        // Titlebar
        let titlebar_bg = if win.focused {
            theme.surface_hi
        } else {
            theme.surface
        };
        g.fill_rounded_rect(r.x, r.y, r.w as u32, TITLEBAR_H as u32, WINDOW_RADIUS, titlebar_bg);
        // Square off bottom corners of titlebar
        g.fill_rect(
            r.x,
            r.y + TITLEBAR_H - WINDOW_RADIUS,
            r.w as u32,
            WINDOW_RADIUS as u32,
            titlebar_bg,
        );

        if win.focused {
            // Subtle accent line on top edge of focused titlebar
            g.fill_rect(
                r.x + WINDOW_RADIUS,
                r.y,
                (r.w - 2 * WINDOW_RADIUS) as u32,
                1,
                theme.accent_cyan,
            );
        }

        // Title text
        let title_limit = r.x + r.w - (CLOSE_SIZE * 3 + 24);
        let mut tx = r.x + 12;
        let ty = r.y + (TITLEBAR_H - 8) / 2;
        for ch in win.title.chars() {
            if tx + 8 > title_limit {
                break;
            }
            g.draw_char(tx, ty, ch, theme.text, None, 1);
            tx += 8;
        }

        // Widgets
        for wg in &win.widgets {
            wg.draw(g, r.x, r.y, theme);
        }

        // Window controls on the right: Minimize [—], Maximize [□], Close [✕]
        let (cx_cur, cy_cur) = cursor;

        // 1. Minimize button
        let min_r = win.minimize_rect();
        let (mx0, my0) = (r.x + min_r.x, r.y + min_r.y);
        let is_min_hover = cx_cur >= mx0 && cx_cur < mx0 + min_r.w && cy_cur >= my0 && cy_cur < my0 + min_r.h;
        if is_min_hover {
            g.fill_rounded_rect(mx0, my0, min_r.w as u32, min_r.h as u32, BUTTON_RADIUS, theme.surface_hi);
            g.draw_rounded_rect(mx0, my0, min_r.w as u32, min_r.h as u32, BUTTON_RADIUS, theme.border);
        }
        let min_col = if is_min_hover { theme.text } else { theme.text_dim };
        g.draw_line(mx0 + 4, my0 + min_r.h - 6, mx0 + min_r.w - 5, my0 + min_r.h - 6, min_col);

        // 2. Maximize / Restore button
        let max_r = win.maximize_rect();
        let (ax0, ay0) = (r.x + max_r.x, r.y + max_r.y);
        let is_max_hover = cx_cur >= ax0 && cx_cur < ax0 + max_r.w && cy_cur >= ay0 && cy_cur < ay0 + max_r.h;
        if is_max_hover {
            g.fill_rounded_rect(ax0, ay0, max_r.w as u32, max_r.h as u32, BUTTON_RADIUS, theme.surface_hi);
            g.draw_rounded_rect(ax0, ay0, max_r.w as u32, max_r.h as u32, BUTTON_RADIUS, theme.border);
        }
        let max_col = if is_max_hover { theme.text } else { theme.text_dim };
        if win.maximized {
            // Restore icon: two overlapping boxes
            g.draw_rect(ax0 + 5, ay0 + 3, (max_r.w - 9) as u32, (max_r.h - 9) as u32, max_col);
            g.draw_rect(ax0 + 3, ay0 + 6, (max_r.w - 9) as u32, (max_r.h - 9) as u32, max_col);
        } else {
            // Maximize icon: single box
            g.draw_rect(ax0 + 4, ay0 + 4, (max_r.w - 8) as u32, (max_r.h - 8) as u32, max_col);
        }

        // 3. Close button
        let cls_r = win.close_rect();
        let (cx0, cy0) = (r.x + cls_r.x, r.y + cls_r.y);
        let is_cls_hover = cx_cur >= cx0 && cx_cur < cx0 + cls_r.w && cy_cur >= cy0 && cy_cur < cy0 + cls_r.h;
        if is_cls_hover {
            g.fill_rounded_rect(cx0, cy0, cls_r.w as u32, cls_r.h as u32, BUTTON_RADIUS, theme.danger);
        }
        let cls_col = if is_cls_hover { Color::new(0xFF, 0xFF, 0xFF) } else { theme.text_dim };
        let inset = 5;
        g.draw_line(
            cx0 + inset,
            cy0 + inset,
            cx0 + cls_r.w - inset - 1,
            cy0 + cls_r.h - inset - 1,
            cls_col,
        );
        g.draw_line(
            cx0 + cls_r.w - inset - 1,
            cy0 + inset,
            cx0 + inset,
            cy0 + cls_r.h - inset - 1,
            cls_col,
        );

        // Resize grip: two parallel diagonal ticks
        let (gr, gb) = (r.right() - 2, r.bottom() - 2);
        g.draw_line(gr - 10, gb - 3, gr - 3, gb - 10, theme.text_dim);
        g.draw_line(gr - 6, gb - 3, gr - 3, gb - 6, theme.text_dim);

        // Border last — focused windows get the accent frame
        let border = if win.focused {
            theme.accent_cyan
        } else {
            theme.border
        };
        g.draw_rounded_rect(r.x, r.y, r.w as u32, r.h as u32, WINDOW_RADIUS, border);
    }



    /// Draws the floating centered rounded dock at bottom of screen.
    fn draw_dock(&self, g: &mut Graphics, theme: &Theme) {
        let dock_r = self.dock_rect();
        let (cx, cy) = self.cursor;

        // Elevation shadow underneath dock
        g.fill_rounded_rect(
            dock_r.x - 3,
            dock_r.y - 2,
            (dock_r.w + 6) as u32,
            (dock_r.h + 8) as u32,
            18,
            Color::new(0x04, 0x06, 0x0E),
        );

        // Glass surface body
        g.fill_rounded_rect(
            dock_r.x,
            dock_r.y,
            dock_r.w as u32,
            dock_r.h as u32,
            16,
            theme.surface,
        );
        // Top highlight sheen
        g.fill_rect(
            dock_r.x + 16,
            dock_r.y + 1,
            (dock_r.w - 32) as u32,
            1,
            Color::new(0x24, 0x33, 0x54),
        );
        // Border
        g.draw_rounded_rect(
            dock_r.x,
            dock_r.y,
            dock_r.w as u32,
            dock_r.h as u32,
            16,
            theme.border,
        );

        // 1. Start Button (TriOS logo)
        let start_r = self.dock_start_rect();
        let is_start_hover = start_r.contains(cx, cy);
        if is_start_hover {
            g.fill_rounded_rect(
                start_r.x,
                start_r.y,
                start_r.w as u32,
                start_r.h as u32,
                BUTTON_RADIUS,
                theme.surface_hi,
            );
            g.draw_rounded_rect(
                start_r.x,
                start_r.y,
                start_r.w as u32,
                start_r.h as u32,
                BUTTON_RADIUS,
                theme.border,
            );
        }
        let sx = start_r.x + 10;
        let sy = start_r.y + 10;
        g.fill_rect(sx + 6, sy, 8, 8, theme.accent_cyan);
        g.fill_rect(sx, sy + 11, 8, 8, theme.accent_violet);
        g.fill_rect(sx + 12, sy + 11, 8, 8, theme.accent_cyan);
        g.draw_rect(sx + 6, sy, 8, 8, Color::new(0xFF, 0xFF, 0xFF));

        // Separator 1
        let sep1_x = start_r.right() + 6;
        g.fill_rect(sep1_x, dock_r.y + 12, 1, 32, theme.border);

        // 2. App Buttons (6 icons)
        let buttons = self.dock_button_rects();
        let targets = self.taskbar_targets;

        for (i, r) in buttons.iter().enumerate() {
            let is_hover = r.contains(cx, cy);
            if is_hover {
                g.fill_rounded_rect(
                    r.x,
                    r.y,
                    r.w as u32,
                    r.h as u32,
                    BUTTON_RADIUS,
                    theme.surface_hi,
                );
                g.draw_rounded_rect(
                    r.x,
                    r.y,
                    r.w as u32,
                    r.h as u32,
                    BUTTON_RADIUS,
                    theme.border,
                );
            }

            let ix = r.x + (r.w - 32) / 2;
            let iy = r.y + (r.h - 32) / 2;

            let icon = match i {
                0 => Icon::Files,
                1 => Icon::Editor,
                2 => Icon::Terminal,
                3 => Icon::Browser,
                4 => Icon::SysMon,
                5 => Icon::Mines,
                _ => Icon::Settings,
            };
            icon.draw(g, ix, iy, 32, theme);

            // Running indicator dot underneath icon
            if let Some(tgt) = targets {
                let target_id = match i {
                    0 => tgt.files,
                    1 => tgt.editor,
                    2 => tgt.terminal,
                    3 => tgt.browser,
                    4 => tgt.sysmon,
                    5 => tgt.mines,
                    _ => tgt.settings,
                };
                if let Some(win) = self.windows.iter().find(|w| w.id == target_id) {
                    let dot_color = if win.focused {
                        theme.accent_cyan
                    } else {
                        theme.text_dim
                    };
                    let dot_w = if win.focused { 6 } else { 4 };
                    let dot_x = r.x + (r.w - dot_w) / 2;
                    let dot_y = dock_r.y + dock_r.h - 5;
                    g.fill_rect(dot_x, dot_y, dot_w as u32, 2, dot_color);
                }
            }
        }

        // Separator 2
        let sep2_x = buttons[6].right() + 6;
        g.fill_rect(sep2_x, dock_r.y + 12, 1, 32, theme.border);

        // 3. System Tray (Network, Volume, 2-line IST clock)
        let tray_r = self.dock_tray_rect();
        let is_tray_hover = tray_r.contains(cx, cy);
        if is_tray_hover || self.quick_settings.open {
            g.fill_rounded_rect(tray_r.x, tray_r.y, tray_r.w as u32, tray_r.h as u32, BUTTON_RADIUS, theme.surface_hi);
            g.draw_rounded_rect(tray_r.x, tray_r.y, tray_r.w as u32, tray_r.h as u32, BUTTON_RADIUS, theme.border);
        }

        let tray_x = sep2_x + 10;
        let tray_y = dock_r.y + 14;

        // Network icon: signal bars
        let n_col = if self.quick_settings.network_on { theme.accent_cyan } else { theme.text_dim };
        g.fill_rect(tray_x, tray_y + 10, 3, 5, n_col);
        g.fill_rect(tray_x + 4, tray_y + 6, 3, 9, n_col);
        g.fill_rect(tray_x + 8, tray_y + 2, 3, 13, n_col);

        // Volume icon: speaker
        let vx = tray_x + 18;
        g.fill_rect(vx, tray_y + 5, 3, 6, theme.text);
        g.draw_line(vx + 3, tray_y + 5, vx + 7, tray_y + 2, theme.text);
        g.draw_line(vx + 3, tray_y + 10, vx + 7, tray_y + 13, theme.text);
        g.draw_line(vx + 7, tray_y + 2, vx + 7, tray_y + 13, theme.text);
        g.draw_line(vx + 10, tray_y + 4, vx + 10, tray_y + 11, theme.text_dim);

        // 2-Line Clock & Date (IST: UTC +5:30)
        draw_ist_clock(g, dock_r.right() - 10, tray_y + 2, theme);
    }

    /// Draws the Start Menu overlay panel.
    fn draw_start_menu(&self, g: &mut Graphics, theme: &Theme) {
        let sm = self.start_menu_rect();
        let (cx, cy) = self.cursor;

        // Shadow behind start menu
        g.fill_rounded_rect(
            sm.x - 4,
            sm.y - 3,
            (sm.w + 8) as u32,
            (sm.h + 10) as u32,
            WINDOW_RADIUS + 4,
            Color::new(0x04, 0x06, 0x10),
        );

        // Glass background
        g.fill_rounded_rect(
            sm.x,
            sm.y,
            sm.w as u32,
            sm.h as u32,
            WINDOW_RADIUS + 2,
            theme.surface,
        );

        // Top sheen
        g.fill_rect(
            sm.x + 16,
            sm.y + 1,
            (sm.w - 32) as u32,
            1,
            Color::new(0x28, 0x37, 0x58),
        );

        // Border
        g.draw_rounded_rect(
            sm.x,
            sm.y,
            sm.w as u32,
            sm.h as u32,
            WINDOW_RADIUS + 2,
            theme.border,
        );

        // 1. Search Bar
        let sb = Rect::new(sm.x + 20, sm.y + 16, sm.w - 40, 36);
        let is_sb_hover = sb.contains(cx, cy);
        let sb_border = if is_sb_hover || !self.start_menu.search_query.is_empty() {
            theme.accent_cyan
        } else {
            theme.border
        };
        g.fill_rounded_rect(sb.x, sb.y, sb.w as u32, sb.h as u32, BUTTON_RADIUS, theme.surface_hi);
        g.draw_rounded_rect(sb.x, sb.y, sb.w as u32, sb.h as u32, BUTTON_RADIUS, sb_border);

        // Search icon (magnifier circle + diagonal handle)
        let sx = sb.x + 12;
        let sy = sb.y + 11;
        g.draw_rect(sx, sy, 9, 9, theme.accent_cyan);
        g.draw_line(sx + 8, sy + 8, sx + 13, sy + 13, theme.accent_cyan);

        // Search text or placeholder
        if self.start_menu.search_query.is_empty() {
            g.draw_text(sb.x + 32, sb.y + 11, "Type here to search apps...", theme.text_dim, None, 1);
        } else {
            let tx = sb.x + 32;
            g.draw_text(tx, sb.y + 11, &self.start_menu.search_query, theme.text, None, 1);
            let tw = Graphics::text_width(&self.start_menu.search_query, 1);
            g.fill_rect(tx + tw + 2, sb.y + 9, 2, 16, theme.accent_cyan);
        }

        // 2. "Pinned" Section Header
        g.draw_text(sm.x + 24, sm.y + 66, "Pinned", theme.text, None, 1);
        let all_apps_hint = "All apps >";
        let aaw = Graphics::text_width(all_apps_hint, 1);
        g.draw_text(sm.right() - 24 - aaw, sm.y + 66, all_apps_hint, theme.text_dim, None, 1);

        // 3. Pinned 4x3 Grid
        let filtered = self.filtered_pinned_apps();
        let grid_x0 = sm.x + 20;
        let grid_y0 = sm.y + 88;
        let cell_w = (sm.w - 40) / 4;
        let cell_h = 70;

        for (i, &(_, icon, name)) in filtered.iter().enumerate() {
            if i >= 12 { break; }
            let col = (i as i32) % 4;
            let row = (i as i32) / 4;
            let cell = Rect::new(grid_x0 + col * cell_w, grid_y0 + row * cell_h, cell_w, cell_h);
            let is_hover = cell.contains(cx, cy);
            let is_selected = i == self.start_menu.selected_pinned;

            if is_selected {
                g.fill_rounded_rect(cell.x + 2, cell.y + 2, (cell.w - 4) as u32, (cell.h - 4) as u32, BUTTON_RADIUS, theme.selection_bg);
                g.draw_rounded_rect(cell.x + 2, cell.y + 2, (cell.w - 4) as u32, (cell.h - 4) as u32, BUTTON_RADIUS, theme.accent_cyan);
            } else if is_hover {
                g.fill_rounded_rect(cell.x + 2, cell.y + 2, (cell.w - 4) as u32, (cell.h - 4) as u32, BUTTON_RADIUS, theme.surface_hi);
                g.draw_rounded_rect(cell.x + 2, cell.y + 2, (cell.w - 4) as u32, (cell.h - 4) as u32, BUTTON_RADIUS, theme.border);
            }

            let ix = cell.x + (cell.w - 32) / 2;
            let iy = cell.y + 6;
            icon.draw(g, ix, iy, 32, theme);

            let tw = Graphics::text_width(name, 1);
            let tx = cell.x + (cell.w - tw) / 2;
            g.draw_text(tx, cell.y + 46, name, theme.text, None, 1);
        }

        // 4. "Recent" Section
        let rec_y = sm.y + 316;
        g.draw_text(sm.x + 24, rec_y, "Recent", theme.text, None, 1);

        let rec_items = [
            (Icon::FileRs, "main.rs", "TriOS Kernel Core"),
            (Icon::FileTxt, "welcome.txt", "Notes & Docs"),
            (Icon::FileElf, "doom.elf", "Executable Binary"),
        ];
        let rec_w = (sm.w - 40 - 16) / 3;
        for (i, (icon, title, desc)) in rec_items.iter().enumerate() {
            let rx = sm.x + 20 + i as i32 * (rec_w + 8);
            let ry = rec_y + 20;
            let rr = Rect::new(rx, ry, rec_w, 44);
            let is_rh = rr.contains(cx, cy);
            if is_rh {
                g.fill_rounded_rect(rr.x, rr.y, rr.w as u32, rr.h as u32, BUTTON_RADIUS, theme.surface_hi);
                g.draw_rounded_rect(rr.x, rr.y, rr.w as u32, rr.h as u32, BUTTON_RADIUS, theme.border);
            } else {
                g.fill_rounded_rect(rr.x, rr.y, rr.w as u32, rr.h as u32, BUTTON_RADIUS, theme.surface);
                g.draw_rounded_rect(rr.x, rr.y, rr.w as u32, rr.h as u32, BUTTON_RADIUS, theme.border);
            }
            icon.draw(g, rx + 8, ry + 10, 24, theme);
            g.draw_text(rx + 36, ry + 8, title, theme.text, None, 1);
            g.draw_text(rx + 36, ry + 24, desc, theme.text_dim, None, 1);
        }

        // 5. Separator line above footer
        let sep_y = sm.y + sm.h - 54;
        g.fill_rect(sm.x + 16, sep_y, (sm.w - 32) as u32, 1, theme.border);

        // 6. User chip
        let chip_x = sm.x + 20;
        let chip_y = sep_y + 10;
        let chip_r = Rect::new(chip_x, chip_y, 140, 36);
        if chip_r.contains(cx, cy) {
            g.fill_rounded_rect(chip_r.x, chip_r.y, chip_r.w as u32, chip_r.h as u32, BUTTON_RADIUS, theme.surface_hi);
        }
        // Avatar circle
        let av_cx = chip_x + 18;
        let av_cy = chip_y + 18;
        for dy in -12..=12 {
            for dx in -12..=12 {
                if dx * dx + dy * dy <= 144 {
                    g.set_pixel(av_cx + dx, av_cy + dy, theme.accent_cyan);
                }
            }
        }
        g.draw_char(av_cx - 4, av_cy - 4, 'J', Color::new(0xFF, 0xFF, 0xFF), None, 1);
        g.draw_text(chip_x + 36, chip_y + 4, "Jojin", theme.text, None, 1);
        g.draw_text(chip_x + 36, chip_y + 18, "Administrator", theme.text_dim, None, 1);

        // 7. Power button
        let pwr_r = Rect::new(sm.right() - 52, chip_y + 2, 34, 32);
        let is_pwr_hover = pwr_r.contains(cx, cy);
        if is_pwr_hover || self.start_menu.power_menu_open {
            g.fill_rounded_rect(pwr_r.x, pwr_r.y, pwr_r.w as u32, pwr_r.h as u32, BUTTON_RADIUS, theme.surface_hi);
            g.draw_rounded_rect(pwr_r.x, pwr_r.y, pwr_r.w as u32, pwr_r.h as u32, BUTTON_RADIUS, theme.accent_cyan);
        } else {
            g.draw_rounded_rect(pwr_r.x, pwr_r.y, pwr_r.w as u32, pwr_r.h as u32, BUTTON_RADIUS, theme.border);
        }
        // Power icon (circle + tick)
        let px = pwr_r.x + 17;
        let py = pwr_r.y + 16;
        for dy in -7i32..=7i32 {
            for dx in -7i32..=7i32 {
                let dist = dx * dx + dy * dy;
                if dist >= 25 && dist <= 49 && (dy > -3 || dx.abs() > 2) {
                    g.set_pixel(px + dx, py + dy, theme.text);
                }
            }
        }
        g.fill_rect(px - 1, py - 8, 2, 7, theme.danger);

        // 8. Power popup menu
        if self.start_menu.power_menu_open {
            let pm_w = 150;
            let pm_h = 114;
            let pm_x = sm.right() - pm_w - 16;
            let pm_y = sm.y + sm.h - pm_h - 60;

            g.fill_rounded_rect(pm_x - 2, pm_y - 2, (pm_w + 4) as u32, (pm_h + 4) as u32, BUTTON_RADIUS + 2, Color::new(0x04, 0x06, 0x10));
            g.fill_rounded_rect(pm_x, pm_y, pm_w as u32, pm_h as u32, BUTTON_RADIUS, theme.surface_hi);
            g.draw_rounded_rect(pm_x, pm_y, pm_w as u32, pm_h as u32, BUTTON_RADIUS, theme.border);

            let options = ["Lock", "Sleep (preview)", "Restart", "Shut down"];
            for (oi, &opt) in options.iter().enumerate() {
                let oy = pm_y + 4 + oi as i32 * 26;
                let or = Rect::new(pm_x + 4, oy, pm_w - 8, 24);
                let is_opt_hover = or.contains(cx, cy) || oi == self.start_menu.power_menu_selected;
                if is_opt_hover {
                    let h_col = if oi == 3 { theme.danger } else { theme.selection_bg };
                    g.fill_rounded_rect(or.x, or.y, or.w as u32, or.h as u32, BUTTON_RADIUS - 2, h_col);
                }
                let t_col = if is_opt_hover && oi == 3 { Color::new(0xFF, 0xFF, 0xFF) } else { theme.text };
                g.draw_text(or.x + 12, or.y + 6, opt, t_col, None, 1);
            }
        }
    }

    /// Draws the Quick Settings panel.
    fn draw_quick_settings(&self, g: &mut Graphics, theme: &Theme) {
        let qs = self.quick_settings_rect();

        // Shadow behind panel
        g.fill_rounded_rect(
            qs.x - 4,
            qs.y - 3,
            (qs.w + 8) as u32,
            (qs.h + 10) as u32,
            WINDOW_RADIUS + 3,
            Color::new(0x04, 0x06, 0x10),
        );

        // Glass background
        g.fill_rounded_rect(
            qs.x,
            qs.y,
            qs.w as u32,
            qs.h as u32,
            WINDOW_RADIUS + 2,
            theme.surface,
        );

        // Highlight sheen
        g.fill_rect(
            qs.x + 16,
            qs.y + 1,
            (qs.w - 32) as u32,
            1,
            Color::new(0x28, 0x37, 0x58),
        );

        // Border
        g.draw_rounded_rect(
            qs.x,
            qs.y,
            qs.w as u32,
            qs.h as u32,
            WINDOW_RADIUS + 2,
            theme.border,
        );

        // Header
        g.draw_text(qs.x + 20, qs.y + 16, "Quick Settings", theme.text, None, 1);
        g.draw_text(qs.x + 20, qs.y + 32, "AC Power | Battery 100%", theme.text_dim, None, 1);

        // 3 Toggle Tiles
        let tile_y = qs.y + 50;
        let tile_w = (qs.w - 40 - 16) / 3;
        let tile_h = 56;

        // Tile 1: Network
        let t1 = Rect::new(qs.x + 20, tile_y, tile_w, tile_h);
        let t1_bg = if self.quick_settings.network_on { theme.surface_hi } else { theme.surface };
        let t1_bd = if self.quick_settings.network_on { theme.accent_cyan } else { theme.border };
        g.fill_rounded_rect(t1.x, t1.y, t1.w as u32, t1.h as u32, BUTTON_RADIUS, t1_bg);
        g.draw_rounded_rect(t1.x, t1.y, t1.w as u32, t1.h as u32, BUTTON_RADIUS, t1_bd);
        let n_col = if self.quick_settings.network_on { theme.accent_cyan } else { theme.text_dim };
        g.fill_rect(t1.x + 12, t1.y + 18, 3, 4, n_col);
        g.fill_rect(t1.x + 16, t1.y + 14, 3, 8, n_col);
        g.fill_rect(t1.x + 20, t1.y + 10, 3, 12, n_col);
        g.draw_text(t1.x + 12, t1.y + 26, "Network", theme.text, None, 1);
        let n_sub = if self.quick_settings.network_on { "Connected" } else { "Off" };
        g.draw_text(t1.x + 12, t1.y + 40, n_sub, theme.text_dim, None, 1);

        // Tile 2: Do Not Disturb
        let t2 = Rect::new(qs.x + 20 + tile_w + 8, tile_y, tile_w, tile_h);
        let t2_bg = if self.quick_settings.dnd_on { theme.surface_hi } else { theme.surface };
        let t2_bd = if self.quick_settings.dnd_on { theme.accent_violet } else { theme.border };
        g.fill_rounded_rect(t2.x, t2.y, t2.w as u32, t2.h as u32, BUTTON_RADIUS, t2_bg);
        g.draw_rounded_rect(t2.x, t2.y, t2.w as u32, t2.h as u32, BUTTON_RADIUS, t2_bd);
        let d_col = if self.quick_settings.dnd_on { theme.accent_violet } else { theme.text_dim };
        g.fill_rect(t2.x + 14, t2.y + 12, 8, 8, d_col);
        g.fill_rect(t2.x + 12, t2.y + 18, 12, 3, d_col);
        g.draw_text(t2.x + 12, t2.y + 26, "DND", theme.text, None, 1);
        let d_sub = if self.quick_settings.dnd_on { "Active" } else { "Off" };
        g.draw_text(t2.x + 12, t2.y + 40, d_sub, theme.text_dim, None, 1);

        // Tile 3: Night Light
        let t3 = Rect::new(qs.x + 20 + (tile_w + 8) * 2, tile_y, tile_w, tile_h);
        let amber = Color::new(0xF5, 0x9E, 0x0B);
        let t3_bg = if self.quick_settings.night_light_on { theme.surface_hi } else { theme.surface };
        let t3_bd = if self.quick_settings.night_light_on { amber } else { theme.border };
        g.fill_rounded_rect(t3.x, t3.y, t3.w as u32, t3.h as u32, BUTTON_RADIUS, t3_bg);
        g.draw_rounded_rect(t3.x, t3.y, t3.w as u32, t3.h as u32, BUTTON_RADIUS, t3_bd);
        let l_col = if self.quick_settings.night_light_on { amber } else { theme.text_dim };
        g.draw_rect(t3.x + 14, t3.y + 11, 8, 8, l_col);
        g.fill_rect(t3.x + 16, t3.y + 13, 6, 6, l_col);
        g.draw_text(t3.x + 12, t3.y + 26, "Night", theme.text, None, 1);
        let l_sub = if self.quick_settings.night_light_on { "Warm" } else { "Off" };
        g.draw_text(t3.x + 12, t3.y + 40, l_sub, theme.text_dim, None, 1);

        // Volume Slider
        let track_w = qs.w - 40;
        let v_val = self.quick_settings.volume as i32;
        g.draw_text(qs.x + 20, qs.y + 122, "Volume (preview)", theme.text, None, 1);
        let v_txt = alloc::format!("{v_val}%");
        g.draw_text(qs.right() - 44, qs.y + 122, &v_txt, theme.text_dim, None, 1);

        let vy = qs.y + 144;
        g.fill_rounded_rect(qs.x + 20, vy, track_w as u32, 8, 4, theme.surface_hi);
        let fill_vw = (track_w * v_val / 100).clamp(0, track_w);
        if fill_vw > 0 {
            g.fill_rounded_rect(qs.x + 20, vy, fill_vw as u32, 8, 4, theme.accent_cyan);
        }
        let knob_vx = qs.x + 20 + fill_vw;
        g.fill_rounded_rect(knob_vx - 6, vy - 4, 12, 16, 6, Color::new(0xFF, 0xFF, 0xFF));

        // Brightness Slider
        let b_val = self.quick_settings.brightness as i32;
        g.draw_text(qs.x + 20, qs.y + 172, "Brightness", theme.text, None, 1);
        let b_txt = alloc::format!("{b_val}%");
        g.draw_text(qs.right() - 44, qs.y + 172, &b_txt, theme.text_dim, None, 1);

        let by = qs.y + 194;
        g.fill_rounded_rect(qs.x + 20, by, track_w as u32, 8, 4, theme.surface_hi);
        let fill_bw = (track_w * b_val / 100).clamp(0, track_w);
        if fill_bw > 0 {
            g.fill_rounded_rect(qs.x + 20, by, fill_bw as u32, 8, 4, amber);
        }
        let knob_bx = qs.x + 20 + fill_bw;
        g.fill_rounded_rect(knob_bx - 6, by - 4, 12, 16, 6, Color::new(0xFF, 0xFF, 0xFF));

        // Footer
        let f_sep = qs.y + qs.h - 38;
        g.fill_rect(qs.x + 16, f_sep, (qs.w - 32) as u32, 1, theme.border);
        g.draw_text(qs.x + 20, f_sep + 12, "TriOS Settings | v1.0.0", theme.text_dim, None, 1);
    }

    /// Draws a floating context menu popup widget.
    fn draw_context_menu(g: &mut Graphics, theme: &Theme, menu: &ContextMenu) {
        let r = menu.rect;

        // Shadow
        g.fill_rounded_rect(r.x - 3, r.y - 2, (r.w + 6) as u32, (r.h + 8) as u32, BUTTON_RADIUS + 2, Color::new(0x04, 0x06, 0x10));
        g.fill_rounded_rect(r.x, r.y, r.w as u32, r.h as u32, BUTTON_RADIUS, theme.surface_hi);
        g.draw_rounded_rect(r.x, r.y, r.w as u32, r.h as u32, BUTTON_RADIUS, theme.border);

        let mut cur_y = r.y + 5;
        for (i, item) in menu.items.iter().enumerate() {
            if item.is_separator {
                g.fill_rect(r.x + 8, cur_y + 2, (r.w - 16) as u32, 1, theme.border);
                cur_y += 6;
            } else {
                let row_r = Rect::new(r.x + 4, cur_y, r.w - 8, 22);
                let is_hover = menu.hovered == Some(i);
                if is_hover {
                    let h_col = if item.danger { theme.danger } else { theme.selection_bg };
                    g.fill_rounded_rect(row_r.x, row_r.y, row_r.w as u32, row_r.h as u32, BUTTON_RADIUS - 2, h_col);
                }
                let t_col = if is_hover && item.danger {
                    Color::new(0xFF, 0xFF, 0xFF)
                } else if item.danger {
                    theme.danger
                } else {
                    theme.text
                };
                g.draw_text(row_r.x + 8, row_r.y + 5, &item.label, t_col, None, 1);

                if let Some(ref sc) = item.shortcut {
                    let sw = Graphics::text_width(sc, 1);
                    g.draw_text(row_r.right() - sw - 8, row_r.y + 5, sc, theme.text_dim, None, 1);
                }
                cur_y += 24;
            }
        }
    }

    /// Draws top-right stacked desktop toast notifications.
    fn draw_notifications(g: &mut Graphics, theme: &Theme, screen_w: i32) {
        let toasts = crate::gui::tick_and_get_toasts();
        for (i, toast) in toasts.iter().enumerate().take(3) {
            let tx = screen_w - TOAST_W - 16;
            let ty = 16 + (i as i32) * (TOAST_H + 10);

            // Shadow
            g.fill_rounded_rect(tx - 3, ty - 2, (TOAST_W + 6) as u32, (TOAST_H + 6) as u32, BUTTON_RADIUS + 2, Color::new(0x04, 0x06, 0x10));
            // Card
            g.fill_rounded_rect(tx, ty, TOAST_W as u32, TOAST_H as u32, BUTTON_RADIUS, theme.surface_hi);
            g.draw_rounded_rect(tx, ty, TOAST_W as u32, TOAST_H as u32, BUTTON_RADIUS, theme.border);

            // Left indicator stripe
            let ind_col = match toast.kind {
                crate::gui::NotificationKind::Success => theme.success,
                crate::gui::NotificationKind::Info => theme.accent_cyan,
                crate::gui::NotificationKind::Warning => theme.warning,
                crate::gui::NotificationKind::Error => theme.danger,
            };
            g.fill_rounded_rect(tx + 2, ty + 4, 4, (TOAST_H - 8) as u32, 2, ind_col);

            // Title & Body
            g.draw_text(tx + 14, ty + 10, &toast.title, theme.text, None, 1);
            g.draw_text(tx + 14, ty + 28, &toast.body, theme.text_dim, None, 1);

            // Remaining life progress bar at bottom
            if toast.initial_ticks > 0 {
                let bar_w = ((TOAST_W - 28) * (toast.remaining_ticks as i32) / (toast.initial_ticks as i32)).max(0);
                g.fill_rect(tx + 14, ty + TOAST_H - 5, bar_w as u32, 2, ind_col);
            }
        }
    }

    /// Draws the Alt+Tab window switcher overlay.
    fn draw_alt_tab(&self, g: &mut Graphics, theme: &Theme) {
        let open_wins: Vec<&Window> = self.windows.iter().filter(|w| !w.minimized).collect();
        let count = open_wins.len();
        if count == 0 { return; }

        let item_w = 80;
        let item_h = 72;
        let at_w = ((count as i32 * (item_w + 10)) + 30).clamp(280, self.screen_w - 40);
        let at_h = 126;
        let at_x = (self.screen_w - at_w) / 2;
        let at_y = (self.screen_h - at_h) / 2;

        // Shadow & panel
        g.fill_rounded_rect(at_x - 4, at_y - 3, (at_w + 8) as u32, (at_h + 8) as u32, WINDOW_RADIUS + 3, Color::new(0x04, 0x06, 0x10));
        g.fill_rounded_rect(at_x, at_y, at_w as u32, at_h as u32, WINDOW_RADIUS + 2, theme.surface);
        g.draw_rounded_rect(at_x, at_y, at_w as u32, at_h as u32, WINDOW_RADIUS + 2, theme.border);

        g.draw_text(at_x + 16, at_y + 12, "Task Switcher (Alt+Tab)", theme.text_dim, None, 1);

        let items_x0 = at_x + 16;
        let items_y0 = at_y + 36;

        for (i, win) in open_wins.iter().enumerate() {
            let rx = items_x0 + i as i32 * (item_w + 10);
            let ry = items_y0;
            let card = Rect::new(rx, ry, item_w, item_h);
            let is_sel = i == self.alt_tab.selected;

            if is_sel {
                g.fill_rounded_rect(card.x, card.y, card.w as u32, card.h as u32, BUTTON_RADIUS, theme.selection_bg);
                g.draw_rounded_rect(card.x, card.y, card.w as u32, card.h as u32, BUTTON_RADIUS, theme.accent_cyan);
            } else {
                g.fill_rounded_rect(card.x, card.y, card.w as u32, card.h as u32, BUTTON_RADIUS, theme.surface_hi);
                g.draw_rounded_rect(card.x, card.y, card.w as u32, card.h as u32, BUTTON_RADIUS, theme.border);
            }

            let icon = if win.title.contains("File") {
                Icon::Files
            } else if win.title.contains("Editor") {
                Icon::Editor
            } else if win.title.contains("Terminal") {
                Icon::Terminal
            } else if win.title.contains("Browser") {
                Icon::Browser
            } else if win.title.contains("SysMon") || win.title.contains("About") {
                Icon::SysMon
            } else if win.title.contains("Mine") {
                Icon::Mines
            } else {
                Icon::ThisPc
            };
            icon.draw(g, card.x + (card.w - 32) / 2, card.y + 8, 32, theme);

            let short_title = if win.title.len() > 8 { &win.title[..8] } else { &win.title };
            let tw = Graphics::text_width(short_title, 1);
            g.draw_text(card.x + (card.w - tw) / 2, card.y + 48, short_title, theme.text, None, 1);
        }
    }

    /// Draws the centered frosted glass Lock / Login Screen (Win+L / Start Menu -> Lock).
    fn draw_lock_screen(&self, g: &mut Graphics, theme: &Theme) {
        // Dim the desktop wallpaper with a dark translucent frosted glass overlay
        for chunk in g.back.chunks_exact_mut(4) {
            chunk[0] = (chunk[0] as u16 * 65 / 256) as u8;
            chunk[1] = (chunk[1] as u16 * 65 / 256) as u8;
            chunk[2] = (chunk[2] as u16 * 80 / 256) as u8;
        }

        let card_w = 380;
        let card_h = 420;
        let cx = (self.screen_w - card_w) / 2;
        let cy = (self.screen_h - card_h) / 2 - 20;

        // Shadow & frosted glass card body
        g.fill_rounded_rect(cx - 6, cy - 4, (card_w + 12) as u32, (card_h + 16) as u32, 20, Color::new(0x04, 0x06, 0x10));
        g.fill_rounded_rect(cx, cy, card_w as u32, card_h as u32, 16, theme.surface);
        g.draw_rounded_rect(cx, cy, card_w as u32, card_h as u32, 16, theme.accent_cyan);

        // User Avatar Circle
        let avatar_r = 36;
        let av_cx = cx + card_w / 2;
        let av_cy = cy + 62;
        let r2 = avatar_r * avatar_r;
        for dy in -avatar_r..=avatar_r {
            let dx = crate::graphics::framebuffer::int_sqrt(r2 - dy * dy);
            g.fill_rect(av_cx - dx, av_cy + dy, (dx * 2 + 1) as u32, 1, theme.surface_hi);
        }
        for dy in -avatar_r..=avatar_r {
            let dx = crate::graphics::framebuffer::int_sqrt(r2 - dy * dy);
            g.fill_rect(av_cx - dx, av_cy + dy, 2, 1, theme.accent_cyan);
            g.fill_rect(av_cx + dx - 1, av_cy + dy, 2, 1, theme.accent_cyan);
        }
        // Avatar monogram "J"
        g.draw_text_ttf(av_cx - 6, av_cy - 10, "J", Color::new(0xFF, 0xFF, 0xFF), crate::graphics::FontType::UI);

        // Username
        let user_str = "Jojin";
        let uw = g.measure_ttf(user_str, crate::graphics::FontType::UI);
        g.draw_text_ttf(cx + (card_w - uw) / 2, cy + 115, user_str, theme.text, crate::graphics::FontType::UI);

        // Role chip
        let role_str = "Administrator";
        let rw = g.measure_ttf(role_str, crate::graphics::FontType::UI);
        g.draw_text_ttf(cx + (card_w - rw) / 2, cy + 138, role_str, theme.accent_cyan, crate::graphics::FontType::UI);

        // Live RTC Clock
        let dt = crate::timer::read_rtc();
        let time_str = alloc::format!("{:02}:{:02}", dt.hour, dt.minute);
        let tw = g.measure_ttf(&time_str, crate::graphics::FontType::UI);
        g.draw_text_ttf(cx + (card_w - tw) / 2, cy + 175, &time_str, Color::new(0xFF, 0xFF, 0xFF), crate::graphics::FontType::UI);

        let date_str = alloc::format!("{:04}-{:02}-{:02}  •  IST", dt.year, dt.month, dt.day);
        let dw = g.measure_ttf(&date_str, crate::graphics::FontType::UI);
        g.draw_text_ttf(cx + (card_w - dw) / 2, cy + 205, &date_str, theme.text_dim, crate::graphics::FontType::UI);

        // Password field
        let pw_x = cx + 40;
        let pw_y = cy + 250;
        let pw_w = card_w - 80;
        let pw_h = 36;
        g.fill_rounded_rect(pw_x, pw_y, pw_w as u32, pw_h as u32, 6, theme.surface_hi);
        g.draw_rounded_rect(pw_x, pw_y, pw_w as u32, pw_h as u32, 6, theme.border);
        g.draw_text_ttf(pw_x + 14, pw_y + 10, "••••••••••••", theme.accent_cyan, crate::graphics::FontType::UI);

        // Unlock Button
        let btn_x = cx + 60;
        let btn_y = cy + 305;
        let btn_w = card_w - 120;
        let btn_h = 36;
        g.fill_rounded_rect(btn_x, btn_y, btn_w as u32, btn_h as u32, 6, theme.accent_cyan);
        let unlock_str = "Unlock Session";
        let un_w = g.measure_ttf(unlock_str, crate::graphics::FontType::UI);
        g.draw_text_ttf(btn_x + (btn_w - un_w) / 2, btn_y + 10, unlock_str, Color::new(0x0B, 0x0F, 0x1A), crate::graphics::FontType::UI);

        // Bottom hint
        let hint_str = "Press Enter or Click to Unlock";
        let hw = g.measure_ttf(hint_str, crate::graphics::FontType::UI);
        g.draw_text_ttf(cx + (card_w - hw) / 2, cy + 365, hint_str, theme.text_dim, crate::graphics::FontType::UI);
    }
}

/// Draws 2-line clock (HH:MM and DD Mon) in Indian Standard Time (UTC +5:30).
fn draw_ist_clock(g: &mut Graphics, right: i32, y: i32, theme: &Theme) {
    let rtc = crate::timer::read_rtc();
    let mut total_mins =
        (rtc.hour as i32 + UTC_OFFSET_HOURS) * 60 + rtc.minute as i32 + UTC_OFFSET_MINS;
    let mut day = rtc.day as i32;
    let month_idx = (rtc.month.saturating_sub(1) as usize) % 12;

    if total_mins >= 24 * 60 {
        total_mins -= 24 * 60;
        day += 1;
    } else if total_mins < 0 {
        total_mins += 24 * 60;
        day = (day - 1).max(1);
    }
    let ist_hour = (total_mins / 60) as u8;
    let ist_min = (total_mins % 60) as u8;

    // Line 1: HH:MM
    let mut time_buf = [b'0'; 5];
    time_buf[0] = b'0' + (ist_hour / 10);
    time_buf[1] = b'0' + (ist_hour % 10);
    time_buf[2] = b':';
    time_buf[3] = b'0' + (ist_min / 10);
    time_buf[4] = b'0' + (ist_min % 10);

    let time_w = 5 * 8;
    let time_x = right - time_w;
    for (i, &b) in time_buf.iter().enumerate() {
        g.draw_char(time_x + (i as i32 * 8), y, b as char, theme.text, None, 1);
    }

    // Line 2: DD Mon (e.g. 18 Oct)
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mon = MONTHS[month_idx];
    let mut date_buf = [b' '; 6];
    date_buf[0] = b'0' + ((day / 10) as u8);
    date_buf[1] = b'0' + ((day % 10) as u8);
    date_buf[2] = b' ';
    let mon_bytes = mon.as_bytes();
    date_buf[3] = mon_bytes[0];
    date_buf[4] = mon_bytes[1];
    date_buf[5] = mon_bytes[2];

    let date_w = 6 * 8;
    let date_x = right - date_w;
    for (i, &b) in date_buf.iter().enumerate() {
        g.draw_char(date_x + (i as i32 * 8), y + 12, b as char, theme.text_dim, None, 1);
    }
}

pub const DESKTOP_ITEMS: [(Icon, &str); 9] = [
    (Icon::ThisPc, "This PC"),
    (Icon::Files, "Files"),
    (Icon::Editor, "Editor"),
    (Icon::Terminal, "Terminal"),
    (Icon::Browser, "Browser"),
    (Icon::SysMon, "SysMon"),
    (Icon::Mines, "Mines"),
    (Icon::Settings, "Settings"),
    (Icon::Trash, "Trash"),
];

pub const PINNED_APPS: [(Icon, &str); 12] = [
    (Icon::Files, "Files"),
    (Icon::Editor, "Editor"),
    (Icon::Terminal, "Terminal"),
    (Icon::Browser, "Browser"),
    (Icon::SysMon, "SysMon"),
    (Icon::Mines, "Mines"),
    (Icon::Settings, "Settings"),
    (Icon::ThisPc, "This PC"),
    (Icon::Trash, "Trash"),
    (Icon::FileTxt, "Notes"),
    (Icon::FileElf, "Games"),
    (Icon::Folder, "Storage"),
];

fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let n_bytes = needle.as_bytes();
    let h_bytes = haystack.as_bytes();
    if n_bytes.len() > h_bytes.len() {
        return false;
    }
    for i in 0..=(h_bytes.len() - n_bytes.len()) {
        let mut matched = true;
        for j in 0..n_bytes.len() {
            if !h_bytes[i + j].eq_ignore_ascii_case(&n_bytes[j]) {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}


// ---------------------------------------------------------------------------
// The software cursor
// ---------------------------------------------------------------------------

/// The pointer sprite: `X` = black outline, `W` = white fill, anything
/// else transparent. Classic arrow, 12 rows, hotspot at (0, 0).
const CURSOR: [&str; 12] = [
    "X",
    "XX",
    "XWX",
    "XWWX",
    "XWWWX",
    "XWWWWX",
    "XWWWWWX",
    "XWWWWWWX",
    "XWWWWWWWX",
    "XWWXXXXXX",
    "XWXWX",
    "XX..XWX",
];

/// Draws the cursor at `at` — always the *last* thing composited, so it
/// sits above every window. Frame rebuild is full-screen anyway (see
/// module docs), so no under-cursor backup buffer is needed; if partial
/// (dirty-rect) redraws arrive in a later phase, this is the function
/// to give save/restore semantics.
fn draw_cursor(g: &mut Graphics, at: (i32, i32)) {
    let (x0, y0) = at;
    for (row, line) in CURSOR.iter().enumerate() {
        for (col, byte) in line.bytes().enumerate() {
            let color = match byte {
                b'X' => Color::rgb(0x000000),
                b'W' => Color::rgb(0xFF_FFFF),
                _ => continue,
            };
            g.set_pixel(x0 + col as i32, y0 + row as i32, color);
        }
    }
}
