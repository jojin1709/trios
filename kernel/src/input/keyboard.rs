//! PS/2 keyboard driver — scancode set 1 decoding → [`KeyEvent`]s.
//!
//! ### What the hardware actually sends
//!
//! When you press a key, the keyboard controller puts one byte on the
//! wire: the key's *make* code. Release it and the same code comes back
//! with bit 7 set: the *break* code. That's the whole protocol — e.g.
//! pressing `A` sends `0x1E`, releasing sends `0x9E` (`0x1E | 0x80`).
//!
//! A few keys prefix their code with `0xE0` (the *extended* byte) to
//! escape the 7-bit code space: arrow keys, right Ctrl/Alt, Delete, etc.
//! An extended break is `0xE0` followed by `code | 0x80`.
//!
//! ### Where modifiers live
//!
//! Shift/Ctrl/Alt *are* keys, so they produce make/break codes like any
//! other — the driver flips bits in a shared [`AtomicU8`] as they arrive.
//! CapsLock differs: it's a toggle, so its make code *flips* the caps bit
//! (its break does nothing — otherwise the bit would cancel itself out).
//! Every [`KeyEvent`] carries a snapshot of the resulting modifier state,
//! plus the layout-resolved character: `Shift+A` is queued as
//! `KeyCode::Char('A')`, plain `A` as `'a'`.
//!
//! ### The layout
//!
//! US-QWERTY, hardcoded as two matches in [`map_code`] — no tables to
//! allocate, no locale machinery (Phase 5+ concerns). The keyboard
//! speaks *scancodes*; only this module knows they exist. Everything
//! downstream sees [`crate::event::KeyEvent`].

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use super::{read_data, wait_input_empty, write_data};
use crate::event::{Event, KeyCode, KeyEvent, Modifiers, push as push_event};

/// Modifier bit positions inside [`MODS`].
const M_SHIFT: u8 = 0b0001;
const M_CTRL: u8 = 0b0010;
const M_ALT: u8 = 0b0100;
const M_CAPS: u8 = 0b1000;

/// Current modifier state. Only ever touched from IRQ1 context (single
/// producer), so plain atomics suffice — no lock, no reentrancy worry.
static MODS: AtomicU8 = AtomicU8::new(0);

/// Set when an `0xE0` prefix byte was seen, consumed by the next byte.
/// Also IRQ1-context only.
static EXTENDED: AtomicBool = AtomicBool::new(false);

/// Brings the keyboard side of the controller up: enables scancode
/// streaming with `0xF4` and waits for the device's `0xFA` ACK.
///
/// The IRQ line itself is unmasked later, by [`super::init`] — this
/// function only talks to the device.
pub fn init() -> Result<(), &'static str> {
    // 0xF4 = "enable data reporting / start scanning" — the device
    // answers with 0xFA if it understood us.
    wait_input_empty()?;
    write_data(0xF4);
    super::wait_ack()?;
    Ok(())
}

/// IRQ1 handler body: read the byte that triggered the interrupt and
/// turn it into a queued [`Event::Key`]. Called from
/// `crate::interrupts::keyboard_interrupt_handler` (interrupts off).
///
/// One byte per IRQ — that's the contract; `handle_irq` must never loop
/// on the data port here (the controller re-raises IRQ1 per byte anyway).
pub fn handle_irq() {
    let byte = read_data();
    decode(byte);
}

/// State machine for one scancode byte.
fn decode(byte: u8) {
    // -- extended-prefix bookkeeping -------------------------------------
    if byte == 0xE0 || byte == 0xE1 {
        // 0xE1 (Pause) also arrives as a multi-byte sequence; treating it
        // as a prefix is imperfect (Pause may emit one stray Unknown key)
        // but harmless — documented in the module docs.
        EXTENDED.store(true, Ordering::SeqCst);
        return;
    }
    let extended = EXTENDED.swap(false, Ordering::SeqCst);

    // -- make vs break ---------------------------------------------------
    // Bit 7 set = break (key released); the physical code is low 7 bits.
    let released = byte & 0x80 != 0;
    let code = byte & 0x7F;

    // -- modifier bookkeeping (before mapping: the snapshot must reflect
    //    *this* event, e.g. pressing Shift then A in quick succession) ----
    let mut mods = MODS.load(Ordering::SeqCst);
    match code {
        0x2A | 0x36 => {
            // Left/right Shift — held key: mirror press/release exactly.
            mods = set_bit(mods, M_SHIFT, !released);
        }
        0x1D => {
            // Ctrl (E0 1D = right Ctrl decodes the same — fine for us).
            mods = set_bit(mods, M_CTRL, !released);
        }
        0x38 => {
            // Alt (E0 38 = right Alt likewise).
            mods = set_bit(mods, M_ALT, !released);
        }
        0x3A => {
            // CapsLock — toggle on *press* only.
            if !released {
                mods ^= M_CAPS;
            }
        }
        _ => {}
    }
    MODS.store(mods, Ordering::SeqCst);

    // -- decode + queue ---------------------------------------------------
    let key = map_code(code, extended, mods);
    push_event(Event::Key(KeyEvent {
        code: key,
        pressed: !released,
        modifiers: modifiers_from(mods),
    }));
}

const fn set_bit(bits: u8, mask: u8, on: bool) -> u8 {
    if on { bits | mask } else { bits & !mask }
}

const fn modifiers_from(bits: u8) -> Modifiers {
    Modifiers {
        shift: bits & M_SHIFT != 0,
        ctrl: bits & M_CTRL != 0,
        alt: bits & M_ALT != 0,
        caps: bits & M_CAPS != 0,
    }
}

/// Translates (physical code, extended flag, modifiers) into a
/// layout-independent [`KeyCode`].
fn map_code(code: u8, extended: bool, mods: u8) -> KeyCode {
    if extended {
        // E0-prefixed keys: cursor cluster + a few editing keys.
        return match code {
            0x48 => KeyCode::ArrowUp,
            0x50 => KeyCode::ArrowDown,
            0x4B => KeyCode::ArrowLeft,
            0x4D => KeyCode::ArrowRight,
            0x53 => KeyCode::Delete,
            0x47 => KeyCode::Home,
            0x4F => KeyCode::End,
            _ => KeyCode::Unknown(code),
        };
    }

    match code {
        // Non-printable singles.
        0x01 => KeyCode::Esc,
        0x0E => KeyCode::Backspace,
        0x0F => KeyCode::Tab,
        0x1C => KeyCode::Enter,
        // Modifier keys announce themselves too (a future text field
        // needs to know Ctrl is held).
        0x2A | 0x36 => KeyCode::Shift,
        0x1D => KeyCode::Ctrl,
        0x38 => KeyCode::Alt,
        0x3A => KeyCode::CapsLock,
        _ => match base_char(code) {
            Some(ch) => KeyCode::Char(apply_case(ch, mods)),
            None => KeyCode::Unknown(code),
        },
    }
}

/// Unshifted US-QWERTY character for a physical code (scancode set 1).
/// Letters are lowercase here; [`apply_case`] resolves shift/CapsLock.
fn base_char(code: u8) -> Option<char> {
    Some(match code {
        // Number row.
        0x02 => '1',
        0x03 => '2',
        0x04 => '3',
        0x05 => '4',
        0x06 => '5',
        0x07 => '6',
        0x08 => '7',
        0x09 => '8',
        0x0A => '9',
        0x0B => '0',
        0x0C => '-',
        0x0D => '=',
        // QWERTY row.
        0x10 => 'q',
        0x11 => 'w',
        0x12 => 'e',
        0x13 => 'r',
        0x14 => 't',
        0x15 => 'y',
        0x16 => 'u',
        0x17 => 'i',
        0x18 => 'o',
        0x19 => 'p',
        0x1A => '[',
        0x1B => ']',
        // Home row.
        0x1E => 'a',
        0x1F => 's',
        0x20 => 'd',
        0x21 => 'f',
        0x22 => 'g',
        0x23 => 'h',
        0x24 => 'j',
        0x25 => 'k',
        0x26 => 'l',
        0x27 => ';',
        0x28 => '\'',
        0x29 => '`',
        0x2B => '\\',
        // Bottom row.
        0x2C => 'z',
        0x2D => 'x',
        0x2E => 'c',
        0x2F => 'v',
        0x30 => 'b',
        0x31 => 'n',
        0x32 => 'm',
        0x33 => ',',
        0x34 => '.',
        0x35 => '/',
        0x39 => ' ',
        _ => return None,
    })
}

/// Shifted glyph for a non-alphabetic base character (`1` → `!`, ...).
fn shifted(ch: char) -> char {
    match ch {
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        '`' => '~',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        _ => ch,
    }
}

/// Applies Shift and CapsLock the way a US keyboard does:
/// * letters — `shift XOR caps` picks the case;
/// * everything else — Shift swaps to the shifted glyph (CapsLock never
///   affects them, exactly like real hardware).
fn apply_case(ch: char, mods: u8) -> char {
    let shift = mods & M_SHIFT != 0;
    let caps = mods & M_CAPS != 0;
    if ch.is_ascii_alphabetic() {
        if shift ^ caps {
            ch.to_ascii_uppercase()
        } else {
            ch
        }
    } else if shift {
        shifted(ch)
    } else {
        ch
    }
}
