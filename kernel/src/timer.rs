//! PIT (Programmable Interval Timer) — the Phase 1 heartbeat source.
//!
//! The PIT is the legacy x86 timer built into every PC (and emulated by
//! QEMU/VirtualBox). Channel 0 is hard-wired to IRQ0 of the 8259 PIC. We
//! program it to fire [`HZ`] times per second; the IRQ0 handler in
//! [`crate::interrupts`] bumps a monotonic tick counter.
//!
//! Phase 5+ can swap this for the LAPIC timer or HPET without touching the
//! [`ticks`]/[`wait_ticks`] API.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::port::Port;

/// Frequency of the PIT input oscillator (1.193182 MHz).
const PIT_OSC_HZ: u32 = 1_193_182;

/// Timer interrupt frequency: 100 Hz = one tick every 10 ms.
pub const HZ: u32 = 100;

/// Ticks per second (equals [`HZ`]); keeps time math readable.
pub const TICKS_PER_SECOND: u64 = HZ as u64;

/// Monotonic tick counter, incremented from the IRQ0 handler.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Programs PIT channel 0 (wired to IRQ0) to fire `hz` times per second.
///
/// # Panics
/// If `hz` cannot be expressed as a 16-bit PIT divisor.
pub fn init(hz: u32) {
    assert!(
        (19..=65_535).contains(&hz),
        "PIT frequency must yield a divisor in 1..=65535"
    );
    let divisor = (PIT_OSC_HZ / hz) as u16;

    // SAFETY: ports 0x43 (PIT command) and 0x40 (channel 0 data) are fixed
    // PC/AT I/O addresses present on every x86; we run with ring-0 I/O
    // privilege and no other code touches the PIT concurrently.
    unsafe {
        let mut cmd = Port::<u8>::new(0x43);
        let mut data = Port::<u8>::new(0x40);
        // Command word: channel 0 | lo/hi byte access | mode 3 (square wave).
        cmd.write(0x36);
        data.write((divisor & 0xFF) as u8);
        data.write((divisor >> 8) as u8);
    }
}

/// Current monotonic tick count since boot (100 per second at [`HZ`]).
pub fn ticks() -> u64 {
    // Relaxed is enough: the counter is self-contained (no other memory is
    // published through it), and x86 gives us strong ordering anyway.
    TICKS.load(Ordering::Relaxed)
}

/// Whole seconds since boot.
pub fn uptime_seconds() -> u64 {
    ticks() / TICKS_PER_SECOND
}

/// Called from the IRQ0 handler with interrupts already disabled by the CPU.
pub(crate) fn increment() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Busy-waits for at least `n` ticks, halting the CPU until each interrupt.
///
/// Interrupts must be enabled before calling, otherwise this never returns.
pub fn wait_ticks(n: u64) {
    let target = ticks().saturating_add(n);
    while ticks() < target {
        x86_64::instructions::hlt();
    }
}

/// Calendar Date & Time read from CMOS Real-Time Clock (RTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Reads the real-world calendar date and time from the motherboard CMOS RTC.
pub fn read_rtc() -> DateTime {
    unsafe {
        let mut addr_port = Port::<u8>::new(0x70);
        let mut data_port = Port::<u8>::new(0x71);

        // Wait until RTC update is not in progress
        for _ in 0..10_000 {
            addr_port.write(0x0A);
            if (data_port.read() & 0x80) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        addr_port.write(0x00);
        let mut second = data_port.read();
        addr_port.write(0x02);
        let mut minute = data_port.read();
        addr_port.write(0x04);
        let mut hour = data_port.read();
        addr_port.write(0x07);
        let mut day = data_port.read();
        addr_port.write(0x08);
        let mut month = data_port.read();
        addr_port.write(0x09);
        let mut year = data_port.read();

        // Read register B to check format (BCD vs Binary, 12-hour vs 24-hour)
        addr_port.write(0x0B);
        let register_b = data_port.read();

        // Convert BCD to binary if bit 2 is clear
        if (register_b & 0x04) == 0 {
            second = (second & 0x0F) + ((second / 16) * 10);
            minute = (minute & 0x0F) + ((minute / 16) * 10);
            hour = ((hour & 0x0F) + (((hour & 0x70) / 16) * 10)) | (hour & 0x80);
            day = (day & 0x0F) + ((day / 16) * 10);
            month = (month & 0x0F) + ((month / 16) * 10);
            year = (year & 0x0F) + ((year / 16) * 10);
        }

        // Convert 12 hour to 24 hour if necessary
        if (register_b & 0x02) == 0 && (hour & 0x80) != 0 {
            hour = ((hour & 0x7F) + 12) % 24;
        }

        let full_year = 2000 + (year as u16);

        DateTime {
            year: full_year,
            month,
            day,
            hour,
            minute,
            second,
        }
    }
}

