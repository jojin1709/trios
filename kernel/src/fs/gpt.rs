//! GUID Partition Table (GPT) parser for TriOS.
//!
//! Standard UEFI GPT partition scheme:
//! - LBA 0: Protective MBR
//! - LBA 1: Primary GPT Header (Signature `EFI PART`)
//! - LBA 2..33: GPT Partition Entries array (128 entries of 128 bytes each)

use alloc::{string::String, vec::Vec};

pub const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";

/// GPT Header (92 bytes, standard LBA 1).
#[derive(Debug, Clone)]
pub struct GptHeader {
    pub revision: u32,
    pub header_size: u32,
    pub current_lba: u64,
    pub backup_lba: u64,
    pub first_usable_lba: u64,
    pub last_usable_lba: u64,
    pub disk_guid: [u8; 16],
    pub partition_entry_lba: u64,
    pub num_partition_entries: u32,
    pub partition_entry_size: u32,
}

/// One GPT Partition Entry (128 bytes).
#[derive(Debug, Clone)]
pub struct GptPartitionEntry {
    pub type_guid: [u8; 16],
    pub unique_guid: [u8; 16],
    pub starting_lba: u64,
    pub ending_lba: u64,
    pub attributes: u64,
    pub name: String,
}

/// Parses the GPT Header from sector 1 (512 bytes).
pub fn parse_header(sector: &[u8; 512]) -> Result<GptHeader, &'static str> {
    if &sector[0..8] != GPT_SIGNATURE {
        return Err("invalid GPT signature");
    }

    let revision = u32::from_le_bytes(sector[8..12].try_into().unwrap());
    let header_size = u32::from_le_bytes(sector[12..16].try_into().unwrap());
    let current_lba = u64::from_le_bytes(sector[24..32].try_into().unwrap());
    let backup_lba = u64::from_le_bytes(sector[32..40].try_into().unwrap());
    let first_usable_lba = u64::from_le_bytes(sector[40..48].try_into().unwrap());
    let last_usable_lba = u64::from_le_bytes(sector[48..56].try_into().unwrap());

    let mut disk_guid = [0u8; 16];
    disk_guid.copy_from_slice(&sector[56..72]);

    let partition_entry_lba = u64::from_le_bytes(sector[72..80].try_into().unwrap());
    let num_partition_entries = u32::from_le_bytes(sector[80..84].try_into().unwrap());
    let partition_entry_size = u32::from_le_bytes(sector[84..88].try_into().unwrap());

    Ok(GptHeader {
        revision,
        header_size,
        current_lba,
        backup_lba,
        first_usable_lba,
        last_usable_lba,
        disk_guid,
        partition_entry_lba,
        num_partition_entries,
        partition_entry_size,
    })
}

/// Parses a 128-byte partition entry. Returns None if the entry is unused (all zero Type GUID).
pub fn parse_partition_entry(bytes: &[u8]) -> Option<GptPartitionEntry> {
    if bytes.len() < 128 {
        return None;
    }

    // Check if Type GUID is completely zero (unused entry)
    if bytes[0..16].iter().all(|&b| b == 0) {
        return None;
    }

    let mut type_guid = [0u8; 16];
    type_guid.copy_from_slice(&bytes[0..16]);

    let mut unique_guid = [0u8; 16];
    unique_guid.copy_from_slice(&bytes[16..32]);

    let starting_lba = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
    let ending_lba = u64::from_le_bytes(bytes[40..48].try_into().unwrap());
    let attributes = u64::from_le_bytes(bytes[48..56].try_into().unwrap());

    // Name is 36 UTF-16LE characters (72 bytes: 56..128)
    let mut name = String::new();
    for chunk in bytes[56..128].chunks_exact(2) {
        let code = u16::from_le_bytes([chunk[0], chunk[1]]);
        if code == 0 {
            break;
        }
        if let Ok(c) = char::try_from(code as u32) {
            name.push(c);
        }
    }

    Some(GptPartitionEntry {
        type_guid,
        unique_guid,
        starting_lba,
        ending_lba,
        attributes,
        name,
    })
}

/// Scans the GPT partition table on disk using the provided sector reader function.
pub fn scan_partitions<F>(read_sec: F) -> Result<(GptHeader, Vec<GptPartitionEntry>), &'static str>
where
    F: Fn(u64, &mut [u8; 512]) -> Result<(), &'static str>,
{
    let mut sector = [0u8; 512];
    // Read LBA 1: GPT Header
    read_sec(1, &mut sector)?;
    let header = parse_header(&sector)?;

    let mut partitions = Vec::new();
    let entries_per_sector = 512 / header.partition_entry_size.max(128) as u64;
    let sectors_to_read = (header.num_partition_entries as u64 + entries_per_sector - 1) / entries_per_sector;

    for s in 0..sectors_to_read.min(32) {
        let lba = header.partition_entry_lba + s;
        if read_sec(lba, &mut sector).is_err() {
            break;
        }

        for i in 0..entries_per_sector {
            let offset = (i as usize) * header.partition_entry_size as usize;
            if offset + 128 <= sector.len() {
                if let Some(entry) = parse_partition_entry(&sector[offset..offset + 128]) {
                    partitions.push(entry);
                }
            }
        }
    }

    Ok((header, partitions))
}
