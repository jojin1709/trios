//! Serial console — TriOS's primary debug channel (Phase 1).
//!
//! Everything the kernel says is logged to COM1 (`0x3F8`) via a 16550 UART.
//! On the host side that shows up as:
//!   * QEMU       : `-serial file:serial.log` (or `-serial stdio`)
//!   * VirtualBox : `--uart1 0x3F8 4 --uartmode1 file <path>`
//!
//! We deliberately drive the raw [`Uart16550`] rather than the crate's
//! `Uart16550Tty` wrapper: the wrapper's constructor unconditionally runs a
//! **loopback self-test**, which not every emulator (or null-modem cable)
//! supports — a failed test would silently disable ALL serial output. The
//! only thing skipped with it is the `\n` → `\r\n` translation, which the
//! local [`SerialWriter`] adapter reimplements below.
//!
//! Known limitation (fine for Phase 1): if a *CPU exception* fires while a
//! log line is being written, the handler would spin on the held serial lock
//! and deadlock. Hardware-interrupt loggers are safe — see [`write_fmt`].

use core::fmt;

use spin::Mutex;
use uart_16550::{BaudRate, Config, Uart16550, backend::PioBackend};

/// Standard PC COM1 base address.
pub const COM1: u16 = 0x3F8;

lazy_static::lazy_static! {
    /// The COM1 UART. `None` if the device could not be initialised — then
    /// every `serial_print*` call becomes a no-op instead of panicking.
    pub static ref SERIAL1: Mutex<Option<Uart16550<PioBackend>>> = Mutex::new(init_port());
}

fn init_port() -> Option<Uart16550<PioBackend>> {
    // SAFETY:
    //  * `COM1` (0x3F8) is the standard PC serial port — emulated by QEMU
    //    (`-serial`) and VirtualBox (`--uart1 0x3F8 4`) and present on real
    //    hardware; all registers touched lie within [COM1, COM1 + 7];
    //  * the kernel runs in ring 0, so port I/O is permitted;
    //  * we take exclusive access for the whole lifetime of the value.
    unsafe {
        let mut uart = Uart16550::new_port(COM1).ok()?;
        // `init` programs baud/line/FIFO settings and verifies the device via
        // a scratch-register (SPR) round-trip — emulators all implement SPR.
        // 115200 baud: ten times faster lines than the crate's 9600 default,
        // which also shrinks the interrupt-off window in `write_fmt`.
        let config = Config {
            baud_rate: BaudRate::Baud115200,
            ..Config::default()
        };
        uart.init(config).ok()?;
        // No `test_loopback()` — see the module docs.
        Some(uart)
    }
}

/// Initialises the UART early in boot. Returns `false` if COM1 is missing.
pub fn init() -> bool {
    SERIAL1.lock().is_some()
}

/// Formats `args` straight to COM1 (no-op if the UART failed to initialise).
///
/// The serial lock is taken with **interrupts disabled**: an IRQ handler
/// that logs must never spin on the lock while the interrupted context was
/// mid-line (that deadlocks with `IF=0` — hit during bring-up when the
/// first PIT tick landed inside the post-`sti` log line). Interrupts that
/// arrive meanwhile are latched by the 8259 and delivered right after the
/// `enable()` below, so no events are lost — only deferred by one line.
pub fn write_fmt(args: core::fmt::Arguments<'_>) {
    use core::fmt::Write as _;
    use x86_64::instructions::interrupts;

    let were_enabled = interrupts::are_enabled();
    interrupts::disable();
    if let Some(uart) = SERIAL1.lock().as_mut() {
        let mut writer = SerialWriter(uart);
        // A UART that stopped accepting bytes must never take the kernel
        // down — swallow write errors.
        let _ = writer.write_fmt(args);
    }
    // The lock guard is dropped at the end of the `if let` above — before
    // interrupts come back on, so a now-pending handler finds it free.
    if were_enabled {
        interrupts::enable();
    }
}

/// `core::fmt::Write` adapter over the raw UART (the raw driver has no
/// `fmt::Write` impl — only the loopback-testing Tty wrapper provides one).
struct SerialWriter<'a>(&'a mut Uart16550<PioBackend>);

impl fmt::Write for SerialWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &byte in s.as_bytes() {
            match byte {
                // Translate Rust newlines to CRLF for terminals (mirrors the
                // behaviour of `Uart16550Tty::write_str`).
                b'\n' => self.0.send_bytes_exact(b"\r\n"),
                _ => self.0.send_bytes_exact(&[byte]),
            }
        }
        Ok(())
    }
}

/// Prints `format_args!` to COM1 without a trailing newline.
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::write_fmt(core::format_args!($($arg)*))
    };
}

/// Prints `format_args!` to COM1 followed by a newline.
///
/// Takes raw token trees (like `println!`) so format strings with inline
/// captures — e.g. `serial_println!("{stack:#?}")` — are passed through
/// `format_args!` intact instead of being printed literally.
#[macro_export]
macro_rules! serial_println {
    () => {
        $crate::serial_print!("\n")
    };
    ($($arg:tt)+) => {
        $crate::serial_print!("{}\n", core::format_args!($($arg)+))
    };
}
