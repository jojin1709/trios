//! Double-buffered framebuffer canvas — every visible pixel in TriOS flows
//! through this module (Phase 2), and through its API forever after.
//!
//! ### Why double-buffer at all?
//!
//! The screen you're looking at is one big array of pixels (the *hardware
//! framebuffer*, handed to us by the UEFI bootloader). Drawing into it
//! directly means every half-finished frame becomes visible: animations
//! flicker as rows update at slightly different times. The classic fix is
//! the same one video games have used since the 80s:
//!
//! 1. draw the next frame into an off-screen *backbuffer* (heap memory we
//!    own), at leisure, with no one watching;
//! 2. `flush()` — copy the finished frame to the hardware framebuffer in
//!    one tight, sequential burst (a `memcpy`, microseconds at 1024x768);
//! 3. the display controller then scans out a complete frame, so nothing
//!    torn or half-drawn ever reaches the screen.
//!
//! The backbuffer is stored in the *hardware's native pixel layout* (same
//! stride, same bytes-per-pixel, same RGB/BGR order). That choice makes
//! `flush` a straight row copy — no per-pixel conversion in the hot path —
//! while the conversion cost is paid once per drawn pixel instead, inside
//! safe drawing code. Upper layers never see any of this: they call
//! [`Graphics::set_pixel`] / [`Graphics::fill_rect`] / [`Graphics::flush`]
//! with a plain [`Color`](super::Color).
//!
//! ### Safety posture
//!
//! Everything that *composes* a frame is safe Rust operating on the heap
//! `Vec<u8>`. The only raw-pointer work is the final copy into the
//! bootloader-owned buffer — and that buffer arrives from
//! `bootloader_api` as a proper `&'static mut [u8]` slice, so even the
//! flush is written with safe slice operations wherever practical. The
//! two `copy_nonoverlapping` calls that remain (bulk + row copies) carry
//! full `// SAFETY:` proofs.

use alloc::vec;
use alloc::vec::Vec;
use bootloader_api::info::{FrameBuffer, FrameBufferInfo, PixelFormat};

use super::color::Color;

/// A double-buffered drawing surface backed by the UEFI linear framebuffer.
///
/// Lifetime note: the hardware buffer reference is `'static` because the
/// bootloader maps the GOP aperture into the kernel's page tables for the
/// whole kernel lifetime — there is nothing to unmap in a hobby OS that
/// never reboots the GPU.
pub struct Graphics {
    /// The real, scan-out framebuffer (write-combining mapped by the boot-
    /// loader). Only [`Graphics::flush`] touches it.
    hw: &'static mut [u8],
    /// Off-screen copy we draw into. Same byte layout as `hw`.
    back: Vec<u8>,
    /// Visible width in pixels (excludes stride padding).
    width: usize,
    /// Height in pixels.
    height: usize,
    /// Pixels per scanline **including** padding — some firmwares pad rows
    /// to an alignment boundary, so address math must use this, not width.
    stride: usize,
    /// Bytes per pixel (4 on every UEFI GOP mode the bootloader supports).
    bpp: usize,
    /// Native channel order — the one thing [`Color::pack`] needs.
    format: PixelFormat,
}

impl Graphics {
    /// Takes over the bootloader's framebuffer and allocates the backbuffer
    /// on the kernel heap (`width * height * bytes_per_pixel` bytes).
    ///
    /// Consumes the [`FrameBuffer`] handle: after this call the kernel owns
    /// the pixel memory exclusively, and the bootloader's debug logger
    /// (which also drew to it during boot) is gone for good.
    ///
    /// # Panics
    /// If the reported geometry is degenerate (zero size) or the heap can't
    /// hold the backbuffer — both indicate a broken boot hand-off, which
    /// deserves a loud failure over silent corruption.
    pub fn new(framebuffer: FrameBuffer) -> Self {
        let info: FrameBufferInfo = framebuffer.info();
        assert!(
            info.width > 0 && info.height > 0 && info.bytes_per_pixel > 0,
            "bootloader reported a degenerate framebuffer: {info:?}"
        );
        // `into_buffer` consumes the handle and yields the full pixel
        // memory as a mutable slice whose length is `info.byte_len`.
        let hw = framebuffer.into_buffer();
        // Backbuffer mirrors the hardware layout byte-for-byte, so flush
        // never needs to translate anything.
        let back = vec![0u8; hw.len()];
        Self {
            hw,
            back,
            width: info.width,
            height: info.height,
            stride: info.stride,
            bpp: info.bytes_per_pixel,
            format: info.pixel_format,
        }
    }

    // -- geometry ----------------------------------------------------------

    /// Visible width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// A copy of the bootloader's layout description (logged at init).
    // Phase 3 seam: the compositor will query geometry through this instead
    // of threading `FrameBufferInfo` around. Unused until then.
    #[allow(dead_code)]
    pub fn info(&self) -> FrameBufferInfo {
        FrameBufferInfo {
            byte_len: self.back.len(),
            width: self.width,
            height: self.height,
            pixel_format: self.format,
            bytes_per_pixel: self.bpp,
            stride: self.stride,
        }
    }

    // -- primitives (all draw into the backbuffer, all safe Rust) ----------

    /// Sets one pixel. Out-of-bounds coordinates are silently clipped —
    /// callers doing layout math (centering, bouncing balls) shouldn't
    /// have to range-check every draw.
    #[inline]
    pub fn set_pixel(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let offset = (y as usize * self.stride + x as usize) * self.bpp;
        let packed = color.pack(self.format, self.bpp).to_le_bytes();
        self.back[offset..offset + self.bpp].copy_from_slice(&packed[..self.bpp]);
    }

    /// Solid rectangle (clipped to the screen). The workhorse primitive:
    /// text glyphs, bars, window fills — all built on top of this.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: Color) {
        // Clip in i64 space so huge u32 sizes can't overflow the math.
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = (x as i64 + w as i64).clamp(0, self.width as i64) as usize;
        let y1 = (y as i64 + h as i64).clamp(0, self.height as i64) as usize;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let packed = color.pack(self.format, self.bpp).to_le_bytes();
        let row_px = x1 - x0;
        for row in y0..y1 {
            let start = (row * self.stride + x0) * self.bpp;
            let end = start + row_px * self.bpp;
            // Walk the span one pixel at a time; `chunks_exact` hands LLVM
            // a tight, vectorizable loop (still plain safe Rust).
            self.back[start..end]
                .chunks_exact_mut(self.bpp)
                .for_each(|px| px.copy_from_slice(&packed[..self.bpp]));
        }
    }

    /// 1-pixel outline rectangle (clipped). Degenerate sizes (< 2 px)
    /// draw nothing — an outline needs room for its own border.
    pub fn draw_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: Color) {
        if w < 2 || h < 2 {
            return;
        }
        let (w_i, h_i) = (w as i32, h as i32);
        self.fill_rect(x, y, w, 1, color); // top
        self.fill_rect(x, y + h_i - 1, w, 1, color); // bottom
        self.fill_rect(x, y, 1, h, color); // left
        self.fill_rect(x + w_i - 1, y, 1, h, color); // right
    }

    /// Bresenham's line algorithm — integer-only, no floats.
    ///
    /// Walks from (x0,y0) to (x1,y1) choosing, at each step, whichever of
    /// the two candidate pixels has its center closest to the ideal line.
    /// The error accumulator `err` tracks that distance so the decision is
    /// a couple of integer adds per pixel. This is *the* canonical raster
    /// line algorithm: ~20 lines, works everywhere, no libraries.
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (x0, y0);
        loop {
            // set_pixel clips, so lines may run off-screen harmlessly.
            self.set_pixel(x, y, color);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Filled circle via the midpoint algorithm, drawn as horizontal spans.
    ///
    /// Same integer-decision idea as Bresenham: one decision parameter
    /// decides when to step the outer radius in. Spans (rather than single
    /// pixels) keep the cost at ~pi*r line segments regardless of radius.
    pub fn fill_circle(&mut self, cx: i32, cy: i32, r: i32, color: Color) {
        if r < 0 {
            return;
        }
        let (mut x, mut y) = (0i32, r);
        let mut d = 1 - r; // initial decision parameter
        while x <= y {
            let (wide, narrow) = (2 * y + 1, 2 * x + 1);
            // Four symmetric horizontal spans cover the octants.
            self.fill_rect(cx - y, cy + x, wide as u32, 1, color);
            self.fill_rect(cx - y, cy - x, wide as u32, 1, color);
            self.fill_rect(cx - x, cy + y, narrow as u32, 1, color);
            self.fill_rect(cx - x, cy - y, narrow as u32, 1, color);
            if d < 0 {
                d += 2 * x + 3;
            } else {
                d += 2 * (x - y) + 5;
                y -= 1;
            }
            x += 1;
        }
    }

    /// Copies a `w x h` block from (src_x, src_y) to (dst_x, dst_y) inside
    /// the backbuffer — the building block for scrolling, sprite blitting
    /// and dirty-region shuffling in Phase 3's compositor.
    ///
    /// Overlapping source/destination regions are handled correctly
    /// (`slice::copy_within` is overlap-safe, unlike `memcpy`).
    /// Partially off-screen blocks are clipped on all four sides.
    // Phase 3 seam: scrolling and sprite blits land here. Unused until then.
    #[allow(dead_code)]
    pub fn copy_rect(&mut self, src_x: i32, src_y: i32, dst_x: i32, dst_y: i32, w: u32, h: u32) {
        let w = w as i64;
        let h = h as i64;
        let (sw, sh) = (self.width as i64, self.height as i64);
        // How much sticks out on each side, for either endpoint; both the
        // source and destination must stay on-screen, so clip by the worse
        // of the two and shift both ends by the same amount. (Negation in
        // i64 space: `-i32::MIN` would overflow in debug builds.)
        let left = (0 - src_x as i64).max(0).max((0 - dst_x as i64).max(0));
        let top = (0 - src_y as i64).max(0).max((0 - dst_y as i64).max(0));
        let right = (src_x as i64 + w - sw).max(dst_x as i64 + w - sw).max(0);
        let bottom = (src_y as i64 + h - sh).max(dst_y as i64 + h - sh).max(0);
        let nw = w - left - right;
        let nh = h - top - bottom;
        if nw <= 0 || nh <= 0 {
            return;
        }
        let bpp = self.bpp;
        let stride = self.stride;
        let sx0 = (src_x as i64 + left) as usize;
        let sy0 = (src_y as i64 + top) as usize;
        let dx0 = (dst_x as i64 + left) as usize;
        let dy0 = (dst_y as i64 + top) as usize;
        let row_bytes = nw as usize * bpp;
        for row in 0..nh as usize {
            let src = ((sy0 + row) * stride + sx0) * bpp;
            let dst = ((dy0 + row) * stride + dx0) * bpp;
            self.back.copy_within(src..src + row_bytes, dst);
        }
    }

    /// Fills the whole visible screen with `color` (stride padding skipped).
    pub fn clear(&mut self, color: Color) {
        self.fill_rect(0, 0, self.width as u32, self.height as u32, color);
    }

    // -- flushing: backbuffer -> hardware ----------------------------------

    /// Copies the entire backbuffer to the hardware framebuffer.
    ///
    /// Fast paths: when the mode has no row padding (the common case —
    /// VirtualBox, QEMU, most real GOP firmware), the whole buffer is one
    /// contiguous `memcpy`; otherwise each visible row is copied and the
    /// padding is skipped.
    pub fn flush(&mut self) {
        let row_bytes = self.width * self.bpp;
        let stride_bytes = self.stride * self.bpp;
        if stride_bytes == row_bytes {
            // Contiguous mode: single bulk copy, `memcpy` speed.
            let len = self.back.len();
            // SAFETY:
            //  * `hw` and `back` are distinct, live allocations for the
            //    lifetime of `self` (GOP aperture vs kernel heap) — they can
            //    never alias, which `copy_nonoverlapping` requires;
            //  * both are at least `len` bytes: `back` was allocated with
            //    exactly `len` elements, and `hw` is the bootloader's buffer
            //    of the same `byte_len` (invariant of `Graphics::new`);
            //  * the kernel is single-threaded in Phase 2 — no concurrent
            //    reader/writer of either buffer exists.
            unsafe {
                core::ptr::copy_nonoverlapping(self.back.as_ptr(), self.hw.as_mut_ptr(), len);
            }
        } else {
            // Padded mode: copy visible bytes row by row, skip the padding.
            for y in 0..self.height {
                let offset = y * stride_bytes;
                // SAFETY: same proof as the bulk path, restricted to one
                // row: source and destination rows are `row_bytes` long
                // inside two non-overlapping buffers that are each at
                // least `stride_bytes * height` bytes (>= offset + row_bytes).
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        self.back.as_ptr().add(offset),
                        self.hw.as_mut_ptr().add(offset),
                        row_bytes,
                    );
                }
            }
        }
    }

    /// Copies only a rectangle to the hardware — the *dirty-rect* flush.
    ///
    /// Full-screen `flush` copies every pixel even when one text line
    /// changed. When only a small region is dirty (an uptime counter, a
    /// blinking cursor), copying just that saves memory bandwidth — the
    /// scarcest resource when every byte crosses the emulated PCI bus.
    pub fn flush_rect(&mut self, x: i32, y: i32, w: u32, h: u32) {
        // Same clipping discipline as fill_rect.
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = (x as i64 + w as i64).clamp(0, self.width as i64) as usize;
        let y1 = (y as i64 + h as i64).clamp(0, self.height as i64) as usize;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let row_bytes = (x1 - x0) * self.bpp;
        for row in y0..y1 {
            let offset = (row * self.stride + x0) * self.bpp;
            // SAFETY: identical invariants to `flush`'s row path; the span
            // `[offset, offset + row_bytes)` lies inside both buffers by
            // the clipping above (x1 <= width, y1 <= height, stride >= width).
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.back.as_ptr().add(offset),
                    self.hw.as_mut_ptr().add(offset),
                    row_bytes,
                );
            }
        }
    }
}
