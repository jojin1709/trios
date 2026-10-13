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
//! [`compose`] paints desktop → windows → **cursor**, always. The cursor
//! is decoration *over* the scene, never part of it: windows under it
//! are composited untouched, and no save/restore buffer is needed
//! because every frame is rebuilt from scratch before `flush`.

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
    pub fn handle_key(&mut self, ev: input::KeyEvent) {
        // Phase 3: only Esc, and only as a close shortcut. Real text
        // input lands here in Phase 5 (per-window focus + input field).
        if ev.pressed && ev.code == input::KeyCode::Esc {
            self.close_requested = true;
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
            close_requested: false,
        });
        id
    }

    /// Brings `id` to the top of the stack and makes it the sole focused
    /// window. (Every window you click gets raised — standard behaviour.)
    pub fn focus(&mut self, id: u64) {
        let z = self.take_z();
        for w in &mut self.windows {
            w.focused = w.id == id;
        }
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.z = z;
        }
        self.focused_id = Some(id);
        // Re-establish the ascending-z invariant (create appends the max,
        // focus can jump any window to the top).
        self.windows.sort_by_key(|w| w.z);
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
    /// 2. on a *press edge*, pick the topmost window under the cursor,
    ///    focus it, and classify the zone (close > grip > titlebar >
    ///    widget > bare body) — that decides what [`Drag`] becomes;
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
            if let Some(idx) = self.window_index_at(cx, cy) {
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
    /// window bottom→top (chrome, then widgets), then the cursor last.
    /// The caller flushes — double-buffering means nothing here is
    /// visible until then.
    pub fn compose(&self, g: &mut Graphics, theme: &Theme) {
        g.clear(theme.bg);
        for win in &self.windows {
            Self::draw_window(g, theme, win);
        }
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
