//! The TriOS boot splash — Phase 2's showcase ("First Pixels").
//!
//! Runs after every Phase 1 subsystem is up. Three acts:
//!
//! 1. **Chrome** — logo, title, subtitle, palette background. One full
//!    flush; from here the screen is ours (the bootloader's text logger is
//!    wiped by the first `clear`).
//! 2. **Status reveal** — the init checklist draws one line at a time,
//!    paced by the PIT tick counter (no sleeps: we `hlt` between ticks and
//!    keep the serial heartbeat alive throughout Phase 1's contract).
//! 3. **Demo + idle** — a ~6 second animation (color-cycling gradient bar,
//!    bouncing ball, live uptime counter) proving double-buffered drawing
//!    holds up at tick rate; then a settled idle frame whose uptime text
//!    is updated once per second via *dirty-rect* flushes — a deliberate
//!    teaser of the compositor techniques Phase 3 needs.
//!
//! Layout is computed from the reported resolution, so the same code looks
//! right at VirtualBox's 800x600, a 1024x768 default, or anything else GOP
//! hands us. Nothing is hardcoded to a particular mode.

use alloc::format;
use alloc::string::String;

use crate::timer;

use super::color::{self, Color};
use super::font::{GLYPH_H, GLYPH_W};
use super::framebuffer::Graphics;

/// How long the animated demo runs, in PIT ticks (~6 s at 100 Hz).
const DEMO_TICKS: u64 = 6 * timer::TICKS_PER_SECOND;

/// Init steps shown in the checklist. Order matches the real boot order.
const STATUS_STEPS: [&str; 5] = ["Serial", "GDT/IDT", "Timer", "Heap", "Framebuffer"];

/// Everything the splash layout needs, computed once from the resolution.
///
/// Vertical rhythm, top to bottom: logo → title → subtitle → demo arena →
/// gradient bar → uptime line → init checklist. The checklist sits at the
/// very bottom so the animated ball never collides with text, and drops
/// into two columns on wide screens so five lines always fit.
struct Layout {
    cx: i32,
    /// Logo disc center + radius.
    logo_cy: i32,
    logo_r: i32,
    /// Title baseline (top-left y of the text cell).
    title_y: i32,
    title_scale: u32,
    subtitle_y: i32,
    subtitle_scale: u32,
    /// Checklist grid: origin, cell size, and column count/stride.
    status_x: i32,
    status_y: i32,
    status_scale: u32,
    status_line_h: i32,
    status_cols: usize,
    status_col_w: i32,
    /// Demo band: gradient bar rectangle.
    bar_x: i32,
    bar_y: i32,
    bar_w: u32,
    bar_h: u32,
    /// Demo band: ball radius and the box it bounces around in.
    ball_r: i32,
    arena_x0: i32,
    arena_y0: i32,
    arena_x1: i32,
    arena_y1: i32,
    /// Static "Hello, TriOS" line position.
    hello_y: i32,
    hello_scale: u32,
    /// Live uptime counter position (also the idle-frame slot).
    uptime_x: i32,
    uptime_y: i32,
    uptime_scale: u32,
}

impl Layout {
    /// Derives a pleasing, resolution-adaptive layout. Every threshold
    /// below is chosen so the composition still reads at 800x600 (the
    /// smallest mode VirtualBox commonly offers) and scales gracefully up.
    /// All vertical slots are fractions of the height, so the proportions
    /// hold at any 4:3-ish or widescreen resolution GOP hands us.
    fn compute(g: &Graphics) -> Self {
        let w = g.width() as i32;
        let h = g.height() as i32;
        let cx = w / 2;

        // Title: 5 glyphs wide, so pick the largest integer scale that
        // keeps "TriOS" comfortably inside the screen (with margin).
        let title_scale = match w {
            ..1000 => 4,     // 160 px wide
            1000..1600 => 5, // 200 px
            _ => 6,          // 240 px
        };
        let logo_r = (h / 12).clamp(18, 52);
        let logo_cy = h * 3 / 10;
        let title_y = logo_cy + logo_r + h / 28;
        let subtitle_scale = if w >= 900 { 2 } else { 1 };
        let subtitle_y = title_y + (GLYPH_H * title_scale) as i32 + h / 50;

        // Demo band across the lower-middle: gradient bar centered, the
        // ball arena squeezed between the subtitle and the bar.
        let bar_w = (w as u32 * 3 / 5).max(160);
        let bar_h = (h / 48).clamp(8, 14) as u32;
        let bar_x = (w - bar_w as i32) / 2;
        let bar_y = h * 7 / 10;
        let ball_r = (h / 36).clamp(8, 22);
        let arena_x0 = ball_r;
        let arena_x1 = w - ball_r;
        let arena_y0 = subtitle_y + (GLYPH_H * subtitle_scale) as i32 + h / 30;
        let arena_y1 = bar_y - ball_r - h / 80;

        let hello_scale = if w >= 1100 { 3 } else { 2 };
        let hello_y = arena_y0 + (arena_y1 - arena_y0) / 4;

        // Uptime line under the bar; checklist under the uptime line,
        // left-aligned with it, in two columns when there's width to spare
        // (three rows max — always fits between the uptime and the bezel).
        let status_scale = if w >= 800 { 2 } else { 1 };
        let uptime_scale = status_scale;
        let uptime_y = bar_y + bar_h as i32 + h / 40;
        let status_line_h = (GLYPH_H * status_scale) as i32 + h / 48;
        let status_y = uptime_y + status_line_h;
        let status_x = w / 8;
        let status_cols = if w >= 700 { 2 } else { 1 };
        let status_col_w = (w - 2 * status_x) / status_cols as i32;

        Layout {
            cx,
            logo_cy,
            logo_r,
            title_y,
            title_scale,
            subtitle_y,
            subtitle_scale,
            status_x,
            status_y,
            status_scale,
            status_line_h,
            status_cols,
            status_col_w,
            bar_x,
            bar_y,
            bar_w,
            bar_h,
            ball_r,
            arena_x0,
            arena_y0,
            arena_x1,
            arena_y1,
            hello_y,
            hello_scale,
            uptime_x: status_x,
            uptime_y,
            uptime_scale,
        }
    }
}

/// Runs the full splash sequence, then never returns: the idle loop below
/// doubles as the kernel's steady state (serial heartbeat included).
pub fn run(g: &mut Graphics) -> ! {
    let layout = Layout::compute(g);

    draw_chrome(g, &layout);
    g.flush();
    serial_println!(
        "[gfx    ] splash: chrome drawn ({}x{})",
        g.width(),
        g.height()
    );

    reveal_status(g, &layout);
    run_demo(g, &layout);

    // Settled idle frame — full redraw, then per-second dirty-rect updates.
    draw_idle(g, &layout);
    idle(g, &layout)
}

/// Act 1: palette background, logo, title, subtitle. Assumes a blank
/// backbuffer (fresh boot) so no overpainting is needed.
fn draw_chrome(g: &mut Graphics, l: &Layout) {
    g.clear(color::BG);

    // Logo: a filled accent disc with a white "T" monogram carved from two
    // rectangles — TriOS's first (very temporary) brand mark. Phase 3 can
    // replace this with vector art; the point here is that a disc is two
    // primitives and the monogram is two more.
    g.fill_circle(l.cx, l.logo_cy, l.logo_r, color::ACCENT);
    let t_thickness = (l.logo_r / 4).max(3);
    let t_bar_w = l.logo_r + t_thickness; // crossbar slightly wider than tall bar
    let t_bar_y = l.logo_cy - l.logo_r / 2;
    g.fill_rect(
        l.cx - t_bar_w / 2,
        t_bar_y,
        t_bar_w as u32,
        t_thickness as u32,
        color::FG,
    );
    g.fill_rect(
        l.cx - t_thickness / 2,
        t_bar_y,
        t_thickness as u32,
        (l.logo_r + l.logo_r / 2) as u32,
        color::FG,
    );

    // Title + subtitle, centered by measuring the rendered width.
    let title = "TriOS";
    let title_w = Graphics::text_width(title, l.title_scale);
    g.draw_text(
        l.cx - title_w / 2,
        l.title_y,
        title,
        color::TITLE,
        None,
        l.title_scale,
    );

    // ASCII hyphen: the 8x8 font predates em-dashes.
    let subtitle = "v0.1.0 - Phase 2: First Pixels";
    let sub_w = Graphics::text_width(subtitle, l.subtitle_scale);
    g.draw_text(
        l.cx - sub_w / 2,
        l.subtitle_y,
        subtitle,
        color::DIM,
        None,
        l.subtitle_scale,
    );
}

/// Draws checklist entry `i` in its grid cell (3+2 split across two
/// columns when the screen is wide, a single column otherwise).
fn draw_status_entry(g: &mut Graphics, l: &Layout, i: usize) {
    let (col, row) = if l.status_cols >= 2 {
        (i / 3, i % 3)
    } else {
        (0, i)
    };
    let x = l.status_x + col as i32 * l.status_col_w;
    let y = l.status_y + row as i32 * l.status_line_h;
    // Hand-drawn checkmark: two Bresenham lines. (The font is ASCII
    // only — U+2714 CHECK MARK isn't in it — and drawing our own
    // doubles as a primitive showcase.)
    draw_check(g, x, y, l.status_scale, color::OK);
    let text_x = x + ((GLYPH_W + 4) * l.status_scale) as i32;
    g.draw_text(text_x, y, STATUS_STEPS[i], color::FG, None, l.status_scale);
}

/// Act 2: checklist reveal, one line per few ticks. Paced by the PIT —
/// `wait_ticks` halts the CPU until the next interrupt, so we burn zero
/// CPU between lines and the serial heartbeat keeps flowing.
fn reveal_status(g: &mut Graphics, l: &Layout) {
    for i in 0..STATUS_STEPS.len() {
        draw_status_entry(g, l, i);
        g.flush();
        // ~80 ms stagger: readable as a sequence, over in half a second.
        timer::wait_ticks(8);
    }
}

/// A checkmark built from two strokes, fitting a `scale`-sized text cell.
fn draw_check(g: &mut Graphics, x: i32, y: i32, scale: u32, color: Color) {
    let s = scale as i32;
    // Short down-right stroke, then a longer up-right stroke.
    g.draw_line(x, y + 3 * s, x + 2 * s, y + 5 * s, color);
    g.draw_line(x + 2 * s, y + 5 * s, x + 7 * s, y + s, color);
}

/// Act 3a: the ~6 s animation. Frame loop driven by tick changes (draws at
/// most once per 10 ms tick, `hlt`s otherwise) — the pattern Phase 3's
/// compositor will generalize into a scheduler.
fn run_demo(g: &mut Graphics, l: &Layout) {
    serial_println!("[gfx    ] splash: demo start ({DEMO_TICKS} ticks)");

    let hello = "Hello, TriOS";
    let hello_w = Graphics::text_width(hello, l.hello_scale);
    let hello_x = l.cx - hello_w / 2;

    // Ball state: integer positions/velocities (px per tick). The ball is
    // *not* cleared by a full-screen wipe each frame — that would defeat
    // the point of double buffering — so we erase its previous position
    // manually before drawing the new one (and redraw the greeting it may
    // have passed over). The bar is cheap enough to repaint wholesale.
    let (mut bx, mut by) = ((l.arena_x0 + l.arena_x1) / 2, l.arena_y0 + 4);
    let (mut vx, mut vy) = (3, 2);
    let mut last_uptime_text = String::new();
    let start = timer::ticks();
    let mut last_tick = u64::MAX;

    while timer::ticks().saturating_sub(start) < DEMO_TICKS {
        let now = timer::ticks();
        if now == last_tick {
            // No new tick yet — park the CPU until the next interrupt.
            x86_64::instructions::hlt();
            continue;
        }
        last_tick = now;

        // -- physics: move, then bounce off the arena walls --
        let (prev_bx, prev_by) = (bx, by);
        bx += vx;
        by += vy;
        if bx <= l.arena_x0 || bx >= l.arena_x1 {
            vx = -vx;
            bx += vx; // un-stick from the wall
        }
        if by <= l.arena_y0 || by >= l.arena_y1 {
            vy = -vy;
            by += vy;
        }

        // -- draw the frame (double-buffered: none of this is visible yet)
        // Erase the previous ball, restore anything it overwrote, then
        // stamp the new position.
        g.fill_circle(prev_bx, prev_by, l.ball_r, color::BG);
        g.draw_text(hello_x, l.hello_y, hello, color::FG, None, l.hello_scale);
        g.fill_circle(bx, by, l.ball_r, color::ACCENT);

        // Gradient bar: per-column hue swept by time — the color-cycling
        // effect, done in pure integer math (see color::hsv).
        let phase = (now * 360 / timer::TICKS_PER_SECOND) as u32;
        for col in 0..l.bar_w {
            let hue = (col * 360 / l.bar_w).wrapping_add(phase) % 360;
            let c = color::hsv(hue, 220, 235);
            g.fill_rect(l.bar_x + col as i32, l.bar_y, 1, l.bar_h, c);
        }

        // Live uptime counter — repainted only when the second changes.
        let uptime = format!("uptime: {} s", timer::uptime_seconds());
        if uptime != last_uptime_text {
            let w = Graphics::text_width("uptime: 999999 s", l.uptime_scale);
            g.fill_rect(
                l.uptime_x,
                l.uptime_y,
                w as u32,
                Graphics::text_height(l.uptime_scale) as u32,
                color::BG,
            );
            g.draw_text(
                l.uptime_x,
                l.uptime_y,
                &uptime,
                color::DIM,
                None,
                l.uptime_scale,
            );
            last_uptime_text = uptime;
        }

        // One full present per tick. (At 1024x768 this is a ~3 MiB memcpy
        // per 10 ms — roughly 1.5% duty cycle even in software emulation.)
        g.flush();
    }

    serial_println!("[gfx    ] splash: demo end");
}

/// Act 3b: the settled idle frame — splash chrome plus a "ready" message,
/// drawn from scratch (the demo's ball/bar are wiped).
fn draw_idle(g: &mut Graphics, l: &Layout) {
    draw_chrome(g, l);
    for i in 0..STATUS_STEPS.len() {
        draw_status_entry(g, l, i);
    }
    let ready = "Ready for Phase 3";
    let ready_w = Graphics::text_width(ready, l.uptime_scale + 1);
    g.draw_text(
        l.cx - ready_w / 2,
        l.arena_y0 + (l.arena_y1 - l.arena_y0) / 3,
        ready,
        color::ACCENT,
        None,
        l.uptime_scale + 1,
    );
    // First uptime line is part of the full frame.
    let uptime = format!("uptime: {} s", timer::uptime_seconds());
    g.draw_text(
        l.uptime_x,
        l.uptime_y,
        &uptime,
        color::DIM,
        None,
        l.uptime_scale,
    );
    g.flush();
    serial_println!("[gfx    ] splash: idle — Ready for Phase 3");
}

/// Steady state: serial heartbeat (unchanged from Phase 1's contract) plus
/// an on-screen uptime counter refreshed once per second with a dirty-rect
/// flush — only the text band crosses the bus, not the whole frame.
fn idle(g: &mut Graphics, l: &Layout) -> ! {
    let mut reported = 0u64;
    // Pre-size the dirty band to the widest string we'll ever draw, so a
    // 5-digit uptime never outgrows the repainted region.
    let band_w = Graphics::text_width("uptime: 999999 s", l.uptime_scale) as u32;
    let band_h = Graphics::text_height(l.uptime_scale) as u32;
    loop {
        let uptime = timer::uptime_seconds();
        if uptime > reported {
            reported = uptime;
            // Phase 1 heartbeat line — verbatim, so serial-based tooling
            // and the README's verified log keep working unchanged.
            serial_println!(
                "[timer  ] heartbeat: uptime {uptime} s, ticks={}",
                timer::ticks()
            );
            // Overpaint the old text with the palette background, draw the
            // new one, ship only those rows to the hardware.
            g.fill_rect(l.uptime_x, l.uptime_y, band_w, band_h, color::BG);
            let text = format!("uptime: {uptime} s");
            g.draw_text(
                l.uptime_x,
                l.uptime_y,
                &text,
                color::DIM,
                None,
                l.uptime_scale,
            );
            g.flush_rect(l.uptime_x, l.uptime_y, band_w, band_h);
        }
        x86_64::instructions::hlt();
    }
}
