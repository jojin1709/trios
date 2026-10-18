//! Graphics — TriOS's framebuffer foundation (Phase 2: "First Pixels").
//!
//! One display, one API, zero knowledge of pixel formats above this line.
//! The modules here turn the UEFI bootloader's raw linear framebuffer into
//! a safe, double-buffered drawing surface with text support:
//!
//! * [`color`] — normalized `Color { r, g, b }` + the splash palette.
//!   Everything RGB/BGR/grayscale-specific is converted in exactly one
//!   place ([`color::Color::pack`]).
//! * [`framebuffer`] — [`Graphics`], the double-buffered canvas: pixel and
//!   rectangle primitives, Bresenham lines, circles, blits, and the
//!   flush/dirty-rect present paths.
//! * [`font`] — 8x8 bitmap text (`font8x8`'s public-domain BASIC table)
//!   with integer pixel-duplication scaling.
//! * [`splash`] — the boot splash: logo, init checklist, animated demo,
//!   idle screen.
//!
//! ### Contract for Phase 3 (window manager) and beyond
//!
//! Upper layers draw with `set_pixel` / `fill_rect` / `draw_text` and call
//! `flush` (or `flush_rect` for damage regions). They must never reach
//! into raw pixels or assume a channel order — that's this module's job,
//! solved once. The compositor (`gui::wm`) owns the [`Graphics`] instance
//! in Phase 3; it replaced the Phase 2 splash idle loop with the
//! event-driven desktop in `main.rs::desktop_run`.

pub mod color;
pub mod font;
pub mod framebuffer;
// Phase 3 skipped the splash (the desktop lands directly); keep the module
// for its drawing helpers and a possible future boot animation — just not
// called from `main.rs` anymore, hence the dead-code allowance.
#[allow(dead_code)]
pub mod splash;
pub mod ttf;

// Flat re-exports so upper layers can `use graphics::{Color, Graphics}`.
// (Phase 3's window manager will live on these two names.)
#[allow(unused_imports)] // used via `graphics::Color` from Phase 3 onward
pub use color::Color;
pub use framebuffer::Graphics;
pub use ttf::FontType;
