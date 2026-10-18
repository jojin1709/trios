//! Minimal standalone ELF64 binary parser and loader for TriOS.
//!
//! Validates standard x86_64 ELF executables:
//!   - Magic bytes `\x7FELF`
//!   - 64-bit class (ELFCLASS64)
//!   - Little-endian data encoding (ELFDATA2LSB)
//!   - Machine type x86_64 (`0x3E`)
//!   - Executable type (ET_EXEC = 2 or ET_DYN = 3)
//! Reads Program Headers (PT_LOAD segments), loads segment images,
//! and resolves entry point addresses for execution.

use alloc::{vec, vec::Vec};

#[derive(Debug, Clone, Copy)]
pub struct ElfHeader64 {
    pub entry_point: u64,
    pub ph_offset: u64,
    pub ph_entry_size: u16,
    pub ph_num: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct ProgramHeader64 {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

pub const PT_LOAD: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooSmall,
    InvalidMagic,
    Not64Bit,
    NotLittleEndian,
    NotX86_64,
    UnsupportedType,
    NoLoadableSegments,
}

/// Represents an in-memory loaded ELF program image with resolved entry point.
pub struct LoadedProgram {
    pub entry_point: usize,
    pub image: Vec<u8>,
}

/// Parses the 64-bit ELF header.
pub fn parse_header(bytes: &[u8]) -> Result<ElfHeader64, ElfError> {
    if bytes.len() < 64 {
        return Err(ElfError::TooSmall);
    }

    // Check ELF magic: 0x7F, 'E', 'L', 'F'
    if bytes[0..4] != [0x7F, b'E', b'L', b'F'] {
        return Err(ElfError::InvalidMagic);
    }

    // Check 64-bit (EI_CLASS = 2)
    if bytes[4] != 2 {
        return Err(ElfError::Not64Bit);
    }

    // Check Little-Endian (EI_DATA = 1)
    if bytes[5] != 1 {
        return Err(ElfError::NotLittleEndian);
    }

    // e_type (offset 16..18): ET_EXEC (2) or ET_DYN (3)
    let e_type = u16::from_le_bytes([bytes[16], bytes[17]]);
    if e_type != 2 && e_type != 3 {
        return Err(ElfError::UnsupportedType);
    }

    // e_machine (offset 18..20): x86_64 = 0x3E
    let e_machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    if e_machine != 0x3E {
        return Err(ElfError::NotX86_64);
    }

    let entry_point = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    let ph_offset = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
    let ph_entry_size = u16::from_le_bytes([bytes[54], bytes[55]]);
    let ph_num = u16::from_le_bytes([bytes[56], bytes[57]]);

    Ok(ElfHeader64 {
        entry_point,
        ph_offset,
        ph_entry_size,
        ph_num,
    })
}

/// Parses a program header at the given index.
pub fn parse_program_header(
    bytes: &[u8],
    header: &ElfHeader64,
    index: u16,
) -> Option<ProgramHeader64> {
    if index >= header.ph_num {
        return None;
    }

    let offset = header.ph_offset as usize + (index as usize * header.ph_entry_size as usize);
    if offset + 56 > bytes.len() {
        return None;
    }

    let chunk = &bytes[offset..offset + 56];
    let p_type = u32::from_le_bytes(chunk[0..4].try_into().unwrap());
    let p_flags = u32::from_le_bytes(chunk[4..8].try_into().unwrap());
    let p_offset = u64::from_le_bytes(chunk[8..16].try_into().unwrap());
    let p_vaddr = u64::from_le_bytes(chunk[16..24].try_into().unwrap());
    let p_paddr = u64::from_le_bytes(chunk[24..32].try_into().unwrap());
    let p_filesz = u64::from_le_bytes(chunk[32..40].try_into().unwrap());
    let p_memsz = u64::from_le_bytes(chunk[40..48].try_into().unwrap());
    let p_align = u64::from_le_bytes(chunk[48..56].try_into().unwrap());

    Some(ProgramHeader64 {
        p_type,
        p_flags,
        p_offset,
        p_vaddr,
        p_paddr,
        p_filesz,
        p_memsz,
        p_align,
    })
}

/// Loads PT_LOAD segments of an ELF executable into a memory buffer and calculates entry point.
pub fn load_executable(bytes: &[u8]) -> Result<LoadedProgram, ElfError> {
    let hdr = parse_header(bytes)?;

    let mut min_vaddr = u64::MAX;
    let mut max_vaddr = 0u64;
    let mut load_count = 0usize;

    for i in 0..hdr.ph_num {
        if let Some(ph) = parse_program_header(bytes, &hdr, i) {
            if ph.p_type == PT_LOAD {
                load_count += 1;
                if ph.p_vaddr < min_vaddr {
                    min_vaddr = ph.p_vaddr;
                }
                let end = ph.p_vaddr + ph.p_memsz;
                if end > max_vaddr {
                    max_vaddr = end;
                }
            }
        }
    }

    if load_count == 0 || min_vaddr == u64::MAX || max_vaddr <= min_vaddr {
        return Err(ElfError::NoLoadableSegments);
    }

    let span = (max_vaddr - min_vaddr) as usize;
    let mut image = vec![0u8; span];

    for i in 0..hdr.ph_num {
        if let Some(ph) = parse_program_header(bytes, &hdr, i) {
            if ph.p_type == PT_LOAD {
                let seg_offset = (ph.p_vaddr - min_vaddr) as usize;
                let file_offset = ph.p_offset as usize;
                let filesz = ph.p_filesz as usize;
                if file_offset + filesz <= bytes.len() && seg_offset + filesz <= span {
                    image[seg_offset..seg_offset + filesz]
                        .copy_from_slice(&bytes[file_offset..file_offset + filesz]);
                }
            }
        }
    }

    let entry_offset = (hdr.entry_point.saturating_sub(min_vaddr)) as usize;
    let entry_point = image.as_ptr() as usize + entry_offset;

    // Clear NO_EXECUTE on the memory pages backing this executable image
    crate::memory::make_executable_range(
        x86_64::VirtAddr::new(image.as_ptr() as u64),
        image.len(),
    );

    Ok(LoadedProgram { entry_point, image })
}

/// Generates a valid minimal 64-bit ELF executable for testing.
///
/// Contains x86_64 position-independent instructions:
/// 1. `SYS_WRITE (1)` to stdout (`fd = 1`) with `"[elf_app] Hello from TriOS User ELF!\n"`
/// 2. `SYS_EXIT (0)` to terminate gracefully via the scheduler
pub fn create_sample_elf() -> Vec<u8> {
    const MSG: &[u8] = b"[elf_app] Hello from TriOS User ELF!\n";
    let msg_len = MSG.len() as u32;

    // x86_64 instructions:
    // mov rax, 1           (SYS_WRITE)
    // mov rdi, 1           (stdout)
    // lea rsi, [rip + 26]  (points to MSG directly following code)
    // mov rdx, msg_len     (length)
    // syscall
    // mov rax, 0           (SYS_EXIT)
    // mov rdi, 0           (exit code 0)
    // syscall
    // ret
    let mut code: Vec<u8> = Vec::new();
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00]); // mov rax, 1
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0x01, 0x00, 0x00, 0x00]); // mov rdi, 1
    code.extend_from_slice(&[0x48, 0x8D, 0x35, 0x1A, 0x00, 0x00, 0x00]); // lea rsi, [rip + 26]
    code.extend_from_slice(&[0x48, 0xC7, 0xC2]);                          // mov rdx, imm32
    code.extend_from_slice(&msg_len.to_le_bytes());
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00]); // mov rax, 0
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0x00, 0x00, 0x00, 0x00]); // mov rdi, 0
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall
    code.push(0xC3);                                                       // ret
    code.extend_from_slice(MSG);

    let total_size = 120 + code.len() as u64;
    let mut elf = vec![0u8; total_size as usize];

    // ELF Header (64 bytes)
    elf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    elf[4] = 2; // ELFCLASS64
    elf[5] = 1; // ELFDATA2LSB
    elf[6] = 1; // EV_CURRENT
    elf[7] = 0; // ELFOSABI_NONE
    elf[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    elf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes()); // EM_X86_64
    elf[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
    elf[24..32].copy_from_slice(&(0x400000u64 + 120).to_le_bytes()); // e_entry
    elf[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff = 64
    elf[48..52].copy_from_slice(&0u32.to_le_bytes()); // e_flags
    elf[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize = 64
    elf[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize = 56
    elf[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum = 1

    // Program Header (56 bytes at offset 64)
    let ph = &mut elf[64..120];
    ph[0..4].copy_from_slice(&PT_LOAD.to_le_bytes()); // p_type = PT_LOAD
    ph[4..8].copy_from_slice(&7u32.to_le_bytes()); // p_flags = RWX
    ph[8..16].copy_from_slice(&0u64.to_le_bytes()); // p_offset = 0
    ph[16..24].copy_from_slice(&0x400000u64.to_le_bytes()); // p_vaddr
    ph[24..32].copy_from_slice(&0x400000u64.to_le_bytes()); // p_paddr
    ph[32..40].copy_from_slice(&total_size.to_le_bytes()); // p_filesz
    ph[40..48].copy_from_slice(&total_size.to_le_bytes()); // p_memsz
    ph[48..56].copy_from_slice(&0x1000u64.to_le_bytes()); // p_align = 4096

    // Code and data payload at offset 120
    elf[120..].copy_from_slice(&code);

    elf
}
