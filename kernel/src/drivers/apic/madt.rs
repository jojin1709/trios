//! ACPI MADT (Multiple APIC Description Table) parser.
//! Entirely allocation-free so it can run before the heap is initialized.

/// An interrupt source override (maps ISA IRQ to Global System Interrupt GSI).
#[derive(Debug, Clone, Copy, Default)]
pub struct InterruptOverride {
    pub bus: u8,
    pub source_irq: u8,
    pub gsi: u32,
    pub flags: u16,
}

/// Parsed MADT configuration.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct MadtInfo {
    pub lapic_paddr: u64,
    pub ioapic_paddr: u64,
    pub ioapic_id: u8,
    pub ioapic_gsi_base: u32,
    pub overrides: [InterruptOverride; 16],
    pub override_count: usize,
    pub pcat_compat: bool,
    pub cpu_cores: usize,
}

impl MadtInfo {
    /// Maps an ISA IRQ (0..15) to its corresponding GSI, accounting for MADT overrides.
    pub fn irq_to_gsi(&self, irq: u8) -> (u32, u16) {
        for i in 0..self.override_count {
            let iso = &self.overrides[i];
            if iso.source_irq == irq {
                return (iso.gsi, iso.flags);
            }
        }
        // Default 1:1 identity map with standard ISA edge/active-high flags (0)
        (irq as u32, 0)
    }
}

/// Parses MADT bytes given its virtual address in kernel space.
pub unsafe fn parse_madt(virt_addr: u64, len: usize) -> Option<MadtInfo> {
    if len < 44 {
        return None;
    }

    let ptr = virt_addr as *const u8;
    let (default_lapic, pcat_compat) = unsafe {
        let default_lapic = core::ptr::read_unaligned(ptr.add(36) as *const u32) as u64;
        let flags = core::ptr::read_unaligned(ptr.add(40) as *const u32);
        (default_lapic, (flags & 1) != 0)
    };

    let mut lapic_paddr = default_lapic;
    let mut ioapic_paddr = 0xFEC00000u64;
    let mut ioapic_id = 0u8;
    let mut ioapic_gsi_base = 0u32;
    let mut overrides = [InterruptOverride::default(); 16];
    let mut override_count = 0usize;
    let mut cpu_cores = 0;

    let mut offset = 44;
    while offset + 2 <= len {
        let (entry_type, entry_len) = unsafe { (*ptr.add(offset), *ptr.add(offset + 1) as usize) };
        if entry_len == 0 || offset + entry_len > len {
            break;
        }

        match entry_type {
            // Type 0: Processor Local APIC
            0 => {
                if entry_len >= 8 {
                    let apic_flags = unsafe { core::ptr::read_unaligned(ptr.add(offset + 4) as *const u32) };
                    if (apic_flags & 1) != 0 {
                        cpu_cores += 1;
                    }
                }
            }
            // Type 1: I/O APIC
            1 => {
                if entry_len >= 12 {
                    unsafe {
                        ioapic_id = *ptr.add(offset + 2);
                        ioapic_paddr = core::ptr::read_unaligned(ptr.add(offset + 4) as *const u32) as u64;
                        ioapic_gsi_base = core::ptr::read_unaligned(ptr.add(offset + 8) as *const u32);
                    }
                }
            }
            // Type 2: Interrupt Source Override (ISO)
            2 => {
                if entry_len >= 10 && override_count < 16 {
                    unsafe {
                        let bus = *ptr.add(offset + 2);
                        let source_irq = *ptr.add(offset + 3);
                        let gsi = core::ptr::read_unaligned(ptr.add(offset + 4) as *const u32);
                        let flags = core::ptr::read_unaligned(ptr.add(offset + 8) as *const u16);
                        overrides[override_count] = InterruptOverride {
                            bus,
                            source_irq,
                            gsi,
                            flags,
                        };
                        override_count += 1;
                    }
                }
            }
            // Type 5: 64-bit Local APIC Address Override
            5 => {
                if entry_len >= 12 {
                    lapic_paddr = unsafe { core::ptr::read_unaligned(ptr.add(offset + 4) as *const u64) };
                }
            }
            _ => {}
        }

        offset += entry_len;
    }

    serial_println!(
        "[apic   ] MADT: LAPIC={:#x} IOAPIC={:#x} (id={}, GSI base={}) cores={} ISOs={}",
        lapic_paddr,
        ioapic_paddr,
        ioapic_id,
        ioapic_gsi_base,
        cpu_cores,
        override_count
    );

    for i in 0..override_count {
        let iso = &overrides[i];
        serial_println!(
            "[apic   ]   ISO: IRQ{} -> GSI {} (bus {}, flags {:#x})",
            iso.source_irq,
            iso.gsi,
            iso.bus,
            iso.flags
        );
    }

    Some(MadtInfo {
        lapic_paddr,
        ioapic_paddr,
        ioapic_id,
        ioapic_gsi_base,
        overrides,
        override_count,
        pcat_compat,
        cpu_cores,
    })
}
