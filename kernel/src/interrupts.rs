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
//! Phase 1 unmasks only the timer line. Phase 3's [`crate::input::init`]
//! unmask IRQ1 + IRQ12 via [`unmask_input`] once both devices answer.

use core::sync::atomic::{AtomicBool, Ordering};

use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::{
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
pub fn init_pic() {
    // SAFETY: this kernel exclusively owns the PICs; `initialize` only
    // writes their command/data ports to set the vector offsets (32/40).
    unsafe { PICS.lock().initialize() };
    // SAFETY: writing interrupt masks is safe at any time. 0xFE unmasks IRQ0
    // on the master (bit clear = enabled); 0xFF masks every slave line —
    // keyboard IRQ1 stays masked until Phase 3.
    unsafe { PICS.lock().write_masks(0xFE, 0xFF) };
}

/// Unmasks the PS/2 input lines (Phase 3): IRQ1 (keyboard, master input 1)
/// and IRQ12 (mouse, slave input 4). Timer IRQ0 stays unmasked; everything
/// else stays masked.
///
/// Called by [`crate::input::init`] only after both devices have been
/// initialised *and* their IDT handlers registered — an unmasked line
/// with no handler would triple-fault.
/// Unmasks the PS/2 input lines (Phase 3): IRQ1 (keyboard, master input 1)
/// and IRQ12 (mouse, slave input 4). Timer IRQ0 stays unmasked; everything
/// else stays masked.
///
/// The master mask is `0xFC`, **not** `0xFD`: bit2 is the slave PIC's
/// *cascade* line (IRQ2), and no slave interrupt — IRQ12 included — can
/// reach the CPU with it masked. Worse, on VirtualBox a slave request that
/// can never be serviced leaves the cascade permanently asserted and
/// wedges PIC delivery entirely (IRQ0 stops firing too — caught by the
/// post-`sti` tick-test panic during bring-up).
///
/// Called by [`crate::input::init`] only after both devices have been
/// initialised *and* their IDT handlers registered — an unmasked line
/// with no handler would triple-fault.
pub fn unmask_input() {
    // Master 0xFC = IRQ0 (timer) + IRQ1 (keyboard) + IRQ2 (cascade);
    // slave 0xEF = only IRQ12 (mouse) unmasked.
    // SAFETY: same ownership as `init_pic`; pure mask write, no side
    // effects beyond (de)livering already-pending requests.
    unsafe { PICS.lock().write_masks(0xFC, 0xEF) };
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
