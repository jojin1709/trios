//! Normalized 24-bit color — the one color type upper layers ever see.
//!
//! The firmware hands the kernel a framebuffer in whatever layout the GOP
//! mode uses (`Rgb`, `Bgr`, grayscale, or an exotic bit-packed format).
//! Instead of leaking that through every drawing call, we accept a plain
//! [`Color`] everywhere and convert **once**, at pixel-write time, via
//! [`Color::pack`]. Phase 3's window manager and Phase 4's shell can
//! therefore never get the red/blue channels swapped by accident.
//!
//! ### The classic framebuffer gotcha
//!
//! UEFI GOP on real hardware and most emulators reports `Bgr`: bytes in
//! memory are `[blue, green, red, alpha]`. Some firmware reports `Rgb`
//! (`[red, green, blue, alpha]`). Swap the two and every photo looks like
//! a photographic negative of Superman. [`Color::pack`] is the single
//! place that mapping lives — audited once, used everywhere.

use bootloader_api::info::PixelFormat;

/// A 24-bit RGB color (alpha is implied: framebuffers in Phase 2 are opaque).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    /// Channels straight up.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// From a `0xRRGGBB` hex literal — matches how colors are written in
    /// CSS/design tools, which is how the splash palette was chosen.
    pub const fn rgb(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xFF) as u8,
            g: ((hex >> 8) & 0xFF) as u8,
            b: (hex & 0xFF) as u8,
        }
    }

    /// Encodes this color as one packed pixel in the framebuffer's native
    /// layout — the value that gets stored at a pixel's byte offset.
    ///
    /// `bytes_per_pixel` may exceed 3 (GOP modes are usually 4 bytes/pixel
    /// with a padding/alpha byte); we return a 32-bit little-endian value
    /// whose low `bytes_per_pixel` bytes are the ones to store.
    pub(crate) fn pack(self, format: PixelFormat, bytes_per_pixel: usize) -> u32 {
        // Silence "field never read" on exotic targets; on our target every
        // arm reads the channels.
        let _ = bytes_per_pixel;
        match format {
            // Memory byte order: R, G, B, (pad) -> LE u32 = pad|B<<16|G<<8|R.
            PixelFormat::Rgb => u32::from_le_bytes([self.r, self.g, self.b, 0]),
            // Memory byte order: B, G, R, (pad) -> LE u32 = pad|R<<16|G<<8|B.
            // (VirtualBox's UEFI GOP lands here.)
            PixelFormat::Bgr => u32::from_le_bytes([self.b, self.g, self.r, 0]),
            // One-byte grayscale: luma via the BT.601 integer weights.
            PixelFormat::U8 => (self.r as u32 * 77 + self.g as u32 * 150 + self.b as u32 * 29) >> 8,
            // GOP bitmask mode: each channel has an explicit bit offset.
            // The bootloader never hands these over (it panics on Bitmask),
            // but handling them keeps `pack` total over the public enum.
            PixelFormat::Unknown {
                red_position,
                green_position,
                blue_position,
            } => {
                (self.r as u32) << red_position
                    | (self.g as u32) << green_position
                    | (self.b as u32) << blue_position
            }
            // `PixelFormat` is #[non_exhaustive] — a future firmware format
            // falls back to Bgr, the most common GOP layout. The boot log
            // prints the detected format so a mismatch is easy to spot.
            _ => u32::from_le_bytes([self.b, self.g, self.r, 0]),
        }
    }
}

// ---------------------------------------------------------------------------
// The TriOS splash palette — "GitHub dark"-flavoured: deep blue-black canvas,
// soft off-white text, one confident blue accent, green for "OK" marks.
// Phase 3 will promote these into a proper theme table.
// ---------------------------------------------------------------------------

/// Splash background — near-black with a hint of blue.
pub const BG: Color = Color::rgb(0x0D1117);
/// Primary text.
pub const FG: Color = Color::rgb(0xC9D1D9);
/// De-emphasised text (subtitles, counters).
pub const DIM: Color = Color::rgb(0x8B949E);
/// Accent blue (logo disc, highlights).
pub const ACCENT: Color = Color::rgb(0x2F81F7);
/// Title blue — slightly lighter than the accent for contrast on dark.
pub const TITLE: Color = Color::rgb(0x58A6FF);
/// Success green for init checkmarks.
pub const OK: Color = Color::rgb(0x3FB950);

/// Integer HSV -> RGB (hue in degrees `0..360`, sat/val `0..=255`).
///
/// Used by the splash's color-cycling gradient bar. Integer-only on purpose:
/// the kernel builds for `x86_64-unknown-none` with SSE disabled, so float
/// math would be soft-float — pointless cost for a decoration.
pub(crate) fn hsv(hue: u32, sat: u8, val: u8) -> Color {
    let hue = hue % 360;
    let v = val as u32;
    let c = v * sat as u32 / 255; // chroma
    // Position within the current 60-degree sector.
    let sector = hue / 60;
    let t = (hue % 60) as i32; // 0..=59
    // Triangle wave: rises 0..=30, falls 30..=0 -> the second-largest channel.
    let x = c * (30 - (t - 30).unsigned_abs()) / 30;
    let m = v - c; // match: shared residual to lift all channels
    let (r, g, b) = match sector {
        0 => (c, x, 0),
        1 => (x, c, 0),
        2 => (0, c, x),
        3 => (0, x, c),
        4 => (x, 0, c),
        _ => (c, 0, x),
    };
    Color::new((r + m) as u8, (g + m) as u8, (b + m) as u8)
}
