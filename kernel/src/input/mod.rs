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
//!    (bit0 = IRQ1, bit1 = IRQ12), pin the translation bit (bit6) on,
//!    clear the system flag (bit2) and the clock-disable bits (4/5),
//!    write it back (cmd `0x60`) — then drain **again**: those same
//!    bits 4/5 *are* the port-disable flags, so the config write is
//!    what re-enables both devices, and a byte can surface immediately;
//! 3. re-enable both ports (`0xAE` keyboard, `0xA8` aux) and drain once
//!    more — anything the devices said while we rewired must never
//!    reach the IRQ handlers as a phantom packet byte 0;
//! 4. keyboard: enable scancode stream (cmd `0xF4` to the device);
//! 5. mouse: **reset** (`0xFF` — wait for ACK `0xFA` + self-test `0xAA`,
//!    consume the ID byte), adopt defaults (`0xF6`), enable data
//!    reporting (`0xF4`) — all prefixed with `0xD4` so the controller
//!    forwards them to the mouse instead of the keyboard;
//! 6. drain one last time (see [`drain_output`]), then unmask IRQ1 +
//!    IRQ12 on the 8259s ([`crate::interrupts::unmask_input`]).
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
//!
//! ### Serial diagnostics (instrumentation only — no behaviour change)
//!
//! VirtualBox 7.2.14 queues host mouse motion (`/PDM/Queue/Mouse/Insert`
//! calls in its own log) that this guest never consumes: the heartbeat
//! shows `[ps2    ] mouse irq count = 0` forever. Until now the *why*
//! was invisible, because [`wait_ack`]/[`wait_byte`] silently discarded
//! every byte that wasn't the one they wanted. The diagnostic surface
//! added for that hunt (all lock-free, IRQ-safe, no allocation):
//!
//! * handshake lines — every init command's full response, e.g.
//!   `[ps2    ] kbd <- 0xF4: FA`, `[ps2    ] mouse <- 0xFF: FA AA 00`,
//!   and on failure `[ps2    ] kbd <- 0xF4: TIMEOUT after 1000000 spins`;
//! * 1 Hz heartbeat lines from `main.rs` — `[ps2    ] kb irq count = N`,
//!   `[ps2    ] kb status @irq = 0x..`, `[ps2    ] kb raw: ..` and the
//!   two `mouse status/raw` twins of the existing `mouse irq count`.

pub mod keyboard;
pub mod mouse;

// Flat re-exports so `main.rs` can say `input::Event` / `input::KeyCode`.
// (`Modifiers` stays at `crate::event::Modifiers` — its only consumer,
// `keyboard.rs`, already imports it from there.)
/// Drain one event from the shared queue (main-loop side).
pub use crate::event::poll as poll_event;
pub use crate::event::{Event, KeyCode, KeyEvent, MouseButtons, MouseEvent};

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

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

/// How many raw IRQ-context bytes each device's [`RawRing`] remembers:
/// the **last 8**, printed once per second by `main.rs` as
/// `[ps2    ] kb raw: 1d f0 ...` / `[ps2    ] mouse raw: 1d f0 ...`.
/// Eight covers a full mouse packet (3 bytes) several times over while
/// staying small enough to be four `AtomicU8`s of state.
pub const RAW_RING_LEN: usize = 8;

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

// -- IRQ-time raw capture ----------------------------------------------------

/// The last [`RAW_RING_LEN`] bytes a device delivered in IRQ context.
///
/// Deliberately **lock-free and allocation-free**: `push` runs inside the
/// IRQ1/IRQ12 handlers (`crate::interrupts`), where taking a
/// `spin::Mutex` could deadlock against the interrupted main loop (the
/// same reasoning documented in `serial::write_fmt`) and allocating
/// would need the heap lock. The state is one `AtomicU8` slot per byte
/// plus one `AtomicU32` write index: the only *writer* is that device's
/// handler (IRQs never nest on this kernel), the only *reader* is the
/// 1 Hz heartbeat in `main.rs`. `Relaxed` ordering is enough everywhere
/// — this is a diagnostic, not a synchronisation protocol.
///
/// Heartbeat lines produced from a snapshot: `[ps2    ] kb raw: ..` and
/// `[ps2    ] mouse raw: ..`.
pub(crate) struct RawRing {
    /// Slot `i` holds the byte pushed when the global index was
    /// `i (mod RAW_RING_LEN)`; slots never written read back as `0x00`.
    bytes: [AtomicU8; RAW_RING_LEN],
    /// Total bytes ever pushed — never reduced until used, so the write
    /// slot is `next % RAW_RING_LEN` and a snapshot can be rotated into
    /// chronological order.
    next: AtomicU32,
}

impl RawRing {
    /// Empty ring: every slot `0x00`, index `0`. `const` so the per-device
    /// statics need no lazy initialisation on the IRQ path.
    const fn new() -> Self {
        Self {
            // Inline `const` block: `AtomicU8` is not `Copy`, so the old
            // `[AtomicU8::new(0); N]` repeat syntax doesn't apply.
            bytes: [const { AtomicU8::new(0) }; RAW_RING_LEN],
            next: AtomicU32::new(0),
        }
    }

    /// Records one IRQ byte: store at `idx % RAW_RING_LEN`, then bump the
    /// index. Called once per interrupt, before any decoding, so garbage
    /// bytes are captured too — that's the point (a frozen `[ps2    ] kb
    /// raw:` line with a climbing `kb irq count` means bytes *arrive*
    /// but never look like scancodes).
    pub(crate) fn push(&self, byte: u8) {
        let idx = self.next.fetch_add(1, Ordering::Relaxed);
        self.bytes[(idx % RAW_RING_LEN as u32) as usize].store(byte, Ordering::Relaxed);
    }

    /// Oldest-to-newest snapshot: rotation starts at the *next* write
    /// slot, so the last element is always the most recent byte. Until
    /// the ring has wrapped once, never-written slots appear as `0x00`
    /// at the front — i.e. the trailing bytes are always real.
    pub(crate) fn snapshot(&self) -> [u8; RAW_RING_LEN] {
        let start = (self.next.load(Ordering::Relaxed) % RAW_RING_LEN as u32) as usize;
        let mut out = [0u8; RAW_RING_LEN];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self.bytes[(start + i) % RAW_RING_LEN].load(Ordering::Relaxed);
        }
        out
    }
}

/// Formats a byte slice as space-separated hex for one serial line.
///
/// This exists so the *whole* line is built inside a single
/// `serial_println!` call: `serial::write_fmt` takes the UART lock once
/// per call, so a line assembled from several `serial_print!` pieces can
/// be split in the middle by an IRQ handler's log when interrupts are
/// enabled (the 1 Hz heartbeat). Init-time callers run with IF=0 and
/// would be safe either way, but one type serves both.
///
/// Case matches the two conventions already in the log: [`HexBytes::upper`]
/// for init/handshake lines (`[ps2    ] mouse <- 0xFF: FA AA 00`),
/// [`HexBytes::lower`] for heartbeat lines (`[ps2    ] kb raw: 1d f0`).
pub struct HexBytes<'a>(&'a [u8], bool);

impl<'a> HexBytes<'a> {
    /// Uppercase hex (`FA AA 00`) — init-handshake and drain lines.
    pub const fn upper(bytes: &'a [u8]) -> Self {
        Self(bytes, true)
    }

    /// Lowercase hex (`1d f0 2a`) — the 1 Hz heartbeat raw-byte lines.
    pub const fn lower(bytes: &'a [u8]) -> Self {
        Self(bytes, false)
    }
}

impl core::fmt::Display for HexBytes<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for (i, &byte) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            if self.1 {
                write!(f, "{byte:02X}")?;
            } else {
                write!(f, "{byte:02x}")?;
            }
        }
        Ok(())
    }
}

// -- handshake helpers -------------------------------------------------------

/// Collects every byte one init command pulls off the data port so the
/// handshake prints as a single greppable serial line.
///
/// Why this exists: [`wait_ack`]/[`wait_byte`] used to discard non-wanted
/// bytes *silently*, so a handshake that went wrong in VirtualBox left no
/// trace — the log jumped straight to `keyboard: init FAILED` (or, worse,
/// to a silent success while the mouse ignored later commands). Each wait
/// helper now feeds every byte it reads (wanted or not) plus its spin
/// count into this log, which prints exactly one of:
///
/// * success — `[ps2    ] kbd <- 0xF4: FA`
/// * success, whole reset exchange — `[ps2    ] mouse <- 0xFF: FA AA 00`
/// * timeout — `[ps2    ] kbd <- 0xF4: TIMEOUT after 1000000 spins`,
///   with `; saw <bytes>` appended when the device said *something*
///   first (e.g. `... TIMEOUT after 62500 spins; saw FA AA` = ACK and
///   self-test arrived, only the optional device-ID byte didn't)
///
/// The spin count on a timeout line is the caller's whole budget, so it
/// also names the wait that died: `1000000` = an ACK wait ([`SPIN_LIMIT`]),
/// `62500` = the mouse's optional ID-byte wait (`SPIN_LIMIT >> 4`).
/// Error semantics of the helpers are unchanged — this type only
/// observes, it never decides.
pub(crate) struct CommandLog {
    /// Device tag printed before `<-` (`kbd` or `mouse`).
    dev: &'static str,
    /// Command byte sent, printed as `0x{cmd:02X}` (e.g. `0xF4`).
    cmd: u8,
    /// Response bytes in arrival order. Bytes past [`RAW_RING_LEN`] are
    /// dropped — init responses are ≤4 bytes, so this never bites.
    bytes: [u8; RAW_RING_LEN],
    /// How many entries of `bytes` are live.
    len: usize,
    /// Loop iterations spent waiting across *all* waits for this command
    /// (mouse-reset retries included) — see the type docs for how to read
    /// it on a timeout line.
    spins: u32,
    /// Set once a line has been printed, so a tolerated timeout (the
    /// mouse ID byte, which isn't guaranteed by every emulator) can't
    /// print a second line for the same command afterwards.
    flushed: bool,
}

impl CommandLog {
    /// Starts collecting the response to `cmd` from `dev`.
    pub(crate) const fn new(dev: &'static str, cmd: u8) -> Self {
        Self {
            dev,
            cmd,
            bytes: [0; RAW_RING_LEN],
            len: 0,
            spins: 0,
            flushed: false,
        }
    }

    /// Records one loop iteration of a wait on this command.
    fn tick(&mut self) {
        self.spins += 1;
    }

    /// Records one byte read off the data port, wanted or not.
    fn record(&mut self, byte: u8) {
        if self.len < RAW_RING_LEN {
            self.bytes[self.len] = byte;
            self.len += 1;
        }
    }

    /// Prints the success line (`[ps2    ] kbd <- 0xF4: FA`), once.
    /// Called by the *caller* after its last wait succeeded, because one
    /// logical command can span several waits (mouse reset: ACK + self
    /// test + ID) that must end up on a single line.
    pub(crate) fn flush(&mut self) {
        if self.flushed {
            return;
        }
        self.flushed = true;
        if self.len == 0 {
            // Unreachable today (success implies ≥1 byte) — but a bare
            // `[ps2    ] kbd <- 0xF4:` line would be worse than useless.
            serial_println!("[ps2    ] {} <- 0x{:02X}: (no bytes)", self.dev, self.cmd);
        } else {
            serial_println!(
                "[ps2    ] {} <- 0x{:02X}: {}",
                self.dev,
                self.cmd,
                HexBytes::upper(&self.bytes[..self.len])
            );
        }
    }

    /// Prints the timeout line and marks the command logged. Called by
    /// the wait helpers immediately before they return `Err`, so the
    /// line is emitted even when the caller's `?` short-circuits out of
    /// the init sequence.
    fn flush_timeout(&mut self) {
        if self.flushed {
            return;
        }
        self.flushed = true;
        if self.len == 0 {
            serial_println!(
                "[ps2    ] {} <- 0x{:02X}: TIMEOUT after {} spins",
                self.dev,
                self.cmd,
                self.spins
            );
        } else {
            serial_println!(
                "[ps2    ] {} <- 0x{:02X}: TIMEOUT after {} spins; saw {}",
                self.dev,
                self.cmd,
                self.spins,
                HexBytes::upper(&self.bytes[..self.len])
            );
        }
    }
}

/// Waits until the controller is ready to accept a byte we want to send
/// (status bit1 clear = input buffer empty).
///
/// `op` names the operation for the failure line, e.g.
/// `[ps2    ] write config (cmd 0x60): input buffer never emptied after
/// 1000000 spins`. Two reasons it lives here instead of in the caller's
/// message: (a) [`init`] deliberately *continues* past some waits
/// (`let _ = wait_input_empty(..)` — a slow controller must not abort
/// the boot), and without the name those timeouts would leave no trace
/// at all; (b) every caller that does report an error (`*_FAILED`
/// lines, [`CommandLog`] handshake lines) already adds its own context,
/// so the `Err` string itself stays generic and unchanged.
///
/// Called only during init with IF=0 (see module docs), so this serial
/// line can't be split mid-message by an IRQ handler's log.
pub(crate) fn wait_input_empty(op: &'static str) -> Result<(), &'static str> {
    for _ in 0..SPIN_LIMIT {
        if read_status() & 0b10 == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    serial_println!("[ps2    ] {op}: input buffer never emptied after {SPIN_LIMIT} spins");
    Err("PS/2 input buffer never emptied (controller busy)")
}

/// Waits until the controller has a byte for us (status bit0 set).
///
/// Same `op`-naming contract as [`wait_input_empty`]: the timeout line
/// (`[ps2    ] read config (cmd 0x20): output buffer stayed empty after
/// 1000000 spins`) names the wait that died, which is the whole
/// operation — this helper has a single caller.
pub(crate) fn wait_output_full(op: &'static str) -> Result<(), &'static str> {
    for _ in 0..SPIN_LIMIT {
        if read_status() & 0b01 != 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    serial_println!("[ps2    ] {op}: output buffer stayed empty after {SPIN_LIMIT} spins");
    Err("PS/2 output buffer stayed empty (no response)")
}

/// Reads whatever byte(s) the firmware/emulator left lying around so the
/// first command we send starts from a clean slate.
///
/// Instrumentation: bytes discarded here produce no handshake line of
/// their own (no command was in flight), so a byte the firmware left
/// behind would otherwise vanish without a trace. Anything actually
/// thrown away is now printed as `[ps2    ] drain: discarded stale bytes
/// 00 AA` — a stuck output buffer full of `0x00`s was previously
/// indistinguishable from a dead controller.
pub(crate) fn drain_output() {
    // Bounded: real leftovers are 0..=3 bytes (BAT codes, stale ACKs).
    let mut stale = [0u8; 16];
    let mut n = 0usize;
    for _ in 0..16 {
        if read_status() & 0b01 == 0 {
            break;
        }
        let byte = read_data();
        if n < stale.len() {
            stale[n] = byte;
            n += 1;
        }
    }
    if n > 0 {
        serial_println!(
            "[ps2    ] drain: discarded stale bytes {}",
            HexBytes::upper(&stale[..n])
        );
    }
}

/// Reads the controller configuration byte (cmd `0x20`).
pub(crate) fn read_config() -> Result<u8, &'static str> {
    wait_input_empty("read config (cmd 0x20)")?;
    write_cmd(0x20);
    wait_output_full("read config (cmd 0x20)")?;
    Ok(read_data())
}

/// Writes the controller configuration byte (cmd `0x60` + data byte).
pub(crate) fn write_config(cfg: u8) -> Result<(), &'static str> {
    wait_input_empty("write config (cmd 0x60)")?;
    write_cmd(0x60);
    wait_input_empty("write config (cmd 0x60)")?;
    write_data(cfg);
    Ok(())
}

/// Waits for a device ACK (`0xFA`), consuming any interleaved bytes
/// (`0xFE` resend, `0xAA` self-test, stray scancodes) — **and recording
/// every one of them in `log`** so a handshake that goes wrong prints
/// `[ps2    ] kbd <- 0xF4: <bytes>` (or the `TIMEOUT after N spins`
/// variant) instead of hiding the evidence. See [`CommandLog`].
///
/// Returns the ACK byte on success — same semantics as before; the only
/// change is that skipped bytes are no longer invisible.
pub(crate) fn wait_ack(log: &mut CommandLog) -> Result<u8, &'static str> {
    for _ in 0..SPIN_LIMIT {
        log.tick();
        if read_status() & 0b01 != 0 {
            let byte = read_data();
            log.record(byte);
            if byte == 0xFA {
                return Ok(byte);
            }
            // Anything else: keep waiting for the real ACK (or time
            // out) — the byte is no longer lost, it's in `log`.
        } else {
            core::hint::spin_loop();
        }
    }
    // Emit `[ps2    ] <dev> <- 0x<cmd>: TIMEOUT after N spins[; saw ..]`
    // before the `Err` short-circuits the caller's `?`.
    log.flush_timeout();
    Err("PS/2 ACK (0xFA) timeout — device not responding")
}

/// Waits for one *specific* byte (`want`) on the data port, consuming
/// any other bytes that arrive first — each one recorded in `log` (see
/// [`CommandLog`]). Bounded spin — a device that never produces the byte
/// yields an error, never a hang.
///
/// Used by the mouse reset handshake: after `0xFF` the device answers
/// `0xFA` (ACK, consumed by [`wait_ack`]), then `0xAA` (self-test pass)
/// and its ID byte — each of which must be waited for in turn without
/// mistaking an earlier response for a later one. With logging, the
/// three waits on one [`CommandLog`] coalesce into the single line
/// `[ps2    ] mouse <- 0xFF: FA AA 00`.
pub(crate) fn wait_byte(
    log: &mut CommandLog,
    want: u8,
    max_spins: u32,
) -> Result<(), &'static str> {
    for _ in 0..max_spins {
        log.tick();
        if read_status() & 0b01 != 0 {
            let byte = read_data();
            log.record(byte);
            if byte == want {
                return Ok(());
            }
            // Some other byte (a prior response, a stray scancode) —
            // recorded in `log`, then discarded here while we keep
            // waiting for the one we want.
        } else {
            core::hint::spin_loop();
        }
    }
    // Same timeout line as `wait_ack`, with this wait's own spin budget
    // (e.g. 62500 for the mouse ID byte — `SPIN_LIMIT >> 4`).
    log.flush_timeout();
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
    let _ = wait_input_empty("quiesce kbd port (cmd 0xAD)");
    write_cmd(0xAD);
    let _ = wait_input_empty("quiesce aux port (cmd 0xA7)");
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
    //    enable both device clocks (clear bits 4/5) and pin the
    //    translation bit (bit6) ON rather than merely preserving whatever
    //    the firmware left there. `keyboard.rs` decodes *scancode set 1*,
    //    which only reaches the wire because the controller translates
    //    set 2 → set 1; a firmware that had cleared bit6 would hand us
    //    raw set-2 bytes and the keyboard would die with no handshake
    //    error anywhere (the ACK to `0xF4` would still be `0xFA`, just
    //    every *scancode* wrong). One explicit OR makes that broken
    //    state unrepresentable. Bit6 does not affect aux data: mouse
    //    packets pass untranslated on real 8042s and in VirtualBox
    //    alike (`kbd_update_irq` applies translation only in the
    //    keyboard branch), so the mouse path is indifferent to it.
    let new_cfg = (cfg | 0b0100_0111) & !0b0011_0000;
    match write_config(new_cfg) {
        Ok(()) => serial_println!(
            "[ps2    ] config byte {cfg:#04x} -> {new_cfg:#04x} (IRQ1 + IRQ12 routed, translation on)"
        ),
        Err(e) => serial_println!("[ps2    ] controller config write FAILED: {e}"),
    }
    // Drain *after* the config write, unconditionally (failed write
    // included — the previous drain was many milliseconds of spins
    // ago). The config write replaces the mode register wholesale, and
    // bits 4/5 of it are the port-disable flags we just cleared, so
    // both device clocks are live from this instant on even though
    // `0xAE`/`0xA8` haven't run yet. IRQs are off and the PIC is still
    // masked, so nothing that arrives here can be lost — but a byte
    // left sitting in the output buffer would later surface as the
    // first byte read after `sti`, i.e. a phantom "packet byte 0" for
    // the mouse decoder (and a phantom scancode for the keyboard).
    drain_output();

    // 4. Re-enable both ports (0xAE = keyboard, 0xA8 = aux). The clocks
    //    were already un-gated via the config byte above; these commands
    //    clear the port-level enables the disable commands set.
    let _ = wait_input_empty("enable kbd port (cmd 0xAE)");
    write_cmd(0xAE);
    let _ = wait_input_empty("enable aux port (cmd 0xA8)");
    write_cmd(0xA8);
    // Ports are definitively live now — sweep one more time before the
    // device handshakes below start consuming responses of their own,
    // so `wait_ack` in keyboard/mouse init sees only its own `0xFA`
    // (any stray would show up in `CommandLog` as a prefixed byte and
    // muddy the very lines this instrumentation exists to read).
    drain_output();

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

    // 7. Last sweep, now that *both* device handshakes have finished
    //    (mouse `0xFA` to `0xF4` was the last byte consumed above). Any
    //    byte produced behind our back — a late self-test remnant, host
    //    motion that landed while IF=0 — is still just sitting in the
    //    output buffer with the PIC masked. Read it out so the first
    //    IRQ after `sti` is a freshly raised one: a stale byte left
    //    here would arrive as phantom "packet byte 0" (mouse) or a
    //    phantom scancode (keyboard) at decoder index 0, where the
    //    sync-bit fence can't reject it if it happens to have bit3
    //    set. The decoder resyncs either way; this keeps the handshake
    //    lines and the first heartbeat raw-bytes line honest.
    drain_output();

    // 8. Let the 8259 deliver IRQ1/IRQ12 now that handlers are registered
    //    in the IDT (they were, at `init_idt` time) and devices are ready.
    interrupts::unmask_input();
    // Read the masks straight back off ports 0x21/0xA1 *before* announcing
    // success: this is the decisive line of the whole bring-up. It must
    // print `master=0xF8 slave=0xEF` — i.e. master bit2 (the IRQ2 cascade)
    // clear, or IRQ12 can never reach the CPU and the `[ps2    ] mouse irq
    // count = 0` heartbeat returns. See `interrupts::log_masks`.
    interrupts::log_masks("input::init phase-3");
    serial_println!("[ps2    ] PIC unmasked: IRQ1 (vector 33) + IRQ12 (vector 44)");
}

/// Gives the mouse driver the screen bounds its absolute cursor position
/// must stay inside (called from `main.rs` as soon as the framebuffer
/// geometry is known — before the first packet can arrive).
pub fn set_screen_size(width: i32, height: i32) {
    mouse::set_bounds(width, height);
}
