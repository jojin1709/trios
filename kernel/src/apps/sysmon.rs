//! TriOS System Monitor & Task Manager Application ("SysMon").
//!
//! Provides a desktop graphical task manager and system diagnostics utility featuring:
//! - Real-time CPU telemetry (core detection via ACPI, TSC frequency, uptime)
//! - Memory utilization gauges (UEFI physical memory map, kernel heap usage bar)
//! - Compositor rendering frame rates and network stack telemetry
//! - Kernel thread and task table (listing running, ready, and sleeping threads)
//! - Interactive background task spawning to test live preemptive multitasking
//!
//! Developed by JOJIN JOHN (@jojin1709) for TriOS.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::acpi::ACPI;
use crate::allocator;
use crate::event::KeyEvent;
use crate::gui::widgets::Widget;
use crate::gui::{Theme, WidgetOutcome, WindowManager};
use crate::memory;
use crate::net::NET;
use crate::perf;
use crate::sched::{self, ThreadInfo};
use crate::timer;

// ---------------------------------------------------------------------------
// Widget Indices (Creation Order Contract)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub const SM_TITLE: usize = 0;
#[allow(dead_code)]
pub const SM_SUB: usize = 1;
#[allow(dead_code)]
pub const SM_PANEL_SYS: usize = 2;
pub const SM_CPU: usize = 3;
pub const SM_MEM: usize = 4;
pub const SM_MEM_BAR: usize = 5;
pub const SM_TELEMETRY: usize = 6;
#[allow(dead_code)]
pub const SM_PANEL_TASKS: usize = 7;
#[allow(dead_code)]
pub const SM_TASKS_HDR: usize = 8;
pub const SM_TASK_ROWS_START: usize = 9;
pub const SM_TASK_ROW_COUNT: usize = 6;
pub const SM_BTN_SPAWN: usize = SM_TASK_ROWS_START + SM_TASK_ROW_COUNT; // 15
pub const SM_BTN_REFRESH: usize = SM_BTN_SPAWN + 1; // 16
#[allow(dead_code)]
pub const SM_FOOTER: usize = SM_BTN_REFRESH + 1; // 17

// ---------------------------------------------------------------------------
// Layout Metrics
// ---------------------------------------------------------------------------

const WIN_W: i32 = 540;
const WIN_H: i32 = 376;

// ---------------------------------------------------------------------------
// SysMon State
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct SysMon {
    /// Window ID registered with the WindowManager.
    pub win: u64,
    /// Number of tasks dynamically spawned by the user.
    pub spawned_count: u32,
    /// Last tick count at which a 1 Hz refresh occurred.
    pub last_refresh_tick: u64,
}

impl SysMon {
    /// Builds the System Monitor window and its widgets in contract order.
    pub fn new(wm: &mut WindowManager, theme: &Theme) -> Self {
        let win = wm.create_window("System Monitor", 220, 80, WIN_W, WIN_H);
        if let Some(w) = wm.window_mut(win) {
            // Widget 0: App Title
            w.widgets.push(Widget::label(
                "TriOS Task Manager & System Monitor v1.0",
                14,
                38,
                theme.text_primary,
            ));
            // Widget 1: Developer Subtitle
            w.widgets.push(Widget::label(
                "Developer: JOJIN JOHN (@jojin1709) | x86_64 UEFI Bare-Metal",
                14,
                52,
                theme.text_secondary,
            ));

            // Widget 2: System Overview Panel
            w.widgets.push(Widget::panel(
                12,
                68,
                WIN_W - 24,
                90,
                theme.window_bg,
                Some(theme.border),
            ));

            // Widget 3: CPU Info Label
            w.widgets.push(Widget::label(
                "CPU: x86_64 Multi-Core | Frequency: ~2.50 GHz | Uptime: 0s",
                20,
                76,
                theme.text_primary,
            ));

            // Widget 4: Memory Usage Label
            w.widgets.push(Widget::label(
                "RAM: Calculating... | Heap: 0 / 32768 KiB",
                20,
                92,
                theme.text_secondary,
            ));

            // Widget 5: Memory Usage Bar
            w.widgets.push(Widget::progress(20, 108, WIN_W - 40, 10, 0.20));

            // Widget 6: Compositor & Network Telemetry
            w.widgets.push(Widget::label(
                "Compositor: ~175 FPS | Net: Intel e1000 Gigabit (10.0.2.15)",
                20,
                124,
                theme.text_secondary,
            ));

            // Widget 7: Tasks & Processes Panel
            w.widgets.push(Widget::panel(
                12,
                166,
                WIN_W - 24,
                155,
                theme.window_bg,
                Some(theme.border),
            ));

            // Widget 8: Task Table Header
            w.widgets.push(Widget::label(
                "TID   THREAD NAME            STATE       STACK",
                20,
                174,
                theme.accent,
            ));

            // Widgets 9..14: 6 Task Rows
            for i in 0..SM_TASK_ROW_COUNT {
                w.widgets.push(Widget::label(
                    "",
                    20,
                    192 + (i as i32) * 18,
                    theme.text_primary,
                ));
            }

            // Widget 15: Spawn Task Button
            w.widgets.push(Widget::button(14, 338, 110, 22, "+ Spawn Task"));

            // Widget 16: Refresh Button
            w.widgets.push(Widget::button(132, 338, 80, 22, "Refresh"));

            // Widget 17: Footer Status Label
            w.widgets.push(Widget::label(
                "Auto-refreshes 1 Hz | Ctrl+5 to focus | Esc to close",
                222,
                342,
                theme.text_secondary,
            ));
        }

        let mut sysmon = Self {
            win,
            spawned_count: 0,
            last_refresh_tick: 0,
        };
        sysmon.refresh(wm);
        sysmon
    }

    /// Queries live system telemetry and updates all window widgets.
    pub fn refresh(&mut self, wm: &mut WindowManager) -> bool {
        let uptime = timer::uptime_seconds();

        // 1. CPU Telemetry
        let cores = ACPI.lock().as_ref().map(|a| a.cpu_cores).unwrap_or(1);
        let mhz = perf::cycles_per_us();
        let ghz_str = if mhz > 0 {
            let int_part = mhz / 1000;
            let frac_part = (mhz % 1000) / 10;
            format!("{int_part}.{frac_part:02} GHz")
        } else {
            String::from("N/A")
        };
        let cpu_str = format!("CPU: x86_64 ({cores} cores @ {ghz_str}) | Uptime: {uptime}s");
        wm.set_label(self.win, SM_CPU, cpu_str);

        // 2. Memory Telemetry
        let fstats = memory::stats();
        let heap_free = allocator::remaining();
        let heap_total = allocator::HEAP_SIZE as usize;
        let heap_used = heap_total.saturating_sub(heap_free);
        let heap_used_kib = heap_used / 1024;
        let heap_total_kib = heap_total / 1024;
        let frame_pct = if fstats.total > 0 {
            (fstats.allocated as f32 / fstats.total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let ram_info = format!(
            "Frames: {} used / {} free ({} total) | Heap: {} / {} KiB",
            fstats.allocated, fstats.free, fstats.total, heap_used_kib, heap_total_kib
        );
        wm.set_label(self.win, SM_MEM, ram_info);
        wm.set_progress(self.win, SM_MEM_BAR, frame_pct);

        // 3. Telemetry (Compositor & Network)
        let perf_snap = perf::get_latest();
        let fps = perf_snap.frames_per_sec;
        let comp_ms = perf_snap.compose_avg_us as f32 / 1000.0;
        let ip = NET.lock().ip;
        let net_str = format!(
            "Compositor: {fps} FPS (draw ~{comp_ms:.1} ms) | IP: {}.{}.{}.{} (e1000)",
            ip[0], ip[1], ip[2], ip[3]
        );
        wm.set_label(self.win, SM_TELEMETRY, net_str);

        // 4. Kernel Thread Table
        let threads: Vec<ThreadInfo> = sched::thread_snapshot();
        for i in 0..SM_TASK_ROW_COUNT {
            let row_idx = SM_TASK_ROWS_START + i;
            if i < threads.len() {
                let t = &threads[i];
                let stack_info = if t.id == 0 { "boot-stack" } else { "32 KiB" };
                let row_text = format!(
                    "#{:<4} {:<22} {:<11} {}",
                    t.id, t.name, t.state_str, stack_info
                );
                wm.set_label(self.win, row_idx, row_text);
            } else {
                wm.set_label(self.win, row_idx, String::new());
            }
        }

        true
    }

    /// Handles pointer clicks (Spawn Task or Refresh buttons).
    pub fn on_action(
        &mut self,
        widget_index: usize,
        outcome: WidgetOutcome,
        wm: &mut WindowManager,
    ) -> bool {
        if outcome != WidgetOutcome::Clicked {
            return false;
        }

        if widget_index == SM_BTN_SPAWN {
            self.spawned_count += 1;
            let count = self.spawned_count;
            let name = format!("worker_{count}");
            sched::spawn(&name, demo_worker_thread);
            serial_println!("[sysmon ] spawned user task '{name}' from System Monitor");
            self.refresh(wm);
            return true;
        }

        if widget_index == SM_BTN_REFRESH {
            return self.refresh(wm);
        }

        false
    }

    /// Key handler for SysMon window (e.g. 'r' to refresh, 's' to spawn).
    pub fn on_key(&mut self, _event: KeyEvent, _wm: &mut WindowManager) -> bool {
        false
    }

    /// 1 Hz periodic tick hook: auto-refreshes system telemetry.
    pub fn on_tick(&mut self, wm: &mut WindowManager) -> bool {
        let current_tick = timer::ticks();
        // Refresh approximately once per second (100 ticks at 100 Hz)
        if current_tick.saturating_sub(self.last_refresh_tick) >= 100 {
            self.last_refresh_tick = current_tick;
            return self.refresh(wm);
        }
        false
    }
}

/// Sample background worker thread spawned by the Task Manager.
extern "C" fn demo_worker_thread() {
    let mut iter = 0u64;
    loop {
        sched::sleep_ticks(200); // 2 seconds
        iter += 1;
        serial_println!("[worker ] user worker active: heartbeat #{iter}");
    }
}
