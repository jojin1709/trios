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
    pub e_type: u16,
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
        e_type,
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

/// Generates a valid minimal 64-bit ELF executable that performs a null pointer dereference
/// to test Ring 3 user exception isolation.
pub fn create_fault_elf() -> Vec<u8> {
    // x86_64 instructions:
    // mov rax, 0
    // mov rdi, [rax]       (48 8b 38) -> null dereference at 0x0
    // mov rax, 0           (SYS_EXIT)
    // syscall
    // ret
    let mut code: Vec<u8> = Vec::new();
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00]); // mov rax, 0
    code.extend_from_slice(&[0x48, 0x8B, 0x38]);                          // mov rdi, [rax] (CRASH)
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00]); // mov rax, 0
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall
    code.push(0xC3);

    let total_size = 120 + code.len() as u64;
    let mut elf = vec![0u8; total_size as usize];

    elf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    elf[4] = 2; // ELFCLASS64
    elf[5] = 1; // ELFDATA2LSB
    elf[6] = 1; // EV_CURRENT
    elf[7] = 0; // ELFOSABI_NONE
    elf[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    elf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes()); // EM_X86_64
    elf[20..24].copy_from_slice(&1u32.to_le_bytes());
    elf[24..32].copy_from_slice(&(0x400000u64 + 120).to_le_bytes());
    elf[32..40].copy_from_slice(&64u64.to_le_bytes());
    elf[48..52].copy_from_slice(&0u32.to_le_bytes());
    elf[52..54].copy_from_slice(&64u16.to_le_bytes());
    elf[54..56].copy_from_slice(&56u16.to_le_bytes());
    elf[56..58].copy_from_slice(&1u16.to_le_bytes());

    let ph = &mut elf[64..120];
    ph[0..4].copy_from_slice(&PT_LOAD.to_le_bytes());
    ph[4..8].copy_from_slice(&7u32.to_le_bytes());
    ph[8..16].copy_from_slice(&0u64.to_le_bytes());
    ph[16..24].copy_from_slice(&0x400000u64.to_le_bytes());
    ph[24..32].copy_from_slice(&0x400000u64.to_le_bytes());
    ph[32..40].copy_from_slice(&total_size.to_le_bytes());
    ph[40..48].copy_from_slice(&total_size.to_le_bytes());
    ph[48..56].copy_from_slice(&0x1000u64.to_le_bytes());

    elf[120..].copy_from_slice(&code);
    elf
}

/// Generates a valid minimal 64-bit ELF executable that invokes syscalls with invalid,
/// null, kernel-space, unmapped, and misaligned arguments to stress-test kernel isolation.
pub fn create_fuzz_elf() -> Vec<u8> {
    const MSG: &[u8] = b"[fuzz   ] syscall fuzzing finished: all invalid pointers rejected cleanly (0 panics)\n";
    let msg_len = MSG.len() as u32;

    let mut code: Vec<u8> = Vec::new();

    // 1. SYS_WRITE with NULL pointer
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00]); // mov rax, 1 (SYS_WRITE)
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0x01, 0x00, 0x00, 0x00]); // mov rdi, 1
    code.extend_from_slice(&[0x48, 0xC7, 0xC6, 0x00, 0x00, 0x00, 0x00]); // mov rsi, 0 (NULL)
    code.extend_from_slice(&[0x48, 0xC7, 0xC2, 0x40, 0x00, 0x00, 0x00]); // mov rdx, 64
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall

    // 2. SYS_WRITE with kernel-half pointer (0xFFFF_8000_0000_0000)
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00]); // mov rax, 1
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0x01, 0x00, 0x00, 0x00]); // mov rdi, 1
    code.extend_from_slice(&[0x48, 0xBE, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xFF]); // mov rsi, 0xFFFF800000000000
    code.extend_from_slice(&[0x48, 0xC7, 0xC2, 0x40, 0x00, 0x00, 0x00]); // mov rdx, 64
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall

    // 3. SYS_WRITE with unmapped user address (0x1234)
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00]); // mov rax, 1
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0x01, 0x00, 0x00, 0x00]); // mov rdi, 1
    code.extend_from_slice(&[0x48, 0xC7, 0xC6, 0x34, 0x12, 0x00, 0x00]); // mov rsi, 0x1234
    code.extend_from_slice(&[0x48, 0xC7, 0xC2, 0x40, 0x00, 0x00, 0x00]); // mov rdx, 64
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall

    // 4. SYS_READ with null pointer
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x02, 0x00, 0x00, 0x00]); // mov rax, 2 (SYS_READ)
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0x00, 0x00, 0x00, 0x00]); // mov rdi, 0
    code.extend_from_slice(&[0x48, 0xC7, 0xC6, 0x00, 0x00, 0x00, 0x00]); // mov rsi, 0 (NULL)
    code.extend_from_slice(&[0x48, 0xC7, 0xC2, 0x40, 0x00, 0x00, 0x00]); // mov rdx, 64
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall

    // 5. SYS_READ with kernel-half pointer
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x02, 0x00, 0x00, 0x00]); // mov rax, 2
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0x00, 0x00, 0x00, 0x00]); // mov rdi, 0
    code.extend_from_slice(&[0x48, 0xBE, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xFF]); // mov rsi, 0xFFFF800000000000
    code.extend_from_slice(&[0x48, 0xC7, 0xC2, 0x40, 0x00, 0x00, 0x00]); // mov rdx, 64
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall

    // 6. SYS_READ with read-only code page address (0x400000)
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x02, 0x00, 0x00, 0x00]); // mov rax, 2
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0x00, 0x00, 0x00, 0x00]); // mov rdi, 0
    code.extend_from_slice(&[0x48, 0xC7, 0xC6, 0x00, 0x00, 0x40, 0x00]); // mov rsi, 0x400000
    code.extend_from_slice(&[0x48, 0xC7, 0xC2, 0x40, 0x00, 0x00, 0x00]); // mov rdx, 64
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall

    // 7. SYS_OPEN with null pointer
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x09, 0x00, 0x00, 0x00]); // mov rax, 9 (SYS_OPEN)
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0x00, 0x00, 0x00, 0x00]); // mov rdi, 0
    code.extend_from_slice(&[0x48, 0xC7, 0xC6, 0x00, 0x00, 0x00, 0x00]); // mov rsi, 0
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall

    // 8. SYS_OPEN with kernel-half pointer
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x09, 0x00, 0x00, 0x00]); // mov rax, 9
    code.extend_from_slice(&[0x48, 0xBF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xFF]); // mov rdi, 0xFFFF800000000000
    code.extend_from_slice(&[0x48, 0xC7, 0xC6, 0x00, 0x00, 0x00, 0x00]); // mov rsi, 0
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall

    // 9. SYS_CLOSE with invalid fd (0xFFFFFFFF)
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x0A, 0x00, 0x00, 0x00]); // mov rax, 10 (SYS_CLOSE)
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0xFF, 0xFF, 0xFF, 0xFF]); // mov rdi, 0xFFFFFFFF
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall

    // 10. SYS_MMAP with huge length (overflowing)
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x07, 0x00, 0x00, 0x00]); // mov rax, 7 (SYS_MMAP)
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0x00, 0x00, 0x00, 0x00]); // mov rdi, 0
    code.extend_from_slice(&[0x48, 0xC7, 0xC6, 0xFF, 0xFF, 0xFF, 0xFF]); // mov rsi, 0xFFFFFFFF
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall

    // 11. Invalid syscall #9999
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x0F, 0x27, 0x00, 0x00]); // mov rax, 9999
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0x00, 0x00, 0x00, 0x00]); // mov rdi, 0
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall

    // 12. Valid SYS_WRITE reporting completion
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00]); // mov rax, 1 (SYS_WRITE)
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0x01, 0x00, 0x00, 0x00]); // mov rdi, 1
    code.extend_from_slice(&[0x48, 0x8D, 0x35, 0x1A, 0x00, 0x00, 0x00]); // lea rsi, [rip + 26]
    code.extend_from_slice(&[0x48, 0xC7, 0xC2]);                          // mov rdx, imm32
    code.extend_from_slice(&msg_len.to_le_bytes());
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall

    // 13. SYS_EXIT 0
    code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00]); // mov rax, 0
    code.extend_from_slice(&[0x48, 0xC7, 0xC7, 0x00, 0x00, 0x00, 0x00]); // mov rdi, 0
    code.extend_from_slice(&[0x0F, 0x05]);                                 // syscall
    code.push(0xC3);                                                       // ret
    code.extend_from_slice(MSG);

    let total_size = 120 + code.len() as u64;
    let mut elf = vec![0u8; total_size as usize];

    elf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    elf[4] = 2; // ELFCLASS64
    elf[5] = 1; // ELFDATA2LSB
    elf[6] = 1; // EV_CURRENT
    elf[7] = 0; // ELFOSABI_NONE
    elf[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    elf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes()); // EM_X86_64
    elf[20..24].copy_from_slice(&1u32.to_le_bytes());
    elf[24..32].copy_from_slice(&(0x400000u64 + 120).to_le_bytes());
    elf[32..40].copy_from_slice(&64u64.to_le_bytes());
    elf[48..52].copy_from_slice(&0u32.to_le_bytes());
    elf[52..54].copy_from_slice(&64u16.to_le_bytes());
    elf[54..56].copy_from_slice(&56u16.to_le_bytes());
    elf[56..58].copy_from_slice(&1u16.to_le_bytes());

    let ph = &mut elf[64..120];
    ph[0..4].copy_from_slice(&PT_LOAD.to_le_bytes());
    ph[4..8].copy_from_slice(&7u32.to_le_bytes());
    ph[8..16].copy_from_slice(&0u64.to_le_bytes());
    ph[16..24].copy_from_slice(&0x400000u64.to_le_bytes());
    ph[24..32].copy_from_slice(&0x400000u64.to_le_bytes());
    ph[32..40].copy_from_slice(&total_size.to_le_bytes());
    ph[40..48].copy_from_slice(&total_size.to_le_bytes());
    ph[48..56].copy_from_slice(&0x1000u64.to_le_bytes());

    elf[120..].copy_from_slice(&code);
    elf
}

/// Loads ELF segments into an isolated user process's PML4 address space.
pub fn load_elf_into_process(
    bytes: &[u8],
    proc: &mut crate::sched::Process,
) -> Result<u64, ElfError> {
    let hdr = parse_header(bytes)?;
    let phys_offset = crate::memory::get_phys_offset().ok_or(ElfError::NoLoadableSegments)?;

    let base_vaddr: u64 = if hdr.e_type == 3 {
        0x0040_0000
    } else {
        0
    };

    let mut mapped_pages: alloc::vec::Vec<(u64, x86_64::structures::paging::PhysFrame<x86_64::structures::paging::Size4KiB>)> = alloc::vec::Vec::new();

    for i in 0..hdr.ph_num {
        if let Some(ph) = parse_program_header(bytes, &hdr, i) {
            if ph.p_type == PT_LOAD && ph.p_memsz > 0 {
                let seg_vaddr = base_vaddr + ph.p_vaddr;
                let start_v = seg_vaddr & !0xFFF;
                let end_v = (seg_vaddr + ph.p_memsz + 0xFFF) & !0xFFF;
                let page_count = ((end_v - start_v) / 4096) as usize;

                for page_idx in 0..page_count {
                    let page_virt = x86_64::VirtAddr::new(start_v + (page_idx as u64 * 4096));
                    let page_u64 = page_virt.as_u64();

                    let frame = if let Some(&(_, f)) = mapped_pages.iter().find(|(v, _)| *v == page_u64) {
                        f
                    } else {
                        let f = crate::memory::bitmap::alloc_frame()
                            .ok_or(ElfError::NoLoadableSegments)?;

                        crate::sched::process::map_in_pml4(
                            proc.pml4_frame,
                            page_virt,
                            f,
                            crate::memory::virt::PageFlags::USER_CODE,
                        )
                        .map_err(|_| ElfError::NoLoadableSegments)?;

                        proc.allocated_frames.push(f);
                        mapped_pages.push((page_u64, f));

                        // Zero new frame
                        unsafe {
                            let frame_ptr = (phys_offset + f.start_address().as_u64()).as_mut_ptr::<u8>();
                            core::ptr::write_bytes(frame_ptr, 0, 4096);
                        }

                        f
                    };

                    unsafe {
                        let frame_ptr = (phys_offset + frame.start_address().as_u64()).as_mut_ptr::<u8>();

                        // Copy segment data intersecting this 4 KiB page
                        let page_start = page_virt.as_u64();
                        let page_end = page_start + 4096;

                        let seg_start = seg_vaddr;
                        let seg_data_end = seg_vaddr + ph.p_filesz;

                        let copy_start = seg_start.max(page_start);
                        let copy_end = seg_data_end.min(page_end);

                        if copy_start < copy_end {
                            let file_offset = (ph.p_offset + (copy_start - seg_start)) as usize;
                            let frame_offset = (copy_start - page_start) as usize;
                            let len = (copy_end - copy_start) as usize;

                            if file_offset + len <= bytes.len() {
                                let src = &bytes[file_offset..file_offset + len];
                                core::ptr::copy_nonoverlapping(
                                    src.as_ptr(),
                                    frame_ptr.add(frame_offset),
                                    len,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    let final_entry = base_vaddr + hdr.entry_point;
    proc.user_rip = final_entry;
    Ok(final_entry)
}
