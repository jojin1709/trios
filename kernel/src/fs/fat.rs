//! FAT12 / FAT16 / FAT32 filesystem driver for TriOS.
//!
//! Conforms to Microsoft FAT specification:
//! - Auto-detects FAT12, FAT16, and FAT32 based on volume cluster count.
//! - Parses BPB (BIOS Parameter Block) and File Allocation Tables.
//! - Supports directory traversal, subdirectory resolution, and file reads.
//! - Supports FAT32 writes: file creation, writing, appending, deletion, directory creation (`mkdir`), renaming, and FAT consistency checking (`fsck_lite`).

use alloc::{format, string::String, vec, vec::Vec};

/// FAT Directory Entry Attributes
pub const ATTR_READ_ONLY: u8 = 0x01;
pub const ATTR_HIDDEN: u8 = 0x02;
pub const ATTR_SYSTEM: u8 = 0x04;
pub const ATTR_VOLUME_ID: u8 = 0x08;
pub const ATTR_DIRECTORY: u8 = 0x10;
pub const ATTR_ARCHIVE: u8 = 0x20;
pub const ATTR_LFN: u8 = 0x0F;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatType {
    Fat12,
    Fat16,
    Fat32,
}

#[derive(Debug, Clone)]
pub struct FatEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: usize,
    pub first_cluster: u32,
}

#[derive(Debug, Clone)]
pub struct FatBpb {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    pub root_entries: u16,
    pub total_sectors: u32,
    pub sectors_per_fat: u32,
    pub root_cluster: u32,
    pub fat_type: FatType,
    pub volume_label: String,
}

/// Consistency check report returned by `fsck_lite`.
#[derive(Debug, Clone)]
pub struct FsckReport {
    pub volume_label: String,
    pub fat_type: FatType,
    pub total_sectors: u32,
    pub sectors_per_cluster: u8,
    pub total_clusters: u32,
    pub used_clusters: u32,
    pub free_clusters: u32,
    pub file_count: usize,
    pub dir_count: usize,
    pub errors: Vec<String>,
    pub is_clean: bool,
}

pub struct FatFileSystem {
    pub partition_lba: u64,
    pub bpb: FatBpb,
    pub fat_cache: Vec<u8>,
    pub data_start_lba: u64,
    pub root_dir_lba: u64,
    pub root_dir_sectors: u32,
}

impl FatFileSystem {
    /// Mounts a FAT filesystem located at `partition_lba` using the sector reader.
    pub fn mount<F>(partition_lba: u64, read_sec: &F) -> Result<Self, &'static str>
    where
        F: Fn(u64, &mut [u8; 512]) -> Result<(), &'static str>,
    {
        let mut sector = [0u8; 512];
        read_sec(partition_lba, &mut sector)?;

        // Validate boot sector signature (0x55, 0xAA)
        if sector[510] != 0x55 || sector[511] != 0xAA {
            return Err("invalid boot sector signature (missing 0x55AA)");
        }

        let bytes_per_sector = u16::from_le_bytes([sector[11], sector[12]]);
        if bytes_per_sector != 512 {
            return Err("unsupported sector size (only 512 bytes supported)");
        }

        let sectors_per_cluster = sector[13];
        if sectors_per_cluster == 0 {
            return Err("invalid sectors per cluster");
        }

        let reserved_sectors = u16::from_le_bytes([sector[14], sector[15]]);
        let num_fats = sector[16];
        let root_entries = u16::from_le_bytes([sector[17], sector[18]]);

        let tot_sec16 = u16::from_le_bytes([sector[19], sector[20]]);
        let tot_sec32 = u32::from_le_bytes(sector[32..36].try_into().unwrap());
        let total_sectors = if tot_sec16 != 0 {
            tot_sec16 as u32
        } else {
            tot_sec32
        };

        let fat_sz16 = u16::from_le_bytes([sector[22], sector[23]]);
        let fat_sz32 = u32::from_le_bytes(sector[36..40].try_into().unwrap());
        let (sectors_per_fat, root_cluster) = if fat_sz16 != 0 {
            (fat_sz16 as u32, 0u32)
        } else {
            let rclus = u32::from_le_bytes(sector[44..48].try_into().unwrap());
            (fat_sz32, rclus)
        };

        let root_dir_sectors = ((root_entries as u32 * 32) + (bytes_per_sector as u32 - 1))
            / bytes_per_sector as u32;

        let root_dir_lba = partition_lba + reserved_sectors as u64 + (num_fats as u64 * sectors_per_fat as u64);
        let data_start_lba = root_dir_lba + root_dir_sectors as u64;

        let data_sectors = total_sectors.saturating_sub((data_start_lba - partition_lba) as u32);
        let total_clusters = data_sectors / sectors_per_cluster as u32;

        let fat_type = if total_clusters < 4085 {
            FatType::Fat12
        } else if total_clusters < 65525 {
            FatType::Fat16
        } else {
            FatType::Fat32
        };

        let volume_label = if fat_type == FatType::Fat32 {
            let slice = &sector[71..82];
            String::from_utf8_lossy(slice).trim().into()
        } else {
            let slice = &sector[43..54];
            String::from_utf8_lossy(slice).trim().into()
        };

        let bpb = FatBpb {
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sectors,
            num_fats,
            root_entries,
            total_sectors,
            sectors_per_fat,
            root_cluster,
            fat_type,
            volume_label,
        };

        // Cache initial sectors of FAT
        let fat_lba = partition_lba + reserved_sectors as u64;
        let cache_sectors = sectors_per_fat.min(128) as usize;
        let mut fat_cache = vec![0u8; cache_sectors * 512];
        for s in 0..cache_sectors {
            let mut sec = [0u8; 512];
            read_sec(fat_lba + s as u64, &mut sec)?;
            fat_cache[s * 512..(s + 1) * 512].copy_from_slice(&sec);
        }

        Ok(Self {
            partition_lba,
            bpb,
            fat_cache,
            data_start_lba,
            root_dir_lba,
            root_dir_sectors,
        })
    }

    /// Resolves the next cluster in a cluster chain from the FAT table.
    pub fn next_cluster(&self, cluster: u32) -> Option<u32> {
        match self.bpb.fat_type {
            FatType::Fat12 => {
                let offset = (cluster + (cluster / 2)) as usize;
                if offset + 1 >= self.fat_cache.len() {
                    return None;
                }
                let val = u16::from_le_bytes([self.fat_cache[offset], self.fat_cache[offset + 1]]);
                let next = if (cluster & 1) != 0 {
                    (val >> 4) as u32
                } else {
                    (val & 0x0FFF) as u32
                };
                if next >= 0x0FF8 {
                    None
                } else {
                    Some(next)
                }
            }
            FatType::Fat16 => {
                let offset = (cluster as usize) * 2;
                if offset + 1 >= self.fat_cache.len() {
                    return None;
                }
                let next = u16::from_le_bytes([self.fat_cache[offset], self.fat_cache[offset + 1]]) as u32;
                if next >= 0xFFF8 {
                    None
                } else {
                    Some(next)
                }
            }
            FatType::Fat32 => {
                let offset = (cluster as usize) * 4;
                if offset + 3 >= self.fat_cache.len() {
                    return None;
                }
                let next = u32::from_le_bytes(self.fat_cache[offset..offset + 4].try_into().unwrap()) & 0x0FFF_FFFF;
                if next >= 0x0FFF_FFF8 {
                    None
                } else {
                    Some(next)
                }
            }
        }
    }

    /// Converts a standard 8.3 filename string into an 11-byte FAT directory name array.
    pub fn to_dos_8_3(name: &str) -> [u8; 11] {
        let mut dos = [b' '; 11];
        let trimmed = name.trim();
        let (base, ext) = match trimmed.rfind('.') {
            Some(idx) => (&trimmed[..idx], &trimmed[idx + 1..]),
            None => (trimmed, ""),
        };

        let mut i = 0;
        for b in base.bytes() {
            if i >= 8 {
                break;
            }
            dos[i] = b.to_ascii_uppercase();
            i += 1;
        }

        let mut j = 0;
        for b in ext.bytes() {
            if j >= 3 {
                break;
            }
            dos[8 + j] = b.to_ascii_uppercase();
            j += 1;
        }

        dos
    }

    /// Reads all 32-byte directory entries from raw directory sector bytes.
    fn parse_directory_bytes(&self, bytes: &[u8]) -> Vec<FatEntry> {
        let mut entries = Vec::new();
        for chunk in bytes.chunks_exact(32) {
            let first = chunk[0];
            if first == 0x00 {
                // End of directory marker
                break;
            }
            if first == 0xE5 {
                // Deleted entry
                continue;
            }

            let attr = chunk[11];
            if attr == ATTR_LFN || (attr & ATTR_VOLUME_ID) != 0 {
                // Skip LFN fragments and Volume labels
                continue;
            }

            let is_dir = (attr & ATTR_DIRECTORY) != 0;
            let mut name = String::new();

            // 8-character base name
            for &b in &chunk[0..8] {
                if b != b' ' && b != 0 {
                    name.push(b as char);
                }
            }

            // 3-character extension
            let mut ext = String::new();
            for &b in &chunk[8..11] {
                if b != b' ' && b != 0 {
                    ext.push(b as char);
                }
            }

            if !ext.is_empty() {
                name.push('.');
                name.push_str(&ext);
            }

            let clus_hi = u16::from_le_bytes([chunk[20], chunk[21]]) as u32;
            let clus_lo = u16::from_le_bytes([chunk[26], chunk[27]]) as u32;
            let first_cluster = (clus_hi << 16) | clus_lo;
            let size = u32::from_le_bytes(chunk[28..32].try_into().unwrap()) as usize;

            if name != "." && name != ".." && !name.is_empty() {
                entries.push(FatEntry {
                    name,
                    is_dir,
                    size: if is_dir { 0 } else { size },
                    first_cluster,
                });
            }
        }
        entries
    }

    /// Lists files and directories in the given path (e.g. "/" or "/EFI" or "/EFI/BOOT").
    pub fn list_dir<F>(&self, path: &str, read_sec: &F) -> Result<Vec<FatEntry>, &'static str>
    where
        F: Fn(u64, &mut [u8; 512]) -> Result<(), &'static str>,
    {
        let trimmed = path.trim_matches('/');
        if trimmed.is_empty() {
            // Root directory listing
            if self.bpb.fat_type == FatType::Fat32 {
                return self.list_cluster_dir(self.bpb.root_cluster, read_sec);
            } else {
                let mut dir_bytes = vec![0u8; self.root_dir_sectors as usize * 512];
                for s in 0..self.root_dir_sectors {
                    let mut sec = [0u8; 512];
                    read_sec(self.root_dir_lba + s as u64, &mut sec)?;
                    dir_bytes[s as usize * 512..(s as usize + 1) * 512].copy_from_slice(&sec);
                }
                return Ok(self.parse_directory_bytes(&dir_bytes));
            }
        }

        // Navigate components sequentially
        let components: Vec<&str> = trimmed.split('/').collect();
        let mut curr_cluster = if self.bpb.fat_type == FatType::Fat32 {
            self.bpb.root_cluster
        } else {
            0
        };

        for (idx, comp) in components.iter().enumerate() {
            let entries = if idx == 0 && self.bpb.fat_type != FatType::Fat32 {
                let mut dir_bytes = vec![0u8; self.root_dir_sectors as usize * 512];
                for s in 0..self.root_dir_sectors {
                    let mut sec = [0u8; 512];
                    read_sec(self.root_dir_lba + s as u64, &mut sec)?;
                    dir_bytes[s as usize * 512..(s as usize + 1) * 512].copy_from_slice(&sec);
                }
                self.parse_directory_bytes(&dir_bytes)
            } else {
                self.list_cluster_dir(curr_cluster, read_sec)?
            };

            let found = entries
                .into_iter()
                .find(|e| e.name.eq_ignore_ascii_case(comp) && e.is_dir);

            match found {
                Some(e) => curr_cluster = e.first_cluster,
                None => return Err("directory not found"),
            }
        }

        self.list_cluster_dir(curr_cluster, read_sec)
    }

    /// Reads a directory stored at a cluster chain.
    fn list_cluster_dir<F>(&self, start_cluster: u32, read_sec: &F) -> Result<Vec<FatEntry>, &'static str>
    where
        F: Fn(u64, &mut [u8; 512]) -> Result<(), &'static str>,
    {
        let mut curr = Some(start_cluster);
        let mut dir_bytes = Vec::new();

        while let Some(c) = curr {
            if c < 2 {
                break;
            }
            let cluster_lba = self.data_start_lba + ((c - 2) as u64 * self.bpb.sectors_per_cluster as u64);
            for s in 0..self.bpb.sectors_per_cluster {
                let mut sec = [0u8; 512];
                read_sec(cluster_lba + s as u64, &mut sec)?;
                dir_bytes.extend_from_slice(&sec);
            }
            curr = self.next_cluster(c);
            if dir_bytes.len() > 128 * 1024 {
                break; // Safety limit
            }
        }

        Ok(self.parse_directory_bytes(&dir_bytes))
    }

    /// Reads the entire contents of a file located at `path`.
    pub fn read_file<F>(&self, path: &str, read_sec: &F) -> Result<Vec<u8>, &'static str>
    where
        F: Fn(u64, &mut [u8; 512]) -> Result<(), &'static str>,
    {
        let trimmed = path.trim_matches('/');
        let (dir_part, filename) = match trimmed.rfind('/') {
            Some(idx) => (&trimmed[..idx], &trimmed[idx + 1..]),
            None => ("", trimmed),
        };

        let entries = self.list_dir(dir_part, read_sec)?;
        let target = entries
            .into_iter()
            .find(|e| e.name.eq_ignore_ascii_case(filename) && !e.is_dir)
            .ok_or("file not found")?;

        let mut data = Vec::with_capacity(target.size);
        let mut curr = Some(target.first_cluster);

        while let Some(c) = curr {
            if c < 2 {
                break;
            }
            let cluster_lba = self.data_start_lba + ((c - 2) as u64 * self.bpb.sectors_per_cluster as u64);
            for s in 0..self.bpb.sectors_per_cluster {
                let mut sec = [0u8; 512];
                read_sec(cluster_lba + s as u64, &mut sec)?;
                data.extend_from_slice(&sec);
                if data.len() >= target.size {
                    break;
                }
            }
            if data.len() >= target.size {
                break;
            }
            curr = self.next_cluster(c);
        }

        data.truncate(target.size);
        Ok(data)
    }

    // -----------------------------------------------------------------------
    // FAT32 Persistent Write Operations
    // -----------------------------------------------------------------------

    /// Returns the sector LBAs corresponding to a directory.
    /// If dir_cluster == 0 (root directory on FAT12/FAT16), returns the fixed root directory LBAs.
    /// Otherwise, follows the cluster chain starting at dir_cluster and returns all sector LBAs in the chain.
    fn get_dir_sectors<F>(&self, dir_cluster: u32, _read_sec: &F) -> Result<Vec<u64>, &'static str> {
        let mut sectors = Vec::new();
        if dir_cluster == 0 && self.bpb.fat_type != FatType::Fat32 {
            for s in 0..self.root_dir_sectors {
                sectors.push(self.root_dir_lba + s as u64);
            }
            return Ok(sectors);
        }

        let mut curr = Some(dir_cluster);
        while let Some(c) = curr {
            if c < 2 {
                break;
            }
            let cluster_lba = self.data_start_lba + ((c - 2) as u64 * self.bpb.sectors_per_cluster as u64);
            for s in 0..self.bpb.sectors_per_cluster {
                sectors.push(cluster_lba + s as u64);
            }
            curr = self.next_cluster(c);
            if sectors.len() > 256 {
                break; // safety limit
            }
        }
        Ok(sectors)
    }

    /// Sets an entry in the FAT table and writes the updated sector to disk.
    pub fn set_fat_entry<W>(
        &mut self,
        cluster: u32,
        value: u32,
        write_sec: &W,
    ) -> Result<(), &'static str>
    where
        W: Fn(u64, &[u8; 512]) -> Result<(), &'static str>,
    {
        match self.bpb.fat_type {
            FatType::Fat12 => {
                let offset = (cluster + (cluster / 2)) as usize;
                if offset + 1 >= self.fat_cache.len() {
                    return Err("cluster beyond cached FAT range");
                }
                let old_val = u16::from_le_bytes([self.fat_cache[offset], self.fat_cache[offset + 1]]);
                let new_val = if (cluster & 1) != 0 {
                    (old_val & 0x000F) | ((value as u16 & 0x0FFF) << 4)
                } else {
                    (old_val & 0xF000) | (value as u16 & 0x0FFF)
                };
                self.fat_cache[offset..offset + 2].copy_from_slice(&new_val.to_le_bytes());

                let sec_idx = (offset / 512) as u64;
                let fat1_lba = self.partition_lba + self.bpb.reserved_sectors as u64 + sec_idx;

                let mut sec_buf = [0u8; 512];
                let sec_start = (sec_idx as usize) * 512;
                sec_buf.copy_from_slice(&self.fat_cache[sec_start..sec_start + 512]);
                write_sec(fat1_lba, &sec_buf)?;

                if self.bpb.num_fats > 1 {
                    let fat2_lba = fat1_lba + self.bpb.sectors_per_fat as u64;
                    write_sec(fat2_lba, &sec_buf)?;
                }

                if offset % 512 == 511 && (sec_start + 1024 <= self.fat_cache.len()) {
                    let next_sec_lba = fat1_lba + 1;
                    sec_buf.copy_from_slice(&self.fat_cache[sec_start + 512..sec_start + 1024]);
                    write_sec(next_sec_lba, &sec_buf)?;
                    if self.bpb.num_fats > 1 {
                        let fat2_next_lba = next_sec_lba + self.bpb.sectors_per_fat as u64;
                        write_sec(fat2_next_lba, &sec_buf)?;
                    }
                }
                Ok(())
            }
            FatType::Fat16 => {
                let offset = (cluster as usize) * 2;
                if offset + 1 >= self.fat_cache.len() {
                    return Err("cluster beyond cached FAT range");
                }
                self.fat_cache[offset..offset + 2].copy_from_slice(&(value as u16).to_le_bytes());

                let sec_idx = (offset / 512) as u64;
                let fat1_lba = self.partition_lba + self.bpb.reserved_sectors as u64 + sec_idx;

                let mut sec_buf = [0u8; 512];
                let sec_start = (sec_idx as usize) * 512;
                sec_buf.copy_from_slice(&self.fat_cache[sec_start..sec_start + 512]);
                write_sec(fat1_lba, &sec_buf)?;

                if self.bpb.num_fats > 1 {
                    let fat2_lba = fat1_lba + self.bpb.sectors_per_fat as u64;
                    write_sec(fat2_lba, &sec_buf)?;
                }
                Ok(())
            }
            FatType::Fat32 => {
                let offset = (cluster as usize) * 4;
                if offset + 3 >= self.fat_cache.len() {
                    return Err("cluster beyond cached FAT range");
                }
                let old_val = u32::from_le_bytes(self.fat_cache[offset..offset + 4].try_into().unwrap());
                let new_val = (old_val & 0xF000_0000) | (value & 0x0FFF_FFFF);
                self.fat_cache[offset..offset + 4].copy_from_slice(&new_val.to_le_bytes());

                let sec_idx = (offset / 512) as u64;
                let fat1_lba = self.partition_lba + self.bpb.reserved_sectors as u64 + sec_idx;

                let mut sec_buf = [0u8; 512];
                let sec_start = (sec_idx as usize) * 512;
                sec_buf.copy_from_slice(&self.fat_cache[sec_start..sec_start + 512]);
                write_sec(fat1_lba, &sec_buf)?;

                if self.bpb.num_fats > 1 {
                    let fat2_lba = fat1_lba + self.bpb.sectors_per_fat as u64;
                    write_sec(fat2_lba, &sec_buf)?;
                }
                Ok(())
            }
        }
    }

    /// Finds and allocates the next free cluster in the FAT table, marking it as EOC.
    pub fn alloc_cluster<W>(&mut self, write_sec: &W) -> Result<u32, &'static str>
    where
        W: Fn(u64, &[u8; 512]) -> Result<(), &'static str>,
    {
        let data_sectors = self.bpb.total_sectors.saturating_sub((self.data_start_lba - self.partition_lba) as u32);
        let total_clusters = data_sectors / self.bpb.sectors_per_cluster.max(1) as u32;

        let max_clusters = match self.bpb.fat_type {
            FatType::Fat12 => 4084.min(total_clusters),
            FatType::Fat16 => 65524.min(total_clusters),
            FatType::Fat32 => (self.fat_cache.len() as u32 / 4).min(total_clusters),
        };

        let eoc_mark = match self.bpb.fat_type {
            FatType::Fat12 => 0x0FFF,
            FatType::Fat16 => 0xFFFF,
            FatType::Fat32 => 0x0FFF_FFFF,
        };

        for c in 2..max_clusters {
            let is_free = match self.bpb.fat_type {
                FatType::Fat12 => {
                    let offset = (c + (c / 2)) as usize;
                    if offset + 1 < self.fat_cache.len() {
                        let val = u16::from_le_bytes([self.fat_cache[offset], self.fat_cache[offset + 1]]);
                        let entry = if (c & 1) != 0 { val >> 4 } else { val & 0x0FFF };
                        entry == 0
                    } else {
                        false
                    }
                }
                FatType::Fat16 => {
                    let offset = (c as usize) * 2;
                    if offset + 1 < self.fat_cache.len() {
                        let val = u16::from_le_bytes([self.fat_cache[offset], self.fat_cache[offset + 1]]);
                        val == 0
                    } else {
                        false
                    }
                }
                FatType::Fat32 => {
                    let offset = (c as usize) * 4;
                    if offset + 3 < self.fat_cache.len() {
                        let val = u32::from_le_bytes(self.fat_cache[offset..offset + 4].try_into().unwrap()) & 0x0FFF_FFFF;
                        val == 0
                    } else {
                        false
                    }
                }
            };

            if is_free {
                self.set_fat_entry(c, eoc_mark, write_sec)?;

                // Zero out the cluster's sectors on disk
                let cluster_lba = self.data_start_lba + ((c - 2) as u64 * self.bpb.sectors_per_cluster as u64);
                let zero_buf = [0u8; 512];
                for s in 0..self.bpb.sectors_per_cluster {
                    write_sec(cluster_lba + s as u64, &zero_buf)?;
                }

                return Ok(c);
            }
        }
        Err("disk full: no free clusters in FAT")
    }

    /// Frees an entire cluster chain in the FAT table, setting each cluster entry back to 0.
    pub fn free_cluster_chain<W>(
        &mut self,
        start_cluster: u32,
        write_sec: &W,
    ) -> Result<(), &'static str>
    where
        W: Fn(u64, &[u8; 512]) -> Result<(), &'static str>,
    {
        let mut curr = start_cluster;
        let eoc_limit = match self.bpb.fat_type {
            FatType::Fat12 => 0x0FF8,
            FatType::Fat16 => 0xFFF8,
            FatType::Fat32 => 0x0FFF_FFF8,
        };

        while curr >= 2 && curr < eoc_limit {
            let next = match self.next_cluster(curr) {
                Some(n) => n,
                None => 0,
            };
            self.set_fat_entry(curr, 0, write_sec)?;
            if next < 2 || next >= eoc_limit {
                break;
            }
            curr = next;
        }
        Ok(())
    }

    /// Resolves the starting cluster for a directory path (e.g. "" for root).
    fn resolve_dir_cluster<F>(&self, path: &str, read_sec: &F) -> Result<u32, &'static str>
    where
        F: Fn(u64, &mut [u8; 512]) -> Result<(), &'static str>,
    {
        let trimmed = path.trim_matches('/');
        if trimmed.is_empty() {
            return Ok(if self.bpb.fat_type == FatType::Fat32 {
                self.bpb.root_cluster
            } else {
                0
            });
        }

        let components: Vec<&str> = trimmed.split('/').collect();
        let mut curr_cluster = if self.bpb.fat_type == FatType::Fat32 {
            self.bpb.root_cluster
        } else {
            0
        };

        for (idx, comp) in components.iter().enumerate() {
            let entries = if idx == 0 && self.bpb.fat_type != FatType::Fat32 {
                let mut dir_bytes = vec![0u8; self.root_dir_sectors as usize * 512];
                for s in 0..self.root_dir_sectors {
                    let mut sec = [0u8; 512];
                    read_sec(self.root_dir_lba + s as u64, &mut sec)?;
                    dir_bytes[s as usize * 512..(s as usize + 1) * 512].copy_from_slice(&sec);
                }
                self.parse_directory_bytes(&dir_bytes)
            } else {
                self.list_cluster_dir(curr_cluster, read_sec)?
            };

            let found = entries
                .into_iter()
                .find(|e| e.name.eq_ignore_ascii_case(comp) && e.is_dir)
                .ok_or("directory path component not found")?;
            curr_cluster = found.first_cluster;
        }

        Ok(curr_cluster)
    }

    /// Writes data to a file on the FAT volume, creating it if it does not exist.
    pub fn write_file<R, W>(
        &mut self,
        path: &str,
        data: &[u8],
        read_sec: &R,
        write_sec: &W,
    ) -> Result<(), &'static str>
    where
        R: Fn(u64, &mut [u8; 512]) -> Result<(), &'static str>,
        W: Fn(u64, &[u8; 512]) -> Result<(), &'static str>,
    {
        let trimmed = path.trim_matches('/');
        let (dir_part, filename) = match trimmed.rfind('/') {
            Some(idx) => (&trimmed[..idx], &trimmed[idx + 1..]),
            None => ("", trimmed),
        };

        if filename.is_empty() {
            return Err("invalid filename");
        }

        let dir_cluster = self.resolve_dir_cluster(dir_part, read_sec)?;
        let dir_sectors = self.get_dir_sectors(dir_cluster, read_sec)?;
        let dos_name = Self::to_dos_8_3(filename);

        // 1. Check if file already exists in directory to reuse or delete old cluster chain
        let mut dir_sec = [0u8; 512];
        let mut target_sec_lba = 0u64;
        let mut target_offset_in_sec = None;

        for &lba in &dir_sectors {
            read_sec(lba, &mut dir_sec)?;
            for (idx, chunk) in dir_sec.chunks_exact_mut(32).enumerate() {
                if chunk[0] == 0x00 || chunk[0] == 0xE5 {
                    if target_offset_in_sec.is_none() {
                        target_sec_lba = lba;
                        target_offset_in_sec = Some(idx * 32);
                    }
                    if chunk[0] == 0x00 {
                        break;
                    }
                } else if chunk[0..11] == dos_name && (chunk[11] & ATTR_DIRECTORY) == 0 {
                    // Match found: existing file to overwrite!
                    let old_hi = u16::from_le_bytes([chunk[20], chunk[21]]) as u32;
                    let old_lo = u16::from_le_bytes([chunk[26], chunk[27]]) as u32;
                    let old_clus = (old_hi << 16) | old_lo;
                    if old_clus >= 2 {
                        self.free_cluster_chain(old_clus, write_sec)?;
                    }
                    target_sec_lba = lba;
                    target_offset_in_sec = Some(idx * 32);
                    break;
                }
            }
            if target_offset_in_sec.is_some() && dir_sec[target_offset_in_sec.unwrap()] == dos_name[0] {
                break;
            }
        }

        let entry_offset = target_offset_in_sec.ok_or("directory full")?;

        // 2. Allocate clusters and write file payload
        let bytes_per_cluster = self.bpb.sectors_per_cluster as usize * 512;
        let num_clusters = if data.is_empty() {
            0
        } else {
            (data.len() + bytes_per_cluster - 1) / bytes_per_cluster
        };

        let mut first_cluster = 0u32;
        let mut prev_cluster = 0u32;

        for clus_idx in 0..num_clusters {
            let new_c = self.alloc_cluster(write_sec)?;
            if clus_idx == 0 {
                first_cluster = new_c;
            } else {
                self.set_fat_entry(prev_cluster, new_c, write_sec)?;
            }
            prev_cluster = new_c;

            // Write data chunk into cluster sectors
            let clus_data_start = clus_idx * bytes_per_cluster;
            let clus_data_end = (clus_data_start + bytes_per_cluster).min(data.len());
            let chunk = &data[clus_data_start..clus_data_end];

            let cluster_lba = self.data_start_lba + ((new_c - 2) as u64 * self.bpb.sectors_per_cluster as u64);
            for s in 0..self.bpb.sectors_per_cluster {
                let mut sec = [0u8; 512];
                let sec_start = s as usize * 512;
                let sec_end = (sec_start + 512).min(chunk.len());
                if sec_start < chunk.len() {
                    sec[0..sec_end - sec_start].copy_from_slice(&chunk[sec_start..sec_end]);
                }
                write_sec(cluster_lba + s as u64, &sec)?;
            }
        }

        // 3. Write 32-byte directory entry
        read_sec(target_sec_lba, &mut dir_sec)?;
        let entry = &mut dir_sec[entry_offset..entry_offset + 32];
        entry[0..11].copy_from_slice(&dos_name);
        entry[11] = ATTR_ARCHIVE;
        entry[12] = 0; // NT reserved
        entry[13] = 0;
        entry[14..16].copy_from_slice(&0u16.to_le_bytes()); // time
        entry[16..18].copy_from_slice(&0x5421u16.to_le_bytes()); // date
        entry[18..20].copy_from_slice(&0x5421u16.to_le_bytes());
        entry[20..22].copy_from_slice(&((first_cluster >> 16) as u16).to_le_bytes());
        entry[22..24].copy_from_slice(&0u16.to_le_bytes());
        entry[24..26].copy_from_slice(&0x5421u16.to_le_bytes());
        entry[26..28].copy_from_slice(&((first_cluster & 0xFFFF) as u16).to_le_bytes());
        entry[28..32].copy_from_slice(&(data.len() as u32).to_le_bytes());

        write_sec(target_sec_lba, &dir_sec)?;
        Ok(())
    }

    /// Appends data to an existing or new file on the FAT volume.
    pub fn append_file<R, W>(
        &mut self,
        path: &str,
        data: &[u8],
        read_sec: &R,
        write_sec: &W,
    ) -> Result<(), &'static str>
    where
        R: Fn(u64, &mut [u8; 512]) -> Result<(), &'static str>,
        W: Fn(u64, &[u8; 512]) -> Result<(), &'static str>,
    {
        let mut existing = self.read_file(path, read_sec).unwrap_or_default();
        existing.extend_from_slice(data);
        self.write_file(path, &existing, read_sec, write_sec)
    }

    /// Deletes a file on the FAT volume, freeing its clusters.
    pub fn delete_file<R, W>(
        &mut self,
        path: &str,
        read_sec: &R,
        write_sec: &W,
    ) -> Result<(), &'static str>
    where
        R: Fn(u64, &mut [u8; 512]) -> Result<(), &'static str>,
        W: Fn(u64, &[u8; 512]) -> Result<(), &'static str>,
    {
        let trimmed = path.trim_matches('/');
        let (dir_part, filename) = match trimmed.rfind('/') {
            Some(idx) => (&trimmed[..idx], &trimmed[idx + 1..]),
            None => ("", trimmed),
        };

        let dir_cluster = self.resolve_dir_cluster(dir_part, read_sec)?;
        let dir_sectors = self.get_dir_sectors(dir_cluster, read_sec)?;
        let dos_name = Self::to_dos_8_3(filename);
        let mut dir_sec = [0u8; 512];

        for &lba in &dir_sectors {
            read_sec(lba, &mut dir_sec)?;
            for chunk in dir_sec.chunks_exact_mut(32) {
                if chunk[0] == 0x00 {
                    break;
                }
                if chunk[0] != 0xE5 && chunk[0..11] == dos_name {
                    // Match found!
                    let clus_hi = u16::from_le_bytes([chunk[20], chunk[21]]) as u32;
                    let clus_lo = u16::from_le_bytes([chunk[26], chunk[27]]) as u32;
                    let first_clus = (clus_hi << 16) | clus_lo;

                    chunk[0] = 0xE5;
                    write_sec(lba, &dir_sec)?;

                    if first_clus >= 2 {
                        self.free_cluster_chain(first_clus, write_sec)?;
                    }
                    return Ok(());
                }
            }
        }
        Err("file to delete not found")
    }

    /// Creates a new directory at `path` on the FAT volume.
    pub fn mkdir<R, W>(
        &mut self,
        path: &str,
        read_sec: &R,
        write_sec: &W,
    ) -> Result<(), &'static str>
    where
        R: Fn(u64, &mut [u8; 512]) -> Result<(), &'static str>,
        W: Fn(u64, &[u8; 512]) -> Result<(), &'static str>,
    {
        let trimmed = path.trim_matches('/');
        let (dir_part, dirname) = match trimmed.rfind('/') {
            Some(idx) => (&trimmed[..idx], &trimmed[idx + 1..]),
            None => ("", trimmed),
        };

        if dirname.is_empty() {
            return Err("invalid directory name");
        }

        let parent_cluster = self.resolve_dir_cluster(dir_part, read_sec)?;
        let parent_sectors = self.get_dir_sectors(parent_cluster, read_sec)?;
        let dos_name = Self::to_dos_8_3(dirname);

        // 1. Allocate a cluster for the new directory
        let new_dir_cluster = self.alloc_cluster(write_sec)?;

        // 2. Initialize new directory cluster with '.' and '..' entries
        let mut first_sec = [0u8; 512];

        // '.' entry (points to new_dir_cluster)
        first_sec[0..11].copy_from_slice(b".          ");
        first_sec[11] = ATTR_DIRECTORY;
        first_sec[20..22].copy_from_slice(&((new_dir_cluster >> 16) as u16).to_le_bytes());
        first_sec[26..28].copy_from_slice(&((new_dir_cluster & 0xFFFF) as u16).to_le_bytes());

        // '..' entry (points to parent_cluster; 0 if root)
        let parent_ptr = if parent_cluster == self.bpb.root_cluster || parent_cluster == 0 {
            0
        } else {
            parent_cluster
        };
        first_sec[32..43].copy_from_slice(b"..         ");
        first_sec[43] = ATTR_DIRECTORY;
        first_sec[52..54].copy_from_slice(&((parent_ptr >> 16) as u16).to_le_bytes());
        first_sec[58..60].copy_from_slice(&((parent_ptr & 0xFFFF) as u16).to_le_bytes());

        let new_lba = self.data_start_lba + ((new_dir_cluster - 2) as u64 * self.bpb.sectors_per_cluster as u64);
        write_sec(new_lba, &first_sec)?;

        // 3. Add directory entry to parent directory
        let mut p_sec = [0u8; 512];
        let mut target_sec_lba = 0u64;
        let mut target_offset = None;

        for &lba in &parent_sectors {
            read_sec(lba, &mut p_sec)?;
            for (idx, chunk) in p_sec.chunks_exact_mut(32).enumerate() {
                if chunk[0] == 0x00 || chunk[0] == 0xE5 {
                    target_sec_lba = lba;
                    target_offset = Some(idx * 32);
                    break;
                }
            }
            if target_offset.is_some() {
                break;
            }
        }

        let offset = target_offset.ok_or("parent directory full")?;
        read_sec(target_sec_lba, &mut p_sec)?;
        let entry = &mut p_sec[offset..offset + 32];
        entry[0..11].copy_from_slice(&dos_name);
        entry[11] = ATTR_DIRECTORY;
        entry[12] = 0;
        entry[13] = 0;
        entry[14..16].copy_from_slice(&0u16.to_le_bytes());
        entry[16..18].copy_from_slice(&0x5421u16.to_le_bytes());
        entry[18..20].copy_from_slice(&0x5421u16.to_le_bytes());
        entry[20..22].copy_from_slice(&((new_dir_cluster >> 16) as u16).to_le_bytes());
        entry[22..24].copy_from_slice(&0u16.to_le_bytes());
        entry[24..26].copy_from_slice(&0x5421u16.to_le_bytes());
        entry[26..28].copy_from_slice(&((new_dir_cluster & 0xFFFF) as u16).to_le_bytes());
        entry[28..32].copy_from_slice(&0u32.to_le_bytes());

        write_sec(target_sec_lba, &p_sec)?;
        Ok(())
    }

    /// Renames a file or directory on the FAT volume.
    pub fn rename<R, W>(
        &mut self,
        old_path: &str,
        new_path: &str,
        read_sec: &R,
        write_sec: &W,
    ) -> Result<(), &'static str>
    where
        R: Fn(u64, &mut [u8; 512]) -> Result<(), &'static str>,
        W: Fn(u64, &[u8; 512]) -> Result<(), &'static str>,
    {
        let old_trimmed = old_path.trim_matches('/');
        let (old_dir, old_file) = match old_trimmed.rfind('/') {
            Some(idx) => (&old_trimmed[..idx], &old_trimmed[idx + 1..]),
            None => ("", old_trimmed),
        };

        let new_trimmed = new_path.trim_matches('/');
        let (new_dir, new_file) = match new_trimmed.rfind('/') {
            Some(idx) => (&new_trimmed[..idx], &new_trimmed[idx + 1..]),
            None => ("", new_trimmed),
        };

        let old_dir_cluster = self.resolve_dir_cluster(old_dir, read_sec)?;
        let old_sectors = self.get_dir_sectors(old_dir_cluster, read_sec)?;
        let old_dos = Self::to_dos_8_3(old_file);

        let new_dir_cluster = self.resolve_dir_cluster(new_dir, read_sec)?;
        let new_sectors = self.get_dir_sectors(new_dir_cluster, read_sec)?;
        let new_dos = Self::to_dos_8_3(new_file);

        // 1. Locate old directory entry and preserve cluster / size / attributes
        let mut sec_buf = [0u8; 512];
        let mut preserved_entry = [0u8; 32];
        let mut old_lba = 0u64;

        'find_old: for &lba in &old_sectors {
            read_sec(lba, &mut sec_buf)?;
            for chunk in sec_buf.chunks_exact_mut(32) {
                if chunk[0] == 0x00 {
                    break;
                }
                if chunk[0] != 0xE5 && chunk[0..11] == old_dos {
                    preserved_entry.copy_from_slice(chunk);
                    chunk[0] = 0xE5; // Mark old as deleted
                    old_lba = lba;
                    break 'find_old;
                }
            }
        }

        if old_lba == 0 {
            return Err("source file to rename not found");
        }

        // 2. Find empty slot in target directory
        let mut target_lba = 0u64;
        let mut target_offset = None;

        for &lba in &new_sectors {
            read_sec(lba, &mut sec_buf)?;
            for (idx, chunk) in sec_buf.chunks_exact(32).enumerate() {
                if chunk[0] == 0x00 || chunk[0] == 0xE5 {
                    target_lba = lba;
                    target_offset = Some(idx * 32);
                    break;
                }
            }
            if target_offset.is_some() {
                break;
            }
        }

        let offset = target_offset.ok_or("destination directory full")?;

        // 3. Write old directory sector with deleted mark
        write_sec(old_lba, &sec_buf)?;

        // 4. Write new directory entry
        read_sec(target_lba, &mut sec_buf)?;
        preserved_entry[0..11].copy_from_slice(&new_dos);
        sec_buf[offset..offset + 32].copy_from_slice(&preserved_entry);
        write_sec(target_lba, &sec_buf)?;

        Ok(())
    }

    /// Performs a consistency check of the FAT filesystem (fsck-lite).
    pub fn fsck_lite<R>(&self, read_sec: &R) -> Result<FsckReport, &'static str>
    where
        R: Fn(u64, &mut [u8; 512]) -> Result<(), &'static str>,
    {
        let mut errors = Vec::new();

        // 1. Check BPB basic invariants
        if self.bpb.bytes_per_sector != 512 {
            errors.push(format!("Corrupt BPB: bytes_per_sector = {}", self.bpb.bytes_per_sector));
        }
        if self.bpb.sectors_per_cluster == 0 {
            errors.push(String::from("Corrupt BPB: sectors_per_cluster is zero"));
        }
        if self.bpb.num_fats == 0 {
            errors.push(String::from("Corrupt BPB: num_fats is zero"));
        }

        // 2. Scan FAT table for cluster statistics
        let data_sectors = self.bpb.total_sectors.saturating_sub(
            (self.data_start_lba - self.partition_lba) as u32,
        );
        let total_clusters = data_sectors / self.bpb.sectors_per_cluster.max(1) as u32;

        let mut free_clusters = 0u32;
        let mut used_clusters = 0u32;

        for c in 2..total_clusters {
            match self.next_cluster(c) {
                None => {
                    let is_zero = match self.bpb.fat_type {
                        FatType::Fat12 => {
                            let offset = (c + (c / 2)) as usize;
                            if offset + 1 < self.fat_cache.len() {
                                let val = u16::from_le_bytes([self.fat_cache[offset], self.fat_cache[offset + 1]]);
                                let entry = if (c & 1) != 0 { val >> 4 } else { val & 0x0FFF };
                                entry == 0
                            } else {
                                false
                            }
                        }
                        FatType::Fat16 => {
                            let offset = (c as usize) * 2;
                            if offset + 1 < self.fat_cache.len() {
                                let val = u16::from_le_bytes([self.fat_cache[offset], self.fat_cache[offset + 1]]);
                                val == 0
                            } else {
                                false
                            }
                        }
                        FatType::Fat32 => {
                            let offset = (c as usize) * 4;
                            if offset + 3 < self.fat_cache.len() {
                                let val = u32::from_le_bytes(self.fat_cache[offset..offset + 4].try_into().unwrap()) & 0x0FFF_FFFF;
                                val == 0
                            } else {
                                false
                            }
                        }
                    };
                    if is_zero {
                        free_clusters += 1;
                    } else {
                        used_clusters += 1;
                    }
                }
                Some(_) => {
                    used_clusters += 1;
                }
            }
        }

        // 3. Traverse directory tree to check link integrity
        let mut file_count = 0usize;
        let mut dir_count = 1usize; // root dir
        let root_entries = self.list_dir("/", read_sec).unwrap_or_default();

        for entry in root_entries {
            if entry.is_dir {
                dir_count += 1;
                if let Ok(sub) = self.list_dir(&entry.name, read_sec) {
                    file_count += sub.iter().filter(|e| !e.is_dir).count();
                }
            } else {
                file_count += 1;
                // Verify cluster bounds
                if entry.first_cluster >= 2 && entry.first_cluster > total_clusters + 2 {
                    errors.push(format!("File '{}' has invalid cluster pointer {}", entry.name, entry.first_cluster));
                }
            }
        }

        let is_clean = errors.is_empty();

        Ok(FsckReport {
            volume_label: self.bpb.volume_label.clone(),
            fat_type: self.bpb.fat_type,
            total_sectors: self.bpb.total_sectors,
            sectors_per_cluster: self.bpb.sectors_per_cluster,
            total_clusters,
            used_clusters,
            free_clusters,
            file_count,
            dir_count,
            errors,
            is_clean,
        })
    }
}
