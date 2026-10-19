//! I/O Advanced Programmable Interrupt Controller (IOAPIC) driver.

use spin::Mutex;

const REG_IOREGSEL: usize = 0x00;
const REG_IOWIN: usize = 0x10;

const IOAPICID: u8 = 0x00;
const IOAPICVER: u8 = 0x01;
const IOREDTBL_BASE: u8 = 0x10;

pub static IO_APIC: Mutex<Option<IoApic>> = Mutex::new(None);

pub struct IoApic {
    base_virt: u64,
    max_redirection_entries: u8,
}

impl IoApic {
    /// Creates a new I/O APIC instance for the given virtual MMIO base address.
    pub unsafe fn new(base_virt: u64) -> Self {
        Self {
            base_virt,
            max_redirection_entries: 0,
        }
    }

    unsafe fn read(&self, reg: u8) -> u32 {
        let sel_ptr = (self.base_virt + REG_IOREGSEL as u64) as *mut u32;
        let win_ptr = (self.base_virt + REG_IOWIN as u64) as *const u32;
        unsafe {
            core::ptr::write_volatile(sel_ptr, reg as u32);
            core::ptr::read_volatile(win_ptr)
        }
    }

    unsafe fn write(&self, reg: u8, val: u32) {
        let sel_ptr = (self.base_virt + REG_IOREGSEL as u64) as *mut u32;
        let win_ptr = (self.base_virt + REG_IOWIN as u64) as *mut u32;
        unsafe {
            core::ptr::write_volatile(sel_ptr, reg as u32);
            core::ptr::write_volatile(win_ptr, val);
        }
    }

    /// Initializes the I/O APIC and masks all redirection table entries.
    pub unsafe fn init(&mut self) {
        let id = unsafe { (self.read(IOAPICID) >> 24) & 0x0F };
        let ver_reg = unsafe { self.read(IOAPICVER) };
        let ver = ver_reg & 0xFF;
        self.max_redirection_entries = (((ver_reg >> 16) & 0xFF) + 1) as u8;

        serial_println!(
            "[ioapic ] I/O APIC initialized: ID={id} Version={ver:#x} Entries={}",
            self.max_redirection_entries
        );

        // Mask all redirection entries initially with safe vectors (32 + i)
        for i in 0..self.max_redirection_entries {
            unsafe {
                self.set_entry(i, 32 + i, 0, false, false, true);
            }
        }
    }

    /// Programs an I/O APIC redirection table entry.
    pub unsafe fn set_entry(
        &self,
        index: u8,
        vector: u8,
        dest_apic_id: u8,
        active_low: bool,
        level_triggered: bool,
        masked: bool,
    ) {
        if index >= self.max_redirection_entries {
            return;
        }

        let mut low: u32 = vector as u32;
        if active_low {
            low |= 1 << 13;
        }
        if level_triggered {
            low |= 1 << 15;
        }
        if masked {
            low |= 1 << 16;
        }

        let high: u32 = (dest_apic_id as u32) << 24;

        let reg_low = IOREDTBL_BASE + (index * 2);
        let reg_high = reg_low + 1;

        // Mask entry before reprogramming
        unsafe {
            self.write(reg_low, low | (1 << 16));
            self.write(reg_high, high);
            self.write(reg_low, low);
        }
    }

    /// Routes a GSI using MADT ISO flags to the target CPU and IDT vector.
    pub unsafe fn route_gsi(&self, gsi: u32, vector: u8, dest_apic_id: u8, flags: u16) {
        let active_low = (flags & 0x02) != 0;
        let level_triggered = (flags & 0x08) != 0;

        unsafe {
            self.set_entry(
                gsi as u8,
                vector,
                dest_apic_id,
                active_low,
                level_triggered,
                false,
            );
        }

        serial_println!(
            "[ioapic ] routed GSI {} -> vector {} (dest={}, low={}, level={})",
            gsi,
            vector,
            dest_apic_id,
            active_low,
            level_triggered
        );
    }

    /// Masks an individual GSI.
    #[allow(dead_code)]
    pub unsafe fn mask_gsi(&self, gsi: u32) {
        if gsi < self.max_redirection_entries as u32 {
            let reg_low = IOREDTBL_BASE + (gsi as u8 * 2);
            unsafe {
                let val = self.read(reg_low);
                self.write(reg_low, val | (1 << 16));
            }
        }
    }
}
