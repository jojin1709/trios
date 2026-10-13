//! The panic handler — last-stop diagnostics on the serial port.
//!
//! Everything is logged to COM1 (the only output channel in Phase 1); then the
//! CPU is halted. Panics in interrupt context re-enter this handler while the
//! serial lock may be held (see the note in `serial.rs`) — accepted risk for
//! Phase 1, addressed by a lock-free log in a later phase.

use core::arch::asm;

// `serial_println!` is in scope via `#[macro_use] mod serial;` in main.rs —
// importing it here as well would be a duplicate binding.

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    serial_println!();
    serial_println!("================ KERNEL PANIC ================");
    // `{info}` renders the message plus optional source location.
    serial_println!("{info}");
    if let Some(location) = info.location() {
        serial_println!(
            "  at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        );
    }

    // Poor man's stack anchor: debug info/frame pointers are off by default,
    // so a full backwalk isn't reliable — but RSP/RBP still tell the reader
    // where the panic ran.
    let rsp: u64;
    let rbp: u64;
    unsafe {
        // SAFETY: reading RSP/RBP writes only the output operands and touches
        // no memory; `options(nomem, nostack, preserves_flags)` tells rustc it
        // may freely schedule neighbouring code around these reads.
        asm!(
            "mov {rsp}, rsp",
            rsp = out(reg) rsp,
            options(nomem, nostack, preserves_flags)
        );
        asm!(
            "mov {rbp}, rbp",
            rbp = out(reg) rbp,
            options(nomem, nostack, preserves_flags)
        );
    }
    serial_println!("  rsp = {rsp:#018x}   rbp = {rbp:#018x}");
    serial_println!("System halted — reset or power off the VM to reboot.");

    loop {
        x86_64::instructions::hlt();
    }
}
