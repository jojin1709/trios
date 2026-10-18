//! Procedural vector-style icon renderer for TriOS (Phase 18: Desktop Experience).
//!
//! Provides scalable procedural icons (32x32 and 48x48) drawn with rounded rects,
//! lines, geometric patterns, and TriOS theme tokens:
//! - Applications: This PC, Files, Editor, Terminal, Browser, SysMon, Mines, Settings, Trash
//! - File Types: Rust Source (.rs), Text Document (.txt), Executable (.elf), Directory Folder

use crate::graphics::{Color, Graphics};
use crate::gui::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    ThisPc,
    Files,
    Editor,
    Terminal,
    Browser,
    SysMon,
    Mines,
    Settings,
    Trash,
    FileRs,
    FileTxt,
    FileElf,
    Folder,
}

#[allow(dead_code)]
struct Canvas<'a>(&'a mut Graphics);

#[allow(dead_code)]
impl<'a> Canvas<'a> {
    #[inline(always)]
    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Color) {
        if w > 0 && h > 0 {
            self.0.fill_rect(x, y, w as u32, h as u32, color);
        }
    }
    #[inline(always)]
    pub fn draw_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Color) {
        if w > 0 && h > 0 {
            self.0.draw_rect(x, y, w as u32, h as u32, color);
        }
    }
    #[inline(always)]
    pub fn fill_rounded_rect(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32, color: Color) {
        if w > 0 && h > 0 {
            self.0.fill_rounded_rect(x, y, w as u32, h as u32, radius, color);
        }
    }
    #[inline(always)]
    pub fn draw_rounded_rect(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32, color: Color) {
        if w > 0 && h > 0 {
            self.0.draw_rounded_rect(x, y, w as u32, h as u32, radius, color);
        }
    }
    #[inline(always)]
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        self.0.draw_line(x0, y0, x1, y1, color);
    }
    #[inline(always)]
    pub fn draw_str_transparent(&mut self, x: i32, y: i32, text: &str, color: Color) {
        self.0.draw_text(x, y, text, color, None, 1);
    }
    #[inline(always)]
    pub fn set_pixel(&mut self, x: i32, y: i32, color: Color) {
        self.0.set_pixel(x, y, color);
    }
}

impl Icon {
    /// Draws the icon at the specified top-left coordinate `(x, y)` with dimension `size x size`.
    pub fn draw(&self, g: &mut Graphics, x: i32, y: i32, size: i32, theme: &Theme) {
        let mut cv = Canvas(g);
        match self {
            Icon::ThisPc => draw_this_pc(&mut cv, x, y, size, theme),
            Icon::Files | Icon::Folder => draw_folder(&mut cv, x, y, size, theme),
            Icon::Editor => draw_editor(&mut cv, x, y, size, theme),
            Icon::Terminal => draw_terminal(&mut cv, x, y, size, theme),
            Icon::Browser => draw_browser(&mut cv, x, y, size, theme),
            Icon::SysMon => draw_sysmon(&mut cv, x, y, size, theme),
            Icon::Mines => draw_mines(&mut cv, x, y, size, theme),
            Icon::Settings => draw_settings(&mut cv, x, y, size, theme),
            Icon::Trash => draw_trash(&mut cv, x, y, size, theme),
            Icon::FileRs => draw_file_rs(&mut cv, x, y, size, theme),
            Icon::FileTxt => draw_file_txt(&mut cv, x, y, size, theme),
            Icon::FileElf => draw_file_elf(&mut cv, x, y, size, theme),
        }
    }
}

/// Helper to scale relative coordinates from a nominal 48-grid.
#[inline(always)]
fn scale(val: i32, size: i32) -> i32 {
    (val * size) / 48
}

// ---------------------------------------------------------------------------
// 1. This PC (Modern High-Tech Monitor & Base)
// ---------------------------------------------------------------------------
fn draw_this_pc(g: &mut Canvas, x: i32, y: i32, size: i32, theme: &Theme) {
    let screen_w = scale(40, size);
    let screen_h = scale(28, size);
    let screen_x = x + (size - screen_w) / 2;
    let screen_y = y + scale(4, size);
    let r = scale(4, size).max(2);

    // Bezel
    g.fill_rounded_rect(screen_x, screen_y, screen_w, screen_h, r, Color::rgb(0x1C2540));
    g.draw_rounded_rect(screen_x, screen_y, screen_w, screen_h, r, theme.border);

    // Display glass
    let m = scale(3, size).max(1);
    g.fill_rounded_rect(
        screen_x + m,
        screen_y + m,
        screen_w - m * 2,
        screen_h - m * 2,
        (r - 1).max(1),
        Color::rgb(0x0E1726),
    );

    // Blue glow / mini desktop wallpaper inside display
    let inner_w = screen_w - m * 2;
    let inner_h = screen_h - m * 2;
    let glow_y = screen_y + m + inner_h / 2;
    g.fill_rect(screen_x + m + 2, glow_y, inner_w - 4, inner_h / 2 - 2, Color::rgb(0x132742));
    // Triad accent in mini-screen
    let mid_x = screen_x + screen_w / 2;
    g.draw_line(mid_x, screen_y + m + 4, mid_x - scale(4, size), glow_y, theme.accent_cyan);
    g.draw_line(mid_x - scale(4, size), glow_y, mid_x + scale(4, size), glow_y, theme.accent_violet);
    g.draw_line(mid_x + scale(4, size), glow_y, mid_x, screen_y + m + 4, theme.accent_cyan);

    // Stand neck
    let neck_w = scale(6, size).max(2);
    let neck_h = scale(6, size).max(2);
    let neck_x = x + (size - neck_w) / 2;
    let neck_y = screen_y + screen_h;
    g.fill_rect(neck_x, neck_y, neck_w, neck_h, Color::rgb(0x283556));

    // Stand base
    let base_w = scale(22, size);
    let base_h = scale(4, size).max(2);
    let base_x = x + (size - base_w) / 2;
    let base_y = neck_y + neck_h;
    g.fill_rounded_rect(base_x, base_y, base_w, base_h, 2, Color::rgb(0x334155));
    g.draw_rounded_rect(base_x, base_y, base_w, base_h, 2, theme.accent_cyan);
}

// ---------------------------------------------------------------------------
// 2. Files / Folder (Golden Amber Modern Directory Folder)
// ---------------------------------------------------------------------------
fn draw_folder(g: &mut Canvas, x: i32, y: i32, size: i32, _theme: &Theme) {
    let w = scale(42, size);
    let h = scale(32, size);
    let fx = x + (size - w) / 2;
    let fy = y + (size - h) / 2;
    let r = scale(4, size).max(2);

    let tab_w = scale(18, size);
    let tab_h = scale(8, size);

    // Back folder tab
    g.fill_rounded_rect(fx, fy, tab_w, tab_h, r, Color::rgb(0xD97706));

    // Back folder body
    g.fill_rounded_rect(fx, fy + scale(4, size), w, h - scale(4, size), r, Color::rgb(0xB45309));

    // Inside paper sheet peeking out
    let sheet_inset = scale(4, size);
    g.fill_rounded_rect(
        fx + sheet_inset,
        fy + scale(2, size),
        w - sheet_inset * 2,
        scale(10, size),
        2,
        Color::rgb(0xE2E8F0),
    );

    // Front folder flap (lighter vibrant amber with rounded corners)
    let flap_h = scale(24, size);
    let flap_y = fy + h - flap_h;
    g.fill_rounded_rect(fx, flap_y, w, flap_h, r, Color::rgb(0xF59E0B));
    g.draw_rounded_rect(fx, flap_y, w, flap_h, r, Color::rgb(0xFDE68A));

    // Subtle horizontal divider line on flap
    g.draw_line(
        fx + scale(4, size),
        flap_y + scale(5, size),
        fx + w - scale(4, size),
        flap_y + scale(5, size),
        Color::rgb(0xD97706),
    );
}

// ---------------------------------------------------------------------------
// 3. Editor (Clean Document with Folded Corner & Code Lines)
// ---------------------------------------------------------------------------
fn draw_editor(g: &mut Canvas, x: i32, y: i32, size: i32, theme: &Theme) {
    let w = scale(32, size);
    let h = scale(40, size);
    let dx = x + (size - w) / 2;
    let dy = y + (size - h) / 2;
    let r = scale(3, size).max(2);
    let fold = scale(10, size);

    // Main document body
    g.fill_rounded_rect(dx, dy, w, h, r, Color::rgb(0x1E293B));
    g.draw_rounded_rect(dx, dy, w, h, r, theme.border);

    // Folded top-right corner
    let fold_x = dx + w - fold;
    let fold_y = dy;
    g.fill_rect(fold_x, fold_y, fold, fold, Color::rgb(0x0F172A));
    g.draw_line(fold_x, dy, fold_x, dy + fold, theme.accent_cyan);
    g.draw_line(fold_x, dy + fold, dx + w, dy + fold, theme.accent_cyan);
    g.draw_line(fold_x, dy, dx + w, dy + fold, Color::rgb(0x334155));

    // Code lines in accent colors
    let line_x = dx + scale(5, size);
    let max_line_w = w - scale(10, size);

    // Line 1: Header / keyword (violet)
    let l1_y = dy + scale(8, size);
    g.fill_rounded_rect(line_x, l1_y, scale(12, size), scale(3, size).max(2), 1, theme.accent_violet);

    // Line 2: Function / statement (cyan)
    let l2_y = dy + scale(15, size);
    g.fill_rounded_rect(line_x, l2_y, max_line_w - scale(4, size), scale(3, size).max(2), 1, theme.accent_cyan);

    // Line 3: Indented code line (dim text)
    let l3_y = dy + scale(22, size);
    g.fill_rounded_rect(line_x + scale(4, size), l3_y, scale(16, size), scale(3, size).max(2), 1, theme.text_dim);

    // Line 4: Closing block (green)
    let l4_y = dy + scale(29, size);
    g.fill_rounded_rect(line_x, l4_y, scale(10, size), scale(3, size).max(2), 1, theme.success);
}

// ---------------------------------------------------------------------------
// 4. Terminal (Dark Sleek Console with Glowing `>_` Prompt)
// ---------------------------------------------------------------------------
fn draw_terminal(g: &mut Canvas, x: i32, y: i32, size: i32, theme: &Theme) {
    let w = scale(40, size);
    let h = scale(34, size);
    let tx = x + (size - w) / 2;
    let ty = y + (size - h) / 2;
    let r = scale(4, size).max(2);

    // Outer dark console shell
    g.fill_rounded_rect(tx, ty, w, h, r, Color::rgb(0x090D16));
    g.draw_rounded_rect(tx, ty, w, h, r, theme.accent_cyan);

    // Top window dots
    let dot_y = ty + scale(4, size);
    let dot_r = scale(2, size).max(1);
    g.fill_rect(tx + scale(5, size), dot_y, dot_r * 2, dot_r * 2, theme.danger);
    g.fill_rect(tx + scale(10, size), dot_y, dot_r * 2, dot_r * 2, theme.warning);
    g.fill_rect(tx + scale(15, size), dot_y, dot_r * 2, dot_r * 2, theme.success);

    // Console separator line
    let sep_y = ty + scale(8, size);
    g.draw_line(tx + 2, sep_y, tx + w - 3, sep_y, Color::rgb(0x1E293B));

    // `>` prompt in glowing cyan
    let p_x = tx + scale(6, size);
    let p_y = sep_y + scale(6, size);
    let p_sz = scale(5, size);
    g.draw_line(p_x, p_y, p_x + p_sz, p_y + p_sz, theme.accent_cyan);
    g.draw_line(p_x + p_sz, p_y + p_sz, p_x, p_y + p_sz * 2, theme.accent_cyan);

    // `_` cursor in glowing green/white
    let c_x = p_x + p_sz + scale(4, size);
    let c_y = p_y + p_sz * 2;
    let c_w = scale(7, size);
    g.fill_rect(c_x, c_y, c_w, scale(2, size).max(1), theme.success);
}

// ---------------------------------------------------------------------------
// 5. Browser (Glowing Globe with Latitude & Longitude)
// ---------------------------------------------------------------------------
fn draw_browser(g: &mut Canvas, x: i32, y: i32, size: i32, theme: &Theme) {
    let radius = scale(18, size);
    let cx = x + size / 2;
    let cy = y + size / 2;

    // Outer sphere background
    let r2 = radius * radius;
    for dy in -radius..=radius {
        let dx_max = crate::graphics::framebuffer::int_sqrt(r2 - dy * dy);
        let py = cy + dy;
        g.fill_rect(cx - dx_max, py, dx_max * 2 + 1, 1, Color::rgb(0x0F1F38));
    }

    // Outer circle rim in cyan
    for dy in -radius..=radius {
        let dx = crate::graphics::framebuffer::int_sqrt(r2 - dy * dy);
        g.fill_rect(cx - dx, cy + dy, 1, 1, theme.accent_cyan);
        g.fill_rect(cx + dx, cy + dy, 1, 1, theme.accent_cyan);
    }

    // Equator horizontal line
    g.draw_line(cx - radius + 1, cy, cx + radius - 1, cy, theme.accent_cyan);

    // North & South parallels
    let par_y = radius / 2;
    let dx_par = crate::graphics::framebuffer::int_sqrt(r2 - par_y * par_y);
    g.draw_line(cx - dx_par + 1, cy - par_y, cx + dx_par - 1, cy - par_y, Color::rgb(0x38BDF8));
    g.draw_line(cx - dx_par + 1, cy + par_y, cx + dx_par - 1, cy + par_y, Color::rgb(0x38BDF8));

    // Prime meridian vertical line
    g.draw_line(cx, cy - radius + 1, cx, cy + radius - 1, theme.accent_cyan);

    // Meridian ellipse
    let ell_w = radius / 2;
    for dy in -radius..=radius {
        let dx_outer = crate::graphics::framebuffer::int_sqrt(r2 - dy * dy);
        let dx_ell = (dx_outer * ell_w) / radius;
        g.fill_rect(cx - dx_ell, cy + dy, 1, 1, Color::rgb(0x818CF8));
        g.fill_rect(cx + dx_ell, cy + dy, 1, 1, Color::rgb(0x818CF8));
    }
}

// ---------------------------------------------------------------------------
// 6. SysMon (Telemetry Graph & Cardiac Pulse Wave)
// ---------------------------------------------------------------------------
fn draw_sysmon(g: &mut Canvas, x: i32, y: i32, size: i32, theme: &Theme) {
    let w = scale(40, size);
    let h = scale(34, size);
    let sx = x + (size - w) / 2;
    let sy = y + (size - h) / 2;
    let r = scale(4, size).max(2);

    // Background monitor card
    g.fill_rounded_rect(sx, sy, w, h, r, Color::rgb(0x0B1220));
    g.draw_rounded_rect(sx, sy, w, h, r, theme.border);

    // Grid lines inside
    let grid_color = Color::rgb(0x162038);
    let mid_y = sy + h / 2;
    g.draw_line(sx + 2, mid_y, sx + w - 3, mid_y, grid_color);
    g.draw_line(sx + 2, sy + scale(8, size), sx + w - 3, sy + scale(8, size), grid_color);
    g.draw_line(sx + 2, sy + h - scale(8, size), sx + w - 3, sy + h - scale(8, size), grid_color);
    g.draw_line(sx + w / 3, sy + 2, sx + w / 3, sy + h - 3, grid_color);
    g.draw_line(sx + (w * 2) / 3, sy + 2, sx + (w * 2) / 3, sy + h - 3, grid_color);

    // Pulse ECG wave in vibrant emerald green
    let p0 = (sx + scale(3, size), mid_y);
    let p1 = (sx + scale(12, size), mid_y);
    let p2 = (sx + scale(16, size), mid_y - scale(11, size));
    let p3 = (sx + scale(20, size), mid_y + scale(9, size));
    let p4 = (sx + scale(24, size), mid_y - scale(4, size));
    let p5 = (sx + scale(27, size), mid_y);
    let p6 = (sx + w - scale(3, size), mid_y);

    g.draw_line(p0.0, p0.1, p1.0, p1.1, theme.success);
    g.draw_line(p1.0, p1.1, p2.0, p2.1, theme.success);
    g.draw_line(p2.0, p2.1, p3.0, p3.1, theme.success);
    g.draw_line(p3.0, p3.1, p4.0, p4.1, theme.success);
    g.draw_line(p4.0, p4.1, p5.0, p5.1, theme.success);
    g.draw_line(p5.0, p5.1, p6.0, p6.1, theme.success);

    // Glowing pulse dot
    g.fill_rect(p2.0 - 1, p2.1 - 1, 3, 3, theme.accent_cyan);
}

// ---------------------------------------------------------------------------
// 7. Mines (TriSweeper Naval Mine & Red Detonator)
// ---------------------------------------------------------------------------
fn draw_mines(g: &mut Canvas, x: i32, y: i32, size: i32, theme: &Theme) {
    let cx = x + size / 2;
    let cy = y + size / 2;
    let r = scale(12, size);
    let r_spike = scale(18, size);

    // 8 spikes radiating outward
    let offsets: [(i32, i32); 8] = [
        (0, -1), (0, 1), (-1, 0), (1, 0),
        (-1, -1), (1, -1), (-1, 1), (1, 1)
    ];

    for (dx, dy) in offsets {
        let x1 = cx + (dx * r_spike * 707) / 1000;
        let y1 = cy + (dy * r_spike * 707) / 1000;
        let x2 = cx + (dx * (r_spike + scale(3, size)) * 707) / 1000;
        let y2 = cy + (dy * (r_spike + scale(3, size)) * 707) / 1000;
        g.draw_line(cx, cy, x2, y2, Color::rgb(0x475569));
        // Spike tip cap
        g.fill_rect(x1 - 1, y1 - 1, 3, 3, theme.warning);
    }

    // Mine body sphere
    let r2 = r * r;
    for dy in -r..=r {
        let dx = crate::graphics::framebuffer::int_sqrt(r2 - dy * dy);
        g.fill_rect(cx - dx, cy + dy, dx * 2 + 1, 1, Color::rgb(0x1E293B));
    }
    // Rim
    for dy in -r..=r {
        let dx = crate::graphics::framebuffer::int_sqrt(r2 - dy * dy);
        g.fill_rect(cx - dx, cy + dy, 1, 1, Color::rgb(0x64748B));
        g.fill_rect(cx + dx, cy + dy, 1, 1, Color::rgb(0x64748B));
    }

    // Central red detonator LED
    let led_r = scale(4, size).max(2);
    g.fill_rounded_rect(cx - led_r, cy - led_r, led_r * 2 + 1, led_r * 2 + 1, 2, theme.danger);
    g.fill_rect(cx - 1, cy - 1, 2, 2, Color::rgb(0xFECACA)); // highlight gleam
}

// ---------------------------------------------------------------------------
// 8. Settings (Precision Mechanical Gear)
// ---------------------------------------------------------------------------
fn draw_settings(g: &mut Canvas, x: i32, y: i32, size: i32, theme: &Theme) {
    let cx = x + size / 2;
    let cy = y + size / 2;
    let outer_r = scale(16, size);
    let inner_r = scale(12, size);
    let hole_r = scale(6, size);

    // 8 gear teeth
    for angle_idx in 0..8 {
        let (sin_val, cos_val) = match angle_idx {
            0 => (0, 1000),
            1 => (707, 707),
            2 => (1000, 0),
            3 => (707, -707),
            4 => (0, -1000),
            5 => (-707, -707),
            6 => (-1000, 0),
            _ => (-707, 707),
        };
        let tooth_cx = cx + (sin_val * (outer_r + scale(2, size))) / 1000;
        let tooth_cy = cy + (cos_val * (outer_r + scale(2, size))) / 1000;
        let tw = scale(5, size).max(2);
        g.fill_rounded_rect(tooth_cx - tw / 2, tooth_cy - tw / 2, tw, tw, 1, Color::rgb(0x64748B));
    }

    // Gear body
    let r2 = inner_r * inner_r;
    for dy in -inner_r..=inner_r {
        let dx = crate::graphics::framebuffer::int_sqrt(r2 - dy * dy);
        g.fill_rect(cx - dx, cy + dy, dx * 2 + 1, 1, Color::rgb(0x475569));
    }
    for dy in -inner_r..=inner_r {
        let dx = crate::graphics::framebuffer::int_sqrt(r2 - dy * dy);
        g.fill_rect(cx - dx, cy + dy, 1, 1, Color::rgb(0x94A3B8));
        g.fill_rect(cx + dx, cy + dy, 1, 1, Color::rgb(0x94A3B8));
    }

    // Center hub hole
    let h2 = hole_r * hole_r;
    for dy in -hole_r..=hole_r {
        let dx = crate::graphics::framebuffer::int_sqrt(h2 - dy * dy);
        g.fill_rect(cx - dx, cy + dy, dx * 2 + 1, 1, Color::rgb(0x0B0F1A));
    }
    g.draw_line(cx - scale(2, size), cy, cx + scale(2, size), cy, theme.accent_cyan);
}

// ---------------------------------------------------------------------------
// 9. Trash / Recycle Bin
// ---------------------------------------------------------------------------
fn draw_trash(g: &mut Canvas, x: i32, y: i32, size: i32, theme: &Theme) {
    let w = scale(30, size);
    let h = scale(34, size);
    let bx = x + (size - w) / 2;
    let by = y + (size - h) / 2;
    let r = scale(3, size).max(2);

    // Lid handle
    let handle_w = scale(10, size);
    let handle_h = scale(4, size);
    g.draw_rounded_rect(x + (size - handle_w) / 2, by, handle_w, handle_h, 1, Color::rgb(0x94A3B8));

    // Lid rim
    let lid_y = by + scale(3, size);
    g.fill_rounded_rect(bx - 2, lid_y, w + 4, scale(4, size), 2, Color::rgb(0x64748B));
    g.draw_rounded_rect(bx - 2, lid_y, w + 4, scale(4, size), 2, theme.border);

    // Bin body (tapered rounded rect)
    let body_y = lid_y + scale(5, size);
    let body_h = h - scale(8, size);
    g.fill_rounded_rect(bx, body_y, w, body_h, r, Color::rgb(0x1E293B));
    g.draw_rounded_rect(bx, body_y, w, body_h, r, Color::rgb(0x475569));

    // Vertical ribs on bin
    let rib_y1 = body_y + scale(4, size);
    let rib_y2 = body_y + body_h - scale(4, size);
    let r1 = bx + scale(7, size);
    let r2 = bx + w / 2;
    let r3 = bx + w - scale(7, size);

    g.draw_line(r1, rib_y1, r1, rib_y2, Color::rgb(0x64748B));
    g.draw_line(r2, rib_y1, r2, rib_y2, theme.accent_cyan);
    g.draw_line(r3, rib_y1, r3, rib_y2, Color::rgb(0x64748B));
}

// ---------------------------------------------------------------------------
// 10. File Type: .rs (Rust Source File with Orange Rust Crab Badge)
// ---------------------------------------------------------------------------
fn draw_file_rs(g: &mut Canvas, x: i32, y: i32, size: i32, theme: &Theme) {
    let w = scale(30, size);
    let h = scale(38, size);
    let fx = x + (size - w) / 2;
    let fy = y + (size - h) / 2;
    let r = scale(3, size).max(2);

    // Document base
    g.fill_rounded_rect(fx, fy, w, h, r, Color::rgb(0x1C1917));
    g.draw_rounded_rect(fx, fy, w, h, r, Color::rgb(0xEA580C));

    // Top dog-ear fold
    let fold = scale(8, size);
    g.fill_rect(fx + w - fold, fy, fold, fold, Color::rgb(0x0C0A09));
    g.draw_line(fx + w - fold, fy, fx + w - fold, fy + fold, Color::rgb(0xEA580C));
    g.draw_line(fx + w - fold, fy + fold, fx + w, fy + fold, Color::rgb(0xEA580C));

    // Rust Badge block in center
    let badge_w = w - scale(6, size);
    let badge_h = scale(14, size);
    let bx = fx + scale(3, size);
    let by = fy + scale(12, size);
    g.fill_rounded_rect(bx, by, badge_w, badge_h, 2, Color::rgb(0xC2410C));

    // "RS" text badge
    g.draw_str_transparent(bx + scale(4, size), by + scale(3, size), "RS", Color::rgb(0xFFEDD5));

    // Bottom code lines
    g.fill_rect(fx + scale(4, size), fy + scale(29, size), w - scale(8, size), scale(2, size).max(1), Color::rgb(0x78716C));
    g.fill_rect(fx + scale(4, size), fy + scale(33, size), scale(12, size), scale(2, size).max(1), theme.accent_cyan);
}

// ---------------------------------------------------------------------------
// 11. File Type: .txt (Plain Text Document)
// ---------------------------------------------------------------------------
fn draw_file_txt(g: &mut Canvas, x: i32, y: i32, size: i32, theme: &Theme) {
    let w = scale(30, size);
    let h = scale(38, size);
    let fx = x + (size - w) / 2;
    let fy = y + (size - h) / 2;
    let r = scale(3, size).max(2);

    // Clean sheet
    g.fill_rounded_rect(fx, fy, w, h, r, Color::rgb(0x1E293B));
    g.draw_rounded_rect(fx, fy, w, h, r, theme.border);

    // Dog-ear fold
    let fold = scale(8, size);
    g.fill_rect(fx + w - fold, fy, fold, fold, Color::rgb(0x0B0F1A));
    g.draw_line(fx + w - fold, fy, fx + w - fold, fy + fold, Color::rgb(0x64748B));
    g.draw_line(fx + w - fold, fy + fold, fx + w, fy + fold, Color::rgb(0x64748B));

    // Horizontal text lines
    let lx = fx + scale(4, size);
    let lw = w - scale(8, size);
    for row in 0..4 {
        let ly = fy + scale(12 + row * 6, size);
        let rw = if row == 3 { scale(10, size) } else { lw };
        g.fill_rect(lx, ly, rw, scale(2, size).max(1), theme.text_dim);
    }
}

// ---------------------------------------------------------------------------
// 12. File Type: .elf (Executable Binary)
// ---------------------------------------------------------------------------
fn draw_file_elf(g: &mut Canvas, x: i32, y: i32, size: i32, theme: &Theme) {
    let w = scale(30, size);
    let h = scale(38, size);
    let fx = x + (size - w) / 2;
    let fy = y + (size - h) / 2;
    let r = scale(3, size).max(2);

    // Violet binary badge
    g.fill_rounded_rect(fx, fy, w, h, r, Color::rgb(0x2E1065));
    g.draw_rounded_rect(fx, fy, w, h, r, theme.accent_violet);

    // Dog-ear
    let fold = scale(8, size);
    g.fill_rect(fx + w - fold, fy, fold, fold, Color::rgb(0x0B0F1A));
    g.draw_line(fx + w - fold, fy, fx + w - fold, fy + fold, theme.accent_violet);
    g.draw_line(fx + w - fold, fy + fold, fx + w, fy + fold, theme.accent_violet);

    // "ELF" tag
    g.draw_str_transparent(fx + scale(4, size), fy + scale(10, size), "ELF", theme.accent_cyan);

    // Gear / runner indicator
    let mid_x = fx + w / 2;
    let mid_y = fy + scale(26, size);
    let dot = scale(2, size).max(1);
    g.fill_rect(mid_x - dot, mid_y - dot, dot * 2, dot * 2, theme.success);
    g.draw_line(mid_x - scale(6, size), mid_y, mid_x + scale(6, size), mid_y, theme.accent_violet);
    g.draw_line(mid_x, mid_y - scale(6, size), mid_x, mid_y + scale(6, size), theme.accent_violet);
}
