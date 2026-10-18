//! PS/2 mouse driver — 3-byte packet decoding → [`MouseEvent`]s + the
//! absolute cursor position.
//!
//! ### The packet protocol
//!
//! Once data reporting is enabled (`0xF4`), the mouse streams fixed
//! **3-byte packets** on every movement/button change, read via IRQ12:
//!
//! ```text
//! byte 0:  bit7  Y overflow      bit6  X overflow
//!          bit5  Y sign          bit4  X sign
//!          bit3  ALWAYS 1  <-- the sync bit, our packet fence
//!          bit2  middle btn      bit1 right btn   bit0 left btn
//! byte 1:  X movement, two's complement (i8)
//! byte 2:  Y movement, two's complement (i8), +Y = *up* on the wire
//! ```
//!
//! ### Sync recovery
//!
//! Byte 0 of a standard PS/2 mouse packet has bit 3 always set to 1.
//! Bytes 1 and 2 carry two's-complement movement deltas (dx, dy), which
//! legitimately have bit 3 set whenever the delta is negative or has
//! magnitude >= 8. Therefore, bit 3 is only tested when framing byte 0.
//! Inter-byte timeout (>30 ms via PIT ticks) and sign-bit validation
//! (bits 4/5 of byte 0 matching the sign of dx/dy) ensure robust framing
//! recovery without corrupting valid movement.
//!
//! ### Axis flip
//!
//! PS/2 +Y points up; screens grow +Y downward. The driver negates Y
//! once, here, so every consumer works in screen coordinates.
//!
//! ### Two cursor views
//!
//! This module owns the **absolute** position (clamped to the screen
//! bounds given by [`set_bounds`]) — the hardware-facing truth. The
//! window manager keeps its *own* copy updated per-event while it
//! processes a drained batch, so a press+drag+release that arrives in
//! one burst is hit-tested against the position at each step (see
//! `gui::wm`). Both advance by the same deltas, so they agree once the
//! queue is drained.

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, Ordering};

use spin::Mutex;

use super::{
    CommandLog, RAW_RING_LEN, RawRing, SPIN_LIMIT, drain_output, read_data, wait_ack, wait_byte,
    wait_input_empty, write_cmd, write_data,
};
use crate::event::{Event, MouseButtons, MouseEvent, push as push_event};

/// Screen width the absolute position must stay inside.
static BOUND_W: AtomicI32 = AtomicI32::new(1);
/// Screen height (see [`BOUND_W`]).
static BOUND_H: AtomicI32 = AtomicI32::new(1);
/// Absolute cursor X, clamped to `0..BOUND_W` by the IRQ handler.
static CURSOR_X: AtomicI32 = AtomicI32::new(0);
/// Absolute cursor Y, clamped to `0..BOUND_H`.
static CURSOR_Y: AtomicI32 = AtomicI32::new(0);
/// Total IRQ12 deliveries since boot — the decisive "are packets even
/// arriving?" counter. Bumped on *every* mouse interrupt, before any
/// parsing, so it reflects the hardware/PIC path independently of the
/// packet decoder. The main loop prints it once per second (see
/// `main.rs`): if it stays 0 after the user clicks into the VM window,
/// VirtualBox never forwarded a byte (capture still on / host-side
/// issue); if it climbs but the cursor doesn't move, it's a decode bug.
static IRQ_COUNT: AtomicU32 = AtomicU32::new(0);

/// Status register (`0x64`) latched at the moment the last IRQ12
/// arrived: bit0 = output buffer full (should be set — the IRQ means a
/// byte is waiting), bit5 = aux source (must be *set* for mouse bytes —
/// the handler refuses to decode a clear one), bit6 = device timeout,
/// bit7 = parity error. Pure instrumentation (a status read has no side
/// effects). Printed as `[ps2    ] mouse status @irq = 0x{status:02x}`:
/// a *climbing* `mouse irq count` with a status that never shows bit5
/// set would mean the controller is routing keyboard bytes onto IRQ12.
static LAST_STATUS: AtomicU8 = AtomicU8::new(0);

/// Last [`RAW_RING_LEN`] bytes delivered on IRQ12, captured before the
/// aux-bit check and the decoder run (see [`super::RawRing`]). Printed
/// as `[ps2    ] mouse raw: {b0:02x} {b1:02x} {b2:02x} ...` — in the
/// VirtualBox bug under investigation this line is all zeros with a
/// flat `mouse irq count`, which together mean "the host queued motion
/// the guest device never delivered", *not* a decode failure.
static RAW: RawRing = RawRing::new();

/// Reassembly state for the 3-byte stream. Only the IRQ12 handler
/// touches it (interrupts don't nest), so a spin lock is never contended
/// — but it keeps the mutable state safe Rust instead of `static mut`.
struct PacketState {
    /// How many bytes of the current packet we have (0..=2).
    idx: usize,
    bytes: [u8; 3],
    /// Last reported button state (packets only carry changes faithfully
    /// every time — we mirror them so `MouseEvent` can carry full state).
    buttons: MouseButtons,
    /// PIT tick count when the last byte arrived, for framing timeout.
    last_tick: u64,
}

impl PacketState {
    const fn new() -> Self {
        Self {
            idx: 0,
            bytes: [0; 3],
            buttons: MouseButtons {
                left: false,
                right: false,
                middle: false,
            },
            last_tick: 0,
        }
    }
}

static PACKET: Mutex<PacketState> = Mutex::new(PacketState::new());

/// Enables the auxiliary (mouse) port and starts data reporting. The
/// port itself is enabled by [`super::init`] (`0xA8` + config byte), so
/// this is *device*-side bring-up, run with interrupts still off:
///
/// 1. `0xFF` — reset. The mouse answers `0xFA` (ACK), then runs its
///    self-test and answers `0xAA` ("BAT OK"), then reports its device
///    ID (`0x00` for a plain 2-button mouse). **This step is not
///    optional**: some emulators (VirtualBox especially) only start
///    honoring `0xF4` data-reporting after a clean reset — skipping it
///    is the classic "mouse dead in one VM, fine in another" bug. Every
///    response is read from the data port with a bounded spin, so a
///    dead device fails loudly instead of hanging the boot.
/// 2. `0xD4 0xF6` — mouse: "adopt defaults" (resolutions/rates we don't
///    override yet), ACK'd;
/// 3. `0xD4 0xF4` — mouse: "enable data reporting", ACK'd. From here on
///    every movement produces an IRQ12 per 3-byte packet.
///
/// `0xD4` is the controller's *next byte goes to the mouse* prefix —
/// without it, `0xF4` would be re-sent to the keyboard (where it also
/// means "enable scanning", confusing but harmless).
///
/// A `0xFE` (Resend) answer to any step is retried a few times — real
/// and emulated mice occasionally drop the first command after reset.
///
/// Observability: every response byte is reported on serial by
/// [`CommandLog`] — `[ps2    ] mouse <- 0xFF: FA AA 00` for the reset
/// exchange, `[ps2    ] mouse <- 0xF6: FA`, `[ps2    ] mouse <- 0xF4:
/// FA` — and a dead device prints `[ps2    ] mouse <- 0xFF: TIMEOUT
/// after N spins` instead of silently producing nothing. That matters
/// because this function has *always* returned `Ok` in VirtualBox while
/// the guest mouse still never moved: the handshake was the one place
/// nobody could see.
pub fn init() -> Result<(), &'static str> {
    drain_output();

    // 1. Reset + self-test. `wait_byte` consumes every response byte in
    //    order: 0xFA (ACK), 0xAA (self-test pass). The trailing device
    //    ID byte (0x00) is swallowed here on purpose (it isn't an error
    //    if it never arrives — not every emulator sends it, hence the
    //    ignored `Err` below), and the packet decoder never sees it
    //    because IRQ12 is still masked (super::init unmasks only after
    //    this function returns). One `log` spans all three waits so the
    //    serial log gets the single line
    //    `[ps2    ] mouse <- 0xFF: FA AA 00`.
    let mut log = CommandLog::new("mouse", 0xFF);
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        wait_input_empty("mouse reset (cmd 0xD4 0xFF)")?;
        write_cmd(0xD4);
        wait_input_empty("mouse reset (cmd 0xD4 0xFF)")?;
        write_data(0xFF);
        match wait_ack(&mut log) {
            Ok(0xFA) => break,
            Ok(0xFE) if attempts < 3 => continue, // resend requested
            Ok(other) => {
                return Err(if other == 0xFC || other == 0xFD {
                    "mouse reset failed (self-test reported error)"
                } else {
                    "unexpected byte after mouse reset (not ACK)"
                });
            }
            Err(e) => return Err(e),
        }
    }
    // Self-test "Bat Good" — bounded wait, must follow the reset ACK.
    wait_byte(&mut log, 0xAA, SPIN_LIMIT)?;
    // Device ID byte (0x00): present on real mice and VirtualBox, but
    // not guaranteed by every emulator — ignore a timeout. If it does
    // time out, `wait_byte` has already printed the command's TIMEOUT
    // line (with `saw FA AA`), so the `flush` below becomes a no-op.
    let _ = wait_byte(&mut log, 0x00, SPIN_LIMIT >> 4);
    log.flush();

    // 2. Defaults. Reported as `[ps2    ] mouse <- 0xF6: FA`. A late
    //    device-ID byte that missed the wait above shows up *here*,
    //    logged as `mouse <- 0xF6: 00 FA` — the visibility this whole
    //    instrumentation pass is for.
    let mut log = CommandLog::new("mouse", 0xF6);
    wait_input_empty("mouse defaults (cmd 0xD4 0xF6)")?;
    write_cmd(0xD4);
    wait_input_empty("mouse defaults (cmd 0xD4 0xF6)")?;
    write_data(0xF6);
    wait_ack(&mut log)?;
    log.flush();

    // 3. Go live. Reported as `[ps2    ] mouse <- 0xF4: FA`. From here
    //    on every movement should produce an IRQ12 per 3-byte packet
    //    (`[ps2    ] mouse irq count = ..` in the 1 Hz heartbeat).
    let mut log = CommandLog::new("mouse", 0xF4);
    wait_input_empty("mouse data report (cmd 0xD4 0xF4)")?;
    write_cmd(0xD4);
    wait_input_empty("mouse data report (cmd 0xD4 0xF4)")?;
    write_data(0xF4);
    wait_ack(&mut log)?;
    log.flush();
    Ok(())
}

/// How many IRQ12s have been delivered since boot (see [`IRQ_COUNT`]).
/// Printed once per second by the desktop loop — the one number that
/// distinguishes "VirtualBox sent nothing" from "we failed to decode".
pub fn irq_count() -> u32 {
    IRQ_COUNT.load(Ordering::Relaxed)
}

/// Status byte (`0x64`) as observed at the most recent IRQ12 (see
/// [`LAST_STATUS`]). Printed as `[ps2    ] mouse status @irq = 0x{b:02x}`.
pub fn status_at_irq() -> u8 {
    LAST_STATUS.load(Ordering::Relaxed)
}

/// Oldest-to-newest snapshot of the last [`RAW_RING_LEN`] IRQ12 bytes
/// (see [`RAW`]). Printed as `[ps2    ] mouse raw: {b0:02x} ...` — the
/// most recent byte is always the last one on the line.
pub fn raw_bytes() -> [u8; RAW_RING_LEN] {
    RAW.snapshot()
}

/// Sets the screen bounds and parks the cursor in the centre (called
/// from `main.rs` once the framebuffer geometry is known).
pub fn set_bounds(width: i32, height: i32) {
    let w = width.max(1);
    let h = height.max(1);
    BOUND_W.store(w, Ordering::SeqCst);
    BOUND_H.store(h, Ordering::SeqCst);
    CURSOR_X.store(w / 2, Ordering::SeqCst);
    CURSOR_Y.store(h / 2, Ordering::SeqCst);
}

/// Current absolute cursor position, clamped to the screen.
pub fn position() -> (i32, i32) {
    (
        CURSOR_X.load(Ordering::SeqCst),
        CURSOR_Y.load(Ordering::SeqCst),
    )
}

/// Moves the absolute cursor by `(dx, dy)`, clamped to the screen
/// bounds — **keyboard-cursor fallback** (`main.rs` arrow keys) calls
/// this so the driver's position stays in step with the window
/// manager's cursor view when arrows drive the pointer instead of
/// IRQ12 packets.
///
/// Pure arithmetic over the same atomics (and the same clamp) the
/// packet path uses; no hardware access, no IRQ12/decode change — the
/// PS/2 driver logic itself is untouched. Keeping the two views equal
/// matters: if a real packet ever arrives again, its delta must be
/// applied to a position that already matches the WM's.
pub fn nudge(dx: i32, dy: i32) {
    let max_x = (BOUND_W.load(Ordering::SeqCst) - 1).max(0);
    let max_y = (BOUND_H.load(Ordering::SeqCst) - 1).max(0);
    let cx = (CURSOR_X.load(Ordering::SeqCst) + dx).clamp(0, max_x);
    let cy = (CURSOR_Y.load(Ordering::SeqCst) + dy).clamp(0, max_y);
    CURSOR_X.store(cx, Ordering::SeqCst);
    CURSOR_Y.store(cy, Ordering::SeqCst);
}

/// IRQ12 handler body: read the byte that triggered the interrupt,
/// reassemble the packet, and queue one [`Event::Mouse`] per packet.
/// Called from `crate::interrupts::mouse_interrupt_handler` (IF=0).
pub fn handle_irq() {
    // Count first, parse second: the counter must reflect *hardware*
    // deliveries even when the byte turns out to be garbage.
    IRQ_COUNT.fetch_add(1, Ordering::Relaxed);

    // Aux-bit guard: status bit5 marks bytes that came from the second
    // (mouse) port. IRQ12 *should* only ever carry aux data — but if a
    // controller/emulator misroutes one keyboard byte here, decoding it
    // as packet data would poison the 3-byte framer. The byte must still
    // be read out of 0x60 (otherwise the controller re-raises the IRQ
    // forever); we just refuse to feed it to the decoder.
    let status = super::read_status();
    // Latch the status for the heartbeat *before* the guard can return:
    // `[ps2    ] mouse status @irq = 0x{b:02x}` must show what the
    // hardware said even for bytes the decoder rejects (instrumentation
    // only — the status read above is reused, nothing new is read).
    LAST_STATUS.store(status, Ordering::Relaxed);
    let byte = read_data();
    // Capture raw before the aux check, so rejected bytes are visible
    // too: `[ps2    ] mouse raw: ..` is the unfiltered IRQ12 stream.
    RAW.push(byte);
    if status & 0b0010_0000 == 0 {
        // One-time log: expected count is 0 for the lifetime of a
        // healthy system, so even a single occurrence is worth seeing —
        // but we never spam per-byte (a wedged controller would flood).
        static LOGGED: AtomicBool = AtomicBool::new(false);
        if !LOGGED.swap(true, Ordering::Relaxed) {
            serial_println!(
                "[ps2    ] mouse: non-aux byte on IRQ12 (status={status:#04x}) — ignored, one-time log"
            );
        }
        return;
    }
    let mut state = PACKET.lock();

    // -- packet synchronisation (see module docs) -------------------------
    let now = crate::timer::ticks();
    if now.wrapping_sub(state.last_tick) > 3 {
        // If more than 30 ms elapsed since the previous byte, any in-flight
        // partial packet was abandoned on the wire. Reset to expect byte 0.
        state.idx = 0;
    }
    state.last_tick = now;

    if state.idx == 0 && byte & 0b1000 == 0 {
        return; // not a byte-0 candidate: bit 3 must always be 1
    }

    let idx = state.idx; // copy first: indexing needs an immutable borrow
    state.bytes[idx] = byte;
    state.idx = idx + 1;
    if state.idx < 3 {
        return;
    }
    state.idx = 0;

    // -- decode a complete packet ----------------------------------------
    let [b0, b1, b2] = state.bytes;
    if b0 & 0b1000 == 0 {
        return;
    }

    // Sign-bit validation: in standard PS/2 without overflow, bit 4 matches X sign (b1 >= 0x80)
    // and bit 5 matches Y sign (b2 >= 0x80). If they conflict, framing is corrupt.
    if b0 & 0b1100_0000 == 0 {
        let x_sign = b0 & 0b0001_0000 != 0;
        let y_sign = b0 & 0b0010_0000 != 0;
        if x_sign != (b1 >= 0x80) || y_sign != (b2 >= 0x80) {
            return;
        }
    }

    let old_buttons = state.buttons; // kept for the button-edge log below
    state.buttons = MouseButtons::from_mask(b0 & 0b111);

    // Movement arrives as two's-complement i8; `as i8` sign-extends the
    // raw byte in one instruction. (The 9th sign bits in b0 and the
    // overflow bits are ignored: per-packet deltas that large only occur
    // when the mouse outruns the PS/2 stream — we'd rather drop the
    // overflow than invent motion.)
    let mut dx = b1 as i8 as i32;
    let mut dy = b2 as i8 as i32;
    if b0 & 0b1100_0000 != 0 {
        // X/Y overflow set: the delta is unreliable — discard it.
        dx = 0;
        dy = 0;
    }
    // Axis flip: wire +Y is up, screen +Y is down.
    let dy = -dy;

    // -- absolute position, clamped to the screen -------------------------
    let max_x = (BOUND_W.load(Ordering::SeqCst) - 1).max(0);
    let max_y = (BOUND_H.load(Ordering::SeqCst) - 1).max(0);
    let cx = (CURSOR_X.load(Ordering::SeqCst) + dx).clamp(0, max_x);
    let cy = (CURSOR_Y.load(Ordering::SeqCst) + dy).clamp(0, max_y);
    CURSOR_X.store(cx, Ordering::SeqCst);
    CURSOR_Y.store(cy, Ordering::SeqCst);

    // -- diagnostics: prove packets arrive, without spamming the serial ---
    // Motion can arrive at 100+ Hz, so only the *first* packet and button
    // press/release edges are logged (edges are human-paced). Seeing no
    // `[ps2    ] mouse:` line here while the host cursor sits frozen over
    // the window means VirtualBox is still in "mouse integration" mode:
    // without Guest Additions it shows the *host* cursor and never
    // forwards relative PS/2 motion (fixed by `controlvm mouseintegration
    // off` in run-virtualbox.ps1).
    static FIRST_PACKET: AtomicBool = AtomicBool::new(false);
    if !FIRST_PACKET.swap(true, Ordering::Relaxed) {
        serial_println!(
            "[ps2    ] mouse: first packet received (dx={dx}, dy={dy}, buttons={:#04x})",
            b0 & 0b111
        );
    }
    let buttons = state.buttons;
    for (name, was, now) in [
        ("L", old_buttons.left, buttons.left),
        ("R", old_buttons.right, buttons.right),
        ("M", old_buttons.middle, buttons.middle),
    ] {
        if was != now {
            let verb = if now { "pressed" } else { "released" };
            serial_println!("[ps2    ] mouse btn {name} {verb} at ({cx},{cy})");
        }
    }

    // -- queue the event (lock discipline: see crate::event) --------------
    drop(state); // release the packet lock *before* touching the queue
    push_event(Event::Mouse(MouseEvent { dx, dy, buttons }));
}
