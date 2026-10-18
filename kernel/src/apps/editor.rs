//! Phase 4 (cycle 2) — the Text Editor: the "Editor" window.
//!
//! ### Honest scope: one line at a time
//!
//! There is no multi-line text widget (a reflowing, cursor-aware multi-row
//! editor is a phase of its own), so this app edits like a listing: 12
//! content labels show the file, `ArrowUp`/`ArrowDown` pick the current
//! line into a single-line [`TextInput`](crate::gui::widgets::TextInput),
//! and what you type there is committed back into the line buffer when the
//! selection moves or when you save. `Save` joins the buffer with `"\n"`
//! and hands it to [`FileSystem::write`].
//!
//! ### The widget-index contract
//!
//! Creation order *is* the address space (see [`super`]). Push order in
//! [`Editor::new`] must stay: filename field, [`ED_LINE_COUNT`] rows,
//! overflow row, edit field, Load, Save, status.
//!
//! ### Line painting
//!
//! Same `"> "` prefix trick as the explorer: the selected line is text-
//! marked, every other row is shown with two leading spaces, and the
//! trailing [`ED_MORE`] row says `"... N more lines"` when the file runs
//! past the window. The view is a fixed window over the *first*
//! [`ED_LINE_COUNT`] lines — selection can travel beyond it (the edit
//! field always shows the current line), scrolling is a later cycle.
//!
//! ### Why commit-before-move
//!
//! [`Editor::commit_edit`] writes the edit field back into
//! `lines[selected]` before the selection leaves a line and before every
//! save. Skipping it would silently drop the last line you typed — the
//! buffer would still hold the text `open_path` loaded.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::clip;
use crate::event::{KeyCode, KeyEvent};
use crate::fs::FileSystem;
use crate::gui::widgets::Widget;
use crate::gui::{Theme, WidgetOutcome, WindowManager};

// ---------------------------------------------------------------------------
// Widget indices (creation order — see module docs)
// ---------------------------------------------------------------------------

/// The filename `TextInput` — type a path, then Load/Save.
pub const ED_PATH: usize = 0;
/// First of the [`ED_LINE_COUNT`] file-content row labels.
pub const ED_LINES: usize = ED_PATH + 1;
/// How many file lines fit in the window at once.
pub const ED_LINE_COUNT: usize = 12;
/// The `"... N more lines"` overflow label (empty when everything fits).
pub const ED_MORE: usize = ED_LINES + ED_LINE_COUNT;
/// The `TextInput` holding the line selected for editing.
pub const ED_EDIT: usize = ED_MORE + 1;
/// The "Load" button.
pub const ED_LOAD: usize = ED_EDIT + 1;
/// The "Save" button.
pub const ED_SAVE: usize = ED_LOAD + 1;
/// The status label (load/save feedback).
pub const ED_STATUS: usize = ED_SAVE + 1;

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

const WIN_W: i32 = 340;
const WIN_H: i32 = 310;
/// First content row's y, and the row pitch (8 px glyphs + 4 px gap).
const LINE_Y: i32 = 70;
const LINE_H: i32 = 12;
/// Characters of a line before clipping (2 + 36 cells x 8 px + 12 px
/// left margin = 316 px, inside the 340 px window).
const LINE_CLIP: usize = 36;
/// Width of both text fields (leaves the window margin on each side).
const FIELD_W: i32 = 316;

// ---------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------

/// The "Editor" window's state: which file, its lines, which is selected.
///
/// Build with [`Editor::new`] (via [`super::Apps::new`]); the explorer
/// reaches it through [`Editor::open_path`].
#[derive(Debug)]
pub struct Editor {
    /// The "Editor" window's id — the dispatcher's routing key.
    pub win: u64,
    /// The path Load/Save operate on (mirrors the [`ED_PATH`] field;
    /// [`Editor::load_from_field`] and [`Editor::save`] re-read the field
    /// so typing a new path there retargets the next action).
    path: String,
    /// File contents, split on `'\n'`. Empty until something is loaded.
    lines: Vec<String>,
    /// Index into [`Self::lines`] of the `"> "` row / the line shown in
    /// [`ED_EDIT`].
    selected: usize,
}

impl Editor {
    /// Creates the "Editor" window with every widget pushed in contract
    /// order (see module docs) and returns the app. The window starts
    /// with no file: `path` and `lines` are empty until
    /// [`Editor::open_path`] or a Load click fills them.
    pub fn new(wm: &mut WindowManager, theme: &Theme) -> Self {
        let win = wm.create_window("Editor", 330, 250, WIN_W, WIN_H);
        if let Some(w) = wm.window_mut(win) {
            w.widgets.push(Widget::text_input(
                12,
                38,
                FIELD_W,
                22,
                "path e.g. /readme.txt",
            ));
            for i in 0..ED_LINE_COUNT {
                w.widgets.push(Widget::label(
                    "",
                    12,
                    LINE_Y + i as i32 * LINE_H,
                    theme.text_primary,
                ));
            }
            w.widgets
                .push(Widget::label("", 12, 214, theme.text_secondary));
            w.widgets
                .push(Widget::text_input(12, 232, FIELD_W, 22, "current line"));
            w.widgets.push(Widget::button(12, 260, 76, 22, "Load"));
            w.widgets.push(Widget::button(96, 260, 76, 22, "Save"));
            w.widgets
                .push(Widget::label("", 12, 292, theme.text_secondary));
        }
        Self {
            win,
            path: String::new(),
            lines: Vec::new(),
            selected: 0,
        }
    }

    /// Opens `path`: stores it, loads its contents into the line buffer,
    /// puts the path into the [`ED_PATH`] field and focuses that field —
    /// opening a file jumps the user *here*, ready to edit or retarget.
    ///
    /// A missing path loads as an empty buffer with a `file not found:`
    /// status line (the ramfs is live: another app can delete a file
    /// between the explorer's listing and this call).
    pub fn open_path(&mut self, path: &str, wm: &mut WindowManager, fs: &FileSystem) {
        self.path = String::from(path);
        self.load(wm, fs);
        set_field(wm, self.win, ED_PATH, path);
        wm.focus(self.win);
        wm.focus_widget(self.win, ED_PATH);
    }

    /// Handles one finished click inside the "Editor" window: Load rereads
    /// the [`ED_PATH`] field, Save commits [`ED_EDIT`] and writes.
    /// Row/field indices never arrive as clicks (labels don't capture the
    /// pointer), and slider-style outcomes don't exist here — both report
    /// no visible change.
    pub fn on_action(
        &mut self,
        widget_index: usize,
        outcome: WidgetOutcome,
        wm: &mut WindowManager,
        fs: &mut FileSystem,
    ) -> bool {
        if outcome != WidgetOutcome::Clicked {
            return false;
        }
        match widget_index {
            ED_LOAD => {
                self.load_from_field(wm, fs);
                true
            }
            ED_SAVE => self.save(wm, fs),
            _ => false,
        }
    }

    /// Handles one key press while the "Editor" window is focused.
    ///
    /// `ArrowUp`/`ArrowDown` commit the current line, move the selection
    /// and copy the new current line into [`ED_EDIT`]. Everything else —
    /// printable characters, `Backspace`, the arrows a caret uses — is
    /// either typed into a focused field (the WM handles it one line
    /// after this call) or belongs to no app at all.
    ///
    /// The filesystem parameter goes unused: arrow navigation works on
    /// the already-loaded [`Self::lines`]; reading disk again here would
    /// discard unsaved edits.
    pub fn on_key(&mut self, event: KeyEvent, wm: &mut WindowManager, _fs: &FileSystem) -> bool {
        if !event.pressed {
            return false;
        }
        match event.code {
            KeyCode::ArrowUp | KeyCode::ArrowDown => {
                let next = match event.code {
                    KeyCode::ArrowUp => self.selected.checked_sub(1),
                    _ => Some(self.selected + 1),
                };
                let Some(next) = next.filter(|&n| n < self.lines.len()) else {
                    // Top of the buffer, bottom of it, or nothing loaded.
                    return false;
                };
                // The line we are leaving may hold keystrokes not yet in
                // the buffer — write them back before repainting.
                self.commit_edit(wm);
                self.selected = next;
                self.paint(wm);
                let line = self.lines[self.selected].clone();
                set_field(wm, self.win, ED_EDIT, &line);
                true
            }
            _ => false,
        }
    }

    // -- helpers -----------------------------------------------------------

    /// Reads the [`ED_PATH`] field (falling back to the stored path when
    /// the field is empty) and loads it. Both outcomes are visible: the
    /// rows repaint, and the status line reports either the line count or
    /// `file not found: <path>`.
    fn load_from_field(&mut self, wm: &mut WindowManager, fs: &FileSystem) {
        if let Some(path) = wm.text_value(self.win, ED_PATH).filter(|p| !p.is_empty()) {
            self.path = path;
        }
        if self.path.is_empty() {
            wm.set_label(
                self.win,
                ED_STATUS,
                String::from("no path - type one above"),
            );
            return;
        }
        self.load(wm, fs);
    }

    /// Reads [`Self::path`] from the filesystem, resets the selection to
    /// the first line and repaints. Missing files become an empty buffer
    /// (never a panic — the ramfs answers `None`, we answer a message).
    fn load(&mut self, wm: &mut WindowManager, fs: &FileSystem) {
        let content = fs.read(&self.path);
        self.lines = match content {
            Some(text) => text.split('\n').map(String::from).collect(),
            None => Vec::new(),
        };
        self.selected = 0;
        self.paint(wm);
        let status = match content {
            None => format!("file not found: {}", clip(&self.path, 24)),
            Some(_) => format!(
                "Loaded {} ({} lines)",
                clip(&self.path, 24),
                self.lines.len()
            ),
        };
        set_field(
            wm,
            self.win,
            ED_EDIT,
            self.lines.first().map(String::as_str).unwrap_or(""),
        );
        wm.set_label(self.win, ED_STATUS, status);
    }

    /// Commits [`ED_EDIT`] into `lines[selected]`, then joins the buffer
    /// and writes it. The [`ED_PATH`] field wins over the stored path, so
    /// "type a new name, hit Save" is a save-as. Failures (only the root
    /// path can fail) land in the status line instead of panicking.
    fn save(&mut self, wm: &mut WindowManager, fs: &mut FileSystem) -> bool {
        self.commit_edit(wm);
        if let Some(path) = wm.text_value(self.win, ED_PATH).filter(|p| !p.is_empty()) {
            self.path = path;
        }
        if self.path.is_empty() {
            wm.set_label(
                self.win,
                ED_STATUS,
                String::from("no path - type one above"),
            );
            return true;
        }
        let text = self.lines.join("\n");
        if !fs.write(&self.path, &text) {
            wm.set_label(
                self.win,
                ED_STATUS,
                format!("cannot write {}", clip(&self.path, 24)),
            );
            return true;
        }
        let n = self.lines.len();
        serial_println!("[apps  ] editor save: {} ({n} lines)", self.path);
        wm.set_label(
            self.win,
            ED_STATUS,
            format!("Saved {} ({n} lines)", clip(&self.path, 24)),
        );
        crate::gui::notify(
            "File Saved",
            format!("Saved {} ({} lines)", clip(&self.path, 20), n),
            crate::gui::NotificationKind::Success,
        );
        self.paint(wm);
        true
    }

    /// Copies the [`ED_EDIT`] field's text into `lines[selected]` — the
    /// single seam between "what the caret typed" and "what the buffer
    /// holds". No-op when nothing is loaded (there is no line to write).
    fn commit_edit(&mut self, wm: &WindowManager) {
        if self.lines.is_empty() {
            return;
        }
        if let Some(text) = wm.text_value(self.win, ED_EDIT)
            && let Some(line) = self.lines.get_mut(self.selected)
        {
            *line = text;
        }
    }

    /// Repaints the [`ED_LINE_COUNT`] content rows (with the `"> "`
    /// selection marker) and the [`ED_MORE`] overflow row. Rows past the
    /// end of the file go blank; rows past [`ED_LINE_COUNT`] are reported
    /// by the overflow row, not drawn.
    fn paint(&self, wm: &mut WindowManager) {
        for i in 0..ED_LINE_COUNT {
            let text = match self.lines.get(i) {
                Some(line) => {
                    let mark = if i == self.selected { ">" } else { " " };
                    let lnum = i + 1;
                    format!("{mark}{lnum:2}| {}", clip(line, LINE_CLIP.saturating_sub(5)))
                }
                None => String::new(),
            };
            wm.set_label(self.win, ED_LINES + i, text);
        }
        let overflow = self.lines.len().saturating_sub(ED_LINE_COUNT);
        let more = if overflow > 0 {
            format!("... {overflow} more lines")
        } else {
            String::new()
        };
        wm.set_label(self.win, ED_MORE, more);
    }
}

/// Writes `value` into the text field at `index` (caret to the end).
///
/// The WM exposes no public setter for a field's value — it owns *focus*,
/// not content — so this reaches through [`WindowManager::window_mut`].
/// A closed window or stale index is a silent no-op: Esc can close the
/// window mid-batch, and that must never panic.
fn set_field(wm: &mut WindowManager, win: u64, index: usize, value: &str) {
    if let Some(w) = wm.window_mut(win)
        && let Some(Widget::TextInput(t)) = w.widgets.get_mut(index)
    {
        t.value = String::from(value);
        t.cursor = t.value.len();
    }
}
