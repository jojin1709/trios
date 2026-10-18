//! Kernel event model — input event types + the shared event queue (Phase 3).
//!
//! ### Why an event queue?
//!
//! Phase 1/2 code was purely *polled*: the main loop read the tick counter
//! and nothing else. Input changes that — a key press or mouse packet
//! arrives **asynchronously**, from IRQ context, whenever the human feels
//! like it. Two designs are possible:
//!
//! 1. *Do the work in the IRQ handler* (draw directly, move windows from
//!    inside the interrupt). Tempting, but wrong: handlers must be short,
//!    must not take locks the main loop holds, and reentrancy bugs there
//!    are near-impossible to debug.
//! 2. *Record the event, handle it in the main loop* — the classic
//!    producer/consumer split. IRQ handlers do the minimum (decode a
//!    scancode, stash an event) and the main loop drains the queue at its
//!    own pace. This is what TriOS does.
//!
//! ### Locking discipline (the deadlock lesson)
//!
//! The queue is a [`spin::Mutex`] around a **fixed-size ring buffer** —
//! never grows, so no heap allocation ever happens while the lock is held
//! (allocating from IRQ context could deadlock against the main loop
//! holding the heap lock).
//!
//! Every lock acquisition is wrapped like [`crate::serial::write_fmt`]:
//!
//! * **Main loop**: `cli` → lock → work → unlock → `sti`. While it holds
//!   the lock, interrupts are off, so no IRQ handler can ever spin on it.
//! * **IRQ handler**: the CPU already cleared `IF` on interrupt entry, so
//!   the lock it takes is guaranteed free (the only other holder is the
//!   main loop, which ran with `IF=0` while holding it).
//!
//! Interrupts that arrive during the main loop's `cli` window are latched
//! by the 8259 and delivered right after `sti` — deferred, never lost.
//! This is exactly the pattern `serial.rs` uses for the COM1 lock.
//!
//! ### Queue overflow
//!
//! Capacity is finite ([`CAPACITY`]). If a producer ever outruns the
//! consumer (e.g. a flood of mouse packets while we redraw), the *newest*
//! event is dropped and counted in [`dropped()`]; existing events are
//! never overwritten. At 100 ticks/s and a main loop that drains every
//! iteration, overflow should never happen in practice — the counter is
//! there so UAT can prove it.

use core::fmt;

use spin::Mutex;
use x86_64::instructions::interrupts;

/// Ring-buffer capacity (events). 512 ≈ 5 s of pure tick events at 100 Hz
/// — enough to cover the ~2 s of ticks that accumulate between `sti` and
/// the first drain of the desktop loop without dropping anything.
const CAPACITY: usize = 512;

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

/// A decoded key transition.
///
/// The scancode (hardware "key 0x1C went down") is translated to a layout-
/// independent [`KeyCode`] *before* the event is queued, with the current
/// modifier state baked in — so consumers never touch the PS/2 layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// Which key changed.
    pub code: KeyCode,
    /// `true` on press (make), `false` on release (break).
    pub pressed: bool,
    /// Modifier state **at the moment of the transition**.
    pub modifiers: Modifiers,
}

/// Layout-independent identity of a key — US-QWERTY characters plus the
/// non-printable keys the GUI needs (arrows, Esc, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    /// A printable character, already case/shift-resolved (`'a'`, `'A'`,
    /// `'!'`, `' '` ...). Shift+letter yields `'A'`, CapsLock XOR shift
    /// for letters, as on a real US keyboard.
    Char(char),
    Enter,
    Backspace,
    Tab,
    Esc,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Delete,
    Home,
    End,
    /// Modifier keys report their own press/release events too, so a
    /// future text-input field can track Ctrl/Alt combos.
    Shift,
    Ctrl,
    Alt,
    CapsLock,
    /// Windows / Super / GUI key (Phase 18).
    Super,
    /// A scancode we don't map (or a make/break sequence from an
    /// extended key we don't care about yet). Kept so callers can log
    /// unknowns instead of silently swallowing them.
    Unknown(u8),
}

impl fmt::Display for KeyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Enter => f.write_str("Enter"),
            KeyCode::Backspace => f.write_str("Backspace"),
            KeyCode::Tab => f.write_str("Tab"),
            KeyCode::Esc => f.write_str("Esc"),
            KeyCode::ArrowUp => f.write_str("ArrowUp"),
            KeyCode::ArrowDown => f.write_str("ArrowDown"),
            KeyCode::ArrowLeft => f.write_str("ArrowLeft"),
            KeyCode::ArrowRight => f.write_str("ArrowRight"),
            KeyCode::Delete => f.write_str("Delete"),
            KeyCode::Home => f.write_str("Home"),
            KeyCode::End => f.write_str("End"),
            KeyCode::Shift => f.write_str("Shift"),
            KeyCode::Ctrl => f.write_str("Ctrl"),
            KeyCode::Alt => f.write_str("Alt"),
            KeyCode::CapsLock => f.write_str("CapsLock"),
            KeyCode::Super => f.write_str("Super"),
            KeyCode::Unknown(b) => write!(f, "Key#{b:#04x}"),
        }
    }
}

/// Keyboard modifier state snapshot (see [`KeyEvent::modifiers`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// CapsLock is a *toggle*, not a held key — it lives here because a
    /// letter's case depends on `shift XOR caps`.
    pub caps: bool,
}

/// Mouse buttons carried with every [`MouseEvent`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseButtons {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
}

impl MouseButtons {
    /// From the PS/2 packet's low 3 bits: bit0 = left, bit1 = right,
    /// bit2 = middle (the controller's own encoding).
    pub const fn from_mask(mask: u8) -> Self {
        Self {
            left: mask & 0b001 != 0,
            right: mask & 0b010 != 0,
            middle: mask & 0b100 != 0,
        }
    }
}

/// One processed mouse packet (plus the button state it carried).
///
/// `dx`/`dy` are **screen-space** deltas: PS/2 reports +Y = up, the screen
/// has +Y = down, so the driver has already negated Y. Consumers just add
/// them to a position — no axis games downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub dx: i32,
    pub dy: i32,
    pub buttons: MouseButtons,
}

/// Everything the main loop reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    /// A PIT tick (published by the IRQ0 handler). `n` is the tick count
    /// at delivery — animations and the 1 Hz heartbeat ride on this.
    Tick(u64),
}

// ---------------------------------------------------------------------------
// The queue
// ---------------------------------------------------------------------------

/// Fixed-capacity ring buffer: `buf[tail]` is the oldest event,
/// `buf[(head + 1) % CAPACITY]` is the next free slot (one slot is always
/// left empty so `head == tail` unambiguously means "empty").
struct EventQueue {
    buf: [Option<Event>; CAPACITY],
    head: usize,
    tail: usize,
    /// Events rejected because the ring was full (diagnostic only).
    dropped: u64,
}

impl EventQueue {
    const fn new() -> Self {
        Self {
            // `Option<Event>: Copy` (every payload field is Copy), so the
            // repeat-expression is const-initializable — no runtime init.
            buf: [None; CAPACITY],
            head: 0,
            tail: 0,
            dropped: 0,
        }
    }

    fn push(&mut self, event: Event) -> bool {
        let next = (self.head + 1) % CAPACITY;
        if next == self.tail {
            // Full: drop the newcomer rather than evict an unhandled event.
            self.dropped += 1;
            return false;
        }
        self.buf[self.head] = Some(event);
        self.head = next;
        true
    }

    fn pop(&mut self) -> Option<Event> {
        if self.head == self.tail {
            return None;
        }
        let event = self.buf[self.tail].take();
        self.tail = (self.tail + 1) % CAPACITY;
        event
    }
}

static QUEUE: Mutex<EventQueue> = Mutex::new(EventQueue::new());

/// Runs `f` with the queue locked under the serial.rs discipline:
/// interrupts off for the whole critical section (see module docs).
///
/// `f` must be short and must not allocate — it runs with `IF=0` on the
/// main path and inside IRQ handlers on the producer path.
fn with_locked<R>(f: impl FnOnce(&mut EventQueue) -> R) -> R {
    let were_enabled = interrupts::are_enabled();
    interrupts::disable();
    let result = f(&mut QUEUE.lock());
    if were_enabled {
        interrupts::enable();
    }
    result
}

/// Enqueues one event (producers: IRQ handlers; consumer never pushes).
///
/// Returns `false` if the queue was full and the event was dropped
/// (counted in [`dropped`]). Safe to call from any context — IRQ handlers
/// get no re-enable (the CPU's `IRET` restores `IF` for them).
///
/// [`dropped`]: dropped
pub fn push(event: Event) -> bool {
    with_locked(|q| q.push(event))
}

/// Dequeues the oldest event, or `None` when the queue is empty.
/// (The main loop's only queue call — drains everything each pass.)
pub fn poll() -> Option<Event> {
    with_locked(|q| q.pop())
}

/// How many events were dropped to overflow since boot (0 = healthy).
pub fn dropped() -> u64 {
    with_locked(|q| q.dropped)
}
