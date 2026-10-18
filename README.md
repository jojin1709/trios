> [!CAUTION]
> **STRICT LEGAL & COPYRIGHT WARNING:**
> **All source code, kernel architecture, designs, documentation, and assets in this repository are strictly copyrighted by JOJIN JOHN ([@jojin1709](https://github.com/jojin1709)).** Copying, cloning, mirroring, redistributing, or using any part of this code or project without explicit prior written permission from the author is **STRICTLY PROHIBITED** and will result in immediate legal action, DMCA takedown notices, and civil/criminal copyright infringement proceedings.

<div align="center">

# TriOS

### An operating system merging the best of Windows, Linux and Android — written in Rust from scratch.

**Developed by [JOJIN JOHN](https://github.com/jojin1709)**

<p><strong>no_std x86_64 &nbsp;•&nbsp; UEFI Native &nbsp;•&nbsp; Preemptive Scheduler &nbsp;•&nbsp; Composited GUI Desktop</strong></p>

```powershell
# Build and run the interactive GUI directly in VirtualBox
cargo build --release
powershell -ExecutionPolicy Bypass -File .\run-gui.ps1
```

---

[![Rust Nightly](https://img.shields.io/badge/rust-nightly--2024-red.svg)](https://www.rust-lang.org/)
[![Target x86_64](https://img.shields.io/badge/target-x86__64--unknown--none-blue.svg)](https://doc.rust-lang.org/nightly/rustc/platform-support/x86_64-unknown-none.html)
[![Bootloader UEFI](https://img.shields.io/badge/bootloader-UEFI--v0.11-green.svg)](https://crates.io/crates/bootloader)
[![License: All Rights Reserved](https://img.shields.io/badge/License-All_Rights_Reserved-red.svg)](LICENSE)
[![LinkedIn](https://img.shields.io/badge/LinkedIn-Connect-0A66C2?style=flat&logo=linkedin&logoColor=white)](https://www.linkedin.com/in/jojin-john/)

---

</div>

> [!TIP]
> **Visual Live Demo:** Run `powershell -ExecutionPolicy Bypass -File .\run-gui.ps1` to open the active VirtualBox window directly with full mouse & keyboard controls.

## Table of Contents

- [Overview](#overview)
- [Architecture & Status](#architecture--status)
- [Key Features](#key-features)
- [Live Preview](#live-preview)
- [Quick Start](#quick-start)
  - [Prerequisites](#prerequisites)
  - [Build](#build)
  - [Run in VirtualBox](#run-in-virtualbox)
- [Desktop & Controls](#desktop--controls)
  - [Keyboard Shortcuts](#keyboard-shortcuts)
  - [Mouse & Keyboard Fallback](#mouse--keyboard-fallback)
- [Project Layout](#project-layout)
- [Boot Sequence](#boot-sequence)
- [Roadmap](#roadmap)
- [License](#license)

---

## Overview

TriOS is a modern, bare-metal operating system built from the ground up in Rust without any standard library (`#![no_std]`). It boots via UEFI into a 32-bit color 1080p GOP framebuffer, initializes hardware interrupt controllers, programs the timer, sets up virtual memory with a 32 MiB heap, and boots into a futuristic glass-aesthetic graphical window manager powered by an event-driven loop and a preemptive multitasking kernel scheduler.

---

## Architecture & Status

TriOS has completed **Phase 0 through Phase 22**:

| Phase | Milestone | Status | Key Subsystems |
| :--- | :--- | :---: | :--- |
| **Phase 0** | Bootloader & Image Build | **Done** | UEFI boot, direct physical memory map, dual-target cargo workspace |
| **Phase 1** | Bare-Metal CPU Core | **Done** | GDT/TSS with IST1 stack, IDT, 8259 PIC remapping, PIT 100 Hz, COM1 UART serial, 32 MiB heap |
| **Phase 2** | Framebuffer & Graphics | **Done** | Double-buffered GOP canvas, 8×8 bitmap font renderer, drawing primitives |
| **Phase 3** | Input & Window Manager | **Done** | PS/2 keyboard & mouse drivers, event queue, z-ordered window manager (drag, resize, focus, close) |
| **Phase 4** | Shell, VFS & Text Fields | **Done** | In-memory RAM filesystem (`ramfs`), TextInput widgets, Files Explorer, Text Editor, Terminal shell |
| **Phase 5** | Preemptive Multitasking | **Done** | Thread control blocks, cooperative/preemptive scheduler, context switching (`switch_context`), background worker |
| **Phase 6** | User Descriptors & Syscalls | **Done** | GDT User DPL-3 code/data, TSS RSP0 privilege stack, IA32_EFER/STAR/LSTAR/FMASK MSRs, fast `syscall` dispatcher |
| **Phase 7** | PCI Bus & AHCI SATA Driver | **Done** | PCI configuration space scanner, AHCI Generic Host Control MMIO, SATA disk device detection |
| **Phase 8** | Standalone ELF Loader & Exec | **Done** | ELF64 header & PT_LOAD segment parser, page table NO_EXECUTE clearing, binary execution via terminal (`elfinfo`, `elfrun`) |
| **Phase 9** | Persistent Storage & FAT VFS | **Done** | GUID Partition Table (GPT) parser, AHCI DMA sector reading, FAT12/16/32 volume mounter, disk traversal (`gpt`, `fatls`, `disks`) |
| **Phase 10** | Intel E1000 NIC & Network Stack | **Done** | Intel 82540EM MMIO driver, RX/TX DMA rings, Ethernet II framing, ARP cache/resolution, IPv4 dispatcher, ICMP Echo ping client (`ping`, `arp`, `netinfo`) |
| **Phase 11** | UDP, DNS Resolver & CMOS RTC | **Done** | UDP datagram framing, RFC 1035 DNS client over port 53 (`dns <host>`), hardware CMOS RTC driver (`date`), live Taskbar clock widget |
| **Phase 12** | ACPI Subsystem & Power Control | **Done** | RSDP/XSDT parser, FADT PM1a control, MADT CPU core counting, ACPI S5 shutdown (`poweroff`), hardware reboot (`reboot`), `acpi` inspection |
| **Phase 13** | TCP Protocol & HTTP/1.0 Client | **Done** | RFC 793 TCP state machine (SYN/ACK, seq/ack tracking), bare-metal HTTP/1.0 client (`curl <url>`) |
| **Phase 14** | Desktop Web Browser Application | **Done** | GUI Browser app (`Ctrl+4`) with address bar, HTML tokenizer, formatted layout, and scrolling |
| **Phase 15** | System Monitor & Task Manager | **Done** | Real-time CPU/RAM gauges, thread table, compositor FPS telemetry, and background worker spawning (`Ctrl+5`) |
| **Phase 16** | Modern Desktop UI/UX & TriSweeper | **Done** | Desktop shortcut icons, modern taskbar with active indicators, window controls (`_`, `□`, `✕`), and TriSweeper puzzle game (`Ctrl+6`) |
| **Phase 17** | Display + Visual Foundation | **Done** | Native 1080p GOP resolution, futuristic glass theme (`gui/theme.rs`), cached vertical gradient wallpaper with dot grid & faint emblem, floating centered dock, 32px titlebars with 10px rounded corners & drop shadows, floating toast, and 32 MiB heap |
| **Phase 18** | Desktop Experience | **Done** | Procedural vector icons (`gui/icons.rs`), Start Menu with app search & ACPI power popup, Quick Settings tray with Night Light filter & sliders, desktop icon selection & double-click, reusable Context Menu, top-right Toast notification system, Alt+Tab switcher & Super key |
| **Phase 19** | Typography & System Apps | **Done** | Fontdue TTF vector typography (`Inter` & `JetBrains Mono`), Settings App (`Ctrl+7`), Boot splash animation, Lock Screen (`Ctrl+L`), app polish |
| **Phase 20** | Memory Management | **Done** | Bitmap physical frame allocator, themed page-fault screen, thread stack guard pages, kernel virtual memory API (`map_page`/`unmap_page`/`alloc_pages`/`free_pages`), hybrid slab/size-class heap allocator (32B..2048B), `meminfo` shell command & SysMon physical frame gauges |
| **Phase 21** | Real Processes & Ring 3 | **Done** | Per-process PML4 page tables (shared kernel half, isolated user half), Ring 3 execution via `iretq`/`sysretq`, full syscall suite (`exit`, `write`, `read`, `yield`, `uptime`, `getpid`, `sleep_ms`, `mmap`, `munmap`, `open`, `close`), strict user pointer validation, process table & teardown, user fault isolation, `ps` & `kill` commands |
| **Phase 22** | VFS & Persistent Writes | **Done** | Unified Virtual File System (`/` ramfs, `/disk` persistent FAT), AHCI DMA sector writing (`write_sectors`), FAT12/16/32 persistent write engine (create, write, append, delete, mkdir, rename), Editor Save & Files Explorer persistence, `fsck-lite` consistency checker |
| **Phase 23** | Cleanup, Hardening, Docs | **Done** | UI feature audit (real brightness dimming & DND, cosmetic preview tags), host parser test suite (`cargo test`), Ring 3 syscall fuzz test (`fuzz.elf`), architecture docs (`memory-map.md`, `syscall-table.md`, `boot-sequence.md`) |

---

## Key Features

- **Unified Virtual File System & Persistent Writes (Phase 22)**: Complete storage stack with cross-filesystem dispatch and persistent block writing:
  - **Unified VFS Hierarchy**: Mount architecture dispatching `/` to fast in-memory `ramfs` and `/disk` to the active persistent FAT filesystem on SATA storage. All path lookups, directories, and file streams resolve transparently across filesystem boundaries.
  - **AHCI DMA Sector Writing**: Implemented `write_sectors` in `drivers/ahci.rs` utilizing ATA command `ATA_CMD_WRITE_DMA_EXT` (`0x35`), setting the write transfer bit (bit 6) in the AHCI Command Header, staging data through physically contiguous DMA buffers, and awaiting PxCI hardware completion.
  - **Multi-FAT Persistent Write Engine**: Robust write support across FAT12, FAT16, and FAT32 filesystems (`fs/fat.rs`), featuring FAT sector writeback synchronized across both FAT1 and FAT2 tables, free cluster scanning & zeroed allocation (`alloc_cluster`), cluster chain reclamation (`free_cluster_chain`), standard 8.3 space-padded uppercase filename conversion (`to_dos_8_3`), directory expansion, subfolder creation (`mkdir`) with `.` and `..` parent cluster references, file creation/overwriting/appending (`write_file`, `append_file`), and deletion (`delete_file`).
  - **Files Explorer & Text Editor Integration**: Files Explorer provides interactive `+File`, `+Folder`, `Rename`, and `Delete` actions with keyboard (`Delete` key) and pointer support operating on real disk storage under `/disk`. Text Editor seamlessly loads from and saves to `/disk` files via VFS.
  - **Volume Consistency Checker (`fsck-lite`)**: Inspects BPB parameters, scans FAT tables for free/used/bad cluster statistics, traverses directory trees validating cluster link integrity, and reports volume health.
- **Real Processes & Ring 3 Userspace (Phase 21)**: True user/kernel separation and fault isolation:
  - **Per-Process PML4 Address Spaces**: Each process is given an isolated 4-level x86_64 page table. The upper kernel half (entries 1..511) is cloned from boot CR3 to preserve kernel image, direct physical map, stack, and hardware drivers; entry 0 (0..512 GiB) is strictly isolated and private to the user process.
  - **Ring 3 CPU Transition**: Clean transition from CPL 0 to CPL 3 using `iretq` with user data selector `0x1B` (RPL 3) and user code selector `0x23` (RPL 3), initializing a dedicated 16 KiB user stack at 2 GiB (`0x0000_0000_7FFF_0000`).
  - **Fast Syscall Subsystem**: Configures `IA32_STAR`, `IA32_LSTAR`, and `IA32_FMASK` for hardware `syscall`/`sysretq`. Automatically swaps to a dedicated kernel syscall stack (`KERNEL_SYSCALL_RSP`) on entry, preserves callee registers, dispatches the request, and executes `sysretq` back to Ring 3.
  - **Full Syscall Suite**: Implements `SYS_EXIT` (0), `SYS_WRITE` (1), `SYS_READ` (2), `SYS_YIELD` (3), `SYS_UPTIME` (4), `SYS_GETPID` (5), `SYS_SLEEP_MS` (6), `SYS_MMAP` (7), `SYS_MUNMAP` (8), `SYS_OPEN` (9), and `SYS_CLOSE` (10).
  - **Strict User Pointer Validation**: Function `validate_user_buffer` checks canonical user range (`< 0x0080_0000_0000`), verifies non-null pointers, walks page table levels (L4 -> L3 -> L2 -> L1) to ensure `PRESENT` and `USER_ACCESSIBLE` flags, and validates `WRITABLE` for buffer destinations.
  - **User Fault Isolation**: When a Ring 3 process causes an exception (e.g. null pointer dereference in `/bin/fault.elf`), the `#PF` handler intercepts it, logs the faulting address and user RIP, cleanly terminates that process, frees its user physical frames and page tables, restores the baseline kernel PML4, and re-enables interrupts—leaving the kernel, GUI, and desktop applications running smoothly.
  - **Process Management Tools**: Terminal shell commands `ps` (listing active processes and threads with PID, TID, state, and frame count) and `kill <pid>` (terminating user processes).
- **Memory Management Subsystem & Slab Allocator (Phase 20)**: Complete physical and virtual memory overhaul:
  - **Bitmap Frame Allocator**: O(1) bit-indexed physical frame manager covering up to 4 GiB of RAM, initialized directly from the UEFI memory map (`BootInfo::memory_regions`), tracking total, allocated, and free frames.
  - **Kernel Virtual Memory API**: Safe primitives `map_page`, `unmap_page`, `alloc_pages`, and `free_pages` with granular architecture-neutral permission flags (`PageFlags`: `KERNEL_DATA`, `KERNEL_RO`, `KERNEL_CODE`, `USER_DATA`, `USER_CODE`).
  - **Thread Stack Guard Pages**: Thread stacks allocated with an unmapped 4 KiB guard page at the bottom (`STACK_SLOT_PAGES`), guaranteeing immediate `#PF` interception upon stack overflow. Automatic RAII deallocation cleans up frames on thread exit.
  - **Themed Page Fault Diagnostics Screen**: Low-level exception vector 14 handler printing CR2, error code, RIP, and RSP to serial and rendering an emergency themed panic card with crash diagnostics directly to the hardware scanout framebuffer without heap dependencies.
  - **Hybrid Size-Class Slab Allocator**: 7 power-of-two slab caches (32B, 64B, 128B, 256B, 512B, 1024B, 2048B) backed by `LockedHeap`, offering instantaneous small object allocation while preventing heap fragmentation.
  - **Telemetry & Gauges**: Terminal `meminfo` command displaying physical RAM usage, slab allocation stats, and UEFI region summary; System Monitor (`Ctrl+5`) gauges updated with real-time physical frame counts.
- **TTF Vector Typography Engine (Phase 19)**: Replaced the primitive 8x8 bitmap font with authentic TrueType vector rendering powered by `fontdue` in `#![no_std]`. Embeds `Inter-Regular.ttf` for UI text and `JetBrainsMono-Regular.ttf` for code/terminals with sub-pixel rasterization, pre-cached ASCII glyphs, and direct framebuffer alpha blending.
- **Settings App & Live Theme Customizer (Phase 19)**: Modern 6-tab system preferences control center (`Ctrl+7`): Display (GOP resolution & linear framebuffer), Personalization (live accent color swatches: Cyan, Violet, Green, Orange, Pink; procedural wallpapers; glass transparency toggle), Network (Intel e1000 telemetry & IP configuration), Sound (HDA volume control), Date & Time (hardware CMOS RTC clock & IST timezone), and About (hardware specs & **JOJIN JOHN** credits).
- **Boot Splash & Lock Screen (Phase 19)**: Animated boot splash screen with glowing TriOS triad procedural emblem and smooth progress bar; lock screen (`Ctrl+L` or Start Menu) featuring frosted glassmorphism card, user avatar, live CMOS clock, administrator credentials, and session unlock.
- **Enhanced System Applications (Phase 19)**: Files Explorer file sizes and file-type badges (`[DIR]`, `[RS]`, `[TXT]`, `[ELF]`), Text Editor line numbers gutter (`> 1| code`), and Terminal shell prompt `jojin@trios:/$` with command history (Up/Down) and scrollback navigation (PageUp/PageDown).
- **Desktop Experience & Procedural Vector Icons (Phase 18)**: Complete high-resolution desktop experience with 12 handcrafted vector icons (`gui/icons.rs`) drawn at 32x32 and 48x48 (`This PC`, `Files`, `Editor`, `Terminal`, `Browser`, `SysMon`, `Mines`, `Settings`, `Trash`, and file-type badges `[DIR]`, `[RS]`, `[TXT]`, `[ELF]`) using theme tokens.
- **Desktop Icon Interaction & Context Menu (Phase 18)**: Single-click desktop icon selection with rounded rect highlight, double-click application launch via PIT monotonic ticks, centered labels with subtle drop shadows. Right-click desktop opens a sleek glass context menu (`New Folder`, `Refresh`, `Display Settings`, `Open Terminal here`). Files Explorer and Text Editor also provide dedicated context menus.
- **Glass Start Menu with Live App Search (Phase 18)**: Dock Start button or Windows/Super key opens an elevated glass panel above the dock. Includes real-time search filtering across 12 pinned applications, 4x3 pinned application grid, recent items list, user profile chip (`"Jojin"`, Administrator), and ACPI power popup (`Sleep`, `Restart` via ACPI reset, `Shut down` via ACPI S5 poweroff). Dismissable with Esc or clicking outside.
- **Quick Settings & Night Light Color Filter (Phase 18)**: Dock system tray toggle opens an interactive glass Quick Settings panel with quick-action tiles for Network, Do Not Disturb, and Night Light (which dynamically applies an authentic warm tint filter during screen composition), plus interactive Volume and Brightness level sliders.
- **Toast Notification Queue (Phase 18)**: Multi-toast notification system stacking up to 3 glass notification cards at the top-right with 4-second auto-dismissal, glowing cyan status bars, and distinct severity kinds (`Info`, `Success`, `Warning`, `Error`). Connected to network link events, background worker completion, and Text Editor file saves via `kernel::gui::notify()`.
- **Alt+Tab Window Switcher (Phase 18)**: Window switching overlay that previews open window titles and icons, cycles with Tab, and activates the selected window upon Alt release.
- **Futuristic Glass UI & 1080p Visual Foundation (Phase 17)**: Native 1920x1080 high-definition desktop layout backed by a 32 MiB kernel heap. Implements a curated design system in `kernel/src/gui/theme.rs` (`#0B0F1A` base, `#141B2D` surface, `#1C2540` highlight, cyan/violet accents, 10px window radius, 6px button radius, 32px titlebar, 56px dock, 16px drop shadows). Features a procedural vertical gradient wallpaper with a subtle 24px dot-matrix grid and a faint geometric TriOS triad logo, rendered once and cached for 60+ FPS compositing.
- **Floating Centered Glass Dock & System Tray (Phase 17)**: Elevated floating dock (`56px` height) centered at bottom with rounded glass aesthetic. Hosts the stylized TriOS start menu button, 6 modern procedural app icons (`Files`, `Editor`, `Terminal`, `Browser`, `SysMon`, `Mines`) with hover highlights and running status dots, system separator, network signal bars, audio speaker, and 2-line live IST clock (`UTC+5:30`) and date.
- **Glass Window Chrome & Modern Window Controls (Phase 17)**: 32px window titlebars with subtle borders, 10px rounded corners, gradient focus accenting, elevation drop shadows, and 3 titlebar control buttons: Minimize (`—`), Maximize/Restore (`□` / `❐`), and Close (`✕`).
- **Floating Status Toast (Phase 17)**: Non-intrusive floating bottom-left rounded glass notification pill displaying input capture guidance with automatic dismissal after 5 seconds or on initial click.
- **Desktop UI/UX Overhaul & Shortcut Icons (Phase 16)**: Modern desktop shell with 6 clickable desktop shortcut icons (`This PC`, `Files`, `Editor`, `Terminal`, `Browser`, `Mines`), refined Slate & Charcoal palette, 3D window elevation shadows, window controls (`_`, `□`, `✕` with red hover), and taskbar running indicator pills.
- **TriSweeper Retro Desktop Game (Phase 16)**: Interactive 9x9 Minesweeper game (`Ctrl+6`) with 10 hidden mines, first-click protection, recursive zero-neighbor flood fill, HUD with live mine counter, timer, reset face button (`:-) / B-) / X-(`), and Dig/Flag modes.
- **System Monitor & Task Manager (Phase 15)**: Graphical task manager displaying real-time CPU telemetry (core count, TSC frequency, uptime), physical memory & heap usage gauges with visual progress bar, compositor FPS & network telemetry, and live kernel thread table with interactive background task spawning (`Ctrl+5` or taskbar button).
- **GUI Web Browser Application (Phase 14)**: Interactive desktop web browser window featuring a URL address bar, 'Go' button, Enter key navigation, HTML tag tokenizer and text formatting engine, and scrollable viewport with line counter footer, accessible via taskbar button or `Ctrl+4` hotkey.
- **TCP Protocol Stack & HTTP/1.0 Web Client**: Full RFC 793 Transmission Control Protocol (pseudo-header checksums, 3-way handshake SYN/SYN-ACK/ACK, stream transmission, and FIN/ACK teardown) and bare-metal HTTP/1.0 client (`curl <url>`) resolving DNS and fetching live web pages directly over the Internet from within the bare-metal OS.
- **ACPI Power Management & Hardware Reset**: Full ACPI table discovery (RSDP, XSDT, FADT, MADT, DSDT AML), CPU core detection, ACPI S5 poweroff (`poweroff`), and hardware reboot (`reboot`).
- **UDP Protocol & RFC 1035 DNS Resolver**: User Datagram Protocol over IPv4 and domain name resolution querying DNS servers (`10.0.2.3:53`) to resolve Internet hosts into IP addresses (`dns example.com -> 104.20.23.154`).
- **Hardware CMOS Real-Time Clock (RTC)**: Reads calendar date and time from motherboard I/O ports `0x70`/`0x71`, presenting a live wall clock in the desktop Taskbar and via terminal command `date`.
- **Gigabit Ethernet & Network Protocol Stack**: Intel 82540EM (e1000) NIC driver with hardware DMA rings, full Ethernet II framing, dynamic ARP cache & reply engine, IPv4 packet handling, and ICMP Echo ping client/server.
- **Persistent Disk & FAT Filesystem**: Scans GPT partition tables from SATA disks, mounts FAT12/16/32 volumes via AHCI DMA reads, and navigates directories.
- **Standalone ELF64 Executable Loader**: Parses standard 64-bit ELF binaries, maps PT_LOAD segments, configures executable page permissions, and spawns user processes.
- **Preemptive Kernel Multitasking**: Round-robin thread scheduler running background threads concurrently with the desktop GUI event loop.
- **Interactive Window Manager**: Real-time window dragging, resizing via bottom-right grip, focus switching, and z-ordering.
- **Built-in Shell Applications**:
  - **SysMon**: Real-time system monitor & task manager with memory gauges, CPU stats, and thread table (`Ctrl+5`).
  - **Browser**: Graphical web browser over TCP/HTTP with address bar, HTML formatting, and scrolling (`Ctrl+4`).
  - **Terminal**: Interactive CLI supporting the full real command suite (`Ctrl+3`):
    - File & Disk: `ls`, `cat`, `write`, `mkdir`, `rm`, `rename`, `fsck-lite`, `disks`, `gpt`, `fatls`
    - System & Memory: `help`, `echo`, `clear`, `mem`, `meminfo`, `uptime`, `date`, `acpi`, `poweroff`, `reboot`
    - Processes & Executables: `ps`, `kill`, `elfinfo`, `elfrun`
    - Networking: `netinfo`, `arp`, `ping`, `dns`, `tcp`, `curl`
  - **Files Explorer**: Interactive directory browsing of unified VFS (`/` ramfs & `/disk` persistent FAT) with file creation, deletion, and renaming (`Ctrl+1`).
  - **Text Editor**: Live text buffer editing with load and persistent save integration into ramfs and SATA disks (`Ctrl+2`).
  - **Welcome & Settings**: Control center with live theme accent switching, system diagnostics, and hardware telemetry (`Ctrl+7`).
- **UI Control Audit & Real vs. Cosmetic Classification (Phase 23)**:
  - **Brightness Slider**: **REAL** — Dynamically scales framebuffer luminance on `g.back` in `wm.rs::compose` (20%..=100%).
  - **Night Light Filter**: **REAL** — Dynamic warm color temperature matrix filter attenuating blue channel luminance.
  - **Do Not Disturb (DND)**: **REAL** — Suppresses desktop toast notifications while active.
  - **Sound Volume Slider**: **COSMETIC (preview)** — Labeled `Volume (preview)` in Quick Settings & Settings app (audio driver scheduled for Phase 25).
  - **Sleep Option**: **COSMETIC (preview)** — Labeled `Sleep (preview)` in Start Menu power options.
  - **Wallpaper Style Picker**: **COSMETIC (preview)** — Labeled `(preview)` in Settings app.
  - **Resolution Selector**: **REAL Telemetry / COSMETIC Mode Switch (preview)** — Reports live UEFI GOP mode; mode switching is labeled `(preview)`.
- **Dual Input Drivers**: Native PS/2 mouse and keyboard decoders with automatic arrow-key keyboard mouse emulation if hypervisor mouse capture is unavailable.
- **Hardware Telemetry & Logging**: Real-time TSC calibration, COM1 serial output, and per-frame performance tracking.

---

## Live Preview

<div align="center">

![TriOS Desktop Live Preview](live-desktop.png)

*Live TriOS desktop running inside VirtualBox: Lock screen with RTC clock, TTF typography (Inter + JetBrains Mono), Start menu with real-time app search, Quick Settings with real screen dimming, Files Explorer with persistent FAT disk writes, and unified VFS.*

</div>

---

## Quick Start

### Prerequisites

| Tool | Version / Notes |
| :--- | :--- |
| **Rust Nightly** | Pinned in `rust-toolchain.toml` (auto-installed by rustup) |
| **Target `x86_64-unknown-none`** | Bare-metal kernel target |
| **Oracle VirtualBox** | Version 7.x (verified with 7.2.14) |

### Build

```powershell
# Clone and enter directory
cd D:\trios

# Build release UEFI disk image (recommended)
cargo build --release
```

The UEFI disk image will be generated at:
`target/release/trios-uefi-release.img`

### Run in VirtualBox

```powershell
# Option A: Launch directly into interactive GUI window
powershell -ExecutionPolicy Bypass -File .\run-gui.ps1

# Option B: Run automated test harness (headless with scancode injection)
powershell -ExecutionPolicy Bypass -File .\run-virtualbox.ps1
```

> [!NOTE]
> All VirtualBox machine state is stored cleanly inside `.vbox-home/` in the workspace without polluting your global user directories.

---

## Desktop & Controls

### Keyboard Shortcuts

| Shortcut | Action |
| :--- | :--- |
| **Ctrl + 1** | Focus and open **Files Explorer** |
| **Ctrl + 2** | Focus and open **Text Editor** |
| **Ctrl + 3** | Focus and open **Terminal** |
| **Ctrl + 4** | Focus and open **Web Browser** |
| **Ctrl + 5** | Focus and open **System Monitor / Task Manager** |
| **Ctrl + 6** | Focus and open **TriSweeper (Minesweeper Game)** |
| **Ctrl + 7** | Focus and open **Settings App** |
| **Ctrl + L** | Lock desktop screen (Session Lock) |
| **Esc** | Close currently focused window / Dismiss menus |
| **Arrow Up / Down** | Command history in Terminal; move selection in Files/Editor |
| **PageUp / PageDown** | Scroll Terminal transcript history buffer |

### Mouse & Keyboard Fallback

- **Real Mouse**: Left-click to focus, drag titlebars to move, drag bottom-right corner grip to resize.
- **Keyboard Cursor Fallback**: If mouse integration is disabled in VirtualBox:
  - **Arrow Keys**: Move the cursor (hold `Shift` for fine 4px steps).
  - **Space / Enter**: Left-click at the current cursor position.

---

## Project Layout

```text
D:\trios\
├── Cargo.toml                 # Workspace root & image configuration
├── build.rs                   # UEFI kernel image packer
├── rust-toolchain.toml        # Pinned nightly compiler & components
├── run-gui.ps1                # 1-click interactive VirtualBox launcher
├── run-virtualbox.ps1         # Automated test script with COM1 serial dump
├── docs/                      # Architecture & subsystem documentation
│   ├── memory-map.md          # Physical bitmap & virtual memory map
│   ├── syscall-table.md       # Syscall registers, numbers & error codes
│   └── boot-sequence.md       # Full boot sequence & mermaid diagram
├── tests/
│   └── host_parsers.rs        # Host-side cargo unit tests for parsers
└── kernel/
    ├── Cargo.toml             # Kernel crate dependencies
    └── src/
        ├── main.rs            # Kernel entry point, desktop loop & state
        ├── allocator.rs       # 32 MiB heap hybrid slab allocator
        ├── gdt.rs             # GDT, TSS & Ring 3 user descriptors
        ├── interrupts.rs      # IDT, PIC remapping & IRQ handlers
        ├── timer.rs           # PIT 100 Hz timer & RTC wall clock
        ├── serial.rs          # 16550 UART serial logger
        ├── panic.rs           # Themed emergency crash screen
        ├── acpi.rs            # ACPI tables & power management
        ├── apps/              # Desktop userland shell applications
        ├── drivers/           # Hardware drivers (AHCI SATA, Intel e1000)
        ├── fs/                # Unified VFS, FAT32/16/12 & ELF loader
        ├── graphics/          # Double-buffered GOP & Fontdue TTF engine
        ├── gui/               # Glass window manager, dock & icons
        ├── memory/            # Bitmap allocator & virtual memory API
        ├── net/               # TCP/IP network stack, HTTP client & DNS
        ├── sched/             # Preemptive scheduler & Ring 3 processes
        └── syscall/           # Fast hardware syscall/sysret dispatcher
```

---

## Boot Sequence

```mermaid
flowchart LR
    A[UEFI] --> B[GOP]
    B --> C[GDT/IDT]
    C --> D[PIC/PIT]
    D --> E[Frame Allocator]
    E --> F[Heap]
    F --> G[PCI/AHCI/FAT]
    G --> H[NIC]
    H --> I[ACPI]
    I --> J[GUI]
    J --> K[Lock Screen]
```

Detailed boot sequence documentation and stage breakdown can be found in [`docs/boot-sequence.md`](docs/boot-sequence.md).

---

## Roadmap

- [x] **Phase 0**: Bootloader & Image Build (UEFI boot, direct physical memory map, dual-target cargo workspace)
- [x] **Phase 1**: Bare-Metal CPU Core (GDT, TSS IST1 stack, IDT, 8259 PIC remapping, PIT 100 Hz, COM1 UART serial, 32 MiB heap)
- [x] **Phase 2**: Framebuffer & Graphics Canvas (Double-buffered GOP canvas, 8x8 font, graphics primitives)
- [x] **Phase 3**: Input & Window Manager (PS/2 keyboard & mouse drivers, event queue, z-ordered window manager)
- [x] **Phase 4**: Shell, VFS & Text Fields (In-memory ramfs, TextInput, Files Explorer, Text Editor, Terminal shell)
- [x] **Phase 5**: Preemptive Multitasking & Scheduler (Thread control blocks, preemptive scheduler, context switching)
- [x] **Phase 6**: User Descriptors & Fast Syscalls (GDT User DPL-3, TSS RSP0 privilege stack, IA32 MSRs, fast `syscall` dispatcher)
- [x] **Phase 7**: PCI Bus & AHCI SATA Driver (PCI scanner, AHCI HBA MMIO, SATA disk device detection)
- [x] **Phase 8**: Standalone ELF Loader & Exec (ELF64 header & PT_LOAD parser, page permissions, `elfinfo`, `elfrun`)
- [x] **Phase 9**: Persistent Storage & FAT VFS (GPT parser, AHCI DMA sector reading, FAT12/16/32 volume mounter, `gpt`, `fatls`, `disks`)
- [x] **Phase 10**: Intel E1000 NIC & Network Stack (Intel 82540EM MMIO driver, RX/TX DMA rings, Ethernet II, ARP, IPv4, ICMP ping)
- [x] **Phase 11**: UDP, DNS Resolver & CMOS RTC (UDP framing, RFC 1035 DNS client `dns`, hardware CMOS RTC driver `date`, live clock)
- [x] **Phase 12**: ACPI Subsystem & Power Control (RSDP/XSDT parser, FADT PM1a, MADT CPU cores, ACPI S5 `poweroff`, `reboot`, `acpi`)
- [x] **Phase 13**: TCP Protocol & HTTP/1.0 Client (RFC 793 TCP state machine, 3-way handshake, bare-metal `curl <url>`)
- [x] **Phase 14**: Desktop Web Browser Application (GUI browser `Ctrl+4` with address bar, HTML tokenizer, layout)
- [x] **Phase 15**: System Monitor & Task Manager (Real-time CPU/RAM gauges, thread table, compositor FPS telemetry `Ctrl+5`)
- [x] **Phase 16**: Modern Desktop UI/UX & TriSweeper (Desktop shortcut icons, window controls, TriSweeper game `Ctrl+6`)
- [x] **Phase 17**: Display & Visual Foundation (Native 1080p GOP, futuristic glass theme, cached gradient wallpaper, floating dock)
- [x] **Phase 18**: Desktop Experience (Vector icons, Start menu with search, Quick Settings with Night Light, toast queue, Alt+Tab)
- [x] **Phase 19**: Typography & System Apps (Fontdue TTF Inter/JetBrains Mono, Settings app `Ctrl+7`, lock screen `Ctrl+L`)
- [x] **Phase 20**: Memory Management (Bitmap physical frame allocator, themed page fault screen, thread guard pages, slab allocator 32B..2048B, `meminfo`)
- [x] **Phase 21**: Real Processes & Ring 3 (Per-process PML4 page tables, Ring 3 `iretq`/`sysretq`, syscall suite, user pointer validation, fault isolation, `ps`, `kill`)
- [x] **Phase 22**: VFS & Persistent Writes (Unified VFS `/` ramfs & `/disk` FAT, AHCI sector write DMA, multi-FAT write engine, `fsck-lite`)
- [x] **Phase 23**: Cleanup, Hardening, Docs (UI feature audit, real brightness dimming & DND, host parser tests, Ring 3 syscall fuzzing, architecture docs)

---

## Developer & Contact

**TriOS** is designed, created, and actively developed by **[JOJIN JOHN](https://github.com/jojin1709)**.

- **LinkedIn**: [linkedin.com/in/jojin-john](https://www.linkedin.com/in/jojin-john/)
- **GitHub**: [@jojin1709](https://github.com/jojin1709)

---

## License & Legal Notice

**Copyright (c) 2026 JOJIN JOHN. All Rights Reserved.**

> [!CAUTION]
> **LEGAL WARNING:**
> No part of this software, including source code, architecture, design, documentation, or assets, may be copied, reproduced, cloned, mirrored, modified, published, distributed, or used in any form without the express prior written permission of **JOJIN JOHN**.
> 
> Unauthorized copying, scraping, plagiarism, or distribution constitutes willful copyright infringement and will be met with immediate legal prosecution and DMCA enforcement. See [LICENSE](LICENSE) for full details.
