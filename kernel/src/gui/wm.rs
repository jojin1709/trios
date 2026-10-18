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

/// Titlebar height in pixels (also the hit zone for starting a drag).
pub const TITLEBAR_H: i32 = 24;
/// Close ("X") button size, top-right of the titlebar.
pub const CLOSE_SIZE: i32 = 18;
/// Bottom-right resize-grip hit zone (square).
pub const RESIZE_GRIP: i32 = 14;
/// Smallest window we allow (a window you can't use is a bug magnet).
pub const MIN_W: i32 = 100;
/// Smallest window height (fits titlebar + a little content).
pub const MIN_H: i32 = 60;

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
}

impl Window {
    /// Local titlebar zone (drag-to-move target).
    fn titlebar_rect(&self) -> Rect {
        Rect::new(0, 0, self.rect.w, TITLEBAR_H)
    }

    /// Local close-button zone (checked *before* the titlebar, since it
    /// lives inside it).
    fn close_rect(&self) -> Rect {
        Rect::new(self.rect.w - CLOSE_SIZE - 3, 3, CLOSE_SIZE, CLOSE_SIZE)
    }

    /// Local bottom-right resize zone.
    fn grip_rect(&self) -> Rect {
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
    /// Which windows the three taskbar buttons focus, in button order
    /// (Files, Editor, Terminal). `None` until `main.rs` calls
    /// [`set_taskbar_targets`](Self::set_taskbar_targets) — the WM creates
    /// windows for whoever asks and has no idea which ids belong to the
    /// shell's apps, so it must not guess; an unwired taskbar draws its
    /// buttons but every click reports `None` (see
    /// [`taskbar_click`](Self::taskbar_click)).
    taskbar_targets: Option<TaskbarTargets>,
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
    pub fn set_taskbar_targets(&mut self, files: u64, editor: u64, terminal: u64, browser: u64) {
        self.taskbar_targets = Some(TaskbarTargets {
            files,
            editor,
            terminal,
            browser,
        });
    }

    /// Which taskbar button sits at `(x, y)`? The window id to focus, or
    /// `None` — point off every button, *or* targets never wired (see
    /// [`set_taskbar_targets`]), *or* the button's window no longer
    /// exists (the user can Esc/X an app away; focusing a dead id would
    /// poison `focused_id`, see [`focus`], so the click simply does
    /// nothing).
    pub fn taskbar_click(&self, x: i32, y: i32) -> Option<u64> {
        let targets = self.taskbar_targets?;
        let idx = taskbar_button_rects(self.screen_h)
            .iter()
            .position(|r| r.contains(x, y))?;
        let id = match idx {
            0 => targets.files,
            1 => targets.editor,
            2 => targets.terminal,
            _ => targets.browser,
        };
        self.windows.iter().any(|w| w.id == id).then_some(id)
    }

    /// The taskbar's band in global coords: the `TASKBAR_H` rows directly
    /// above the [`HINT_BAR_H`] hint bar (`screen_h - HINT_BAR_H -
    /// TASKBAR_H .. screen_h - HINT_BAR_H`).
    ///
    /// This is the *hit-test* mirror of `draw_taskbar`'s `y` computation
    /// — both derive from the same two constants, so what `compose`
    /// paints and what `handle_mouse` treats as chrome agree by
    /// construction. On a screen shorter than the two bars the y goes
    /// negative; `Rect::contains` still answers truthfully and every
    /// draw primitive clips, so a degenerate screen degrades instead of
    /// panicking.
    fn taskbar_rect(&self) -> Rect {
        Rect::new(
            0,
            self.screen_h - HINT_BAR_H - TASKBAR_H,
            self.screen_w,
            TASKBAR_H,
        )
    }

    // -- input -------------------------------------------------------------

    /// Topmost window containing the point (windows are z-sorted
    /// ascending, so we scan from the top down).
    fn window_index_at(&self, x: i32, y: i32) -> Option<usize> {
        self.windows.iter().rposition(|w| w.rect.contains(x, y))
    }

    /// Feeds one mouse event through the interaction state machine.
    ///
    /// Event order matters; this is the whole protocol:
    ///
    /// 1. advance the cursor by this event's delta (clamped);
    /// 2. on a *press edge*, check the **taskbar band first** — it is
    ///    chrome drawn *over* the windows, so it wins the hit test and
    ///    swallows the press (focusing its app when a button was hit,
    ///    nothing otherwise); only a press outside the band picks the
    ///    topmost window under the cursor, focuses it, and classifies the
    ///    zone (close > grip > titlebar > widget > bare body) — that
    ///    decides what [`Drag`] becomes;
    /// 3. on a *release edge*, hand the coordinates to the capturing
    ///    widget (it answers with a [`WidgetOutcome`] — a click or a
    ///    slider value) and end the drag;
    /// 4. while the button is still down, apply window move/resize;
    /// 5. finally route hover/drag motion to every widget so pressed /
    ///    hover visuals track the cursor.
    ///
    /// Returns the (at most one) action the interaction produced.
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
            self.drag = Drag::None;
            // 2a. The taskbar band is tested *before* any window, because
            // `compose` paints it over them: a press that lands in the
            // band belongs to the chrome, and must not reach the window
            // it is covering (that window would be raised *and* focused
            // from underneath the bar — a focus-stealing bug the user
            // can't see). The whole band swallows the press, not just the
            // three buttons: the gaps and the uptime stamp are chrome
            // too, and a click on bare strip must not fall through either.
            //
            // Consuming here is what keeps the drag state machine clean:
            // `drag` stays `Drag::None` from the reset above, so no zone
            // is classified (→ no `Move`/`Resize`, no widget capture, no
            // close flag), step 4 finds `Drag::None` and writes no
            // geometry, and the later release edge clears `Drag::None`
            // again — a taskbar press therefore cannot start a titlebar
            // drag, move a window, or emit a `WidgetAction`.
            if self.taskbar_rect().contains(cx, cy) {
                if let Some(id) = self.taskbar_click(cx, cy) {
                    // Focus *and* arm the window's first text field: the
                    // taskbar button is the mouse twin of the Ctrl+1/2/3
                    // hotkey, so both paths must end in the same state
                    // (focused window + live caret) or one of them hands
                    // the user a focused window that silently eats every
                    // printable — see `focus_and_type` for that bug.
                    self.focus_and_type(id);
                }
            } else if let Some(idx) = self.window_index_at(cx, cy) {
                // Snapshot decisions *before* focus() re-sorts the vec.
                let (id, zone, widget_target) = {
                    let win = &self.windows[idx];
                    let lx = cx - win.rect.x;
                    let ly = cy - win.rect.y;
                    let zone = if win.close_rect().contains(lx, ly) {
                        Zone::Close
                    } else if win.grip_rect().contains(lx, ly) {
                        Zone::Grip
                    } else if win.titlebar_rect().contains(lx, ly) {
                        Zone::Titlebar
                    } else {
                        Zone::Body
                    };
                    // Later widgets draw on top → they get first refusal.
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

                // Clicking anywhere on a window raises + focuses it.
                self.focus(id);

                // Look the window up *by id* — focus() just re-sorted the
                // vec, so any pre-focus index would now be stale. (The
                // window was found a moment ago, so this cannot fail.)
                if let Some(idx) = self.windows.iter().position(|w| w.id == id) {
                    let (rect_x, rect_y) = {
                        let w = &self.windows[idx].rect;
                        (w.x, w.y)
                    };
                    let (lx, ly) = (cx - rect_x, cy - rect_y);
                    match zone {
                        Zone::Close => self.windows[idx].close_requested = true,
                        Zone::Grip => self.drag = Drag::Resize { id },
                        Zone::Titlebar => {
                            self.drag = Drag::Move {
                                id,
                                off_x: lx,
                                off_y: ly,
                            }
                        }
                        Zone::Body => {
                            // Keyboard focus follows the press: a
                            // TextInput hit takes the field focus; a
                            // button/slider/bare body clears it (typing
                            // must never continue "into" a field the
                            // user obviously walked away from).
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
            }
        }

        // 3. release edge: finish the capturing widget's interaction.
        let mut action = None;
        if released {
            if let Drag::Widget { id, index } = self.drag {
                // Look up by id, NOT by cursor position: the release may
                // well happen outside the window (dragged off the edge),
                // and the widget still needs its `pointer_up` — a slider
                // must stop dragging even when let go off-window.
                if let Some(idx) = self.windows.iter().position(|w| w.id == id) {
                    let w = &mut self.windows[idx];
                    let (lx, ly) = (cx - w.rect.x, cy - w.rect.y);
                    // `get_mut`: the id/index pair came from a capture on
                    // this same window, but defensive bounds checks beat
                    // panics in an input path.
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
                    // By id again: a fast drag outruns the window, so the
                    // cursor may already be outside the dragged window.
                    if let Some(idx) = self.windows.iter().position(|w| w.id == id) {
                        let r = &mut self.windows[idx].rect;
                        // Keep a bit of titlebar on-screen so a dragged
                        // window can always be grabbed again.
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
                        // Right/bottom edges track the cursor; clamped to
                        // the minimum size and the screen edge.
                        let max_w = (self.screen_w - r.x).max(MIN_W);
                        let max_h = (self.screen_h - r.y).max(MIN_H);
                        r.w = (cx - r.x).clamp(MIN_W, max_w);
                        r.h = (cy - r.y).clamp(MIN_H, max_h);
                    }
                }
                Drag::Widget { .. } | Drag::None => {}
            }
        }

        // 5. widget hover/drag routing — every widget, every event.
        let captured = match self.drag {
            Drag::Widget { id, index } => Some((id, index)),
            _ => None,
        };
        for win in &mut self.windows {
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

    /// Paints the whole desktop into the backbuffer: background, every
    /// window bottom→top (chrome, then widgets), the always-on-top input
    /// hint bar, the taskbar strip above it, then the cursor last. The
    /// caller flushes — double-buffering means nothing here is visible
    /// until then.
    pub fn compose(&self, g: &mut Graphics, theme: &Theme) {
        g.clear(theme.bg);
        for win in &self.windows {
            Self::draw_window(g, theme, win);
        }
        draw_hint_bar(g, theme);
        // Between the hint bar and the cursor: the taskbar is chrome like
        // the hint bar (painted over the windows every frame), and the
        // cursor stays the topmost thing on screen — see module docs.
        draw_taskbar(g, theme);
        draw_cursor(g, self.cursor);
    }

    /// Draws one window: body + titlebar, widgets, close box, grip,
    /// and finally the border (last, so it crisps every edge).
    fn draw_window(g: &mut Graphics, theme: &Theme, win: &Window) {
        let r = win.rect;

        // Titlebar: accent when focused, muted otherwise — the at-a-glance
        // focus cue (spec: "darker if unfocused").
        let titlebar = if win.focused {
            theme.titlebar_bg_focused
        } else {
            theme.titlebar_bg
        };
        g.fill_rect(r.x, r.y, r.w as u32, TITLEBAR_H as u32, titlebar);
        // Body.
        g.fill_rect(
            r.x,
            r.y + TITLEBAR_H,
            r.w as u32,
            (r.h - TITLEBAR_H).max(0) as u32,
            theme.window_bg,
        );

        // Title text, clipped so long titles stop before the close box
        // (drawn glyph-by-glyph: no allocation per frame).
        let title_limit = r.x + r.w - CLOSE_SIZE - 8;
        let mut tx = r.x + 8;
        let ty = r.y + (TITLEBAR_H - 8) / 2;
        for ch in win.title.chars() {
            if tx + 8 > title_limit {
                break;
            }
            g.draw_char(tx, ty, ch, theme.text_primary, None, 1);
            tx += 8;
        }

        // Widgets (window-local coords -> global by adding the origin).
        for wg in &win.widgets {
            wg.draw(g, r.x, r.y, theme);
        }

        // Close box ("X").
        let c = win.close_rect();
        let (cx, cy) = (r.x + c.x, r.y + c.y);
        g.fill_rect(cx, cy, c.w as u32, c.h as u32, theme.window_bg);
        g.draw_rect(cx, cy, c.w as u32, c.h as u32, theme.border);
        let inset = 5;
        g.draw_line(
            cx + inset,
            cy + inset,
            cx + c.w - inset - 1,
            cy + c.h - inset - 1,
            theme.text_primary,
        );
        g.draw_line(
            cx + c.w - inset - 1,
            cy + inset,
            cx + inset,
            cy + c.h - inset - 1,
            theme.text_primary,
        );

        // Resize grip: two parallel diagonal ticks in the corner.
        let (gr, gb) = (r.right() - 1, r.bottom() - 1);
        g.draw_line(gr - 11, gb - 3, gr - 3, gb - 11, theme.text_secondary);
        g.draw_line(gr - 6, gb - 3, gr - 3, gb - 6, theme.text_secondary);

        // Border last — focused windows get the accent frame.
        let border = if win.focused {
            theme.accent
        } else {
            theme.border
        };
        g.draw_rect(r.x, r.y, r.w as u32, r.h as u32, border);
    }
}

// ---------------------------------------------------------------------------
// The input-capture hint bar
// ---------------------------------------------------------------------------

/// Font scale for the hint bar's text (1 => 8 px cells).
///
/// The message is one 119-char line, so at scale 1
/// [`Graphics::text_width`] reports 119 × 8 = **952 px** — inside the
/// 1024 px framebuffer (36 px of margin on each side). Scale 2 would be
/// 119 × 16 = **1904 px > 1024**, i.e. it would run off the right edge;
/// scale 1 is therefore the largest integer scale that fits (see
/// [`HINT_LINES`]).
const HINT_SCALE: u32 = 1;
/// Vertical padding above/below the text inside the bar.
const HINT_PAD: i32 = 6;
/// The message: a single centered line (the old two-line version overflowed
/// the 1024 px framebuffer).
///
/// ASCII-only, 119 chars → 119 × 8 = **952 px** at [`HINT_SCALE`], so it
/// centers with 36 px to spare on each side. The draw code still clamps
/// `x ≥ 0` and every primitive clips, so a narrower screen degrades by
/// trimming the right edge rather than wrapping or panicking.
///
/// The copy describes what *actually works today*: the keyboard cursor
/// fallback (`main.rs` Key arm — arrows move, Space/Enter click, Esc
/// closes, Ctrl+1/2/3 switches apps) plus VirtualBox's own capture
/// controls (click to capture, Right Ctrl to release). The PS/2 mouse
/// itself may or may not be delivering packets on a given boot — see
/// `[ps2    ] mouse irq count` on serial — so the bar deliberately
/// promises only the input paths that cannot be dead: nothing here
/// depends on IRQ12, while every key it names is proven by the
/// keyboard's own climbing `kb irq count`.
const HINT_LINES: [&str; 1] = [
    "Click here to capture input | Right Ctrl = release | Arrows = move | Space/Enter = click | Esc = close | Ctrl+1/2/3/4 = apps",
];
/// One text line's height in pixels at [`HINT_SCALE`] (8 × 1 = 8 px).
const HINT_LINE_H: i32 = crate::graphics::font::GLYPH_H as i32 * HINT_SCALE as i32;
/// Total bar height: padding + exactly one line (6 + 8 + 6) = **20 px**
/// (was 44 px when the message spanned two lines at scale 2).
const HINT_BAR_H: i32 = HINT_PAD * 2 + HINT_LINE_H * HINT_LINES.len() as i32;

/// Draws the always-visible input-capture hint bar, pinned to the bottom
/// of the screen.
///
/// One centered line inside a [`HINT_BAR_H`]-tall strip, so the whole bar
/// occupies only the bottom 20 px of the framebuffer.
///
/// Why this exists: VirtualBox starts **uncaptured** — the host cursor
/// is shown and keystrokes go to Windows until the user clicks inside
/// the VM window once (that's when "Auto capture keyboard" engages).
/// The guest cannot detect that host-side state, so instead of guessing
/// it says out loud, on every frame, exactly what to do. It deliberately
/// never times out: a user who looked away needs the same message when
/// they look back.
///
/// Drawn *after* the windows (always on top — OS chrome, same layer as
/// the taskbar) and *before* the cursor (so the pointer stays visible
/// over it). The taskbar strip sits directly *above* this bar (see
/// [`draw_taskbar`]), so between them they own the bottom 48 px; the two
/// never overlap, which is why the order they are painted in doesn't
/// matter. This bar itself is decoration only:
/// [`WindowManager::handle_mouse`] never knows it's there, so a press
/// landing on it falls through to whatever window is beneath — which is
/// also why it never blocks clicks on the desktop (the taskbar, unlike
/// this bar, *is* hit-tested — see [`WindowManager::taskbar_click`]).
fn draw_hint_bar(g: &mut Graphics, theme: &Theme) {
    let w = g.width() as i32;
    let h = g.height() as i32;
    let y = h - HINT_BAR_H;

    // Accent strip + a bright 1-px rule on its top edge, so the bar reads
    // as chrome laid over the desktop rather than another window border.
    // (Negative `y` on a screen shorter than the bar simply clips — every
    // primitive clips — so no degenerate-geometry special case is needed.)
    g.fill_rect(0, y, w as u32, HINT_BAR_H as u32, theme.accent);
    g.fill_rect(0, y, w as u32, 1, theme.text_primary);

    for (i, line) in HINT_LINES.iter().enumerate() {
        let x = ((w - Graphics::text_width(line, HINT_SCALE)) / 2).max(0);
        let ty = y + HINT_PAD + i as i32 * HINT_LINE_H;
        g.draw_text(x, ty, line, theme.text_primary, None, HINT_SCALE);
    }
}

// ---------------------------------------------------------------------------
// The taskbar
// ---------------------------------------------------------------------------

/// Taskbar strip height in pixels — the bottom chrome band holding the
/// three app buttons plus the uptime stamp.
///
/// It sits **directly above** the [`HINT_BAR_H`] hint bar, so the bottom
/// of the screen stacks up like this (constants compose, so any height
/// follows the same rule; shown for the two real targets):
///
/// ```text
///                     800 px screen        768 px screen
/// hint bar            y 780 .. 800         y 748 .. 768   (20 px)
/// taskbar             y 752 .. 780         y 720 .. 748   (28 px)
/// windows             may run under it — drawn first, occluded by both
/// cursor              topmost, always      (compose order)
/// ```
///
/// Like the hint bar this is *chrome*: `compose` paints it after every
/// window and [`WindowManager::handle_mouse`] checks the band before it
/// checks any window, so a press on the strip can never reach, raise or
/// focus what is painted underneath it.
pub const TASKBAR_H: i32 = 28;
/// Left margin of the first button — also the margin the right-aligned
/// uptime stamp keeps from the strip's right edge (8 px each side, so
/// the strip frames its contents symmetrically).
const TASKBAR_X0: i32 = 8;
/// Button width: "Terminal" is the longest caption (8 chars × 8 px = 64
/// px at scale 1), so 90 px leaves 13 px of padding on either side of
/// the centered text.
const TASKBAR_BTN_W: i32 = 90;
/// Gap between buttons (the same 4 px separates the last button from
/// whatever follows it).
const TASKBAR_BTN_GAP: i32 = 4;
/// Vertical padding between the strip's edges and its buttons: buttons
/// are `TASKBAR_H - 2 × 6 = 16` px tall — an 8 px caption with 4 px
/// above and below, which is also what leaves room for the strip's own
/// 1 px top rule.
const TASKBAR_PAD_Y: i32 = 6;
/// Button height, **derived** from the padding so the two can't drift:
/// shrink [`TASKBAR_PAD_Y`] and the buttons grow to match.
const TASKBAR_BTN_H: i32 = TASKBAR_H - TASKBAR_PAD_Y * 2;
/// Button captions, **in button order** (left to right). Index `i` here
/// is the rect index [`taskbar_button_rects`] lays out *and* the index
/// [`WindowManager::taskbar_click`] resolves against
/// [`TaskbarTargets`]' same-ordered fields — one array defines the whole
/// left-to-right contract.
const TASKBAR_LABELS: [&str; 4] = ["Files", "Editor", "Terminal", "Browser"];

/// The four button rects in **global** coordinates, left to right:
///
/// ```text
/// Files     x   8 ..  98     Editor    x 102 .. 192
/// Terminal  x 196 .. 286     Browser   x 290 .. 380
/// all       y = (strip top) + TASKBAR_PAD_Y, h = TASKBAR_BTN_H (16)
/// ```
fn taskbar_button_rects(screen_h: i32) -> [Rect; 4] {
    let y = screen_h - HINT_BAR_H - TASKBAR_H + TASKBAR_PAD_Y;
    let mut x = TASKBAR_X0;
    let mut rects = [Rect::new(0, 0, TASKBAR_BTN_W, TASKBAR_BTN_H); 4];
    for r in &mut rects {
        r.x = x;
        r.y = y;
        x += TASKBAR_BTN_W + TASKBAR_BTN_GAP;
    }
    rects
}

/// Draws the taskbar strip: body, 1 px top rule, buttons, clock stamp.
fn draw_taskbar(g: &mut Graphics, theme: &Theme) {
    let w = g.width() as i32;
    let h = g.height() as i32;
    let y = h - HINT_BAR_H - TASKBAR_H;

    g.fill_rect(0, y, w as u32, TASKBAR_H as u32, theme.window_bg);
    g.fill_rect(0, y, w as u32, 1, theme.text_primary);

    let buttons = taskbar_button_rects(h);
    for (label, r) in TASKBAR_LABELS.iter().zip(buttons.iter()) {
        g.fill_rect(r.x, r.y, r.w as u32, r.h as u32, theme.button_bg);
        g.draw_rect(r.x, r.y, r.w as u32, r.h as u32, theme.border);
        let tw = Graphics::text_width(label, 1);
        let th = Graphics::text_height(1);
        g.draw_text(
            r.x + (r.w - tw) / 2,
            r.y + (r.h - th) / 2,
            label,
            theme.text_primary,
            None,
            1,
        );
    }

    let stamp_y = y + (TASKBAR_H - Graphics::text_height(1)) / 2;
    let min_x = buttons[3].right() + TASKBAR_X0;
    draw_uptime_stamp(g, w - TASKBAR_X0, min_x, stamp_y, theme.text_secondary);
}

/// Draws `Uptime: <secs> s` with its **right** edge at `right`, never
/// starting left of `min_x`, at (x, y).
///
/// Glyph-by-glyph into a stack digit buffer instead of `format!`: every
/// primitive that runs per frame inherits the compositor's
/// no-allocation-per-frame rule (the titlebar text follows it too), and
/// `compose` runs on *every* dirty event — a window drag repaints at
/// ~100 Hz, so a tiny String alloc/free pair per frame would be pure
/// overhead for a number nobody measures. Right-alignment normally needs
/// the finished string's width before drawing; the digit count is
/// computed first (u64 seconds ⇒ at most 20 digits) so the width is
/// known before a single pixel is touched.
///
/// `min_x` exists for narrow screens: see `draw_taskbar`.
fn draw_uptime_stamp(g: &mut Graphics, right: i32, min_x: i32, y: i32, color: Color) {
    let dt = crate::timer::read_rtc();
    let mut buf = [0u8; 32];
    let y4 = dt.year;
    buf[0] = b'0' + ((y4 / 1000) % 10) as u8;
    buf[1] = b'0' + ((y4 / 100) % 10) as u8;
    buf[2] = b'0' + ((y4 / 10) % 10) as u8;
    buf[3] = b'0' + (y4 % 10) as u8;
    buf[4] = b'-';
    buf[5] = b'0' + ((dt.month / 10) % 10);
    buf[6] = b'0' + (dt.month % 10);
    buf[7] = b'-';
    buf[8] = b'0' + ((dt.day / 10) % 10);
    buf[9] = b'0' + (dt.day % 10);
    buf[10] = b' ';
    buf[11] = b'0' + ((dt.hour / 10) % 10);
    buf[12] = b'0' + (dt.hour % 10);
    buf[13] = b':';
    buf[14] = b'0' + ((dt.minute / 10) % 10);
    buf[15] = b'0' + (dt.minute % 10);
    buf[16] = b':';
    buf[17] = b'0' + ((dt.second / 10) % 10);
    buf[18] = b'0' + (dt.second % 10);
    buf[19] = b' ';
    buf[20] = b'U';
    buf[21] = b'T';
    buf[22] = b'C';

    let len = 23;
    let cell = crate::graphics::font::GLYPH_W as i32; // 8 px
    let total = len as i32 * cell;
    let mut x = (right - total).max(min_x);

    for i in 0..len {
        g.draw_char(x, y, buf[i] as char, color, None, 1);
        x += cell;
    }
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
