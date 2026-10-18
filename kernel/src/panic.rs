//! The panic handler and themed panic screen diagnostics.
//!
//! Provides clean COM1 serial logging and a direct hardware framebuffer panic
//! screen rendered without relying on the heap or memory allocators.

use bootloader_api::info::PixelFormat;
use core::arch::asm;
use spin::Mutex;

#[derive(Clone, Copy)]
pub struct PanicFrameBuffer {
    pub base: usize,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub format: PixelFormat,
}

static PANIC_FB: Mutex<Option<PanicFrameBuffer>> = Mutex::new(None);

/// Initializes the raw linear framebuffer target for emergency panic screens.
pub fn init_panic_screen(
    base: usize,
    width: usize,
    height: usize,
    stride: usize,
    format: PixelFormat,
) {
    *PANIC_FB.lock() = Some(PanicFrameBuffer {
        base,
        width,
        height,
        stride,
        format,
    });
}

fn draw_raw_rect(
    fb: &PanicFrameBuffer,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: (u8, u8, u8),
) {
    for py in y..(y + h).min(fb.height) {
        for px in x..(x + w).min(fb.width) {
            let offset = (py * fb.stride + px) * 4;
            unsafe {
                let ptr = (fb.base as *mut u8).add(offset);
                match fb.format {
                    PixelFormat::Rgb => {
                        *ptr = color.0;
                        *ptr.add(1) = color.1;
                        *ptr.add(2) = color.2;
                        *ptr.add(3) = 0;
                    }
                    PixelFormat::Bgr => {
                        *ptr = color.2;
                        *ptr.add(1) = color.1;
                        *ptr.add(2) = color.0;
                        *ptr.add(3) = 0;
                    }
                    _ => {}
                }
            }
        }
    }
}

fn draw_raw_char(
    fb: &PanicFrameBuffer,
    x: usize,
    y: usize,
    c: char,
    fg: (u8, u8, u8),
    scale: usize,
) {
    let ascii = (c as u32).min(127) as usize;
    let glyph = font8x8::legacy::BASIC_LEGACY[ascii];
    for row in 0..8 {
        let byte = glyph[row];
        for col in 0..8 {
            if (byte & (1 << col)) != 0 {
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = x + col * scale + sx;
                        let py = y + row * scale + sy;
                        if px < fb.width && py < fb.height {
                            let offset = (py * fb.stride + px) * 4;
                            unsafe {
                                let ptr = (fb.base as *mut u8).add(offset);
                                match fb.format {
                                    PixelFormat::Rgb => {
                                        *ptr = fg.0;
                                        *ptr.add(1) = fg.1;
                                        *ptr.add(2) = fg.2;
                                        *ptr.add(3) = 0;
                                    }
                                    PixelFormat::Bgr => {
                                        *ptr = fg.2;
                                        *ptr.add(1) = fg.1;
                                        *ptr.add(2) = fg.0;
                                        *ptr.add(3) = 0;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn draw_raw_text(
    fb: &PanicFrameBuffer,
    x: usize,
    y: usize,
    text: &str,
    fg: (u8, u8, u8),
    scale: usize,
) {
    let mut cur_x = x;
    for c in text.chars() {
        if c == '\n' {
            continue;
        }
        draw_raw_char(fb, cur_x, y, c, fg, scale);
        cur_x += 8 * scale;
    }
}

/// Helper to format u64 into hex without heap allocation
fn format_hex(mut val: u64, buf: &mut [u8; 18]) -> &str {
    buf[0] = b'0';
    buf[1] = b'x';
    for i in (0..16).rev() {
        let nibble = (val & 0xF) as u8;
        buf[2 + i] = if nibble < 10 {
            b'0' + nibble
        } else {
            b'A' + (nibble - 10)
        };
        val >>= 4;
    }
    unsafe { core::str::from_utf8_unchecked(buf) }
}

/// Renders the themed panic screen and halts the CPU.
pub fn show_panic_screen(
    reason: &str,
    cr2: u64,
    error_code: u64,
    rip: u64,
    rsp: u64,
) -> ! {
    serial_println!();
    serial_println!("==================== TRIOS KERNEL PANIC ====================");
    serial_println!("Reason:        {reason}");
    serial_println!("Faulting CR2:  {cr2:#018x}");
    serial_println!("Error Code:    {error_code:#010x}");
    serial_println!("Instruction:   {rip:#018x}");
    serial_println!("Stack Pointer: {rsp:#018x}");
    serial_println!("System halted — reset or power off machine to reboot.");

    if let Some(fb) = *PANIC_FB.lock() {
        // Deep obsidian background
        draw_raw_rect(&fb, 0, 0, fb.width, fb.height, (15, 14, 23));

        // Themed card in center
        let card_w = 760.min(fb.width.saturating_sub(40));
        let card_h = 420.min(fb.height.saturating_sub(40));
        let card_x = (fb.width.saturating_sub(card_w)) / 2;
        let card_y = (fb.height.saturating_sub(card_h)) / 2;

        // Outer border
        draw_raw_rect(&fb, card_x, card_y, card_w, card_h, (70, 60, 85));
        // Inner card body
        draw_raw_rect(
            &fb,
            card_x + 2,
            card_y + 2,
            card_w - 4,
            card_h - 4,
            (24, 22, 34),
        );

        // Header bar: Crimson accent
        draw_raw_rect(&fb, card_x + 2, card_y + 2, card_w - 4, 48, (215, 45, 65));

        // Header title
        draw_raw_text(
            &fb,
            card_x + 20,
            card_y + 16,
            "TRIOS SYSTEM HALT  [ KERNEL PANIC ]",
            (255, 255, 255),
            2,
        );

        // Subtitle / message
        draw_raw_text(
            &fb,
            card_x + 24,
            card_y + 68,
            "A fatal CPU exception occurred. System halted to prevent data corruption.",
            (190, 190, 210),
            1,
        );

        // Exception Reason
        draw_raw_text(&fb, card_x + 24, card_y + 100, "EXCEPTION:", (245, 200, 90), 2);
        draw_raw_text(&fb, card_x + 180, card_y + 100, reason, (255, 255, 255), 2);

        // Inner diagnostics box
        let diag_y = card_y + 135;
        let diag_h = 190;
        draw_raw_rect(
            &fb,
            card_x + 20,
            diag_y,
            card_w - 40,
            diag_h,
            (16, 14, 22),
        );

        let mut buf1 = [0u8; 18];
        let mut buf2 = [0u8; 18];
        let mut buf3 = [0u8; 18];
        let mut buf4 = [0u8; 18];

        let cr2_str = format_hex(cr2, &mut buf1);
        let rip_str = format_hex(rip, &mut buf2);
        let rsp_str = format_hex(rsp, &mut buf3);
        let err_str = format_hex(error_code, &mut buf4);

        draw_raw_text(
            &fb,
            card_x + 36,
            diag_y + 20,
            "CR2 (Fault Virtual Address):",
            (140, 150, 175),
            1,
        );
        draw_raw_text(&fb, card_x + 300, diag_y + 20, cr2_str, (88, 166, 255), 1);

        draw_raw_text(
            &fb,
            card_x + 36,
            diag_y + 45,
            "RIP (Instruction Pointer):",
            (140, 150, 175),
            1,
        );
        draw_raw_text(&fb, card_x + 300, diag_y + 45, rip_str, (88, 166, 255), 1);

        draw_raw_text(
            &fb,
            card_x + 36,
            diag_y + 70,
            "RSP (Stack Pointer):",
            (140, 150, 175),
            1,
        );
        draw_raw_text(&fb, card_x + 300, diag_y + 70, rsp_str, (88, 166, 255), 1);

        draw_raw_text(
            &fb,
            card_x + 36,
            diag_y + 95,
            "Error Code (Flags):",
            (140, 150, 175),
            1,
        );
        draw_raw_text(&fb, card_x + 300, diag_y + 95, err_str, (245, 120, 120), 1);

        if crate::memory::is_guard_page(cr2) {
            draw_raw_text(
                &fb,
                card_x + 36,
                diag_y + 130,
                "[!] STACK OVERFLOW: Fault in kernel thread stack guard page!",
                (245, 80, 80),
                1,
            );
        } else {
            draw_raw_text(
                &fb,
                card_x + 36,
                diag_y + 130,
                "Memory protection: Page not present or permission violation.",
                (160, 170, 190),
                1,
            );
        }

        draw_raw_text(
            &fb,
            card_x + 36,
            diag_y + 155,
            "Operating System: TriOS x86_64 UEFI | Author: JOJIN JOHN",
            (100, 110, 130),
            1,
        );

        // Footer instructions
        draw_raw_text(
            &fb,
            card_x + 24,
            card_y + card_h - 40,
            ">> Action: Reset or power off the virtual machine / host computer to reboot.",
            (220, 220, 230),
            1,
        );
    }

    x86_64::instructions::interrupts::disable();
    loop {
        x86_64::instructions::hlt();
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let rsp: u64;
    let rbp: u64;
    unsafe {
        asm!(
            "mov {rsp}, rsp",
            rsp = out(reg) rsp,
            options(nomem, nostack, preserves_flags)
        );
        asm!(
            "mov {rbp}, rbp",
            rbp = out(reg) rbp,
            options(nomem, nostack, preserves_flags)
        );
    }

    let reason = if let Some(msg) = info.message().as_str() {
        msg
    } else {
        "Kernel panic encountered"
    };

    show_panic_screen(reason, 0, 0, rbp, rsp);
}
