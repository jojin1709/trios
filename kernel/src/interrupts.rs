//! CPU exceptions and hardware interrupts: IDT, fault handlers, 8259 PIC.
//!
//! Vector layout:
//!   * `0..32` — CPU exceptions (#BP, #DF, #PF, #GP, ...);
//!   * `32`    — IRQ0 (PIT timer), remapped here from the default `0x08`
//!     so it can't collide with the CPU's own vectors;
//!   * `33`    — IRQ1 (PS/2 keyboard), Phase 3;
//!   * `40`    — IRQ8 (RTC), masked until a later phase;
//!   * `44`    — IRQ12 (PS/2 mouse, through the slave PIC), Phase 3.
//!
//! Phase 1 unmasks only the timer line: [`init_pic`] writes master `0xFE`
//! (`1111_1110`, bit0 clear = IRQ0 enabled) and slave `0xFF` (all eight
//! slave lines masked, cascade included — harmless there, the slave has
//! nothing to forward yet). Phase 3's [`crate::input::init`] unmasks both
//! input lines, IRQ1 and IRQ12, via [`unmask_input`] once both devices
//! answer — and *that* write must clear master **bit2** too: IRQ12 lives
//! on the slave PIC and every slave interrupt reaches the CPU *through*
//! master IRQ2, so a master mask with bit2 set starves the mouse no
//! matter what the slave mask says (the `0xFC` arithmetic bug, fixed and
//! documented on [`unmask_input`]).
//!
//! Every mask write is followed by [`log_masks`], which reads the OCW1
//! registers back off ports `0x21`/`0xA1` and prints the *hardware's*
//! answer — see there for why echoing the intended value would have hid
//! this exact bug.

use core::sync::atomic::{AtomicBool, Ordering};

use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::{
    instructions::port::Port,
    registers::control::Cr2,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode},
};

use crate::{event, gdt, input, timer};

/// Master PIC vector base after remapping.
pub const PIC_1_OFFSET: u8 = 32;
/// Slave PIC vector base (the master owns 8 vectors: `32..40`).
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

/// The kernel's chained 8259 PICs, guarded against concurrent access.
///
/// SAFETY (construction): `ChainedPics::new` is `unsafe` because the caller
/// must promise the two offsets don't collide with CPU exceptions (`0..32`);
/// 32 and 40 sit above that range, and nothing else reprograms the PICs.
pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

/// Hardware interrupt vectors delivered through the PICs.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    /// IRQ0 — PIT timer. The only line unmasked in Phase 1.
    Timer = PIC_1_OFFSET,
    /// IRQ1 — PS/2 keyboard (master PIC input 1). Phase 3.
    Keyboard = PIC_1_OFFSET + 1,
    /// IRQ12 — PS/2 mouse. Sits on the *slave* PIC (input 4),
    /// so its vector is `PIC_2_OFFSET + 4 = 44`. Phase 3.
    Mouse = PIC_2_OFFSET + 4,
}

impl InterruptIndex {
    /// The IDT vector this interrupt is delivered on.
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            // SAFETY: `DOUBLE_FAULT_IST_INDEX` points at IST1, a dedicated
            // 40 KiB stack installed in gdt.rs, so a double fault never
            // overflows the (possibly corrupted) normal stack.
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.general_protection_fault
            .set_handler_fn(general_protection_fault_handler);
        idt[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::Mouse.as_u8()].set_handler_fn(mouse_interrupt_handler);
        idt
    };
}

/// Loads the IDT into the CPU.
pub fn init_idt() {
    IDT.load();
}

/// Remaps both PICs above the exception range and unmasks IRQ0 only.
///
/// The masks written here are read back and logged by [`log_masks`]
/// immediately afterwards, so the serial log opens with the phase-1
/// baseline (`master=0xFE slave=0xFF`) the Phase 3 [`unmask_input`]
/// line can then be compared against.
pub fn init_pic() {
    // SAFETY: this kernel exclusively owns the PICs; `initialize` only
    // writes their command/data ports to set the vector offsets (32/40).
    unsafe { PICS.lock().initialize() };
    // SAFETY: writing interrupt masks is safe at any time. 0xFE =
    // 1111_1110 unmasks IRQ0 on the master (bit clear = enabled); 0xFF
    // masks every slave line — keyboard IRQ1 stays masked until Phase 3.
    // (Bit2 — the cascade — is set here, i.e. masked: that's fine at this
    // stage because the slave is fully masked too, so it never raises one.
    // `unmask_input` is what has to clear it.)
    unsafe { PICS.lock().write_masks(0xFE, 0xFF) };
    // Phase-1 baseline, read back from the hardware rather than echoed —
    // see `log_masks` for why that distinction is the whole point.
    log_masks("init_pic phase-1");
}

/// Unmasks the PS/2 input lines (Phase 3): IRQ1 (keyboard, master input 1)
/// and IRQ12 (mouse, slave input 4). Timer IRQ0 stays unmasked; everything
/// else stays masked.
///
/// The master mask is `0xF8` = `1111_1000`:
///
/// * bit0 clear — IRQ0 (PIT timer) stays enabled, as in phase 1;
/// * bit1 clear — IRQ1 (keyboard, master input 1) enabled;
/// * **bit2 clear — IRQ2, the slave PIC's *cascade* line, enabled**;
/// * bits3..7 set — IRQ3..IRQ7 stay masked (COM ports, LPT, floppy, IDE).
///
/// The slave mask is `0xEF` = `1110_1111`: only bit4 clear — IRQ12 (mouse)
/// enabled, IRQ8/9/10/11/13/14/15 masked.
///
/// The cascade bit is the entire reason this value is what it is. Every
/// slave interrupt — IRQ12 included — reaches the CPU *through* master
/// IRQ2; with bit2 set on the master mask the slave PIC is unreachable
/// however permissive its own mask is, so `mouse irq count` stays `0`
/// forever while the machine otherwise looks perfectly healthy. **Keyboard
/// activity proves nothing about the cascade**: IRQ1 lives on the master
/// PIC and never crosses it, so a working keyboard alongside a dead mouse
/// is exactly the symptom a masked IRQ2 produces (observed in both
/// VirtualBox and QEMU).
///
/// This is also why the old value was wrong in a way that read as right:
/// `0xFC` = `1111_1100` clears bit0 and bit1 (timer + keyboard — both
/// demonstrably work) but leaves **bit2 set**, and the "obvious"
/// alternative `0xFD` = `1111_1101` leaves bit2 set *too*. Both of the
/// plausible-looking candidates mask the cascade; only `0xF8` clears it.
/// The neighbouring values agreeing on the one bit that mattered is
/// precisely the trap that shipped a mouse which never fired an IRQ.
///
/// Called by [`crate::input::init`] only after both devices have been
/// initialised *and* their IDT handlers registered — an unmasked line
/// with no handler would triple-fault. The caller logs the masks back
/// from the hardware via [`log_masks`] right after this returns
/// (expected: `master=0xF8 slave=0xEF`).
pub fn unmask_input() {
    // Master 0xF8 = 1111_1000: IRQ0 (timer) + IRQ1 (keyboard) + IRQ2
    // (cascade) unmasked; IRQ3..7 still masked. The bit2 clear is the
    // fix — without it no slave IRQ, IRQ12 included, can reach the CPU.
    // Slave 0xEF = 1110_1111: only IRQ12 (mouse) unmasked.
    // SAFETY: same ownership as `init_pic`; pure mask write, no side
    // effects beyond (de)livering already-pending requests.
    unsafe { PICS.lock().write_masks(0xF8, 0xEF) };
}

/// Reads both PIC mask registers back **off the hardware** and logs them
/// as one greppable line:
///
/// ```text
/// [pic    ] masks master=0xF8 slave=0xEF (input::init phase-3)
/// ```
///
/// Uppercase, zero-padded hex; `context` names the call site so the two
/// boot-time readings stay tellable apart — `init_pic phase-1` must show
/// `master=0xFE slave=0xFF` (IRQ0 only), `input::init phase-3` must show
/// `master=0xF8 slave=0xEF` (cascade open, IRQ12 open). Anything else in
/// those positions names the exact bit that is wrong.
///
/// **Why read back instead of printing what we wrote**: the bug this
/// diagnostic was added for was a mask whose *arithmetic* was wrong while
/// the code's intent (per its comment) was right — a log line echoing the
/// argument it was about to write would have printed `0xFC` and looked
/// convincing forever. [`ChainedPics::initialize`] also rewrites the masks
/// at the end (it restores whatever it saved on entry), so intended and
/// actual values can diverge without a read-back ever noticing. This
/// prints what the chips are really running.
///
/// The read is a plain OCW1 fetch from the data ports — master `0x21`,
/// slave `0xA1`, the same two reads `ChainedPics::read_masks` performs.
/// Reading the 8259's data port returns the currently-programmed OCW1
/// (interrupt mask) register and has **no side effects**: no command is
/// latched, no interrupt is acknowledged or disturbed, nothing to
/// synchronise against. Deliberately done with raw [`Port`]s rather than
/// `PICS.lock()`, so the diagnostic can never deadlock against a context
/// that already holds the PIC mutex.
pub fn log_masks(context: &str) {
    /// Master PIC data port — reads return OCW1 (IRQ0..IRQ7 mask).
    const MASTER_DATA: u16 = 0x21;
    /// Slave PIC data port — reads return OCW1 (IRQ8..IRQ15 mask).
    const SLAVE_DATA: u16 = 0xA1;

    // SAFETY: ring 0 with I/O privilege; `0x21`/`0xA1` are the standard
    // 8259 data-port addresses and a *read* only returns the OCW1 mask
    // register (documented above — no side effects), so this cannot
    // disturb PIC state or pending interrupts.
    let (master, slave) = unsafe {
        (
            Port::<u8>::new(MASTER_DATA).read(),
            Port::<u8>::new(SLAVE_DATA).read(),
        )
    };
    serial_println!("[pic    ] masks master=0x{master:02X} slave=0x{slave:02X} ({context})");
}

// ---------------------------- CPU exceptions -----------------------------

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    // Recoverable by design (a debugger may be attached) — log and continue.
    serial_println!("[irq    ] EXCEPTION: breakpoint\n{stack_frame:#?}");
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    serial_println!("[irq    ] EXCEPTION: DOUBLE FAULT (error {error_code:#x})\n{stack_frame:#?}");
    panic!("double fault — halting");
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    // `read_raw` (safe) never fails, unlike `Cr2::read`, which rejects
    // non-canonical values — exactly what a wild pointer can produce.
    let cr2 = Cr2::read_raw();
    serial_println!(
        "[irq    ] EXCEPTION: page fault — cr2 = {cr2:#x}, error = {error_code:?}\n{stack_frame:#?}"
    );
    panic!("unhandled page fault at address {cr2:#x}");
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    serial_println!(
        "[irq    ] EXCEPTION: general protection fault (error {error_code:#x})\n{stack_frame:#?}"
    );
    panic!("general protection fault");
}

// -------------------------- hardware interrupts --------------------------

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    timer::increment();

    // Phase 5: update sleeping threads on timer tick
    crate::sched::timer_tick(timer::ticks());

    // Phase 3: publish the tick to the event queue so the desktop loop
    // can drive animations/heartbeat off `Event::Tick` instead of
    // re-polling the counter every pass (see `crate::event` docs).
    event::push(event::Event::Tick(timer::ticks()));

    // One-time proof line: the log shows the very first delivered tick
    // without flooding the capture at 100 Hz.
    static FIRST_TICK: AtomicBool = AtomicBool::new(false);
    if !FIRST_TICK.swap(true, Ordering::Relaxed) {
        serial_println!("[irq    ] first PIT tick delivered (IRQ0 -> vector 32)");
    }

    // SAFETY: end-of-interrupt tells both PICs this interrupt is finished —
    // without it, no further IRQs are delivered. The offset matches the
    // remap in `init_pic`, and this handler only ever runs for IRQ0.
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8())
    };
}

/// IRQ1 (vector 33): one byte is waiting in the controller's data
/// register — hand it to the scancode decoder, then acknowledge the PIC.
extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    input::keyboard::handle_irq();

    // SAFETY: EOI for IRQ1 (master vector 33) — matches the remap in
    // `init_pic`; without it the master PIC would mask the line forever.
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8())
    };
}

/// IRQ12 (vector 44): one byte of a mouse packet is ready. The slave
/// PIC's IRQ12 is *cascaded* through master IRQ2 — `notify_end_of_interrupt`
/// knows that and EOIs both chips for vectors >= 40.
extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    input::mouse::handle_irq();

    // SAFETY: EOI for IRQ12 (slave vector 44). `notify_end_of_interrupt`
    // sees a slave-range vector and EOIs the slave PIC, then the master's
    // cascade line (IRQ2) — required or no further slave IRQ fires.
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Mouse.as_u8())
    };
}
