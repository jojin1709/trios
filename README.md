# TriOS

A hobby OS merging the best of Windows, Linux and Android — written in Rust.

## Status: Phase 0 + 1 + 2 + 3 (OS-000 / OS-001 / OS-002 / OS-003)

Bootable UEFI disk image with serial console, GDT/TSS/IDT, 8259 PIC, PIT
timer, frame allocator, a 16 MiB kernel heap, a double-buffered framebuffer
with a bitmap font — **and, since Phase 3: an interactive GUI desktop with
PS/2 keyboard + mouse input, an event queue, a window manager (drag, resize,
focus, z-order) and a small widget toolkit** (labels, buttons, panels,
progress bars, sliders). The Phase 2 boot splash is skipped in favour of
landing straight on the desktop; its code is retained in `graphics/splash.rs`.

Verified boot log (VirtualBox 7.2.14, headless — release image):

```
+--------------------------------------------------------+
|        TriOS - Phase 3: Interactive GUI boot           |
|    no_std x86_64 | UEFI | serial + gfx + PS/2 + GUI    |
+--------------------------------------------------------+
[serial ] COM1 ready
[boot   ] kernel image offset 0x8000000000
[boot   ] kernel stack 0x10000001000..0x10000015000
[boot   ] direct map of physical memory at 0x20000000000
[memory ] 109 regions | usable 236.3 MiB | bootloader 620 KiB | RAM top 0x100000000
[gdt    ] GDT + TSS loaded (IST1 double-fault stack top 0x800003e2c0)
[idt    ] IDT loaded (#BP #DF #PF #GP + IRQ0 @ vector 32)
[pic    ] 8259s remapped to vectors 32..47, IRQ0 unmasked (rest masked)
[pit    ] channel 0 armed at 100 Hz (divisor 11931)
[heap   ] 16384 KiB at 0x2000013c000 (phys 0x13c000) | 16777216 bytes free
[memory ] page tables: active L4 at phys 0x101000 (6/512 entries used)
[heap   ] alloc test PASS: vec[0..64] sum=14112 last=0xdeadbeef
[ps2    ] config byte 0x67 -> 0x47 (IRQ1 + IRQ12 routed)
[ps2    ] keyboard: scanning enabled (scancode set 1)
[ps2    ] mouse: aux port enabled, data reporting on
[ps2    ] PIC unmasked: IRQ1 (vector 33) + IRQ12 (vector 44)
[irq    ] first PIT tick delivered (IRQ0 -> vector 32)
[irq    ] interrupts enabled (sti)
[timer  ] tick test PASS: 101 ticks in ~1 s
[gfx    ] framebuffer 1024x768, stride 1024, format Bgr, 4 B/px, 3072 KiB
[gfx    ] backbuffer 3072 KiB on heap | 13631176 heap bytes free
[gui    ] desktop ready: 1024x768, 3 windows
[timer  ] heartbeat: uptime 1 s, ticks=100
[timer  ] heartbeat: uptime 2 s, ticks=200
[timer  ] heartbeat: uptime 3 s, ticks=300
```

## Quick start

### Prerequisites

| Tool | Version / notes |
|------|-----------------|
| Rust nightly | auto-installed by rustup (see `rust-toolchain.toml`) |
| rustup | with targets `x86_64-unknown-none` and `x86_64-unknown-uefi` |
| VirtualBox | 7.x (verified with 7.2.14) |
| QEMU + OVMF | optional, provided for completeness but untested on this machine |

All Rust nightly components (llvm-tools-preview, rust-src, rustfmt, clippy)
and the two custom targets are auto-installed by rustup when you run any
`cargo` command inside the repository for the first time.

### Build

```powershell
cd D:\trios

# Release (recommended — smaller image, faster boot)
cargo build --release

# Debug (with debug symbols)
cargo build
```

The build produces a UEFI disk image at:

- `target/release/trios-uefi-release.img`
- `target/debug/trios-uefi.img`

### Run (VirtualBox — verified)

```powershell
# Default: boots the release image headless for 15 s, dumps serial.log, exits
.\run-virtualbox.ps1

# Customise
.\run-virtualbox.ps1 -Seconds 20 -Image target\debug\trios-uefi.img

# Visible VirtualBox window for visual UAT (waits until you close the window)
.\run-virtualbox.ps1 -Gui

# Keep the VM registered for interactive debugging
.\run-virtualbox.ps1 -KeepVm
```

The script writes all VirtualBox artefacts (VM config, machine home) inside
the repository under `.vbox-home\`, so nothing leaks to your user profile.
The serial console is captured to `serial.log` in the repo root.

The script forces PS/2 input devices (`modifyvm --mouse ps2 --keyboard ps2`).
TriOS only implements PS/2, while VirtualBox 7 defaults `OSType=Other_64` to
USB Tablet/USB Keyboard (HID) — without this the host input never reaches the
guest's PS/2 drivers (frozen cursor, dead keyboard).

### Run (QEMU — untested on this machine)

```powershell
# Requires qemu-system-x86_64 on PATH and OVMF firmware.
# See the header of run-qemu.ps1 for OVMF auto-detection details.
.\run-qemu.ps1
```

QEMU was not installed on the development machine; the instructions are
provided for completeness and should work on any host with QEMU + OVMF.
Report issues if you hit them.

## Project layout

```
D:\trios\
├── Cargo.toml                 # workspace root + UEFI image build
├── build.rs                   # copies kernel ELF → bootloader → disk image
├── rust-toolchain.toml        # pins nightly + components + targets
├── .cargo/
│   └── config.toml            # bindeps flag, target defaults
├── run-virtualbox.ps1         # VirtualBox boot harness, headless or -Gui (verified)
├── run-qemu.ps1               # QEMU + OVMF boot harness (untested)
└── kernel/
    ├── Cargo.toml
    └── src/
        ├── main.rs            # boot sequence, entry point, desktop event loop
        ├── serial.rs          # COM1 UART driver + serial_print! macros
        ├── gdt.rs             # GDT + TSS (IST1 double-fault stack)
        ├── interrupts.rs      # IDT, fault handlers, 8259 PIC, IRQ0/1/12
        ├── timer.rs           # PIT 100 Hz tick counter
        ├── event.rs           # Phase 3: event types + fixed ring queue (IRQ→main)
        ├── input/             # Phase 3: PS/2 devices
        │   ├── mod.rs         # controller init, port helpers, IRQ unmask
        │   ├── keyboard.rs    # IRQ1 → scancode set 1 → KeyEvent
        │   └── mouse.rs       # IRQ12 → 3-byte packets → MouseEvent + cursor
        ├── memory.rs          # memory-map summary, frame allocator, paging
        ├── allocator.rs       # 16 MiB kernel heap (linked-list)
        ├── mmio.rs            # volatile MMIO wrapper (Phase 4+ seam)
        ├── panic.rs           # panic handler with register dump
        ├── graphics/          # Phase 2: framebuffer, font, splash (retained)
        │   ├── mod.rs
        │   ├── color.rs       # Color, RGB/BGR packing, palette, hsv()
        │   ├── framebuffer.rs # double-buffered Graphics canvas + primitives
        │   ├── font.rs        # 8x8 bitmap font (font8x8 crate) text renderer
        │   └── splash.rs      # boot splash (skipped in Phase 3, kept for later)
        └── gui/               # Phase 3: the desktop
            ├── mod.rs         # toolkit surface (Theme, Widget, WindowManager)
            ├── theme.rs       # one color table for the whole GUI
            ├── widgets.rs     # Label, Button, Panel, ProgressBar, Slider
            └── wm.rs          # windows, z-order, drag/resize, cursor, compose
```

## Boot sequence (what happens at startup)

1. UEFI firmware loads the bootloader (built by the `bootloader` crate v0.11).
2. Bootloader exits UEFI boot services, sets up a new page table, maps the
   kernel ELF at a high virtual offset, maps all physical RAM (direct map),
   **maps the GOP framebuffer**, and jumps to `kernel_main`.
3. Kernel initialises COM1 serial (115200 baud), prints the banner.
4. Memory map is parsed: usable frames are collected; a 16 MiB contiguous
   region is reserved for the heap.
5. GDT + TSS are loaded (IST1 stack catches double faults).
6. IDT is loaded (#BP, #DF, #PF, #GP, IRQ0, IRQ1, IRQ12).
7. 8259 PICs are remapped to vectors 32..47; only IRQ0 is unmasked (for now).
8. PIT channel 0 is programmed to 100 Hz.
9. Heap is initialised; a `Vec` smoke test runs.
10. The mouse driver is given the framebuffer geometry (cursor clamping).
11. PS/2 input is brought up with interrupts still **disabled**: controller
    config byte (IRQ routing), keyboard scancode streaming, mouse aux port +
    data reporting; then IRQ1/IRQ12 are unmasked on the PICs
    (master `0xFC` — note the **cascade bit**, without which IRQ12 can never
    reach the CPU — and slave `0xEF`).
12. Interrupts are enabled (`sti`); the first PIT tick is confirmed.
13. Framebuffer is taken from the boot info; a `Graphics` canvas with a
    full backbuffer is allocated on the heap.
14. The **desktop** takes over (see below): three demo windows, event loop,
    serial heartbeat once per second.

## The desktop (Phase 3 showcase)

The compositor rebuilds the whole backbuffer each frame (background →
windows bottom-to-top → software cursor) and presents it with a single
bulk `memcpy` — the same double-buffered flush the Phase 2 splash used.
The event cycle:

```
hlt → (IRQs fill the ring queue while the CPU sleeps) → drain ALL events
    → act (move windows, update labels, animate) → compose → flush → hlt
```

Three demo windows are on screen at boot (1024×768 default; layout fits
640×480 too):

| Window | Size @ pos | Contents |
|--------|------------|----------|
| **Welcome** (focused) | 400×200 @ (80, 60) | title, subtitle, a `Click me` button with a live click counter, an animated progress bar, a controls hint |
| **About** | 300×180 @ (200, 140) | live uptime, heap-free bytes, resolution and tick count (refreshed on the 1 Hz heartbeat) |
| **Playground** | 300×150 @ (320, 220) | a draggable slider whose value feeds a label and a mirrored progress bar |

### Controls

| Action | How |
|--------|-----|
| Move the cursor | mouse (PS/2, IRQ12), or **arrow keys** — keyboard fallback when the hypervisor forwards no mouse packets (hold `Shift` for a fine 4 px step; 16 px otherwise) |
| Click at the cursor | left mouse button, or **`Space`/`Enter`** — keyboard fallback, fed through the exact same click path as a real PS/2 press+release |
| Move a window | drag its titlebar |
| Resize a window | drag the bottom-right grip (two diagonal ticks) |
| Raise + focus a window | click anywhere on it (focused = accent titlebar + border) |
| Close a window | click the `X` in its titlebar, or press `Esc` (focused window) |
| Click the button | left-click `Click me` — counter increments, serial logs the press |
| Drive the slider | press + drag on the Playground slider — value and mirror bar follow the thumb even when dragged off the track |

The keyboard fallback exists because some hosts (VirtualBox 7 without
Guest Additions, `mouseintegration off`) deliver **zero** PS/2 mouse
packets — serial shows `[ps2    ] mouse irq count = 0` forever. Arrow keys
move the *same* software cursor the mouse driver owns (same state, same
screen clamp), and `Space`/`Enter` inject a synthetic click through the
window manager's normal mouse-event handling, so focus, drag capture,
close buttons and widgets all behave identically. On desktop-ready the
guest logs `[gui    ] keyboard cursor active: arrows=move,
space/enter=click, esc=close`.

All keyboard input (scancode set 1, US-QWERTY, Shift/CapsLock-aware) and
mouse packets arrive as events via IRQ1/IRQ12 → fixed 512-slot ring queue →
the desktop loop; no input work ever happens in interrupt context.

## User acceptance test (visual)

Serial verification above is complete; the *visual + interactive* check
needs a human (no framebuffer capture in the headless harness). Use the
`-Gui` flag to boot with a visible VirtualBox window:

```powershell
.\run-virtualbox.ps1 -Gui
# interact with the desktop in the VM window; close it when done
# (the script then prints serial.log and leaves the VM registered)
```

Things to confirm on screen:

- Desktop background + three windows appear right after the tick test —
  no splash in between. Welcome is focused (accent titlebar/border).
- The mouse cursor (white arrow, black outline) tracks PS/2 movement and
  never leaves the screen edges.
- Welcome's progress bar sweeps 0→100% every 2 s; the About window's
  uptime/heap/resolution/tick labels update once per second.
- Click `Click me`: the counter increments and
  `[gui    ] button press count = N` appears in serial.log.
- Drag the Playground slider: the thumb follows the cursor (turns green
  while dragging), the value label and mirror bar update; releasing logs
  `[gui    ] slider: ... -> 0.NN`.
- Drag a titlebar to move a window, the bottom-right grip to resize
  (min 100×60); click another window to raise + focus it.
- Press `Esc` (or click `X`) on a window: it closes, serial logs
  `[gui    ] window '...' closed`.
- **BGR sanity check**: accent-blue titlebars/borders/progress must look
  *blue*, not red (VirtualBox GOP reports `Bgr`; `color.rs::pack` swaps).
- No mouse tearing/trails: the cursor is composited last over a fully
  rebuilt frame each refresh.

## Serial log format

| Prefix | Module |
|--------|--------|
| `[serial ]` | COM1 UART init |
| `[boot   ]` | kernel image/stack/direct-map info |
| `[memory ]` | memory map / page-table dump |
| `[gdt    ]` | GDT + TSS load |
| `[idt    ]` | IDT load |
| `[pic    ]` | 8259 remap + mask |
| `[pit    ]` | PIT frequency programming |
| `[heap   ]` | heap init + allocation smoke test |
| `[ps2    ]` | PS/2 controller/keyboard/mouse bring-up, IRQ unmask |
| `[irq    ]` | interrupt enable, first tick, exception logs |
| `[timer  ]` | tick test + heartbeat |
| `[gfx    ]` | framebuffer info, backbuffer |
| `[gui    ]` | desktop ready, clicks, slider values, key presses, window close |

## Known limitations

- **Serial lock and CPU exceptions**: if a CPU exception fires while a log
  line is being written, the handler would spin on the held serial lock and
  deadlock. Hardware-interrupt handlers are safe (interrupts are disabled
  while the lock is held — see `serial::write_fmt`). A lock-free ring buffer
  remains the Phase 4+ answer if this ever bites.
- **No filesystem, scheduler, processes, networking, or text input fields**:
  all Phase 4+. Phase 3 gives the GUI its event loop and window chrome;
  real applications come next.
- **Full-frame redraw every refresh**: the compositor rebuilds the whole
  backbuffer (fine at 100 Hz for a few windows); dirty-rect composition is
  a Phase 4+ optimisation.
- **Framebuffer format**: Rgb/Bgr/U8 are handled; Bitmask/BltOnly pixel
  formats are not (the UEFI bootloader panics on those anyway).
- **BIOS boot not supported**: UEFI only (by design).

## Toolchain details

- **Nightly required**: the `bootloader` crate's build script invokes cargo
  with `-Zbuild-std` (nightly-only), and the kernel uses `abi_x86_interrupt`
  and `alloc_error_handler` language items.
- **Targets**: `x86_64-unknown-none` (kernel), `x86_64-unknown-uefi`
  (bootloader stub).
- **Edition**: 2024.
- **Key crates**: `bootloader`/`bootloader_api` 0.11, `x86_64` 0.15,
  `uart_16550` 0.8, `pic8259` 0.11, `linked_list_allocator` 0.10,
  `lazy_static` 1.5 (spin_no_std), `spin` 0.9, `font8x8` 0.3 (no_std;
  glyph data is public domain, crate is MIT).

## Verification

```powershell
cargo fmt -- --check   # should be silent
cargo clippy -- -D warnings           # root crate
cargo clippy --target x86_64-unknown-none -p kernel -- -D warnings
cargo test                            # 0 tests (no_std kernel; boot-tested)
.\run-virtualbox.ps1                  # full boot verification (serial)
```

## License

MIT
