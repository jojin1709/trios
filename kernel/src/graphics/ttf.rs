//! TTF Font rendering engine with pre-cached glyphs and alpha blending (Phase 19).
//!
//! Embeds Inter-Regular (for UI) and JetBrainsMono-Regular (for code/terminals).
//! Maintains a fast direct-indexed ASCII glyph cache for 60+ FPS zero-latency rendering.

use alloc::vec::Vec;
use fontdue::{Font, FontSettings};
use spin::Mutex;

use super::color::Color;
use super::framebuffer::Graphics;

static INTER_BYTES: &[u8] = include_bytes!("../../fonts/Inter-Regular.ttf");
static MONO_BYTES: &[u8] = include_bytes!("../../fonts/JetBrainsMono-Regular.ttf");

/// Font family selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontType {
    UI,
    Mono,
}

/// Pre-rasterized glyph entry with sub-pixel offsets and alpha coverage bitmap.
#[derive(Clone)]
pub struct CachedGlyph {
    pub width: usize,
    pub height: usize,
    pub xmin: i32,
    pub ymin: i32,
    pub advance_width: i32,
    pub bitmap: Vec<u8>,
}

/// Static typography engine with pre-rasterized ASCII caches (0x20..=0x7E).
pub struct TypeEngine {
    #[allow(dead_code)]
    pub ui_font: Font,
    #[allow(dead_code)]
    pub mono_font: Font,
    pub ui_cache_14: [CachedGlyph; 96],
    pub mono_cache_14: [CachedGlyph; 96],
}

impl TypeEngine {
    pub fn new() -> Self {
        let settings = FontSettings::default();
        let ui_font = Font::from_bytes(INTER_BYTES, settings)
            .expect("failed to parse Inter-Regular.ttf");
        let mono_font = Font::from_bytes(MONO_BYTES, settings)
            .expect("failed to parse JetBrainsMono-Regular.ttf");

        let default_glyph = CachedGlyph {
            width: 0,
            height: 0,
            xmin: 0,
            ymin: 0,
            advance_width: 8,
            bitmap: Vec::new(),
        };

        let mut ui_cache_14: [CachedGlyph; 96] = core::array::from_fn(|_| default_glyph.clone());
        let mut mono_cache_14: [CachedGlyph; 96] = core::array::from_fn(|_| default_glyph.clone());

        let size = 14.0;
        for i in 0..96 {
            let ch = (32 + i) as u8 as char;

            // Rasterize UI font
            let (metrics, bitmap) = ui_font.rasterize(ch, size);
            ui_cache_14[i] = CachedGlyph {
                width: metrics.width,
                height: metrics.height,
                xmin: metrics.xmin,
                ymin: metrics.ymin,
                advance_width: (metrics.advance_width + 0.5) as i32,
                bitmap,
            };

            // Rasterize Mono font
            let (m_metrics, m_bitmap) = mono_font.rasterize(ch, size);
            mono_cache_14[i] = CachedGlyph {
                width: m_metrics.width,
                height: m_metrics.height,
                xmin: m_metrics.xmin,
                ymin: m_metrics.ymin,
                advance_width: (m_metrics.advance_width + 0.5) as i32,
                bitmap: m_bitmap,
            };
        }

        Self {
            ui_font,
            mono_font,
            ui_cache_14,
            mono_cache_14,
        }
    }

    pub fn get_glyph(&self, font: FontType, ch: char) -> Option<&CachedGlyph> {
        let code = ch as usize;
        if (32..128).contains(&code) {
            let idx = code - 32;
            match font {
                FontType::UI => Some(&self.ui_cache_14[idx]),
                FontType::Mono => Some(&self.mono_cache_14[idx]),
            }
        } else {
            None
        }
    }
}

pub static ENGINE: Mutex<Option<TypeEngine>> = Mutex::new(None);

/// Initializes the TTF font engine once at kernel boot.
pub fn init() {
    let mut lock = ENGINE.lock();
    if lock.is_none() {
        *lock = Some(TypeEngine::new());
    }
}

impl Graphics {
    /// Draws anti-aliased TTF text using pre-cached glyphs and smooth alpha blending.
    pub fn draw_text_ttf(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        fg: Color,
        font_type: FontType,
    ) {
        let lock = ENGINE.lock();
        let Some(engine) = lock.as_ref() else {
            // Fallback to legacy bitmap font if TTF engine not yet initialized
            self.draw_text(x, y, text, fg, None, 1);
            return;
        };

        let mut cursor_x = x;
        // Ascent line for 14px font is approximately 11px
        let baseline_y = y + 11;

        let stride = self.stride;
        let screen_w = self.width as i32;
        let screen_h = self.height as i32;
        let is_bgr = matches!(self.format, bootloader_api::info::PixelFormat::Bgr);

        let u32_slice = unsafe {
            core::slice::from_raw_parts_mut(
                self.back.as_mut_ptr() as *mut u32,
                self.back.len() / 4,
            )
        };

        for ch in text.chars() {
            if ch == ' ' {
                cursor_x += if font_type == FontType::Mono { 8 } else { 5 };
                continue;
            }

            if let Some(glyph) = engine.get_glyph(font_type, ch) {
                let gx0 = cursor_x + glyph.xmin;
                let gy0 = baseline_y - glyph.ymin - (glyph.height as i32);

                for gy in 0..glyph.height {
                    let py = gy0 + gy as i32;
                    if py < 0 || py >= screen_h {
                        continue;
                    }
                    let row_offset = py as usize * stride;

                    for gx in 0..glyph.width {
                        let px = gx0 + gx as i32;
                        if px < 0 || px >= screen_w {
                            continue;
                        }

                        let coverage = glyph.bitmap[gy * glyph.width + gx];
                        if coverage == 0 {
                            continue;
                        }

                        let pix_idx = row_offset + px as usize;
                        let dst_packed = u32_slice[pix_idx];

                        // Extract destination RGB channels based on framebuffer format
                        let (dst_r, dst_g, dst_b) = if is_bgr {
                            (
                                ((dst_packed >> 16) & 0xFF) as u32,
                                ((dst_packed >> 8) & 0xFF) as u32,
                                (dst_packed & 0xFF) as u32,
                            )
                        } else {
                            (
                                (dst_packed & 0xFF) as u32,
                                ((dst_packed >> 8) & 0xFF) as u32,
                                ((dst_packed >> 16) & 0xFF) as u32,
                            )
                        };

                        // Alpha blend
                        let a = coverage as u32;
                        let inv_a = 255 - a;
                        let out_r = (fg.r as u32 * a + dst_r * inv_a) / 255;
                        let out_g = (fg.g as u32 * a + dst_g * inv_a) / 255;
                        let out_b = (fg.b as u32 * a + dst_b * inv_a) / 255;

                        // Pack back
                        let packed = if is_bgr {
                            (out_r << 16) | (out_g << 8) | out_b
                        } else {
                            (out_b << 16) | (out_g << 8) | out_r
                        };

                        u32_slice[pix_idx] = packed;
                    }
                }

                cursor_x += glyph.advance_width.max(1);
            } else {
                cursor_x += 8;
            }
        }
    }

    /// Measures width of TTF text string in pixels.
    pub fn measure_ttf(&self, text: &str, font_type: FontType) -> i32 {
        let lock = ENGINE.lock();
        let Some(engine) = lock.as_ref() else {
            return (text.len() * 8) as i32;
        };

        let mut w = 0;
        for ch in text.chars() {
            if ch == ' ' {
                w += if font_type == FontType::Mono { 8 } else { 5 };
                continue;
            }
            if let Some(glyph) = engine.get_glyph(font_type, ch) {
                w += glyph.advance_width.max(1);
            } else {
                w += 8;
            }
        }
        w
    }
}
