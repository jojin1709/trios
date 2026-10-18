//! In-memory filesystem ("ramfs") — the Phase 4 storage foundation (RS-001).
//!
//! ### What this is (and isn't)
//!
//! A tiny tree of files living entirely on the kernel heap (16 MiB, see
//! [`crate::allocator`]). There is **no disk**: nothing is persisted, and
//! a reboot wipes it. That's deliberate — the File Explorer / Text Editor
//! / Terminal need *a* filesystem to talk to long before a block layer
//! exists, and an in-memory one lets Phase 4 ship its UI against a real
//! API without dragging a driver into the critical path.
//!
//! ### Design: flat `Vec` of path-keyed nodes
//!
//! Same philosophy as the GUI widgets ([`crate::gui::widgets`]): plain
//! data + methods, no trait objects, no dyn dispatch. The whole tree is
//! one `Vec<Node>` where each node's **normalized absolute path is its
//! key** (`/notes/hello.txt`). Lookups are O(n) scans — with demo-scale
//! trees (a handful of files) that is free, and it keeps the entire
//! module dependency-free and ~150 lines. If a future phase grows the
//! tree past a few hundred nodes, swapping the Vec for a `BTreeMap` is a
//! private, local change: the public API below doesn't leak the storage.
//!
//! ### Path normalization (one function, every entry point)
//!
//! * a leading `/` is optional (`readme.txt` ≡ `/readme.txt`);
//! * `//` collapses to `/`;
//! * `.` is dropped and `..` is *ignored* — there is no parent of the
//!   root, so `..` can never escape it (no path-traversal bug class at
//!   all: the component simply doesn't exist in this model).
//!
//! Everything outside [`FileSystem::new`] funnels through [`normalize`],
//! so every public method sees the same canonical form.

// The public API is the Phase 4 contract — the File Explorer, Text Editor
// and Terminal land next cycle and consume every method. This is a binary
// crate, so the dead-code lint can't see that future consumer; silence it
// here (and only here), exactly like `graphics::splash` above.
#![allow(dead_code)]

pub mod elf;
pub mod fat;
pub mod gpt;
pub mod vfs;

use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// One immediate child of a directory, as reported by [`FileSystem::list`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Basename only (`hello.txt` for `/notes/hello.txt`) — the explorer
    /// draws this; the full path is `dir + "/" + name`.
    pub name: String,
    /// `true` for directories (size is then always 0).
    pub is_dir: bool,
    /// File size in bytes (UTF-8 length of the contents).
    pub size: usize,
}

/// One node of the tree: either a file (with contents) or a directory
/// (contents always empty — directories are pure structure here).
#[derive(Debug, Clone)]
struct Node {
    /// Normalized absolute path, no trailing slash — the path *is* the
    /// key (`""` is never stored; the root exists implicitly).
    path: String,
    is_dir: bool,
    /// File payload in bytes. Always empty for directories.
    contents: Vec<u8>,
}

/// The in-memory filesystem. Create with [`FileSystem::new`] — the
/// desktop owns the instance (see `main.rs::desktop_run`).
#[derive(Debug, Clone)]
pub struct FileSystem {
    /// Flat node list; see module docs for why not a map.
    nodes: Vec<Node>,
}

// ---------------------------------------------------------------------------
// Path normalization
// ---------------------------------------------------------------------------

/// Canonicalizes `path` per the module-doc rules: optional leading `/`,
/// `//` collapsed, `.` dropped, `..` pops the parent without escaping the root.
///
/// Returns the *bare* form without a leading slash for the root
/// (`normalize("/a/b") == "/a/b"`, `normalize("//a//./b/../c") == "/a/c"`,
/// `normalize("///") == ""`). The empty string means "the root directory
/// itself" — every caller treats it as valid (root always exists).
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => {
                parts.push(p);
            }
        }
    }
    if parts.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for p in parts {
        out.push('/');
        out.push_str(p);
    }
    out
}

// ---------------------------------------------------------------------------
// FileSystem
// ---------------------------------------------------------------------------

impl FileSystem {
    /// A ramfs pre-seeded with demo content, so the File Explorer is
    /// never staring at an empty window. Seeds (paths are the normalized
    /// form; parent dirs are created implicitly by `write`):
    ///
    /// * `/readme.txt`        — a short welcome note;
    /// * `/notes/hello.txt`   — proves nested directories work;
    /// * `/notes/todo.txt`    — a second file in the same directory.
    /// * `/bin/hello.elf`     — valid ELF64 executable.
    pub fn new() -> Self {
        let mut fs = Self { nodes: Vec::new() };
        // Seed contents are constants, so these writes cannot fail
        // (write only rejects the root path — see `write`).
        fs.write(
            "/readme.txt",
            "Welcome to TriOS!\n\n\
             This filesystem lives entirely in RAM — nothing here survives a reboot.\n\
             Phase 4: File Explorer, Text Editor, Terminal.\n\
             Phase 8: Standalone ELF Executable Loader.",
        );
        fs.write(
            "/notes/hello.txt",
            "Hello from ramfs!\n\nYou created nothing: this file was seeded at boot.",
        );
        fs.write(
            "/notes/todo.txt",
            "- File Explorer\n- Text Editor\n- Terminal\n- [x] in-memory VFS\n- [x] ELF64 Loader",
        );
        fs.write_bytes("/bin/hello.elf", &crate::fs::elf::create_sample_elf());
        fs.write_bytes("/bin/fault.elf", &crate::fs::elf::create_fault_elf());
        fs.write_bytes("/bin/fuzz.elf", &crate::fs::elf::create_fuzz_elf());
        fs
    }

    /// Immediate children of `dir` (never recursive). Directories sort
    /// first, then entries alphabetically — a stable, explorer-friendly
    /// order. Unknown directories yield an empty `Vec` (not a panic:
    /// callers race the user, not a database).
    pub fn list(&self, dir: &str) -> Vec<DirEntry> {
        let mut prefix = normalize(dir);
        // `"" -> "/"` and `"/notes" -> "/notes/"` in one step: every
        // child path starts with this prefix, and no node *equals* it
        // (nodes never end in '/'), so the dir itself never lists itself.
        prefix.push('/');
        let mut entries: Vec<DirEntry> = self
            .nodes
            .iter()
            .filter_map(|n| {
                let rest = n.path.strip_prefix(&prefix)?;
                // A '/' still left means a grandchild — not immediate.
                if rest.contains('/') {
                    return None;
                }
                Some(DirEntry {
                    name: String::from(rest),
                    is_dir: n.is_dir,
                    size: if n.is_dir { 0 } else { n.contents.len() },
                })
            })
            .collect();
        // Directories first (`true > false`, so compare the flags
        // *reversed*), then name ascending.
        entries.sort_by(|a, b| (b.is_dir, &a.name).cmp(&(a.is_dir, &b.name)));
        entries
    }

    /// A file's contents decoded as UTF-8 string, or `None` when missing,
    /// a directory, or non-UTF8 binary data.
    pub fn read(&self, path: &str) -> Option<&str> {
        self.read_bytes(path)
            .and_then(|bytes| core::str::from_utf8(bytes).ok())
    }

    /// A file's raw contents in bytes, or `None` when the path is missing or is a directory.
    pub fn read_bytes(&self, path: &str) -> Option<&[u8]> {
        let path = normalize(path);
        self.nodes
            .iter()
            .find(|n| n.path == path && !n.is_dir)
            .map(|n| n.contents.as_slice())
    }

    /// Creates or overwrites the file at `path` with a text string.
    pub fn write(&mut self, path: &str, contents: &str) -> bool {
        self.write_bytes(path, contents.as_bytes())
    }

    /// Creates or overwrites the file at `path` with raw bytes, creating any missing
    /// parent directories (`mkdir -p` semantics). Returns `false` only
    /// for an invalid path — the empty/root path, which names a
    /// directory and therefore cannot be a file.
    pub fn write_bytes(&mut self, path: &str, contents: &[u8]) -> bool {
        let path = normalize(path);
        if path.is_empty() {
            // The root is a directory; "writing" it is a caller bug.
            return false;
        }
        // Ensure every ancestor directory exists. Walk each '/' in the
        // path: "/a/b/c" -> ensure "/a", then "/a/b".
        let mut cut = 0usize;
        while let Some(slash) = path[cut..].find('/') {
            let parent = &path[..cut + slash];
            self.ensure_dir(parent);
            cut += slash + 1;
        }
        // Now the file node itself: overwrite in place if present.
        if let Some(node) = self.nodes.iter_mut().find(|n| n.path == path) {
            node.is_dir = false;
            node.contents = contents.to_vec();
        } else {
            self.nodes.push(Node {
                path,
                is_dir: false,
                contents: contents.to_vec(),
            });
        }
        true
    }

    /// Does anything — file *or* directory — live at `path`? The root
    /// always answers `true`.
    pub fn exists(&self, path: &str) -> bool {
        let path = normalize(path);
        path.is_empty() || self.nodes.iter().any(|n| n.path == path)
    }

    /// How many files (directories excluded) exist anywhere in the tree —
    /// the boot log's "N files" number.
    pub fn file_count(&self) -> usize {
        self.nodes.iter().filter(|n| !n.is_dir).count()
    }

    /// Makes sure the directory `path` (already normalized, never root)
    /// has a node. Idempotent.
    fn ensure_dir(&mut self, path: &str) {
        if path.is_empty() || self.nodes.iter().any(|n| n.path == path) {
            return;
        }
        self.nodes.push(Node {
            path: String::from(path),
            is_dir: true,
            contents: Vec::new(),
        });
    }

    /// Public helper to ensure a directory exists.
    pub fn ensure_dir_pub(&mut self, path: &str) {
        let path = normalize(path);
        self.ensure_dir(&path);
    }

    /// Deletes a file or directory and all its descendants from ramfs.
    pub fn delete(&mut self, path: &str) -> bool {
        let path = normalize(path);
        if path.is_empty() {
            return false;
        }
        let old_len = self.nodes.len();
        let prefix = alloc::format!("{path}/");
        self.nodes.retain(|n| n.path != path && !n.path.starts_with(&prefix));
        self.nodes.len() < old_len
    }

    /// Renames a node in ramfs.
    pub fn rename(&mut self, old_path: &str, new_path: &str) -> Result<(), &'static str> {
        let old_path = normalize(old_path);
        let new_path = normalize(new_path);
        if old_path.is_empty() || new_path.is_empty() {
            return Err("invalid path");
        }
        if let Some(pos) = self.nodes.iter().position(|n| n.path == old_path) {
            self.nodes[pos].path = new_path;
            Ok(())
        } else {
            Err("file not found in ramfs")
        }
    }
}

/// `FileSystem::default()` — an **empty** fs (no seeds). The desktop
/// uses `new()` (seeded); `default` exists for the `clippy::new_without_default`
/// contract and for tests that want a blank tree.
impl Default for FileSystem {
    fn default() -> Self {
        Self { nodes: Vec::new() }
    }
}

pub static GLOBAL_FS: spin::Mutex<Option<FileSystem>> = spin::Mutex::new(None);

/// Initializes the shared global filesystem for system calls and kernel apps.
pub fn init_fs(fs: FileSystem) {
    *GLOBAL_FS.lock() = Some(fs);
}

/// Reads file bytes from the shared global filesystem.
pub fn read_bytes(path: &str) -> Option<Vec<u8>> {
    GLOBAL_FS
        .lock()
        .as_ref()
        .and_then(|fs| fs.read_bytes(path).map(|b| b.to_vec()))
}

/// Writes file bytes to the shared global filesystem.
pub fn write_bytes(path: &str, data: &[u8]) -> bool {
    GLOBAL_FS
        .lock()
        .as_mut()
        .map_or(false, |fs| fs.write_bytes(path, data))
}

/// Checks if a path exists in the shared global filesystem.
pub fn exists(path: &str) -> bool {
    GLOBAL_FS.lock().as_ref().map_or(false, |fs| fs.exists(path))
}
