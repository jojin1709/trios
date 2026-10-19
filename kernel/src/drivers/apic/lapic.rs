//! Local Advanced Programmable Interrupt Controller (LAPIC) driver.

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use x86_64::registers::model_specific::Msr;

const IA32_APIC_BASE_MSR: u32 = 0x1B;
const IA32_APIC_BASE_ENABLE: u64 = 1 << 11;

// LAPIC MMIO register offsets (16-byte aligned)
const REG_LAPIC_ID: usize = 0x020;
const REG_LAPIC_VER: usize = 0x030;
const REG_TPR: usize = 0x080;
const REG_EOI: usize = 0x0B0;
const REG_SVR: usize = 0x0F0;
const REG_ESR: usize = 0x280;
const REG_LVT_TIMER: usize = 0x320;
const REG_LVT_THERMAL: usize = 0x330;
const REG_LVT_PERF: usize = 0x340;
const REG_LVT_LINT0: usize = 0x350;
const REG_LVT_LINT1: usize = 0x360;
const REG_LVT_ERROR: usize = 0x370;
const REG_TIMER_INIT_COUNT: usize = 0x380;
const REG_TIMER_CURR_COUNT: usize = 0x390;
const REG_TIMER_DIV: usize = 0x3E0;

const SVR_APIC_ENABLE: u32 = 1 << 8;
const TIMER_MASKED: u32 = 1 << 16;
const TIMER_MODE_PERIODIC: u32 = 1 << 17;
const DIVIDE_BY_16: u32 = 0x03;

pub static LOCAL_APIC: Mutex<Option<LocalApic>> = Mutex::new(None);
static LAPIC_BASE_VIRT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static APIC_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub struct LocalApic {
    base_virt: u64,
}

impl LocalApic {
    /// Creates a new Local APIC instance for the given virtual MMIO base address.
    pub unsafe fn new(base_virt: u64) -> Self {
        LAPIC_BASE_VIRT.store(base_virt, Ordering::SeqCst);
        Self { base_virt }
    }

    #[inline(always)]
    unsafe fn read(&self, offset: usize) -> u32 {
        let ptr = (self.base_virt + offset as u64) as *const u32;
        unsafe { core::ptr::read_volatile(ptr) }
    }

    #[inline(always)]
    unsafe fn write(&self, offset: usize, val: u32) {
        let ptr = (self.base_virt + offset as u64) as *mut u32;
        unsafe { core::ptr::write_volatile(ptr, val) }
    }

    /// Initializes and enables the Local APIC on the current CPU core.
    pub unsafe fn init(&self, spurious_vector: u8) {
        // 1. Ensure APIC is enabled in IA32_APIC_BASE MSR
        let mut base_msr = Msr::new(IA32_APIC_BASE_MSR);
        let val = unsafe { base_msr.read() };
        if (val & IA32_APIC_BASE_ENABLE) == 0 {
            unsafe { base_msr.write(val | IA32_APIC_BASE_ENABLE); }
        }

        unsafe {
            // 2. Set Spurious Interrupt Vector and enable APIC software bit
            self.write(REG_SVR, (spurious_vector as u32) | SVR_APIC_ENABLE);

            // 3. Clear Task Priority Register (accept all interrupts)
            self.write(REG_TPR, 0);

            // 4. Mask LINT0, LINT1, Performance, Thermal, and Error lines initially
            self.write(REG_LVT_LINT0, TIMER_MASKED);
            self.write(REG_LVT_LINT1, TIMER_MASKED);
            self.write(REG_LVT_PERF, TIMER_MASKED);
            self.write(REG_LVT_THERMAL, TIMER_MASKED);
            self.write(REG_LVT_ERROR, TIMER_MASKED);

            // Clear error register
            self.write(REG_ESR, 0);
            self.write(REG_ESR, 0);

            // Send an initial EOI just in case
            self.eoi();

            let id = (self.read(REG_LAPIC_ID) >> 24) & 0xFF;
            let ver = self.read(REG_LAPIC_VER) & 0xFF;
            serial_println!("[lapic  ] Local APIC enabled: ID={id} Version={ver:#x} Base={:#x}", self.base_virt);
        }
    }

    /// Sends End of Interrupt (EOI) to the Local APIC.
    #[inline(always)]
    pub unsafe fn eoi(&self) {
        unsafe { self.write(REG_EOI, 0); }
    }

    /// Calibrates and arms the Local APIC periodic timer for `target_hz` (e.g. 100 Hz).
    pub unsafe fn start_periodic_timer(&self, vector: u8, target_hz: u32) {
        unsafe {
            // Divide by 16
            self.write(REG_TIMER_DIV, DIVIDE_BY_16);

            // 1. Measure tick rate over ~10ms (100 Hz period = 10ms = 11,931 PIT cycles)
            self.write(REG_LVT_TIMER, TIMER_MASKED);
            self.write(REG_TIMER_INIT_COUNT, 0xFFFFFFFF);

            // Latch and read initial PIT counter
            x86_64::instructions::port::Port::<u8>::new(0x43).write(0x00);
            let l1 = x86_64::instructions::port::Port::<u8>::new(0x40).read() as u16;
            let h1 = x86_64::instructions::port::Port::<u8>::new(0x40).read() as u16;
            let mut prev = l1 | (h1 << 8);
            let mut total_down = 0u32;
            let target_down = 11931u32;

            while total_down < target_down {
                core::hint::spin_loop();
                x86_64::instructions::port::Port::<u8>::new(0x43).write(0x00);
                let l2 = x86_64::instructions::port::Port::<u8>::new(0x40).read() as u16;
                let h2 = x86_64::instructions::port::Port::<u8>::new(0x40).read() as u16;
                let curr = l2 | (h2 << 8);

                if curr <= prev {
                    total_down += (prev - curr) as u32;
                } else {
                    total_down += (prev + (11931 - curr)) as u32;
                }
                prev = curr;
            }

            let elapsed_ticks = 0xFFFFFFFFu32.saturating_sub(self.read(REG_TIMER_CURR_COUNT));
            let count_per_tick = if elapsed_ticks > 1000 {
                elapsed_ticks
            } else {
                // Fallback default (~10MHz bus clock divided by 16 / 100)
                100_000
            };

            // 2. Program Periodic mode
            self.write(REG_LVT_TIMER, (vector as u32) | TIMER_MODE_PERIODIC);
            self.write(REG_TIMER_DIV, DIVIDE_BY_16);
            self.write(REG_TIMER_INIT_COUNT, count_per_tick);

            serial_println!(
                "[lapic  ] timer calibrated & armed: vector={} periodic rate={} Hz (initial_count={})",
                vector,
                target_hz,
                count_per_tick
            );
        }
    }
}

/// Global helper to send EOI if Local APIC is active.
#[inline(always)]
pub fn end_of_interrupt() {
    if APIC_INITIALIZED.load(Ordering::Relaxed) {
        let base = LAPIC_BASE_VIRT.load(Ordering::Relaxed);
        if base != 0 {
            let ptr = (base + REG_EOI as u64) as *mut u32;
            unsafe {
                core::ptr::write_volatile(ptr, 0);
            }
        }
    }
}

/// Sets whether the Local APIC is active and receiving interrupts.
pub fn set_apic_active(active: bool) {
    APIC_INITIALIZED.store(active, Ordering::SeqCst);
}

/// Returns true if Local APIC is active.
pub fn is_apic_active() -> bool {
    APIC_INITIALIZED.load(Ordering::Relaxed)
}
