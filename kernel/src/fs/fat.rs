//! FAT12 / FAT16 / FAT32 filesystem driver for TriOS.
//!
//! Conforms to Microsoft FAT specification:
//! - Auto-detects FAT12, FAT16, and FAT32 based on volume cluster count.
//! - Parses BPB (BIOS Parameter Block) and File Allocation Tables.
//! - Supports directory traversal, subdirectory resolution, and file reads.

use alloc::{string::String, vec, vec::Vec};

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

        let root_dir_sectors =
            ((root_entries as u32 * 32) + (bytes_per_sector as u32 - 1)) / (bytes_per_sector as u32);
        let data_sectors = total_sectors.saturating_sub(
            reserved_sectors as u32 + (num_fats as u32 * sectors_per_fat) + root_dir_sectors,
        );
        let cluster_count = data_sectors / sectors_per_cluster as u32;

        let fat_type = if cluster_count < 4085 {
            FatType::Fat12
        } else if cluster_count < 65525 {
            FatType::Fat16
        } else {
            FatType::Fat32
        };

        // Volume label
        let label_offset = if fat_type == FatType::Fat32 { 71 } else { 43 };
        let mut volume_label = String::new();
        for &b in &sector[label_offset..label_offset + 11] {
            if b > 0x20 && b < 0x7F {
                volume_label.push(b as char);
            }
        }

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

        let fat_start_lba = partition_lba + reserved_sectors as u64;
        let root_dir_lba = fat_start_lba + (num_fats as u64 * sectors_per_fat as u64);
        let data_start_lba = root_dir_lba + root_dir_sectors as u64;

        // Cache first FAT table into memory (up to 128 sectors = 64 KiB)
        let cache_sectors = sectors_per_fat.min(128) as usize;
        let mut fat_cache = vec![0u8; cache_sectors * 512];
        for s in 0..cache_sectors {
            let mut sec = [0u8; 512];
            let _ = read_sec(fat_start_lba + s as u64, &mut sec);
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
        let _bytes_per_cluster = self.bpb.sectors_per_cluster as usize * 512;

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
}
