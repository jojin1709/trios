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

use super::theme::Theme;
use super::widgets::{Widget, WidgetOutcome};
use crate::graphics::{Color, Graphics};
use crate::input::{self, mouse};

#[allow(unused_imports)]
pub use super::theme::{
    BUTTON_RADIUS, DOCK_H, SHADOW_BLUR, TITLEBAR_H, UTC_OFFSET_HOURS, UTC_OFFSET_MINS,
    WINDOW_RADIUS,
};

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
    /// Frames remaining for the boot toast hint.
    toast_frames: u32,
    /// First click flag to dismiss toast early.
    first_click: bool,
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
            toast_frames: 300,
            first_click: false,
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
    pub fn set_taskbar_targets(&mut self, files: u64, editor: u64, terminal: u64, browser: u64, sysmon: u64, mines: u64) {
        self.taskbar_targets = Some(TaskbarTargets {
            files,
            editor,
            terminal,
            browser,
            sysmon,
            mines,
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
        let dock_w = 540.min(self.screen_w - 16);
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

    /// Global rects for the 6 application buttons inside the dock.
    pub fn dock_button_rects(&self) -> [Rect; 6] {
        let d = self.dock_rect();
        let icon_x0 = d.x + 8 + 40 + 10;
        let mut rects = [Rect::new(0, 0, 40, 40); 6];
        for (i, r) in rects.iter_mut().enumerate() {
            r.x = icon_x0 + (i as i32 * 46);
            r.y = d.y + (d.h - 40) / 2;
            r.w = 40;
            r.h = 40;
        }
        rects
    }

    /// Handles a click on the floating dock.
    pub fn taskbar_click(&mut self, x: i32, y: i32) -> Option<u64> {
        let targets = self.taskbar_targets?;
        if self.dock_start_rect().contains(x, y) {
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == targets.sysmon) {
                if w.minimized {
                    w.minimized = false;
                }
            }
            return self.windows.iter().any(|w| w.id == targets.sysmon).then_some(targets.sysmon);
        }
        let idx = self.dock_button_rects()
            .iter()
            .position(|r| r.contains(x, y))?;
        let id = match idx {
            0 => targets.files,
            1 => targets.editor,
            2 => targets.terminal,
            3 => targets.browser,
            4 => targets.sysmon,
            _ => targets.mines,
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

    /// Hit-tests desktop shortcut icons (when clicked on bare desktop).
    pub fn desktop_icon_click(&mut self, x: i32, y: i32) -> Option<u64> {
        let targets = self.taskbar_targets?;
        let icons = desktop_icon_rects();
        let idx = icons.iter().position(|r| r.contains(x, y))?;
        let id = match idx {
            0 => targets.sysmon,
            1 => targets.files,
            2 => targets.editor,
            3 => targets.terminal,
            4 => targets.browser,
            _ => targets.mines,
        };
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.minimized = false;
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
        self.prev_buttons = me.buttons;

        // 2. press edge: choose window + zone.
        if pressed {
            self.first_click = true;
            self.drag = Drag::None;

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
            } else if let Some(id) = self.desktop_icon_click(cx, cy) {
                self.focus_and_type(id);
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
    /// non-minimized windows with modern glass chrome, floating toast, floating dock,
    /// and the software cursor.
    pub fn compose(&mut self, g: &mut Graphics, theme: &Theme) {
        self.ensure_wallpaper(g, theme);
        g.blit_full(&self.wallpaper);

        self.draw_desktop_icons(g, theme);

        for win in &self.windows {
            if win.minimized {
                continue;
            }
            Self::draw_window(g, theme, win, self.cursor);
        }

        self.draw_toast(g, theme);
        self.draw_dock(g, theme);
        draw_cursor(g, self.cursor);
    }

    /// Draws clickable desktop shortcut icons on the left side of the workspace.
    fn draw_desktop_icons(&self, g: &mut Graphics, theme: &Theme) {
        let icons = desktop_icon_rects();
        let (cx, cy) = self.cursor;

        for (i, (&label, r)) in DESKTOP_ICON_LABELS.iter().zip(icons.iter()).enumerate() {
            let is_hover = r.contains(cx, cy);
            if is_hover {
                g.fill_rounded_rect(r.x, r.y, r.w as u32, r.h as u32, BUTTON_RADIUS, theme.surface_hi);
                g.draw_rounded_rect(r.x, r.y, r.w as u32, r.h as u32, BUTTON_RADIUS, theme.border);
            }

            let ix = r.x + (r.w - 24) / 2;
            let iy = r.y + 6;

            match i {
                0 => {
                    // This PC (Monitor)
                    g.draw_rect(ix, iy, 24, 16, theme.text);
                    g.fill_rect(ix + 2, iy + 2, 20, 12, theme.accent_cyan);
                    g.fill_rect(ix + 10, iy + 16, 4, 3, theme.text_dim);
                    g.fill_rect(ix + 6, iy + 19, 12, 2, theme.text);
                }
                1 => {
                    // Files (Folder)
                    g.fill_rect(ix + 2, iy + 2, 8, 4, Color::new(0xF5, 0x9E, 0x0B));
                    g.fill_rect(ix + 2, iy + 5, 20, 15, Color::new(0xD9, 0x77, 0x06));
                    g.fill_rect(ix + 4, iy + 8, 18, 12, Color::new(0xFB, 0xBF, 0x24));
                }
                2 => {
                    // Editor (Document)
                    g.fill_rect(ix + 4, iy + 2, 16, 20, Color::new(0xF1, 0xF5, 0xF9));
                    g.draw_rect(ix + 4, iy + 2, 16, 20, Color::new(0x94, 0xA3, 0xB8));
                    g.fill_rect(ix + 7, iy + 6, 10, 2, Color::new(0x64, 0x74, 0x8B));
                    g.fill_rect(ix + 7, iy + 10, 10, 2, Color::new(0x64, 0x74, 0x8B));
                    g.fill_rect(ix + 7, iy + 14, 6, 2, Color::new(0x64, 0x74, 0x8B));
                }
                3 => {
                    // Terminal (Console)
                    g.fill_rect(ix + 2, iy + 3, 20, 18, Color::new(0x0F, 0x17, 0x2A));
                    g.draw_rect(ix + 2, iy + 3, 20, 18, Color::new(0x47, 0x55, 0x69));
                    g.fill_rect(ix + 2, iy + 3, 20, 4, Color::new(0x1E, 0x29, 0x3B));
                    g.draw_text(ix + 5, iy + 8, ">", Color::new(0x22, 0xC5, 0x5E), None, 1);
                    g.fill_rect(ix + 12, iy + 14, 5, 2, Color::new(0x22, 0xC5, 0x5E));
                }
                4 => {
                    // Browser (Globe)
                    g.draw_rect(ix + 4, iy + 4, 16, 16, Color::new(0x02, 0x84, 0xC7));
                    g.fill_rect(ix + 6, iy + 6, 12, 12, Color::new(0x03, 0x69, 0xA1));
                    g.draw_line(ix + 4, iy + 12, ix + 20, iy + 12, Color::new(0x38, 0xBD, 0xF8));
                    g.draw_line(ix + 12, iy + 4, ix + 12, iy + 20, Color::new(0x38, 0xBD, 0xF8));
                }
                _ => {
                    // Mines (Retro Mine)
                    g.fill_rect(ix + 6, iy + 6, 12, 12, Color::new(0xE1, 0x1D, 0x48));
                    g.draw_rect(ix + 6, iy + 6, 12, 12, Color::new(0xF8, 0x71, 0x71));
                    g.draw_line(ix + 12, iy + 3, ix + 12, iy + 21, Color::new(0xF8, 0x71, 0x71));
                    g.draw_line(ix + 3, iy + 12, ix + 21, iy + 12, Color::new(0xF8, 0x71, 0x71));
                    g.fill_rect(ix + 10, iy + 10, 4, 4, Color::new(0xFF, 0xFF, 0xFF));
                }
            }

            let tw = Graphics::text_width(label, 1);
            let tx = r.x + (r.w - tw) / 2;
            let ty = r.y + 36;
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

    /// Draws floating toast notification at bottom-left (auto-hides after 5s or first click).
    fn draw_toast(&mut self, g: &mut Graphics, theme: &Theme) {
        if self.toast_frames == 0 || self.first_click {
            return;
        }
        self.toast_frames = self.toast_frames.saturating_sub(1);

        let tx = 16;
        let ty = self.screen_h - 48;
        let tw = 370;
        let th = 28;

        g.fill_rounded_rect(tx, ty, tw as u32, th as u32, BUTTON_RADIUS, theme.surface_hi);
        g.draw_rounded_rect(tx, ty, tw as u32, th as u32, BUTTON_RADIUS, theme.border);

        // Cyan status dot
        g.fill_rect(tx + 10, ty + 10, 8, 8, theme.accent_cyan);
        g.draw_rect(tx + 10, ty + 10, 8, 8, Color::new(0xFF, 0xFF, 0xFF));

        let msg = "Click to capture | Right Ctrl = release | Ctrl+1..6";
        g.draw_text(tx + 24, ty + 10, msg, theme.text, None, 1);
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

            let ix = r.x + (r.w - 24) / 2;
            let iy = r.y + (r.h - 24) / 2;

            match i {
                0 => {
                    // Files (Folder)
                    g.fill_rect(ix + 2, iy + 3, 9, 4, Color::new(0xF5, 0x9E, 0x0B));
                    g.fill_rect(ix + 2, iy + 6, 20, 14, Color::new(0xD9, 0x77, 0x06));
                    g.fill_rect(ix + 4, iy + 9, 17, 10, Color::new(0xFB, 0xBF, 0x24));
                }
                1 => {
                    // Editor (Code Document)
                    g.fill_rect(ix + 3, iy + 2, 18, 20, Color::new(0xE6, 0xEA, 0xF5));
                    g.draw_rect(ix + 3, iy + 2, 18, 20, Color::new(0x8A, 0x93, 0xAD));
                    g.fill_rect(ix + 6, iy + 6, 12, 2, Color::new(0x22, 0xD3, 0xEE));
                    g.fill_rect(ix + 6, iy + 10, 10, 2, Color::new(0x8B, 0x5C, 0xF6));
                    g.fill_rect(ix + 6, iy + 14, 7, 2, Color::new(0x34, 0xD3, 0x99));
                }
                2 => {
                    // Terminal (Console)
                    g.fill_rect(ix + 2, iy + 3, 20, 18, Color::new(0x0B, 0x0F, 0x1A));
                    g.draw_rect(ix + 2, iy + 3, 20, 18, theme.border);
                    g.draw_text(ix + 5, iy + 7, ">", Color::new(0x34, 0xD3, 0x99), None, 1);
                    g.fill_rect(ix + 12, iy + 13, 5, 2, Color::new(0x34, 0xD3, 0x99));
                }
                3 => {
                    // Browser (Globe)
                    g.draw_rect(ix + 3, iy + 3, 18, 18, Color::new(0x02, 0x84, 0xC7));
                    g.fill_rect(ix + 5, iy + 5, 14, 14, Color::new(0x03, 0x69, 0xA1));
                    g.draw_line(ix + 3, iy + 12, ix + 21, iy + 12, Color::new(0x38, 0xBD, 0xF8));
                    g.draw_line(ix + 12, iy + 3, ix + 12, iy + 21, Color::new(0x38, 0xBD, 0xF8));
                }
                4 => {
                    // SysMon (Telemetry Graph)
                    g.draw_rect(ix + 2, iy + 4, 20, 16, Color::new(0x1C, 0x25, 0x40));
                    g.fill_rect(ix + 3, iy + 5, 18, 14, Color::new(0x0B, 0x0F, 0x1A));
                    g.draw_line(ix + 4, iy + 13, ix + 7, iy + 13, theme.accent_cyan);
                    g.draw_line(ix + 7, iy + 13, ix + 10, iy + 7, theme.accent_cyan);
                    g.draw_line(ix + 10, iy + 7, ix + 13, iy + 16, theme.accent_cyan);
                    g.draw_line(ix + 13, iy + 16, ix + 16, iy + 11, theme.accent_cyan);
                    g.draw_line(ix + 16, iy + 11, ix + 19, iy + 11, theme.accent_cyan);
                }
                _ => {
                    // Mines (TriSweeper)
                    g.fill_rect(ix + 5, iy + 5, 14, 14, Color::new(0xE1, 0x1D, 0x48));
                    g.draw_rect(ix + 5, iy + 5, 14, 14, theme.danger);
                    g.draw_line(ix + 12, iy + 2, ix + 12, iy + 22, theme.danger);
                    g.draw_line(ix + 2, iy + 12, ix + 22, iy + 12, theme.danger);
                    g.fill_rect(ix + 9, iy + 9, 4, 4, Color::new(0xFF, 0xFF, 0xFF));
                }
            }

            // Running indicator dot underneath icon
            if let Some(tgt) = targets {
                let target_id = match i {
                    0 => tgt.files,
                    1 => tgt.editor,
                    2 => tgt.terminal,
                    3 => tgt.browser,
                    4 => tgt.sysmon,
                    _ => tgt.mines,
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
        let sep2_x = buttons[5].right() + 6;
        g.fill_rect(sep2_x, dock_r.y + 12, 1, 32, theme.border);

        // 3. System Tray (Network, Volume, 2-line IST clock)
        let tray_x = sep2_x + 10;
        let tray_y = dock_r.y + 14;

        // Network icon: signal bars
        g.fill_rect(tray_x, tray_y + 10, 3, 5, theme.accent_cyan);
        g.fill_rect(tray_x + 4, tray_y + 6, 3, 9, theme.accent_cyan);
        g.fill_rect(tray_x + 8, tray_y + 2, 3, 13, theme.accent_cyan);

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

const DESKTOP_ICON_X: i32 = 18;
const DESKTOP_ICON_Y0: i32 = 20;
const DESKTOP_ICON_W: i32 = 64;
const DESKTOP_ICON_H: i32 = 60;
const DESKTOP_ICON_GAP: i32 = 14;

const DESKTOP_ICON_LABELS: [&str; 6] = [
    "This PC",
    "Files",
    "Editor",
    "Terminal",
    "Browser",
    "Mines",
];

fn desktop_icon_rects() -> [Rect; 6] {
    let mut rects = [Rect::new(0, 0, DESKTOP_ICON_W, DESKTOP_ICON_H); 6];
    let mut y = DESKTOP_ICON_Y0;
    for r in &mut rects {
        r.x = DESKTOP_ICON_X;
        r.y = y;
        y += DESKTOP_ICON_H + DESKTOP_ICON_GAP;
    }
    rects
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
