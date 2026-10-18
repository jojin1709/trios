//! Virtual File System (VFS) with unified mount points and path dispatch.
//!
//! Mount layout:
//!   - `/`       -> `RamFs` (in-memory filesystem with system binaries and user files)
//!   - `/disk`   -> `FatFs` (persistent FAT32 partition on active SATA hard drive)
//!
//! Provides transparent file operations across filesystems:
//!   - `read`, `write`, `append`, `create_file`, `delete`, `mkdir`, `rename`, `list_dir`, `exists`, `is_dir`
//!   - `fsck_disk` for FAT volume consistency verification.

use alloc::{format, string::String, vec::Vec};
use spin::Mutex;

use super::fat::{FatFileSystem, FsckReport};
use crate::drivers::ahci;

/// Unified VFS Directory Entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VfsEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: usize,
}

pub static GLOBAL_FAT: Mutex<Option<FatFileSystem>> = Mutex::new(None);

/// Initializes the global VFS with the mounted FAT32 volume.
pub fn init_vfs(fat: FatFileSystem) {
    *GLOBAL_FAT.lock() = Some(fat);
    serial_println!("[vfs    ] unified VFS initialized: '/' (ramfs), '/disk' (FAT32 volume)");
}

/// Normalizes a path, stripping double slashes and collapsing redundant dots.
pub fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        String::from("/")
    } else {
        format!("/{}", parts.join("/"))
    }
}

/// Checks if a path targets the `/disk` mount point.
pub fn is_disk_path(path: &str) -> bool {
    let norm = normalize_path(path);
    norm == "/disk" || norm.starts_with("/disk/")
}

/// Strips the `/disk` prefix from a path to obtain the FAT-relative subpath.
pub fn to_disk_subpath(path: &str) -> String {
    let norm = normalize_path(path);
    if norm == "/disk" {
        String::from("/")
    } else if let Some(sub) = norm.strip_prefix("/disk/") {
        format!("/{sub}")
    } else {
        norm
    }
}

/// Reads the entire contents of a file via VFS.
pub fn read(path: &str) -> Option<Vec<u8>> {
    let norm = normalize_path(path);
    if is_disk_path(&norm) {
        let sub = to_disk_subpath(&norm);
        let mut lock = GLOBAL_FAT.lock();
        lock.as_mut().and_then(|fat| fat.read_file(&sub, &ahci::read_sector).ok())
    } else {
        super::read_bytes(&norm)
    }
}

/// Writes or creates a file via VFS.
pub fn write(path: &str, data: &[u8]) -> Result<(), &'static str> {
    let norm = normalize_path(path);
    if is_disk_path(&norm) {
        let sub = to_disk_subpath(&norm);
        let mut lock = GLOBAL_FAT.lock();
        let fat = lock.as_mut().ok_or("FAT volume not mounted")?;
        fat.write_file(&sub, data, &ahci::read_sector, &ahci::write_sector)
    } else {
        if super::write_bytes(&norm, data) {
            Ok(())
        } else {
            Err("failed to write to ramfs")
        }
    }
}

/// Appends data to an existing or new file via VFS.
pub fn append(path: &str, data: &[u8]) -> Result<(), &'static str> {
    let norm = normalize_path(path);
    if is_disk_path(&norm) {
        let sub = to_disk_subpath(&norm);
        let mut lock = GLOBAL_FAT.lock();
        let fat = lock.as_mut().ok_or("FAT volume not mounted")?;
        fat.append_file(&sub, data, &ahci::read_sector, &ahci::write_sector)
    } else {
        let mut existing = super::read_bytes(&norm).unwrap_or_default();
        existing.extend_from_slice(data);
        if super::write_bytes(&norm, &existing) {
            Ok(())
        } else {
            Err("failed to append to ramfs")
        }
    }
}

/// Creates a new empty file via VFS.
pub fn create_file(path: &str) -> Result<(), &'static str> {
    write(path, &[])
}

/// Deletes a file via VFS.
pub fn delete(path: &str) -> Result<(), &'static str> {
    let norm = normalize_path(path);
    if is_disk_path(&norm) {
        let sub = to_disk_subpath(&norm);
        let mut lock = GLOBAL_FAT.lock();
        let fat = lock.as_mut().ok_or("FAT volume not mounted")?;
        fat.delete_file(&sub, &ahci::read_sector, &ahci::write_sector)
    } else {
        if let Some(fs) = super::GLOBAL_FS.lock().as_mut() {
            if fs.delete(&norm) {
                Ok(())
            } else {
                Err("file not found in ramfs")
            }
        } else {
            Err("ramfs not initialized")
        }
    }
}

/// Creates a new directory via VFS.
pub fn mkdir(path: &str) -> Result<(), &'static str> {
    let norm = normalize_path(path);
    if is_disk_path(&norm) {
        let sub = to_disk_subpath(&norm);
        let mut lock = GLOBAL_FAT.lock();
        let fat = lock.as_mut().ok_or("FAT volume not mounted")?;
        fat.mkdir(&sub, &ahci::read_sector, &ahci::write_sector)
    } else {
        if let Some(fs) = super::GLOBAL_FS.lock().as_mut() {
            fs.ensure_dir_pub(&norm);
            Ok(())
        } else {
            Err("ramfs not initialized")
        }
    }
}

/// Renames a file or directory via VFS.
pub fn rename(old_path: &str, new_path: &str) -> Result<(), &'static str> {
    let old_norm = normalize_path(old_path);
    let new_norm = normalize_path(new_path);

    let old_is_disk = is_disk_path(&old_norm);
    let new_is_disk = is_disk_path(&new_norm);

    if old_is_disk != new_is_disk {
        return Err("cannot rename across filesystem mount boundaries");
    }

    if old_is_disk {
        let old_sub = to_disk_subpath(&old_norm);
        let new_sub = to_disk_subpath(&new_norm);
        let mut lock = GLOBAL_FAT.lock();
        let fat = lock.as_mut().ok_or("FAT volume not mounted")?;
        fat.rename(&old_sub, &new_sub, &ahci::read_sector, &ahci::write_sector)
    } else {
        if let Some(fs) = super::GLOBAL_FS.lock().as_mut() {
            fs.rename(&old_norm, &new_norm)
        } else {
            Err("ramfs not initialized")
        }
    }
}

/// Lists files and directories at `path`.
pub fn list_dir(path: &str) -> Result<Vec<VfsEntry>, &'static str> {
    let norm = normalize_path(path);
    if is_disk_path(&norm) {
        let sub = to_disk_subpath(&norm);
        let lock = GLOBAL_FAT.lock();
        let fat = lock.as_ref().ok_or("FAT volume not mounted")?;
        let fat_entries = fat.list_dir(&sub, &ahci::read_sector)?;
        Ok(fat_entries
            .into_iter()
            .map(|e| VfsEntry {
                name: e.name,
                is_dir: e.is_dir,
                size: e.size,
            })
            .collect())
    } else {
        let mut entries = Vec::new();
        if let Some(fs) = super::GLOBAL_FS.lock().as_ref() {
            for e in fs.list(&norm) {
                entries.push(VfsEntry {
                    name: e.name,
                    is_dir: e.is_dir,
                    size: e.size,
                });
            }
        }
        // If listing root '/', show mounted directory '[DIR] disk'
        if norm == "/" {
            entries.push(VfsEntry {
                name: String::from("disk"),
                is_dir: true,
                size: 0,
            });
            // Stable sort: dirs first, then alphabetical
            entries.sort_by(|a, b| (b.is_dir, &a.name).cmp(&(a.is_dir, &b.name)));
        }
        Ok(entries)
    }
}

/// Checks if a file or directory exists via VFS.
pub fn exists(path: &str) -> bool {
    let norm = normalize_path(path);
    if norm == "/" || norm == "/disk" {
        return true;
    }
    if is_disk_path(&norm) {
        let sub = to_disk_subpath(&norm);
        let (dir_part, name) = match sub.trim_matches('/').rfind('/') {
            Some(idx) => (&sub[..idx + 1], &sub[idx + 2..]),
            None => ("/", sub.trim_matches('/')),
        };
        let lock = GLOBAL_FAT.lock();
        if let Some(fat) = lock.as_ref() {
            if let Ok(entries) = fat.list_dir(dir_part, &ahci::read_sector) {
                return entries.iter().any(|e| e.name.eq_ignore_ascii_case(name));
            }
        }
        false
    } else {
        super::exists(&norm)
    }
}

/// Runs fsck-lite consistency check on the mounted FAT volume.
pub fn fsck_disk() -> Result<FsckReport, &'static str> {
    let lock = GLOBAL_FAT.lock();
    let fat = lock.as_ref().ok_or("no FAT volume mounted at /disk")?;
    fat.fsck_lite(&ahci::read_sector)
}
