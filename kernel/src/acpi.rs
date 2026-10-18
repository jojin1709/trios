//! ACPI (Advanced Configuration and Power Interface) Subsystem.
//!
//! Provides bare-metal discovery and parsing of ACPI tables (RSDP, RSDT/XSDT,
//! FADT, MADT) and hardware power management (ACPI S5 shutdown and hardware reboot).

use spin::Mutex;
use x86_64::instructions::port::Port;
use x86_64::VirtAddr;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct AcpiHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: [u8; 4],
    pub creator_revision: u32,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct AcpiInfo {
    pub rsdp_phys: u64,
    pub oem_id: [u8; 6],
    pub pm1a_cnt: u16,
    pub pm1b_cnt: u16,
    pub slp_typa: u16,
    pub slp_typb: u16,
    pub cpu_cores: usize,
    pub has_fadt: bool,
    pub has_madt: bool,
}

pub static ACPI: Mutex<Option<AcpiInfo>> = Mutex::new(None);

/// Initializes the ACPI subsystem from the RSDP physical address passed by UEFI.
pub fn init(rsdp_phys_opt: Option<u64>, phys_offset: VirtAddr) {
    serial_println!("[acpi   ] initializing ACPI subsystem...");

    let rsdp_phys = match rsdp_phys_opt {
        Some(addr) => addr,
        None => {
            // Fallback: search BIOS memory region 0xE0000..0x100000
            let mut found = None;
            let start = (phys_offset.as_u64() + 0xE0000) as *const u8;
            for i in (0..0x20000).step_by(16) {
                unsafe {
                    if core::slice::from_raw_parts(start.add(i), 8) == b"RSD PTR " {
                        found = Some(0xE0000 + i as u64);
                        break;
                    }
                }
            }
            match found {
                Some(p) => p,
                None => {
                    serial_println!("[acpi   ] RSDP not found in UEFI bootinfo or BIOS memory");
                    return;
                }
            }
        }
    };

    let rsdp_virt = phys_offset.as_u64() + rsdp_phys;
    let rsdp_ptr = rsdp_virt as *const u8;

    // Verify "RSD PTR " signature
    let sig = unsafe { core::slice::from_raw_parts(rsdp_ptr, 8) };
    if sig != b"RSD PTR " {
        serial_println!("[acpi   ] invalid RSDP signature: {:?}", sig);
        return;
    }

    let mut oem_id = [0u8; 6];
    unsafe {
        core::ptr::copy_nonoverlapping(rsdp_ptr.add(9), oem_id.as_mut_ptr(), 6);
    }
    let revision = unsafe { *rsdp_ptr.add(15) };
    let rsdt_phys = unsafe { core::ptr::read_unaligned(rsdp_ptr.add(16) as *const u32) } as u64;

    let xsdt_phys = if revision >= 2 {
        unsafe { core::ptr::read_unaligned(rsdp_ptr.add(24) as *const u64) }
    } else {
        0
    };

    serial_println!(
        "[acpi   ] found RSDP at {rsdp_phys:#x} (rev {}, OEM: '{}{}{}{}{}{}')",
        revision,
        oem_id[0] as char, oem_id[1] as char, oem_id[2] as char,
        oem_id[3] as char, oem_id[4] as char, oem_id[5] as char,
    );

    let mut pm1a_cnt = 0u16;
    let mut pm1b_cnt = 0u16;
    let mut slp_typa = 0u16;
    let mut slp_typb = 0u16;
    let mut cpu_cores = 1;
    let mut has_fadt = false;
    let mut has_madt = false;

    // Prefer XSDT (64-bit pointers) if available, otherwise RSDT (32-bit pointers)
    if xsdt_phys != 0 {
        let xsdt_virt = (phys_offset.as_u64() + xsdt_phys) as *const AcpiHeader;
        let hdr = unsafe { *xsdt_virt };
        if &hdr.signature == b"XSDT" {
            let entries = (hdr.length as usize - 36) / 8;
            let ptrs = (xsdt_virt as usize + 36) as *const u64;
            for i in 0..entries {
                let table_phys = unsafe { core::ptr::read_unaligned(ptrs.add(i)) };
                parse_table(
                    table_phys,
                    phys_offset.as_u64(),
                    &mut pm1a_cnt,
                    &mut pm1b_cnt,
                    &mut slp_typa,
                    &mut slp_typb,
                    &mut cpu_cores,
                    &mut has_fadt,
                    &mut has_madt,
                );
            }
        }
    } else if rsdt_phys != 0 {
        let rsdt_virt = (phys_offset.as_u64() + rsdt_phys) as *const AcpiHeader;
        let hdr = unsafe { *rsdt_virt };
        if &hdr.signature == b"RSDT" {
            let entries = (hdr.length as usize - 36) / 4;
            let ptrs = (rsdt_virt as usize + 36) as *const u32;
            for i in 0..entries {
                let table_phys = unsafe { core::ptr::read_unaligned(ptrs.add(i)) } as u64;
                parse_table(
                    table_phys,
                    phys_offset.as_u64(),
                    &mut pm1a_cnt,
                    &mut pm1b_cnt,
                    &mut slp_typa,
                    &mut slp_typb,
                    &mut cpu_cores,
                    &mut has_fadt,
                    &mut has_madt,
                );
            }
        }
    }

    // Default VirtualBox / QEMU PM1a control block port if FADT table didn't specify
    if pm1a_cnt == 0 {
        pm1a_cnt = 0x604; // QEMU/VirtualBox ACPI poweroff port
    }

    let info = AcpiInfo {
        rsdp_phys,
        oem_id,
        pm1a_cnt,
        pm1b_cnt,
        slp_typa,
        slp_typb,
        cpu_cores,
        has_fadt,
        has_madt,
    };

    serial_println!(
        "[acpi   ] PM1a_CNT={:#x} | cores={} | FADT: {} | MADT: {}",
        pm1a_cnt, cpu_cores, has_fadt, has_madt
    );

    *ACPI.lock() = Some(info);
}

fn parse_table(
    table_phys: u64,
    phys_offset: u64,
    pm1a_cnt: &mut u16,
    pm1b_cnt: &mut u16,
    slp_typa: &mut u16,
    slp_typb: &mut u16,
    cpu_cores: &mut usize,
    has_fadt: &mut bool,
    has_madt: &mut bool,
) {
    let virt = (phys_offset + table_phys) as *const AcpiHeader;
    let hdr = unsafe { *virt };
    let sig = &hdr.signature;

    if sig == b"FACP" {
        // FADT (Fixed ACPI Description Table)
        *has_fadt = true;
        let ptr = virt as *const u8;
        if hdr.length >= 72 {
            let p1a = unsafe { core::ptr::read_unaligned(ptr.add(64) as *const u32) };
            let p1b = unsafe { core::ptr::read_unaligned(ptr.add(68) as *const u32) };
            if p1a != 0 {
                *pm1a_cnt = p1a as u16;
            }
            if p1b != 0 {
                *pm1b_cnt = p1b as u16;
            }

            // Read DSDT pointer to find S5 AML object
            let dsdt_phys = unsafe { core::ptr::read_unaligned(ptr.add(44) as *const u32) } as u64;
            if dsdt_phys != 0 {
                parse_dsdt(dsdt_phys, phys_offset, slp_typa, slp_typb);
            }
        }
    } else if sig == b"APIC" {
        // MADT (Multiple APIC Description Table)
        *has_madt = true;
        let ptr = virt as *const u8;
        let len = hdr.length as usize;
        let mut offset = 44; // Skip header + Local APIC Addr (4) + Flags (4)
        let mut count = 0;
        while offset + 2 <= len {
            let entry_type = unsafe { *ptr.add(offset) };
            let entry_len = unsafe { *ptr.add(offset + 1) } as usize;
            if entry_len == 0 {
                break;
            }
            // Type 0: Processor Local APIC
            if entry_type == 0 && offset + 4 < len {
                let flags = unsafe { core::ptr::read_unaligned(ptr.add(offset + 4) as *const u32) };
                if (flags & 1) != 0 { // Enabled
                    count += 1;
                }
            }
            offset += entry_len;
        }
        if count > 0 {
            *cpu_cores = count;
        }
    }
}

fn parse_dsdt(dsdt_phys: u64, phys_offset: u64, slp_typa: &mut u16, slp_typb: &mut u16) {
    let virt = (phys_offset + dsdt_phys) as *const AcpiHeader;
    let hdr = unsafe { *virt };
    if &hdr.signature != b"DSDT" {
        return;
    }

    let ptr = virt as *const u8;
    let len = hdr.length as usize;
    // Search for "_S5_" in AML bytecode
    for i in 36..len.saturating_sub(10) {
        unsafe {
            if *ptr.add(i) == b'_'
                && *ptr.add(i + 1) == b'S'
                && *ptr.add(i + 2) == b'5'
                && *ptr.add(i + 3) == b'_'
            {
                // Found _S5 object!
                // Package opcode (0x12) is typically at i + 4
                if *ptr.add(i + 4) == 0x12 {
                    // SLP_TYPa is typically in byte i + 7 or i + 8
                    let a = *ptr.add(i + 7);
                    let b = *ptr.add(i + 9);
                    *slp_typa = (a as u16) & 0x07;
                    *slp_typb = (b as u16) & 0x07;
                    return;
                }
            }
        }
    }
}

/// Shuts down the machine cleanly using ACPI S5 sleep state or hypervisor power ports.
pub fn poweroff() -> ! {
    serial_println!("[acpi   ] executing hardware poweroff...");

    let info = ACPI.lock().clone();
    if let Some(acpi) = info {
        let slp_en = 1u16 << 13;
        let val_a = (acpi.slp_typa << 10) | slp_en;
        let val_b = (acpi.slp_typb << 10) | slp_en;

        unsafe {
            if acpi.pm1a_cnt != 0 {
                let mut port = Port::<u16>::new(acpi.pm1a_cnt);
                port.write(val_a);
            }
            if acpi.pm1b_cnt != 0 {
                let mut port = Port::<u16>::new(acpi.pm1b_cnt);
                port.write(val_b);
            }
        }
    }

    // Fallback: VirtualBox / QEMU ACPI shutdown port
    unsafe {
        let mut port_604 = Port::<u16>::new(0x604);
        port_604.write(0x2000);

        let mut port_b004 = Port::<u16>::new(0xB004);
        port_b004.write(0x2000);

        let mut port_4004 = Port::<u16>::new(0x4004);
        port_4004.write(0x3400);

        // VirtualBox VMMDev backdoor shutdown
        let mut port_501 = Port::<u16>::new(0x501);
        port_501.write(0x2000);
    }

    loop {
        x86_64::instructions::hlt();
    }
}

/// Reboots the computer via 8042 keyboard controller, PCI reset register, or triple fault.
pub fn reboot() -> ! {
    serial_println!("[acpi   ] executing hardware reboot...");

    unsafe {
        // Method 1: 8042 Keyboard Controller pulse reset line
        let mut kbd_cmd = Port::<u8>::new(0x64);
        kbd_cmd.write(0xFE);

        // Method 2: PCI Reset Register 0xCF9
        let mut pci_reset = Port::<u8>::new(0xCF9);
        pci_reset.write(0x06); // Reset CPU + System
        pci_reset.write(0x0E);

        // Method 3: Triple Fault via invalid zero IDT
        let null_idt = x86_64::structures::DescriptorTablePointer {
            limit: 0,
            base: VirtAddr::new(0),
        };
        x86_64::instructions::tables::lidt(&null_idt);
        x86_64::instructions::interrupts::int3();
    }

    loop {
        x86_64::instructions::hlt();
    }
}
