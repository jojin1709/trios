//! TriOS — Phase 0 + Phase 1 + Phase 2 + Phase 3 (OS-000 / OS-001 / OS-002 / OS-003).
//!
//! UEFI entry point handed to the `bootloader` crate. This file owns the boot
//! sequence; each subsystem lives in its own module.
//!
//! Boot sequence:
//! serial → banner → boot info → memory map → GDT/TSS → IDT → PIC → PIT →
//! heap (+ allocation smoke test) → PS/2 input → enable interrupts →
//! tick test → framebuffer → desktop (GUI event loop, replaces the Phase 2
//! splash idle: same heartbeat, plus windows, cursor and input).

#![no_std]
#![no_main]
// Nightly-only features (toolchain pinned in rust-toolchain.toml):
//  * `abi_x86_interrupt` — the interrupt-handler calling convention;
//  * `alloc_error_handler` — the `#[alloc_error_handler]` attribute used by
//    `allocator.rs` (still unstable: rust-lang/rust#51540).
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]

// Must come first so `serial_print!`/`serial_println!` are in scope for every
// module declared below.
#[macro_use]
mod serial;

mod acpi;
mod allocator;
mod apps;
mod drivers;
mod event;
mod fs;
mod gdt;
mod graphics;
mod gui;
mod input;
mod interrupts;
mod memory;
mod mmio;
mod net;
mod panic;
mod perf;
mod sched;
mod syscall;
mod timer;

extern crate alloc;

use alloc::{boxed::Box, format, vec::Vec};
use bootloader_api::{BootInfo, BootloaderConfig, config::Mapping, entry_point};
use x86_64::VirtAddr;

/// Bootloader configuration baked into the kernel ELF (read at load time).
///
/// `physical_memory = Dynamic` asks the bootloader to map **all** physical RAM
/// at one dynamic virtual offset. The heap ([`allocator`]) and future MMIO
/// reach physical memory through that direct map — which is why Phase 1
/// never has to edit page tables itself.
const CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config
};

entry_point!(kernel_main, config = &CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // 1. Serial first: everything below is only observable via COM1.
    let serial_ready = serial::init();

    serial_println!();
    serial_println!("+--------------------------------------------------------+");
    serial_println!("|        TriOS - Phase 3: Interactive GUI boot           |");
    serial_println!("|    no_std x86_64 | UEFI | serial + gfx + PS/2 + GUI    |");
    serial_println!("+--------------------------------------------------------+");
    serial_println!(
        "[serial ] COM1 {}",
        if serial_ready {
            "ready"
        } else {
            "INIT FAILED - no output will follow"
        }
    );

    // 2. What did the bootloader hand us?
    serial_println!(
        "[boot   ] kernel image offset {:#x}",
        boot_info.kernel_image_offset
    );
    serial_println!(
        "[boot   ] kernel stack {:#x}..{:#x}",
        boot_info.kernel_stack_bottom,
        boot_info.kernel_stack_bottom + boot_info.kernel_stack_len
    );
    let phys_offset = VirtAddr::new(match boot_info.physical_memory_offset {
        bootloader_api::info::Optional::Some(offset) => offset,
        bootloader_api::info::Optional::None => {
            panic!("direct physical memory map missing — CONFIG was not honored")
        }
    });
    serial_println!("[boot   ] direct map of physical memory at {phys_offset:#x}");
    memory::init_memory(phys_offset);

    // Phase 12: ACPI Table Discovery & Power Management Subsystem
    let rsdp_opt = match boot_info.rsdp_addr {
        bootloader_api::info::Optional::Some(addr) => Some(addr),
        bootloader_api::info::Optional::None => None,
    };
    acpi::init(rsdp_opt, phys_offset);

    // Take the bootloader's framebuffer handle out of the boot info now
    // (while we're borrowing `boot_info` for reads anyway). The actual
    // `Graphics` surface is built later — its backbuffer needs the heap.
    // A missing framebuffer (headless GOP-less firmware) is not fatal:
    // the kernel stays serial-only, exactly like Phase 1.
    let framebuffer = boot_info.framebuffer.take();

    // Phase 3: give the mouse driver its screen bounds before IRQ12 can
    // fire (the cursor position must be clamped to the real geometry).
    if let Some(fb) = &framebuffer {
        let info = fb.info();
        input::set_screen_size(info.width as i32, info.height as i32);
    }

    // 3. Memory map summary + frame allocator over the usable regions.
    let map = memory::summarize(&boot_info.memory_regions);
    serial_println!(
        "[memory ] {} regions | usable {:.1} MiB | bootloader {} KiB | RAM top {:#x}",
        map.region_count,
        map.usable_bytes as f64 / (1024.0 * 1024.0),
        map.bootloader_bytes / 1024,
        map.top_of_ram
    );
    let mut frame_allocator = memory::BumpFrameAllocator::new(&boot_info.memory_regions);

    // 4. CPU structures — before anything can fault or fire.
    gdt::init();
    serial_println!(
        "[gdt    ] GDT + TSS loaded (IST1 double-fault stack top {:#x})",
        gdt::double_fault_stack_top()
    );

    interrupts::init_idt();
    serial_println!("[idt    ] IDT loaded (#BP #DF #PF #GP + IRQ0 @ vector 32)");

    interrupts::init_pic();
    serial_println!(
        "[pic    ] 8259s remapped to vectors {}..{}, IRQ0 unmasked (rest masked)",
        interrupts::PIC_1_OFFSET,
        interrupts::PIC_2_OFFSET + 7
    );

    timer::init(timer::HZ);
    serial_println!(
        "[pit    ] channel 0 armed at {} Hz (divisor {})",
        timer::HZ,
        1_193_182 / timer::HZ
    );

    // 5. Kernel heap via the direct map + frame bump allocator.
    let heap = allocator::init(&mut frame_allocator, phys_offset, allocator::HEAP_SIZE);
    serial_println!(
        "[heap   ] {} KiB at {:#x} (phys {:#x}) | {} bytes free",
        heap.size / 1024,
        heap.virt_start.as_u64(),
        heap.phys_start.as_u64(),
        allocator::remaining()
    );
    memory::log_page_table(phys_offset);

    // Allocation smoke test through the global allocator: Box (heap),
    // Vec (growth), format! (Display + allocation).
    let boxed = Box::new(0xDEAD_BEEFu64);
    let mut numbers = Vec::new();
    for i in 0..64u32 {
        numbers.push(i.wrapping_mul(7));
    }
    let summary = format!(
        "vec[0..{}] sum={} last={:#x}",
        numbers.len(),
        numbers.iter().sum::<u32>(),
        *boxed
    );
    serial_println!("[heap   ] alloc test PASS: {summary}");

    // Phase 5: Initialize kernel thread scheduler
    sched::init();

    // Phase 6: Initialize fast syscall subsystem (MSRs + sys_call_table)
    syscall::init();

    // Phase 7: PCI bus enumeration & AHCI SATA block storage driver
    let sata_ready = drivers::AhciController::init(phys_offset);

    // Phase 9: Persistent Storage — GPT Partition Table & FAT Filesystem
    if sata_ready {
        match fs::gpt::scan_partitions(drivers::ahci::read_sector) {
            Ok((hdr, parts)) => {
                serial_println!(
                    "[gpt    ] valid partition table: {} entries (LBA {}..{})",
                    parts.len(),
                    hdr.first_usable_lba,
                    hdr.last_usable_lba
                );
                for (idx, p) in parts.iter().enumerate() {
                    serial_println!(
                        "[gpt    ] partition #{idx}: '{}' (LBA {}..{})",
                        p.name,
                        p.starting_lba,
                        p.ending_lba
                    );
                }
                if let Some(boot_part) = parts.first() {
                    match fs::fat::FatFileSystem::mount(boot_part.starting_lba, &drivers::ahci::read_sector) {
                        Ok(fat_fs) => {
                            serial_println!(
                                "[fat    ] mounted {:?} volume '{}' (label: '{}')",
                                fat_fs.bpb.fat_type,
                                boot_part.name,
                                fat_fs.bpb.volume_label
                            );
                            if let Ok(entries) = fat_fs.list_dir("/", &drivers::ahci::read_sector) {
                                serial_println!("[fat    ] root directory contains {} entries:", entries.len());
                                for e in entries {
                                    serial_println!(
                                        "[fat    ]   {} {:<12} ({} bytes)",
                                        if e.is_dir { "[DIR ]" } else { "[FILE]" },
                                        e.name,
                                        e.size
                                    );
                                }
                            }
                        }
                        Err(err) => {
                            serial_println!("[fat    ] failed to mount partition: {err}");
                        }
                    }
                }
            }
            Err(err) => {
                serial_println!("[gpt    ] GPT scan error: {err}");
            }
        }
    }

    // Phase 10: Intel 82540EM (e1000) Gigabit Ethernet NIC & Network Stack
    init_network(phys_offset);

    // Spawn a background kernel worker thread to verify concurrent multitasking
    sched::spawn("background_worker", background_worker);

    // 6. Phase 3 input: PS/2 keyboard + mouse. Runs with interrupts still
    //    disabled — the init sequence polls the controller directly, and
    //    IRQ1/IRQ12 only get unmasked at the end of it (after both device
    //    handlers are already registered in the IDT).
    input::init();

    // 7. Hardware interrupts from here on.
    x86_64::instructions::interrupts::enable();
    serial_println!("[irq    ] interrupts enabled (sti)");

    // PERF (measurement only, zero behaviour change): calibrate the TSC
    // against the PIT before any timed code runs. It must come after
    // `timer::init` above (the PIT must be programmed) — and, found while
    // placing this call, also after `sti`: `timer::ticks()` only ever
    // advances from the IRQ0 handler, so at the literal "immediately after
    // timer::init" spot the interrupt flag is still off, the counter is
    // frozen, and the calibration spin would end in
    // "[perf   ] tsc calibration FAILED" on every healthy boot.
    perf::calibrate();

    // The first tick must arrive promptly — spin (not hlt) for a bounded
    // time so a dead PIT/PIC panics with a message instead of hanging the
    // hlt-loop below with no evidence.
    let before = timer::ticks();
    let mut spins = 0u64;
    while timer::ticks() == before {
        core::hint::spin_loop();
        spins += 1;
        if spins > 100_000_000 {
            panic!("no timer interrupt after sti — PIT/PIC misconfigured");
        }
    }

    // Tick test: roughly one second of PIT ticks must arrive.
    timer::wait_ticks(timer::TICKS_PER_SECOND);
    let elapsed = timer::ticks() - before;
    serial_println!("[timer  ] tick test PASS: {elapsed} ticks in ~1 s");

    // 8. Phase 3 pixels + desktop. Build the double-buffered canvas from
    //    the bootloader's framebuffer (backbuffer allocated on the heap),
    //    then hand control to the desktop event loop — which replaces the
    //    Phase 2 splash idle and never returns (serial heartbeat included).
    let mut graphics: Option<graphics::Graphics> = framebuffer.map(|fb| {
        let info = fb.info();
        serial_println!(
            "[gfx    ] framebuffer {}x{}, stride {}, format {:?}, {} B/px, {} KiB",
            info.width,
            info.height,
            info.stride,
            info.pixel_format,
            info.bytes_per_pixel,
            info.byte_len / 1024
        );
        let gfx = graphics::Graphics::new(fb);
        serial_println!(
            "[gfx    ] backbuffer {} KiB on heap | {} heap bytes free",
            info.byte_len / 1024,
            allocator::remaining()
        );
        gfx
    });

    match &mut graphics {
        Some(gfx) => desktop_run(gfx),
        None => {
            // No display — Phase 1 behaviour, verbatim.
            serial_println!("[gfx    ] no framebuffer in boot info — serial-only boot");
            heartbeat_loop()
        }
    }
}

/// Phase 1's steady state, kept for firmware without a framebuffer.
/// (With a display, [`desktop_run`] plays the same role.)
fn heartbeat_loop() -> ! {
    let mut reported = 0u64;
    loop {
        let uptime = timer::uptime_seconds();
        if uptime > reported {
            reported = uptime;
            serial_println!(
                "[timer  ] heartbeat: uptime {uptime} s, ticks={}",
                timer::ticks()
            );
        }
        x86_64::instructions::hlt();
    }
}

// ---------------------------------------------------------------------------
// Phase 3: the desktop
// ---------------------------------------------------------------------------

// Welcome-window widget indices — the *creation order* of the widget vec
// in `desktop_run`. `set_label`/`set_progress` and the click handler refer
// to these; reordering the pushes below requires reordering these too.
const W_WELCOME_BUTTON: usize = 3;
const W_WELCOME_COUNTER: usize = 4;
const W_WELCOME_PROGRESS: usize = 5;

// About-window widget indices (see above).
const W_ABOUT_UPTIME: usize = 0;
const W_ABOUT_HEAP: usize = 1;
const W_ABOUT_RES: usize = 2;
const W_ABOUT_TICKS: usize = 3;

// Playground-window widget indices (see above).
const W_PLAY_SLIDER: usize = 0;
const W_PLAY_VALUE: usize = 1;
const W_PLAY_MIRROR: usize = 2;
// Phase 4 (RS-002): the demo text input (widget #4; #3 is its caption
// label, which nothing ever updates — no index constant needed).
const W_PLAY_TEXT_INPUT: usize = 4;

// Keyboard cursor fallback (VirtualBox delivers no PS/2 mouse packets —
// `[ps2] mouse irq count = 0`): arrow keys drive the *same* software
// cursor the mouse driver owns. Shift swaps to the fine step.
const CURSOR_STEP: i32 = 16;
const CURSOR_STEP_FINE: i32 = 4;

/// Phase 3's steady state: the interactive desktop.
///
/// Replaces the Phase 2 splash idle loop — same serial heartbeat, plus
/// PS/2 input, a window manager and three demo windows. The event cycle:
///
/// ```text
/// hlt -> (IRQs fill the queue while we sleep) -> drain ALL events
///     -> act (windows, labels, animations) -> compose -> flush -> hlt
/// ```
///
/// Never returns; the kernel lives here. Every visible change comes from
/// an event — a tick (animation/heartbeat), a mouse packet (cursor,
/// window drags, widget clicks) or a key (Esc closes the focused window).
fn desktop_run(gfx: &mut graphics::Graphics) -> ! {
    use gui::{Theme, Widget, WidgetOutcome, WindowManager};
    use input::{Event, KeyCode, MouseButtons, MouseEvent};

    let theme = Theme::default();
    let screen_w = gfx.width() as i32;
    let screen_h = gfx.height() as i32;
    let mut wm = WindowManager::new(screen_w, screen_h);

    // Phase 4 foundation (RS-001): the in-memory filesystem. The desktop
    // owns the instance — the File Explorer / Text Editor / Terminal
    // built next cycle reach it through the event loop (`apps` below),
    // which mutates it (New file, editor Save, `write` at the prompt).
    let mut ramfs = fs::FileSystem::new();
    serial_println!("[fs     ] ramfs ready: {} files", ramfs.file_count());

    // -- demo window 1: Welcome (also the initial focus) ------------------
    let welcome = wm.create_window("Welcome", 80, 60, 400, 200);
    {
        let w = wm.window_mut(welcome).expect("welcome window just created");
        w.widgets.push(Widget::label(
            "Welcome to TriOS!",
            12,
            30,
            theme.text_primary,
        ));
        // The title reads better bigger — Label's scale is public.
        if let Some(Widget::Label(l)) = w.widgets.get_mut(0) {
            l.scale = 2;
        }
        w.widgets.push(Widget::label(
            "Phase 3 - Interactive GUI",
            12,
            52,
            theme.text_secondary,
        ));
        w.widgets.push(Widget::panel(
            12,
            70,
            376,
            56,
            theme.button_bg,
            Some(theme.border),
        ));
        w.widgets.push(Widget::button(24, 86, 120, 24, "Click me"));
        w.widgets
            .push(Widget::label("Clicks: 0", 160, 92, theme.text_primary));
        w.widgets.push(Widget::progress(12, 138, 376, 14, 0.0));
        w.widgets.push(Widget::label(
            "Drag titlebars to move, grip to resize, Esc or X to close.",
            12,
            162,
            theme.text_secondary,
        ));
    }

    // -- demo window 2: About (live system info, updated on the heartbeat)
    let about = wm.create_window("About", 200, 140, 300, 180);
    {
        let w = wm.window_mut(about).expect("about window just created");
        w.widgets
            .push(Widget::label("About TriOS", 12, 30, theme.text_primary));
        if let Some(Widget::Label(l)) = w.widgets.get_mut(0) {
            l.scale = 2;
        }
        w.widgets.push(Widget::label(
            format!("Uptime: {} s", timer::uptime_seconds()),
            12,
            56,
            theme.text_primary,
        ));
        w.widgets.push(Widget::label(
            format!("Heap free: {} B", allocator::remaining()),
            12,
            72,
            theme.text_primary,
        ));
        w.widgets.push(Widget::label(
            format!("Resolution: {screen_w}x{screen_h}"),
            12,
            88,
            theme.text_primary,
        ));
        w.widgets.push(Widget::label(
            format!("Ticks: {}", timer::ticks()),
            12,
            104,
            theme.text_primary,
        ));
        w.widgets.push(Widget::label(
            "Esc closes this window.",
            12,
            128,
            theme.text_secondary,
        ));
    }

    // -- demo window 3: Playground (the interactive widgets) --------------
    // Height 210 (was 150): room for the Phase 4 text field below the
    // slider/progress pair.
    let playground = wm.create_window("Playground", 320, 220, 300, 210);
    {
        let w = wm
            .window_mut(playground)
            .expect("playground window just created");
        w.widgets
            .push(Widget::label("Playground", 12, 30, theme.text_primary));
        if let Some(Widget::Label(l)) = w.widgets.get_mut(0) {
            l.scale = 2;
        }
        w.widgets.push(Widget::slider(12, 62, 276, 18, 0.5));
        w.widgets
            .push(Widget::label("Value: 0.50", 12, 88, theme.text_primary));
        w.widgets.push(Widget::progress(12, 110, 276, 14, 0.5));
        // Phase 4 foundation (RS-002): one live text field. Click it to
        // type — while it has focus, keys land here instead of the
        // cursor fallback (see the `has_text_focus` gate in the event
        // loop); Enter commits and hands the keyboard back.
        w.widgets.push(Widget::label(
            "Text input - click, then type:",
            12,
            138,
            theme.text_secondary,
        ));
        w.widgets
            .push(Widget::text_input(12, 156, 276, 24, "type here"));
    }

    // Phase 4 (cycle 2): the shell — Files, Editor and Terminal. Built
    // after the demo windows so they sit above them in the z-order, then
    // primed once (the explorer listing is the only fs-backed view that
    // needs data before the first frame). The constructor itself touches
    // no filesystem — see `apps::Apps::new`.
    let mut apps = apps::Apps::new(&mut wm, &theme);
    // Tell the taskbar which windows its three buttons focus. This is
    // the only place that knows both halves — the shell (which owns the
    // app windows) and the WM (which owns the strip) — so the ids are
    // handed over here rather than the WM ever looking at `apps` or
    // matching window titles (see `WindowManager::set_taskbar_targets`).
    // Until this line the buttons draw but every click reports `None`.
    wm.set_taskbar_targets(apps.explorer.win, apps.editor.win, apps.terminal.win);
    apps.refresh(&mut wm, &ramfs);
    serial_println!("[apps  ] shell ready: Files, Editor, Terminal");

    // Welcome starts focused (it was created first; re-assert so the
    // titlebar/border styling and Esc target are unambiguous).
    wm.focus(welcome);
    serial_println!(
        "[gui    ] desktop ready: {screen_w}x{screen_h}, {} windows",
        wm.windows.len()
    );
    // One-time hint for the keyboard cursor fallback (see the Key arm
    // below): when VirtualBox forwards zero mouse packets, the keyboard
    // is the only working input — say so once, right when the desktop
    // becomes interactive.
    serial_println!("[gui    ] keyboard cursor active: arrows=move, space/enter=click, esc=close");

    // -- the event loop ----------------------------------------------------
    let mut reported = 0u64; // last heartbeat second logged
    let mut clicks = 0u32; // Welcome button press count
    // PERF (measurement only): per-second loop statistics — a plain local
    // (no lock, no allocation) fed at the drain/frame/wake hooks below and
    // flushed once a second by the `report` call in the Tick arm. Module
    // docs in `perf.rs` map every printed number to the question it
    // answers (frames/s = redrawing every tick? compose vs flush = who is
    // expensive? hw_ticks vs evt_ticks = lost IRQs or dropped events?).
    let mut perf = perf::Perf::new();
    loop {
        let mut dirty = false; // something visible changed this pass
        let mut maybe_close = false; // a window might want closing

        // Phase 10: Poll network interface for pending packets
        net::poll_network();

        // Drain *everything* the IRQs produced while we slept; act once
        // per batch, then redraw at most once. (Queue capacity 512 ≈ 5 s
        // of ticks — the backlog from the 1 s tick test above fits.)
        // PERF (measurement only): batch bookkeeping for the 1 Hz report —
        // `events_in_batch` sizes each drain (a climbing max = the loop is
        // falling behind its producers), `mouse_in_batch` separates real
        // input traffic from the 100 Hz tick background.
        let mut events_in_batch = 0u64;
        let mut mouse_in_batch = 0u64;
        while let Some(event) = input::poll_event() {
            events_in_batch += 1;
            if matches!(event, Event::Mouse(_)) {
                mouse_in_batch += 1;
            }
            match event {
                // -- tick: heartbeat, live About labels, progress sweep ---
                Event::Tick(n) => {
                    let sec = n / timer::TICKS_PER_SECOND;
                    if sec > reported {
                        reported = sec;
                        // Same heartbeat line UAT greps for since Phase 1.
                        serial_println!("[timer  ] heartbeat: uptime {sec} s, ticks={n}");
                        // Mouse diagnostics, 1 Hz (see `input::mouse`):
                        // irq count is the *decisive* number — 0 after a
                        // click means VirtualBox never delivered a byte;
                        // a climbing count with a frozen cursor means a
                        // decode/position bug. Cursor position proves the
                        // absolute position is updating even if a future
                        // redraw path regresses.
                        serial_println!(
                            "[ps2    ] mouse irq count = {}",
                            input::mouse::irq_count()
                        );
                        // Keyboard-side twin of the mouse counter, plus
                        // the instrumented status/raw captures added for
                        // the "VirtualBox queues host motion the guest
                        // never reads" hunt (see `input::keyboard` /
                        // `input::mouse`): `kb irq count` climbs while
                        // keys visibly work, so a flat mouse count next
                        // to it isolates the fault to the mouse path —
                        // not the shared PIC/IDT wiring. The `status
                        // @irq` lines are the 0x64 byte each handler
                        // latched when its IRQ fired (bit0 = byte
                        // waiting, bit5 = aux source, bit6 = timeout);
                        // the `raw` lines are each device's last 8 IRQ
                        // bytes, oldest first, newest last.
                        serial_println!(
                            "[ps2    ] kb irq count = {}",
                            input::keyboard::kb_irq_count()
                        );
                        serial_println!(
                            "[ps2    ] kb status @irq = 0x{:02x}",
                            input::keyboard::status_at_irq()
                        );
                        serial_println!(
                            "[ps2    ] kb raw: {}",
                            input::HexBytes::lower(&input::keyboard::raw_bytes())
                        );
                        serial_println!(
                            "[ps2    ] mouse status @irq = 0x{:02x}",
                            input::mouse::status_at_irq()
                        );
                        serial_println!(
                            "[ps2    ] mouse raw: {}",
                            input::HexBytes::lower(&input::mouse::raw_bytes())
                        );
                        let (mx, my) = input::mouse::position();
                        serial_println!("[gui    ] cursor at ({mx}, {my})");
                        wm.set_label(about, W_ABOUT_UPTIME, format!("Uptime: {sec} s"));
                        wm.set_label(about, W_ABOUT_TICKS, format!("Ticks: {n}"));
                        wm.set_label(
                            about,
                            W_ABOUT_HEAP,
                            format!("Heap free: {} B", allocator::remaining()),
                        );
                        wm.set_label(
                            about,
                            W_ABOUT_RES,
                            format!("Resolution: {screen_w}x{screen_h}"),
                        );
                        let dropped = event::dropped();
                        if dropped > 0 {
                            serial_println!(
                                "[gui    ] warning: {dropped} event(s) dropped since boot"
                            );
                        }
                        // PERF (measurement only): the 1 Hz report, printed
                        // right after the heartbeat block's own lines (all
                        // of which are kept exactly as they were) and once
                        // per *reported* second — inside `if sec > reported`
                        // so the counters cover a whole second instead of
                        // being flushed 100×/s. `n` is the tick value this
                        // heartbeat event carried (evt_ticks), against the
                        // hardware truth `timer::ticks()` (hw_ticks) and the
                        // ring-buffer overflow count — the uptime-anomaly
                        // discriminator. `report` resets its counters.
                        perf.report(timer::ticks(), n, event::dropped());
                    }
                    // 2-second 0→1 sweep: proves the tick path drives
                    // animation, not just the log.
                    wm.set_progress(welcome, W_WELCOME_PROGRESS, (n % 200) as f32 / 200.0);
                    dirty = true; // the sweep above is itself a change
                    // Phase 4 (cycle 2): fold in the shell's 1 Hz hook.
                    // Nothing in the apps is time-dependent yet (see
                    // `Apps::on_tick`), so today it contributes `false`.
                    dirty |= apps.on_tick(&mut wm);
                }

                // -- mouse: cursor, drags, widget actions ------------------
                Event::Mouse(me) => {
                    if let Some(action) = wm.handle_mouse(me) {
                        // Phase 4 (cycle 2): shell windows (Files, Editor,
                        // Terminal) handle their own clicks. Actions from
                        // the demo windows fall through to the wiring
                        // below, unchanged. The return value needs no say
                        // in `dirty`: this arm repaints unconditionally
                        // anyway (the cursor moved — see the `dirty = true`
                        // at its end), so the flag would be overwritten
                        // before it was read.
                        apps.on_action(action, &mut wm, &mut ramfs, &theme);
                        match action.outcome {
                            WidgetOutcome::Clicked => {
                                serial_println!(
                                    "[gui    ] click: window {} widget #{}",
                                    action.window_id,
                                    action.widget_index
                                );
                                if action.window_id == welcome
                                    && action.widget_index == W_WELCOME_BUTTON
                                {
                                    clicks += 1;
                                    wm.set_label(
                                        welcome,
                                        W_WELCOME_COUNTER,
                                        format!("Clicks: {clicks}"),
                                    );
                                    serial_println!("[gui    ] button press count = {clicks}");
                                }
                            }
                            WidgetOutcome::ValueChanged(v) => {
                                serial_println!(
                                    "[gui    ] slider: window {} widget #{} -> {v:.2}",
                                    action.window_id,
                                    action.widget_index
                                );
                                if action.window_id == playground
                                    && action.widget_index == W_PLAY_SLIDER
                                {
                                    wm.set_label(
                                        playground,
                                        W_PLAY_VALUE,
                                        format!("Value: {v:.2}"),
                                    );
                                    wm.set_progress(playground, W_PLAY_MIRROR, v);
                                }
                            }
                        }
                    }
                    // The X button flips close_requested inside the WM
                    // without producing an action — always worth a reap.
                    maybe_close = true;
                    dirty = true; // cursor (or hover/press state) moved
                }

                // -- keys: Ctrl+1/2/3 switches windows, otherwise a -----
                // -- focused text field owns the keyboard, Esc closes, --
                // -- and arrows/space/enter drive the keyboard cursor ---
                // -- fallback (mouse irq count = 0) ---------------------
                Event::Key(k) => {
                    if k.pressed {
                        // Ctrl+1/2/3 — the taskbar's keyboard twin: focus
                        // Files / Editor / Terminal. Checked *first*,
                        // ahead of the shell's first-refusal below *and*
                        // the `has_text_focus` gate further down, for two
                        // reasons: (a) it must work while a field is
                        // focused — switching away mid-typing is the
                        // point of a window-switch hotkey, not a bug —
                        // and (b) the event must then be fully consumed,
                        // otherwise `TextInput::handle_key` would splice
                        // the digit into whatever the caret was sitting
                        // in on the way to the new window. Press edge
                        // only: the release half of the same keystroke
                        // must not fire the switch a second time (it
                        // couldn't — the whole arm is press-gated — but
                        // keeping the condition next to the code says
                        // why it is where it is).
                        if k.modifiers.ctrl {
                            let target = match k.code {
                                KeyCode::Char('1') => Some((apps.explorer.win, "Files")),
                                KeyCode::Char('2') => Some((apps.editor.win, "Editor")),
                                KeyCode::Char('3') => Some((apps.terminal.win, "Terminal")),
                                _ => None,
                            };
                            if let Some((id, name)) = target {
                                // Focus *and* arm the window's first text
                                // field. Plain `wm.focus(id)` set the window
                                // focus but left `focused_widget == None`
                                // (only a mouse press ever armed a field),
                                // so `has_text_focus()` answered `false`
                                // and every printable fell through
                                // `Window::handle_key` with nothing to
                                // receive it — silently, with no log line.
                                // That is the bug; see `focus_and_type`.
                                wm.focus_and_type(id);
                                serial_println!("[apps  ] focus: {name}");
                                dirty = true; // focus styling must repaint
                                continue; // consumed — skip the rest
                            }
                        }
                        // Phase 4 (cycle 2): the shell takes first refusal
                        // on every key while one of its own windows is
                        // focused — deliberately *before* the text-focus
                        // gate below, because a TextInput does not consume
                        // Enter/arrows (submitting a command, loading a
                        // file or moving a selection is the app's call).
                        // The answer is kept in `app_changed`: it drives
                        // `dirty` as before, and doubles as the "did the
                        // app do *anything* with this key?" bit for the
                        // dropped-key log in the shell branch below.
                        let app_changed =
                            apps.owns_focus(&wm) && apps.on_key(k, &mut wm, &mut ramfs, &theme);
                        dirty |= app_changed;
                        // Phase 4 (RS-002): while the focused window
                        // routes keys to a text field, printable and
                        // editing keys belong to it. Without this gate
                        // the fallback arms below would fight the caret:
                        // arrows would drag the mouse cursor mid-caret-
                        // move, and space would insert a char *and* fake
                        // a click that could steal focus back. Esc still
                        // closes the window (the field never consumes
                        // it); Enter is the owner's to interpret — the
                        // demo commits and releases the field, so a
                        // keyboard-only user is never trapped in the
                        // caret.
                        if wm.has_text_focus() {
                            // Commit-and-release on Enter is the *demo
                            // field's* behaviour. Shell fields keep their
                            // caret — a terminal prompt must survive its
                            // own Enter — and report through the apps'
                            // serial lines instead of this one.
                            let demo = wm.focused_window() == Some(playground);
                            // Snapshot the armed field *before* the key
                            // reaches it: the diff after `handle_key` is
                            // the end-to-end serial proof (scancode →
                            // KeyEvent → focused TextInput → visible
                            // edit). The demo reads its known widget
                            // index; any other armed field — in this
                            // branch always a shell window's, armed by
                            // `focus_and_type` — reads whichever field is
                            // live. Both report through the *same* one
                            // `[gui    ] text field: "..."` line below,
                            // so the demo's existing proof is reused, not
                            // duplicated.
                            let before = if demo {
                                wm.text_value(playground, W_PLAY_TEXT_INPUT)
                            } else {
                                wm.focused_text_value()
                            };
                            match k.code {
                                KeyCode::Enter if demo => {
                                    serial_println!(
                                        "[gui    ] text field commit: \"{}\"",
                                        before.as_deref().unwrap_or("")
                                    );
                                    wm.clear_widget_focus(playground);
                                }
                                KeyCode::Esc => {
                                    serial_println!("[gui    ] key Esc — closing focused window");
                                    maybe_close = true;
                                }
                                _ => {}
                            }
                            wm.handle_key(k);
                            let after = if demo {
                                wm.text_value(playground, W_PLAY_TEXT_INPUT)
                            } else {
                                wm.focused_text_value()
                            };
                            if after != before
                                && let Some(v) = after
                            {
                                // The end-to-end pipeline proof: scancode
                                // -> KeyEvent -> focused TextInput ->
                                // visible edit + serial line. The shell
                                // fields reporting through the demo's line
                                // format is deliberate: one format, one
                                // grep, and a keyboard-only user's very
                                // first `text field:` line is the evidence
                                // their keystroke landed. (`after == None`
                                // means the demo's commit-and-release just
                                // cleared the field — the `commit:` line
                                // above already reported that case.)
                                serial_println!("[gui    ] text field: \"{v}\"");
                            }
                            dirty = true;
                        } else if apps.owns_focus(&wm) {
                            // A shell window with no field focused: the
                            // app already saw this key above, and the
                            // Phase 3 fallback must stay off — its
                            // synthetic Space/Enter click would race the
                            // app that is handling the very same key
                            // (Enter would submit *and* click whatever
                            // sits under the cursor), and its arrows
                            // would double up with the app's selection.
                            // Esc still closes, as everywhere.
                            match k.code {
                                KeyCode::Shift | KeyCode::Ctrl | KeyCode::Alt => {}
                                KeyCode::Esc => {
                                    serial_println!("[gui    ] key Esc — closing focused window");
                                    maybe_close = true;
                                    dirty = true; // the reap must repaint
                                }
                                // Neither the app (`app_changed == false`)
                                // nor a text field (none is armed here —
                                // `focus_and_type` arms on focus, but a
                                // body click can still disarm) wanted this
                                // key: `wm.handle_key` below has nothing
                                // to forward to either, so it is about to
                                // fall out of the event loop *without a
                                // trace*. Log it. This is the exact hole
                                // the "Ctrl+3, then `help` produced
                                // nothing" bug hid in — a dropped
                                // printable was invisible on serial, so
                                // nothing could tell "the shell ate my
                                // key" from "the scancode never arrived".
                                // Keys the app *did* act on skip this arm:
                                // their own `[apps  ]` line is the record.
                                other if !app_changed => {
                                    serial_println!("[gui    ] key {other}")
                                }
                                _ => {}
                            }
                            wm.handle_key(k);
                            // Any visible change already set `dirty` in
                            // the `apps.on_key` call above; a key the app
                            // ignored moves nothing on screen.
                        } else {
                            match k.code {
                                // Modifier *state* only — the driver already
                                // baked shift/caps into Char keys; skip noise.
                                KeyCode::Shift | KeyCode::Ctrl | KeyCode::Alt => {}
                                KeyCode::Esc => {
                                    serial_println!("[gui    ] key Esc — closing focused window");
                                    maybe_close = true;
                                }
                                // Arrows move the *same* cursor the PS/2 mouse
                                // driver owns — never a second one. Two views
                                // must stay equal: `mouse::nudge` advances the
                                // driver's clamped absolute position (the 1 Hz
                                // `[gui] cursor at` line reads it), and a
                                // synthetic MouseEvent carrying the identical
                                // delta rides the normal `handle_mouse` path,
                                // so hover/widget tracking is exactly what a
                                // real packet would trigger. Same delta + the
                                // same clamp in both places ⇒ they agree. Shift
                                // = fine step.
                                KeyCode::ArrowUp
                                | KeyCode::ArrowDown
                                | KeyCode::ArrowLeft
                                | KeyCode::ArrowRight => {
                                    let step = if k.modifiers.shift {
                                        CURSOR_STEP_FINE
                                    } else {
                                        CURSOR_STEP
                                    };
                                    let (dx, dy) = match k.code {
                                        KeyCode::ArrowUp => (0, -step),
                                        KeyCode::ArrowDown => (0, step),
                                        KeyCode::ArrowLeft => (-step, 0),
                                        _ => (step, 0), // ArrowRight
                                    };
                                    input::mouse::nudge(dx, dy);
                                    event::push(Event::Mouse(MouseEvent {
                                        dx,
                                        dy,
                                        buttons: MouseButtons::default(),
                                    }));
                                }
                                // Space/Enter = synthetic click at the cursor:
                                // the exact LEFT_PRESSED → LEFT_RELEASED pair a
                                // real PS/2 click produces, fed through the same
                                // `handle_mouse` state machine — focus, drag
                                // capture, close-button and widget actions all
                                // behave identically. No parallel hit-test.
                                KeyCode::Char(' ') | KeyCode::Enter => {
                                    let (x, y) = input::mouse::position();
                                    serial_println!("[gui    ] keyboard click at ({x}, {y})");
                                    event::push(Event::Mouse(MouseEvent {
                                        dx: 0,
                                        dy: 0,
                                        buttons: MouseButtons {
                                            left: true,
                                            right: false,
                                            middle: false,
                                        },
                                    }));
                                    event::push(Event::Mouse(MouseEvent {
                                        dx: 0,
                                        dy: 0,
                                        buttons: MouseButtons::default(),
                                    }));
                                }
                                other => serial_println!("[gui    ] key {other}"),
                            }
                            wm.handle_key(k);
                            dirty = true;
                        }
                    }
                }
            }
        }

        // PERF (measurement only): one drain pass done — hand this batch's
        // numbers to the 1 Hz aggregator. Nothing above is affected.
        perf.on_drain(events_in_batch, mouse_in_batch);

        // Close anything the user dismissed (X button or Esc). Only when
        // something *might* have asked for it — `reap` allocates a tiny
        // Vec, and the loop runs at 100 Hz.
        if maybe_close {
            wm.reap();
        }

        if dirty {
            // PERF (measurement only): the compose/flush split, timed with
            // fenced TSC reads. The two calls themselves and every rule
            // around them (the `if dirty` gate especially) are untouched —
            // the timestamps only *bracket* them. compose_avg/max vs
            // flush_avg/max in the 1 Hz report says which half owns the
            // per-frame cost (full-screen clear vs 3 MiB hardware copy).
            let t0 = perf::tsc_now();
            wm.compose(gfx, &theme);
            let t1 = perf::tsc_now();
            gfx.flush();
            let t2 = perf::tsc_now();
            perf.on_frame(perf::us_between(t0, t1), perf::us_between(t1, t2));
        }

        // Yield CPU to any ready background threads before sleeping
        sched::yield_now();

        // Sleep until the next IRQ; the queue fills while we're halted.
        x86_64::instructions::hlt();

        // PERF (measurement only): count this wakeup *after* `hlt` returns,
        // so every increment is a proven resume — an IRQ actually brought
        // the CPU back (before `hlt` would count sleep *entries* instead;
        // the two coincide today because the loop has no other exit path,
        // but the post-`hlt` spot stays semantically "wakeups" either way).
        // wakes/s vs frames/s separates "the loop woke 100×/s but redrew
        // every one of them" from "it woke 100×/s and redrew only N".
        perf.on_wake();
    }
}

/// Background worker thread to verify preemptive/cooperative multitasking and syscalls.
extern "C" fn background_worker() {
    serial_println!("[worker ] background thread started!");

    // Phase 6 verification: execute real syscall instruction from thread
    let test_msg = "[worker ] live syscall SYS_WRITE verified!\n";
    syscall::syscall(
        syscall::table::SYS_WRITE,
        1, // stdout
        test_msg.as_ptr() as usize,
        test_msg.len(),
    );

    let mut iteration = 0u64;
    loop {
        // Sleep for 100 ticks (1 second at 100 Hz)
        sched::sleep_ticks(100);
        iteration += 1;
        let uptime = syscall::syscall(syscall::table::SYS_UPTIME, 0, 0, 0);
        serial_println!(
            "[worker ] alive: iteration #{iteration}, uptime via sys_uptime={uptime}s"
        );
    }
}

/// Discovers and arms Intel 82540EM (e1000) network card on PCI bus.
fn init_network(phys_offset: VirtAddr) {
    serial_println!("[net    ] scanning PCI bus for Ethernet network controllers...");
    let devices = drivers::pci::scan_bus();
    let mut nic_dev = None;
    for dev in devices {
        if dev.vendor_id == drivers::e1000::INTEL_VEND
            && (dev.device_id == drivers::e1000::E1000_DEV_82540EM
                || dev.device_id == drivers::e1000::E1000_DEV_82545EM
                || dev.device_id == drivers::e1000::E1000_DEV_82543GC
                || (dev.class_code == 0x02 && dev.subclass == 0x00))
        {
            serial_println!(
                "[net    ] found Intel E1000 NIC at {}:{}.0 (vendor={:#06x}, dev={:#06x}, BAR0={:#x})",
                dev.location.bus,
                dev.location.device,
                dev.vendor_id,
                dev.device_id,
                dev.bar0
            );
            nic_dev = Some(dev);
            break;
        }
    }

    if let Some(dev) = nic_dev {
        match drivers::E1000Driver::init(&dev, phys_offset.as_u64()) {
            Ok(driver) => {
                let mac = driver.mac;
                *drivers::E1000.lock() = Some(driver);
                serial_println!(
                    "[net    ] E1000 driver initialized! MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                );

                // Broadcast gratuitous ARP so the host/network learns our MAC
                let our_ip = net::NET.lock().ip;
                let _ = net::send_arp_request(our_ip);
            }
            Err(e) => {
                serial_println!("[net    ] E1000 initialization failed: {e}");
            }
        }
    } else {
        serial_println!("[net    ] no supported Ethernet NIC found on PCI bus");
    }
}


