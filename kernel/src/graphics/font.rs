//! Bitmap text rendering on top of the [`Graphics`] canvas.
//!
//! ### The font
//!
//! [`font8x8::legacy::BASIC_LEGACY`] is an 8x8-pixel monochrome font covering
//! ASCII `0x00..0x80`, with a pleasantly chunky "classic PC console" look.
//! Provenance (no licensing landmines):
//!
//! * glyph data: public domain — extracted from an x86 assembly font, then
//!   ported to a public-domain C header by dhepper/font8x8;
//! * crate wrapper: MIT (see `font8x8` crate LICENSE).
//!
//! Each glyph is 8 bytes, one per row; within a byte, **bit 0 (LSB) is the
//! leftmost pixel** — the classic PC font ordering. We index the table by
//! the ASCII code, so `BASIC_LEGACY[b'A' as usize]` is the letter A.
//!
//! ### Scaling
//!
//! Bitmap fonts scale by *pixel duplication*: every source pixel becomes a
//! `scale x scale` block. Ugly by typographic standards, but perfectly
//! crisp at integer scales (no anti-aliasing, no hinting, no float math)
//! — exactly what a boot splash wants. Scale 1 = 8x8 px cells, scale 6 =
//! a 48-px-tall title, and so on. Non-integer scales would need a real
//! vector font, which is firmly a Phase 5+ concern.

use font8x8::legacy::BASIC_LEGACY;

use super::color::Color;
use super::framebuffer::Graphics;

/// Width in pixels of one glyph cell at scale 1.
pub const GLYPH_W: u32 = 8;
/// Height in pixels of one glyph cell at scale 1.
pub const GLYPH_H: u32 = 8;

impl Graphics {
    /// Draws one character's cell at (x, y).
    ///
    /// * `fg` — color of "on" pixels;
    /// * `bg` — `Some(color)` fills the whole 8x8 cell first (opaque text),
    ///   `None` leaves the background visible between strokes (transparent
    ///   text — the default for overlays);
    /// * `scale` — integer pixel-duplication factor (1..=N; clamped to >=1).
    ///
    /// Characters outside the ASCII range are skipped (advance handled by
    /// [`Graphics::draw_text`]) — Phase 2 is deliberately ASCII-only.
    pub fn draw_char(
        &mut self,
        x: i32,
        y: i32,
        ch: char,
        fg: Color,
        bg: Option<Color>,
        scale: u32,
    ) {
        let scale = scale.max(1);
        let cell = GLYPH_W * scale;
        let Some(code) = ch.try_into().ok().filter(|&c: &u8| (c as usize) < 128) else {
            // Out of the table's range — draw a placeholder box so layout
            // gaps don't silently swallow characters (tofu, in font terms).
            self.draw_rect(x, y, cell, GLYPH_H * scale, fg);
            return;
        };
        if let Some(bg) = bg {
            self.fill_rect(x, y, cell, GLYPH_H * scale, bg);
        }
        let glyph = BASIC_LEGACY[code as usize];
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                // LSB-left bit order (see module docs).
                if (bits >> col) & 1 == 0 {
                    continue;
                }
                let px = x + col * scale as i32;
                let py = y + (row as i32) * scale as i32;
                self.fill_rect(px, py, scale, scale, fg);
            }
        }
    }

    /// Draws a single line of ASCII text at (x, y) — no wrapping, no
    /// kerning beyond the font's built-in 8-px cells. See [`Graphics::draw_char`]
    /// for the background/scale semantics.
    pub fn draw_text(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        fg: Color,
        bg: Option<Color>,
        scale: u32,
    ) {
        let scale = scale.max(1);
        let advance = (GLYPH_W * scale) as i32;
        let mut cursor = x;
        for ch in text.chars() {
            self.draw_char(cursor, y, ch, fg, bg, scale);
            cursor += advance;
        }
    }

    /// Width in pixels of `text` drawn at `scale` — for centering and
    /// dirty-rect sizing without measuring twice.
    pub fn text_width(text: &str, scale: u32) -> i32 {
        text.chars().count() as i32 * (GLYPH_W * scale.max(1)) as i32
    }

    /// Height in pixels of one text line at `scale`.
    pub fn text_height(scale: u32) -> i32 {
        (GLYPH_H * scale.max(1)) as i32
    }
}
