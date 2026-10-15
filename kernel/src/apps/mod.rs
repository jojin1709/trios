//! Phase 4 (cycle 2) — the 3-in-1 shell: File Explorer, Text Editor and
//! Terminal, plus the plumbing that routes desktop events into them.
//!
//! ### Why one `Apps` struct
//!
//! `main.rs` owns exactly one event loop, and it must not grow a
//! per-application `match` arm for every event type. [`Apps`] is the
//! single seam: the loop hands it a [`WidgetAction`], a [`KeyEvent`] or
//! the 1 Hz heartbeat and gets back a `bool` ("did anything visible
//! change?") — which is all the compositor needs to decide whether to
//! repaint. Everything else (paths, selections, scrollback) stays behind
//! this module.
//!
//! ### The widget-index contract
//!
//! A window's widget vec is an ordered list, so *creation order is the
//! address space*: `set_label(win, 7, ...)` means nothing unless everyone
//! agrees that index 7 is "row 3's text". Each app therefore publishes
//! its indices as `pub const`s ([`explorer::EX_ROWS`],
//! [`editor::ED_LOAD`], [`terminal::TERM_INPUT`], ...) and the rule is
//! simple — **push in the order the consts are declared, or renumber the
//! consts**. That is the whole contract.
//!
//! ### Why three structs rather than one enum
//!
//! The apps have genuinely different state (a cwd + selection, a line
//! buffer, a scrollback) and genuinely different handlers. Folding them
//! into `enum App { Files(..), Editor(..), Term(..) }` would only move
//! the same `match` one level deeper while forcing every variant to
//! carry every field. Plain structs + a dispatch method reads better and
//! costs nothing.
//!
//! ### Dirty = "repaint"
//!
//! Every handler returns whether it changed something the user can see.
//! The event loop ORs those flags together and calls
//! [`crate::gui::WindowManager::compose`] at most once per batch — the
//! same discipline the demo windows already follow.

pub mod editor;
pub mod explorer;
pub mod terminal;

use alloc::string::String;

pub use editor::Editor;
pub use explorer::Explorer;
pub use terminal::Terminal;

use crate::event::KeyEvent;
use crate::fs::FileSystem;
use crate::gui::{Theme, WidgetAction, WindowManager};

/// The shell's three applications plus their window ids.
///
/// Create once at boot with [`Apps::new`] — the windows exist from the
/// first frame (the CI screenshots at 3/8/15/30 s prove it) rather than
/// appearing on first click, which also means the taskbar buttons and
/// the `Ctrl+1/2/3` hotkeys have something to focus immediately after
/// `desktop_run` starts.
pub struct Apps {
    /// The "Files" window: browse, select, create.
    pub explorer: Explorer,
    /// The "Editor" window: one-line-at-a-time text editing.
    pub editor: Editor,
    /// The "Terminal" window: a command line over the same ramfs.
    pub terminal: Terminal,
}

impl Apps {
    /// Builds all three app windows on `wm` (in that z-order) and
    /// returns the shell. Nothing touches the filesystem yet — the
    /// constructor runs before `main.rs` hands out its `ramfs`, so the
    /// fs-backed views are primed by [`Apps::refresh`] one line later.
    pub fn new(wm: &mut WindowManager, theme: &Theme) -> Self {
        Self {
            explorer: Explorer::new(wm, theme),
            editor: Editor::new(wm, theme),
            terminal: Terminal::new(wm, theme),
        }
    }

    /// Primes every filesystem-backed view (today: the explorer's
    /// listing). Called exactly once from `desktop_run`, right after the
    /// windows are built and before the boot line is logged, so the
    /// first frame already shows real files instead of an empty list.
    ///
    /// Returns whether anything visible changed (always `true` on the
    /// first call — the rows go from blank to populated).
    pub fn refresh(&mut self, wm: &mut WindowManager, fs: &FileSystem) -> bool {
        self.explorer.refresh(wm, fs)
    }

    /// Is `window_id` one of the shell's own windows?
    ///
    /// `main.rs` uses this to decide who owns a key press: the apps get
    /// first refusal on everything when one of them is focused, and the
    /// Phase 3 cursor fallback keeps everything else.
    pub fn owns(&self, window_id: u64) -> bool {
        window_id == self.explorer.win
            || window_id == self.editor.win
            || window_id == self.terminal.win
    }

    /// Is one of the shell's windows the *focused* one? (Convenience
    /// over [`Apps::owns`] + [`WindowManager::focused_window`].)
    pub fn owns_focus(&self, wm: &WindowManager) -> bool {
        wm.focused_window().is_some_and(|id| self.owns(id))
    }

    /// Routes one finished pointer interaction (button press/release) to
    /// the app that owns the widget. Actions from windows the shell
    /// doesn't own — the Phase 3 demo windows — are ignored here and
    /// handled in `main.rs`, which keeps the demo wiring where it was.
    ///
    /// Returns `true` when a label/row the user can see was rewritten.
    pub fn on_action(
        &mut self,
        action: WidgetAction,
        wm: &mut WindowManager,
        fs: &mut FileSystem,
        theme: &Theme,
    ) -> bool {
        if action.window_id == self.explorer.win {
            // `self.explorer` and `self.editor` are disjoint fields, so
            // lending both out at once is fine — the explorer opens
            // files straight into the editor's filename box.
            self.explorer.on_action(
                action.widget_index,
                action.outcome,
                wm,
                fs,
                &mut self.editor,
                theme,
            )
        } else if action.window_id == self.editor.win {
            self.editor
                .on_action(action.widget_index, action.outcome, wm, fs)
        } else if action.window_id == self.terminal.win {
            self.terminal
                .on_action(action.widget_index, action.outcome, wm)
        } else {
            false
        }
    }

    /// Routes one key press to the focused app.
    ///
    /// `main.rs` calls this both when no text field has focus *and*
    /// when a field inside an app window does: a `TextInput`
    /// deliberately does not consume Enter/arrow keys (see
    /// [`crate::gui::widgets::TextInput::handle_key`]), so submitting a
    /// command, loading a file or moving a selection is the app's call.
    /// The actual caret editing still happens in `main.rs`, one line
    /// later, via `wm.handle_key`.
    ///
    /// Returns `true` when something visible changed.
    pub fn on_key(
        &mut self,
        event: KeyEvent,
        wm: &mut WindowManager,
        fs: &mut FileSystem,
        theme: &Theme,
    ) -> bool {
        let Some(window_id) = wm.focused_window() else {
            return false;
        };
        if window_id == self.explorer.win {
            self.explorer.on_key(event, wm, fs, &mut self.editor, theme)
        } else if window_id == self.editor.win {
            self.editor.on_key(event, wm, fs)
        } else if window_id == self.terminal.win {
            self.terminal.on_key(event, wm, fs)
        } else {
            false
        }
    }

    /// The 1 Hz heartbeat hook (`main.rs` folds it into its tick arm).
    ///
    /// Cycle 2's apps are entirely event-driven: every field they show
    /// is rewritten the moment it changes, so there is nothing
    /// time-dependent to repaint *yet* — hence `false`. The hook stays
    /// in the contract because it is the natural home for the first
    /// clock-driven thing the shell grows (a status clock, or an
    /// auto-refresh of the explorer listing once this method also
    /// receives the `&FileSystem`), and dropping it now would be a
    /// breaking change to `Apps` later.
    pub fn on_tick(&mut self, wm: &mut WindowManager) -> bool {
        let _ = wm;
        false
    }
}

/// Truncates `text` to `max` characters, appending `~` when it had to
/// cut (the marker is ASCII because the whole framebuffer font is).
///
/// Widgets draw *unclipped* — a 200-character file name would happily
/// paint over the desktop outside its window — so every string that
/// comes from the filesystem or the user goes through this first.
/// `max` is measured in characters because labels render at 8 px/cell:
/// `max` cells × 8 px must fit the window.
pub(crate) fn clip(text: &str, max: usize) -> String {
    let max = max.max(1);
    if text.chars().count() <= max {
        return String::from(text);
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('~');
    out
}
