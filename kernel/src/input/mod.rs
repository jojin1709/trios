//! PS/2 input — keyboard (IRQ1) and mouse (IRQ12) — Phase 3.
//!
//! ### The PS/2 controller, in one paragraph
//!
//! Every PC (and every emulator — VirtualBox/QEMU emulate it faithfully)
//! still carries the old 8042 *PS/2 controller*: two I/O ports, `0x60`
//! (data) and `0x64` (status/command). The keyboard sits on port 1, the
//! mouse ("auxiliary device") on port 2. Devices talk to the controller
//! with small command bytes and answer with an `0xFA` ACK. The controller
//! raises IRQ1 (keyboard) / IRQ12 (mouse) when a byte is ready for us to
//! read from `0x60`. USB keyboards are irrelevant here — VirtualBox
//! exposes a perfect PS/2 pair, which is why Phase 3 needs no USB stack.
//!
//! ### Init order (runs before `sti`, see `main.rs`)
//!
//! 1. disable both device ports (`0xAD` keyboard, `0xA7` aux) so nothing
//!    streams while we rewire the controller, then drain any byte the
//!    firmware left in the output buffer;
//! 2. read the controller config byte (cmd `0x20`), set IRQ routing bits
//!    (bit0 = IRQ1, bit1 = IRQ12), clear the system flag (bit2) and the
//!    clock-disable bits (4/5), write it back (cmd `0x60`);
//! 3. re-enable both ports (`0xAE` keyboard, `0xA8` aux);
//! 4. keyboard: enable scancode stream (cmd `0xF4` to the device);
//! 5. mouse: **reset** (`0xFF` — wait for ACK `0xFA` + self-test `0xAA`,
//!    consume the ID byte), adopt defaults (`0xF6`), enable data
//!    reporting (`0xF4`) — all prefixed with `0xD4` so the controller
//!    forwards them to the mouse instead of the keyboard;
//! 6. unmask IRQ1 + IRQ12 on the 8259s ([`crate::interrupts::unmask_input`]).
//!
//! Everything polls with bounded spin loops: a dead emulator device must
//! fail loudly in the serial log, never hang the boot.
//!
//! ### Event flow
//!
//! ```text
//! IRQ1  -> interrupts.rs keyboard handler -> keyboard::handle_irq()
//!        -> decode scancode set 1 -> event::push(Event::Key(..))
//! IRQ12 -> interrupts.rs mouse handler    -> mouse::handle_irq()
//!        -> reassemble 3-byte packet      -> event::push(Event::Mouse(..))
//! IRQ0  -> timer handler                  -> event::push(Event::Tick(n))
//!                         main loop: event::poll() drains everything
//! ```
//!
//! Queue locking is documented in [`crate::event`]; this module never
//! touches the queue except through [`event::push`](crate::event::push).

pub mod keyboard;
pub mod mouse;

// Flat re-exports so `main.rs` can say `input::Event` / `input::KeyCode`.
// (`Modifiers` stays at `crate::event::Modifiers` — its only consumer,
// `keyboard.rs`, already imports it from there.)
/// Drain one event from the shared queue (main-loop side).
pub use crate::event::poll as poll_event;
pub use crate::event::{Event, KeyCode, KeyEvent, MouseButtons, MouseEvent};

use x86_64::instructions::port::Port;

use crate::interrupts;

/// The controller's data register: keyboard/mouse bytes go in and out
/// here, and device-bound commands are written here after a `0xD4`
/// prefix (mouse) or directly (keyboard).
const DATA_PORT: u16 = 0x60;
/// Status/command register. Reading: bit0 = output buffer full (a byte
/// waits for us), bit1 = input buffer full (controller still busy with
/// our last command). Writing: commands like `0x20`/`0x60`/`0xA8`/`0xD4`.
const STATUS_PORT: u16 = 0x64;

/// Spin limit for controller handshake waits. A port read is ~1 µs, so
/// this is generous (hundreds of ms worst case) yet still bounded — a
/// broken device yields an error, not a hang.
pub(crate) const SPIN_LIMIT: u32 = 1_000_000;

// -- raw port access ---------------------------------------------------------

/// Reads the status register.
///
/// # Safety
/// `0x64` is the standard PC controller-status I/O address; we run in
/// ring 0 with I/O privilege, and reading a status register has no
/// side effects.
pub(crate) fn read_status() -> u8 {
    // SAFETY: see above.
    unsafe { Port::<u8>::new(STATUS_PORT).read() }
}

/// Reads one byte from the data register (also clears the "output
/// buffer full" status bit).
///
/// # Safety
/// Callers must ensure a byte is available (status bit0 set) — otherwise
/// the read may block or return stale data on real hardware. The wait
/// helpers here guarantee that.
pub(crate) fn read_data() -> u8 {
    // SAFETY: ring-0 port I/O on the standard data port; the caller
    // upholds the availability precondition documented above.
    unsafe { Port::<u8>::new(DATA_PORT).read() }
}

/// Writes one byte to the data register (towards the keyboard, unless a
/// `0xD4` prefix was just written to `0x64`).
///
/// # Safety
/// Caller must have waited for the input buffer to drain first, or the
/// byte is dropped by the controller.
pub(crate) fn write_data(byte: u8) {
    // SAFETY: ring-0 port I/O; handshake precondition is the caller's
    // responsibility (every use here follows `wait_input_empty`).
    unsafe { Port::<u8>::new(DATA_PORT).write(byte) }
}

/// Writes a command byte to the controller (`0x64`).
///
/// # Safety
/// Same input-buffer precondition as [`write_data`].
pub(crate) fn write_cmd(cmd: u8) {
    // SAFETY: ring-0 port I/O on the standard command port.
    unsafe { Port::<u8>::new(STATUS_PORT).write(cmd) }
}

// -- handshake helpers -------------------------------------------------------

/// Waits until the controller is ready to accept a byte we want to send
/// (status bit1 clear = input buffer empty).
pub(crate) fn wait_input_empty() -> Result<(), &'static str> {
    for _ in 0..SPIN_LIMIT {
        if read_status() & 0b10 == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("PS/2 input buffer never emptied (controller busy)")
}

/// Waits until the controller has a byte for us (status bit0 set).
pub(crate) fn wait_output_full() -> Result<(), &'static str> {
    for _ in 0..SPIN_LIMIT {
        if read_status() & 0b01 != 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("PS/2 output buffer stayed empty (no response)")
}

/// Reads whatever byte(s) the firmware/emulator left lying around so the
/// first command we send starts from a clean slate.
pub(crate) fn drain_output() {
    // Bounded: real leftovers are 0..=3 bytes (BAT codes, stale ACKs).
    for _ in 0..16 {
        if read_status() & 0b01 == 0 {
            return;
        }
        let _ = read_data();
    }
}

/// Reads the controller configuration byte (cmd `0x20`).
pub(crate) fn read_config() -> Result<u8, &'static str> {
    wait_input_empty()?;
    write_cmd(0x20);
    wait_output_full()?;
    Ok(read_data())
}

/// Writes the controller configuration byte (cmd `0x60` + data byte).
pub(crate) fn write_config(cfg: u8) -> Result<(), &'static str> {
    wait_input_empty()?;
    write_cmd(0x60);
    wait_input_empty()?;
    write_data(cfg);
    Ok(())
}

/// Waits for a device ACK (`0xFA`), consuming any interleaved bytes
/// (`0xFE` resend, `0xAA` self-test, stray scancodes).
///
/// Returns the ACK byte on success.
pub(crate) fn wait_ack() -> Result<u8, &'static str> {
    for _ in 0..SPIN_LIMIT {
        if read_status() & 0b01 != 0 {
            let byte = read_data();
            if byte == 0xFA {
                return Ok(byte);
            }
            // Anything else: keep waiting for the real ACK (or time out).
        } else {
            core::hint::spin_loop();
        }
    }
    Err("PS/2 ACK (0xFA) timeout — device not responding")
}

/// Waits for one *specific* byte (`want`) on the data port, consuming
/// any other bytes that arrive first. Bounded spin — a device that
/// never produces the byte yields an error, never a hang.
///
/// Used by the mouse reset handshake: after `0xFF` the device answers
/// `0xFA` (ACK, consumed by [`wait_ack`]), then `0xAA` (self-test pass)
/// and its ID byte — each of which must be waited for in turn without
/// mistaking an earlier response for a later one.
pub(crate) fn wait_byte(want: u8, max_spins: u32) -> Result<(), &'static str> {
    for _ in 0..max_spins {
        if read_status() & 0b01 != 0 {
            if read_data() == want {
                return Ok(());
            }
            // Some other byte (a prior response, a stray scancode) —
            // discard it and keep waiting for the one we want.
        } else {
            core::hint::spin_loop();
        }
    }
    Err("PS/2 expected-byte timeout — device not responding")
}

// -- bring-up ----------------------------------------------------------------

/// Brings up both PS/2 devices and unmasks their IRQ lines.
///
/// Runs with interrupts disabled (from `main.rs`, before `sti`), so no
/// handler can fire mid-sequence. Failures are *reported, not fatal*:
/// the desktop still boots (keyboard-less/mouse-less) and the serial log
/// says which device died.
pub fn init() {
    // 1. Quiesce both device ports (0xAD = disable keyboard, 0xA7 =
    //    disable aux) so nothing streams into the output buffer while we
    //    rewire the controller. Both commands are "fire and forget": the
    //    controller ACKs nothing on 0x64 for them, so we just wait for
    //    the input buffer to drain before each write.
    let _ = wait_input_empty();
    write_cmd(0xAD);
    let _ = wait_input_empty();
    write_cmd(0xA7);

    // 2. Flush whatever the firmware/emulator left lying around, then
    //    read the current config byte — both devices are now silent, so
    //    the byte that answers cmd 0x20 cannot be a stray scancode.
    drain_output();
    let cfg = match read_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            serial_println!("[ps2    ] controller config read FAILED: {e}");
            return;
        }
    };

    // 3. Route IRQ1 (bit0) and IRQ12 (bit1) to the PICs, keep the system
    //    flag (bit2) set — per the OSDev spec the OS sets it after POST —
    //    and enable both device clocks (clear bits 4/5).
    let new_cfg = (cfg | 0b0000_0111) & !0b0011_0000;
    match write_config(new_cfg) {
        Ok(()) => serial_println!(
            "[ps2    ] config byte {cfg:#04x} -> {new_cfg:#04x} (IRQ1 + IRQ12 routed)"
        ),
        Err(e) => serial_println!("[ps2    ] controller config write FAILED: {e}"),
    }

    // 4. Re-enable both ports (0xAE = keyboard, 0xA8 = aux). The clocks
    //    were already un-gated via the config byte above; these commands
    //    clear the port-level enables the disable commands set.
    let _ = wait_input_empty();
    write_cmd(0xAE);
    let _ = wait_input_empty();
    write_cmd(0xA8);

    // 5. Keyboard: start streaming scancodes (0xF4 = "enable scanning").
    match keyboard::init() {
        Ok(()) => serial_println!("[ps2    ] keyboard: scanning enabled (scancode set 1)"),
        Err(e) => serial_println!("[ps2    ] keyboard: init FAILED — {e}"),
    }

    // 6. Mouse: reset, adopt defaults, start data reporting.
    match mouse::init() {
        Ok(()) => serial_println!("[ps2    ] mouse: aux port enabled, data reporting on"),
        Err(e) => serial_println!("[ps2    ] mouse: init FAILED — {e} (cursor may not move)"),
    }

    // 7. Let the 8259 deliver IRQ1/IRQ12 now that handlers are registered
    //    in the IDT (they were, at `init_idt` time) and devices are ready.
    interrupts::unmask_input();
    serial_println!("[ps2    ] PIC unmasked: IRQ1 (vector 33) + IRQ12 (vector 44)");
}

/// Gives the mouse driver the screen bounds its absolute cursor position
/// must stay inside (called from `main.rs` as soon as the framebuffer
/// geometry is known — before the first packet can arrive).
pub fn set_screen_size(width: i32, height: i32) {
    mouse::set_bounds(width, height);
}
