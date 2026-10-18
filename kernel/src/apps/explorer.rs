//! Phase 4 (cycle 2) — the File Explorer: the "Files" window.
//!
//! ### What it is
//!
//! A one-directory-at-a-time browser over [`FileSystem`]: a cwd header,
//! [`EX_ROW_COUNT`] entry rows, an "Up" button and a "New file" prompt.
//! Selecting a directory descends into it; selecting a file hands its
//! path to the [`Editor`](super::editor) (which jumps to the front with
//! the path loaded) — this app never edits file *content* itself.
//!
//! ### The widget-index contract
//!
//! Creation order *is* the address space (see [`super`]). The constants
//! below are the published half of that contract: [`Explorer::refresh`]
//! rewrites rows through `set_label` and [`Explorer::on_action`] dispatches
//! clicks by index, and both agree only because [`Explorer::new`] pushes in
//! exactly the order the consts are declared — cwd label, rows, Up, New
//! file, input.
//!
//! ### Selection rendering: `"> "` as text
//!
//! Rows are plain [`Label`]s, so the marker is *text*, not style: the
//! selected row is rewritten as `"> name"`, every other row as `"  name"`.
//! That keeps selection inside the existing widget vocabulary (no
//! "highlighted label" variant) and costs one [`set_label`] per row.
//!
//! ### Why `selected` is an index into a fresh `list`
//!
//! The listing is re-read from `fs` on every key press rather than cached:
//! the tree is a handful of nodes ([`FileSystem::list`] is an O(n) scan),
//! the rows are repainted from the same call, and a cached `Vec` would be
//! one more thing to invalidate when the Terminal writes a file behind our
//! back. A stale index is clamped in [`Explorer::refresh`] and guarded by
//! `entries.get(..)` in the key handler — never a panic.
//!
//! [`Label`]: crate::gui::widgets::Label
//! [`set_label`]: crate::gui::WindowManager::set_label

use alloc::format;
use alloc::string::String;

use super::clip;
use super::editor::Editor;
use crate::event::{KeyCode, KeyEvent};
use crate::fs::FileSystem;
use crate::gui::widgets::Widget;
use crate::gui::{Theme, WidgetOutcome, WindowManager};

// ---------------------------------------------------------------------------
// Widget indices (creation order — see module docs)
// ---------------------------------------------------------------------/// The cwd header label (pushed first, so index 0).
pub const EX_CWD: usize = 0;
/// First of the [`EX_ROW_COUNT`] directory-entry row labels.
pub const EX_ROWS: usize = EX_CWD + 1;
/// How many entries fit in the window at once.
pub const EX_ROW_COUNT: usize = 14;
/// The "Up" button (parent directory).
pub const EX_UP: usize = EX_ROWS + EX_ROW_COUNT;
/// The "New file" button (creates from the [`EX_INPUT`] prompt).
pub const EX_NEW: usize = EX_UP + 1;
/// The "New folder" button.
pub const EX_MKDIR: usize = EX_NEW + 1;
/// The "Rename" button.
pub const EX_RENAME: usize = EX_MKDIR + 1;
/// The "Delete" button.
pub const EX_DELETE: usize = EX_RENAME + 1;
/// The filename prompt feeding "New file" / "New folder" / "Rename".
pub const EX_INPUT: usize = EX_DELETE + 1;

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// Window size (the body is what these constants budget for; the titlebar
/// sits above `y = 0`'s content in window-local coordinates).
const WIN_W: i32 = 300;
const WIN_H: i32 = 286;
/// First row's y, and the row pitch: 8 px glyphs + 4 px of gap.
const ROW_Y: i32 = 54;
const ROW_H: i32 = 12;
/// Button/input band, shared by buttons and the prompt.
const BUTTON_Y: i32 = 228;
const INPUT_Y: i32 = 256;
/// Characters of an entry name before clipping (the `"> "` prefix and an
/// optional directory `/` sit on top: 2 + 30 = 32 cells x 8 px + 12 px
/// left margin = 268 px, inside the 300 px window).
const NAME_CLIP: usize = 29;
/// Characters of the cwd shown in the header (34 x 8 + 12 = 284 < 300).
const CWD_CLIP: usize = 34;

// ---------------------------------------------------------------------------
// Explorer
// ---------------------------------------------------------------------------

/// The "Files" window's state: where we are and which row is selected.
///
/// Build with [`Explorer::new`] (via [`super::Apps::new`]) and prime the
/// listing with [`Explorer::refresh`] before the first frame — `new`
/// creates empty labels, it does not touch the filesystem.
#[derive(Debug)]
pub struct Explorer {
    /// The "Files" window's id — the dispatcher's routing key.
    pub win: u64,
    /// Current directory, always absolute (`"/"`, `"/notes"`, ...).
    cwd: String,
    /// Index into `fs.list(cwd)` of the `"> "` row. Clamped by
    /// [`Explorer::refresh`] whenever the listing shrinks.
    selected: usize,
}

impl Explorer {
    /// Creates the "Files" window with every widget pushed in contract
    /// order (see module docs) and returns the app. Rows start blank —
    /// the filesystem-backed paint is [`Explorer::refresh`], which
    /// [`super::Apps`] calls immediately after construction.
    pub fn new(wm: &mut WindowManager, theme: &Theme) -> Self {
        let win = wm.create_window("Files", 16, 300, WIN_W, WIN_H);
        if let Some(w) = wm.window_mut(win) {
            w.widgets
                .push(Widget::label("/", 12, 38, theme.text_secondary));
            for i in 0..EX_ROW_COUNT {
                w.widgets.push(Widget::label(
                    "",
                    12,
                    ROW_Y + i as i32 * ROW_H,
                    theme.text_primary,
                ));
            }
            w.widgets.push(Widget::button(10, BUTTON_Y, 44, 22, "Up"));
            w.widgets.push(Widget::button(59, BUTTON_Y, 52, 22, "+File"));
            w.widgets.push(Widget::button(116, BUTTON_Y, 58, 22, "+Folder"));
            w.widgets.push(Widget::button(179, BUTTON_Y, 54, 22, "Rename"));
            w.widgets.push(Widget::button(238, BUTTON_Y, 52, 22, "Delete"));
            w.widgets
                .push(Widget::text_input(10, INPUT_Y, 280, 22, "name / new name"));
        }
        Self {
            win,
            cwd: String::from("/"),
            selected: 0,
        }
    }

    /// Repaints the cwd header and every row from `fs.list(self.cwd)`,
    /// clamping [`Self::selected`] to the (possibly shorter) listing.
    ///
    /// Returns `true` unconditionally: every call site — boot priming,
    /// "New file", descending into a directory, going up — changed the
    /// listing, so the frame is dirty by construction.
    pub fn refresh(&mut self, wm: &mut WindowManager, fs: &FileSystem) -> bool {
        let len = crate::fs::vfs::list_dir(&self.cwd).map_or(0, |l| l.len());
        if self.selected >= len {
            self.selected = len.saturating_sub(1);
        }
        self.paint(wm, fs);
        true
    }

    /// Handles one finished click inside the "Files" window.
    pub fn on_action(
        &mut self,
        widget_index: usize,
        outcome: WidgetOutcome,
        wm: &mut WindowManager,
        fs: &mut FileSystem,
        _editor: &mut Editor,
        _theme: &Theme,
    ) -> bool {
        if outcome != WidgetOutcome::Clicked {
            return false;
        }
        match widget_index {
            EX_UP => self.go_up(wm, fs),
            EX_NEW => self.create_file(wm, fs),
            EX_MKDIR => self.create_dir(wm, fs),
            EX_RENAME => self.rename_selected(wm, fs),
            EX_DELETE => self.delete_selected(wm, fs),
            _ => false,
        }
    }

    /// Handles one key press while the "Files" window is focused.
    pub fn on_key(
        &mut self,
        event: KeyEvent,
        wm: &mut WindowManager,
        fs: &mut FileSystem,
        editor: &mut Editor,
        _theme: &Theme,
    ) -> bool {
        if !event.pressed {
            return false;
        }
        match event.code {
            KeyCode::ArrowUp => {
                if self.selected == 0 {
                    return false;
                }
                let entries = crate::fs::vfs::list_dir(&self.cwd).unwrap_or_default();
                self.selected -= 1;
                if let Some(entry) = entries.get(self.selected) {
                    serial_println!("[apps  ] explorer select: {}", entry.name);
                }
                self.paint(wm, fs);
                true
            }
            KeyCode::ArrowDown => {
                let entries = crate::fs::vfs::list_dir(&self.cwd).unwrap_or_default();
                if self.selected + 1 >= entries.len() {
                    return false;
                }
                self.selected += 1;
                if let Some(entry) = entries.get(self.selected) {
                    serial_println!("[apps  ] explorer select: {}", entry.name);
                }
                self.paint(wm, fs);
                true
            }
            KeyCode::Enter => {
                let entries = crate::fs::vfs::list_dir(&self.cwd).unwrap_or_default();
                let Some(entry) = entries.get(self.selected) else {
                    return false;
                };
                let path = join(&self.cwd, &entry.name);
                if entry.is_dir {
                    self.cwd = path;
                    self.selected = 0;
                    self.refresh(wm, fs)
                } else {
                    editor.open_path(&path, wm, fs);
                    true
                }
            }
            KeyCode::Delete if !wm.has_text_focus() => self.delete_selected(wm, fs),
            KeyCode::Backspace if !wm.has_text_focus() => self.go_up(wm, fs),
            _ => false,
        }
    }

    // -- helpers -----------------------------------------------------------

    /// Rewrites the cwd header and all [`EX_ROW_COUNT`] rows from the
    /// current listing.
    fn paint(&self, wm: &mut WindowManager, _fs: &FileSystem) {
        wm.set_label(self.win, EX_CWD, clip(&self.cwd, CWD_CLIP));
        let entries = crate::fs::vfs::list_dir(&self.cwd).unwrap_or_default();
        for i in 0..EX_ROW_COUNT {
            let text = match entries.get(i) {
                Some(entry) => {
                    let tag = if entry.is_dir {
                        "[DIR]"
                    } else if entry.name.ends_with(".rs") {
                        "[RS ]"
                    } else if entry.name.ends_with(".txt") {
                        "[TXT]"
                    } else if entry.name.ends_with(".elf") {
                        "[ELF]"
                    } else {
                        "[FILE]"
                    };
                    let size_str = if entry.is_dir {
                        String::new()
                    } else if entry.size >= 1024 {
                        format!(" {}KB", entry.size / 1024)
                    } else {
                        format!(" {}B", entry.size)
                    };
                    let max_name = NAME_CLIP.saturating_sub(7 + size_str.len()).max(8);
                    let mut name = clip(&entry.name, max_name);
                    if entry.is_dir {
                        name.push('/');
                    }
                    let mark = if i == self.selected { "> " } else { "  " };
                    format!("{mark}{tag} {name}{size_str}")
                }
                None => String::new(),
            };
            wm.set_label(self.win, EX_ROWS + i, text);
        }
    }

    /// Descends to the parent directory (`"/a/b"` -> `"/a"` -> `"/"`).
    fn go_up(&mut self, wm: &mut WindowManager, fs: &FileSystem) -> bool {
        if self.cwd == "/" {
            return false;
        }
        self.cwd = parent_of(&self.cwd);
        self.selected = 0;
        self.refresh(wm, fs)
    }

    /// Creates `cwd/<name>` from the [`EX_INPUT`] prompt.
    fn create_file(&mut self, wm: &mut WindowManager, fs: &mut FileSystem) -> bool {
        let Some(name) = wm.text_value(self.win, EX_INPUT) else {
            return false;
        };
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let path = join(&self.cwd, name);
        if let Err(err) = crate::fs::vfs::write(&path, &[]) {
            serial_println!("[apps  ] explorer create file error: {err}");
            crate::gui::notify("Explorer", format!("Create failed: {err}"), crate::gui::NotificationKind::Error);
            return false;
        }
        if !crate::fs::vfs::is_disk_path(&path) {
            fs.write(&path, "");
        }
        set_field(wm, self.win, EX_INPUT, "");
        self.selected = crate::fs::vfs::list_dir(&self.cwd)
            .ok()
            .and_then(|entries| entries.iter().position(|e| e.name.eq_ignore_ascii_case(name)))
            .unwrap_or(0);
        self.refresh(wm, fs)
    }

    /// Creates a directory `cwd/<name>` from the [`EX_INPUT`] prompt.
    fn create_dir(&mut self, wm: &mut WindowManager, fs: &mut FileSystem) -> bool {
        let Some(name) = wm.text_value(self.win, EX_INPUT) else {
            return false;
        };
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let path = join(&self.cwd, name);
        if let Err(err) = crate::fs::vfs::mkdir(&path) {
            serial_println!("[apps  ] explorer mkdir error: {err}");
            crate::gui::notify("Explorer", format!("mkdir failed: {err}"), crate::gui::NotificationKind::Error);
            return false;
        }
        if !crate::fs::vfs::is_disk_path(&path) {
            fs.ensure_dir_pub(&path);
        }
        set_field(wm, self.win, EX_INPUT, "");
        self.selected = crate::fs::vfs::list_dir(&self.cwd)
            .ok()
            .and_then(|entries| entries.iter().position(|e| e.name.eq_ignore_ascii_case(name)))
            .unwrap_or(0);
        self.refresh(wm, fs)
    }

    /// Deletes the currently selected file or directory.
    fn delete_selected(&mut self, wm: &mut WindowManager, fs: &mut FileSystem) -> bool {
        let entries = crate::fs::vfs::list_dir(&self.cwd).unwrap_or_default();
        let Some(entry) = entries.get(self.selected) else {
            return false;
        };
        if self.cwd == "/" && entry.name == "disk" {
            crate::gui::notify("Explorer", String::from("cannot delete /disk mount"), crate::gui::NotificationKind::Error);
            return false;
        }
        let path = join(&self.cwd, &entry.name);
        if let Err(err) = crate::fs::vfs::delete(&path) {
            serial_println!("[apps  ] explorer delete error: {err}");
            crate::gui::notify("Explorer", format!("delete failed: {err}"), crate::gui::NotificationKind::Error);
            return false;
        }
        if !crate::fs::vfs::is_disk_path(&path) {
            fs.delete(&path);
        }
        crate::gui::notify("Explorer", format!("Deleted {}", entry.name), crate::gui::NotificationKind::Success);
        self.refresh(wm, fs)
    }

    /// Renames the currently selected file or directory to the text in the input box.
    fn rename_selected(&mut self, wm: &mut WindowManager, fs: &mut FileSystem) -> bool {
        let Some(new_name) = wm.text_value(self.win, EX_INPUT) else {
            return false;
        };
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return false;
        }
        let entries = crate::fs::vfs::list_dir(&self.cwd).unwrap_or_default();
        let Some(entry) = entries.get(self.selected) else {
            return false;
        };
        if self.cwd == "/" && entry.name == "disk" {
            crate::gui::notify("Explorer", String::from("cannot rename /disk mount"), crate::gui::NotificationKind::Error);
            return false;
        }
        let old_path = join(&self.cwd, &entry.name);
        let new_path = join(&self.cwd, new_name);
        if let Err(err) = crate::fs::vfs::rename(&old_path, &new_path) {
            serial_println!("[apps  ] explorer rename error: {err}");
            crate::gui::notify("Explorer", format!("rename failed: {err}"), crate::gui::NotificationKind::Error);
            return false;
        }
        if !crate::fs::vfs::is_disk_path(&old_path) {
            let _ = fs.rename(&old_path, &new_path);
        }
        set_field(wm, self.win, EX_INPUT, "");
        crate::gui::notify("Explorer", format!("Renamed to {new_name}"), crate::gui::NotificationKind::Success);
        self.refresh(wm, fs)
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Joins `name` onto `dir`; the root already carries its own slash.
fn join(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The parent of an absolute directory: `"/a/b"` -> `"/a"`,
/// `"/a"` -> `"/"` (and `"/"` itself stays put — callers check first).
fn parent_of(dir: &str) -> String {
    match dir.rfind('/') {
        Some(0) | None => String::from("/"),
        Some(i) => String::from(&dir[..i]),
    }
}

/// Writes `value` into the text field at `index` (caret to the end).
///
/// The WM has no public setter for a field's value — it owns *focus*,
/// not content — so this reaches through [`WindowManager::window_mut`].
/// A closed window or a stale index is a silent no-op: windows can be
/// Esc'd away under us, and a missing field must never panic.
fn set_field(wm: &mut WindowManager, win: u64, index: usize, value: &str) {
    if let Some(w) = wm.window_mut(win)
        && let Some(Widget::TextInput(t)) = w.widgets.get_mut(index)
    {
        t.value = String::from(value);
        t.cursor = t.value.len();
    }
}
