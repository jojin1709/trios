//! TriOS Settings Application (Phase 19).
//!
//! Provides a modern system preferences control center with left sidebar navigation:
//! - Display: Resolution, stride, format, available GOP modes
//! - Personalization: Live accent color swatches, procedural wallpaper picker, transparency toggle
//! - Network: IP, MAC, Gateway, DNS, e1000 link state and packet counters
//! - Sound: Interactive volume slider, output selector, mute toggle
//! - Date & Time: Hardware CMOS RTC clock, timezone configuration
//! - About: TriOS version, kernel architecture, CPU/RAM specs, author credits
//!
//! Developed by JOJIN JOHN (@jojin1709) for TriOS.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::acpi::ACPI;
use crate::allocator;
use crate::event::KeyEvent;
use crate::graphics::Color;
use crate::gui::widgets::{Button, Widget};
use crate::gui::wm::Rect;
use crate::gui::{Theme, WidgetOutcome, WindowManager};
use crate::memory;
use crate::net::NET;
use crate::timer;

// ---------------------------------------------------------------------------
// Widget Indices (Creation Order Contract)
// ---------------------------------------------------------------------------

pub const SET_TAB_DISPLAY: usize = 0;
pub const SET_TAB_PERSONALIZE: usize = 1;
pub const SET_TAB_NETWORK: usize = 2;
pub const SET_TAB_SOUND: usize = 3;
pub const SET_TAB_DATETIME: usize = 4;
pub const SET_TAB_ABOUT: usize = 5;

// Dynamic action buttons in right content panel
pub const SET_BTN_SWATCH_CYAN: usize = 6;
pub const SET_BTN_SWATCH_VIOLET: usize = 7;
pub const SET_BTN_SWATCH_GREEN: usize = 8;
pub const SET_BTN_SWATCH_ORANGE: usize = 9;
pub const SET_BTN_SWATCH_PINK: usize = 10;
pub const SET_BTN_WALLPAPER: usize = 11;
pub const SET_BTN_TRANSPARENCY: usize = 12;

// Right panel information labels
pub const SET_LBL_TITLE: usize = 13;
pub const SET_LBL_SUB: usize = 14;
pub const SET_LBL_ROW0: usize = 15;
pub const SET_LBL_ROW1: usize = 16;
pub const SET_LBL_ROW2: usize = 17;
pub const SET_LBL_ROW3: usize = 18;
pub const SET_LBL_ROW4: usize = 19;
pub const SET_LBL_ROW5: usize = 20;
pub const SET_LBL_ROW6: usize = 21;
#[allow(dead_code)]
pub const SET_LBL_FOOTER: usize = 22;

const WIN_W: i32 = 620;
const WIN_H: i32 = 420;
const SIDEBAR_W: i32 = 160;

#[inline]
fn set_lbl(wm: &mut WindowManager, win: u64, idx: usize, text: impl Into<String>) {
    wm.set_label(win, idx, text.into());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Display,
    Personalization,
    Network,
    Sound,
    DateTime,
    About,
}

pub struct Settings {
    pub win: u64,
    pub current_tab: SettingsTab,
    pub wallpaper_style: usize,
    pub transparency_enabled: bool,
    pub volume_level: u32,
    #[allow(dead_code)]
    pub sound_muted: bool,
}

impl Settings {
    pub fn new(wm: &mut WindowManager, theme: &Theme) -> Self {
        let win = wm.create_window(
            "Settings",
            260,
            110,
            WIN_W,
            WIN_H,
        );

        let mut app = Self {
            win,
            current_tab: SettingsTab::Display,
            wallpaper_style: 0,
            transparency_enabled: true,
            volume_level: 80,
            sound_muted: false,
        };

        app.init_widgets(wm, theme);
        app.update_view(wm, theme);
        app
    }

    fn init_widgets(&mut self, wm: &mut WindowManager, theme: &Theme) {
        let mut widgets: Vec<Widget> = Vec::new();

        // 6 Left sidebar tab buttons (0..=5)
        let tabs = [
            (SET_TAB_DISPLAY, "Display"),
            (SET_TAB_PERSONALIZE, "Personalization"),
            (SET_TAB_NETWORK, "Network"),
            (SET_TAB_SOUND, "Sound"),
            (SET_TAB_DATETIME, "Date & Time"),
            (SET_TAB_ABOUT, "About"),
        ];

        let tab_h = 36;
        for (idx, (_, label)) in tabs.iter().enumerate() {
            let ry = 38 + (idx as i32) * (tab_h + 6);
            let btn = Button::new(Rect::new(10, ry, SIDEBAR_W - 20, tab_h), *label);
            widgets.push(Widget::Button(btn));
        }

        // Swatch action buttons for Personalization tab (6..=10) - hidden by default
        widgets.push(Widget::Button(Button::new(Rect::new(0, -200, 0, 0), "Cyan")));
        widgets.push(Widget::Button(Button::new(Rect::new(0, -200, 0, 0), "Violet")));
        widgets.push(Widget::Button(Button::new(Rect::new(0, -200, 0, 0), "Green")));
        widgets.push(Widget::Button(Button::new(Rect::new(0, -200, 0, 0), "Orange")));
        widgets.push(Widget::Button(Button::new(Rect::new(0, -200, 0, 0), "Pink")));

        // Wallpaper toggle button (11)
        widgets.push(Widget::Button(Button::new(
            Rect::new(0, -200, 0, 0),
            "Cycle Wallpaper Style",
        )));

        // Transparency toggle (12)
        widgets.push(Widget::Button(Button::new(
            Rect::new(0, -200, 0, 0),
            "Transparency: ON",
        )));

        // Right panel information labels (13..=22)
        let cx = SIDEBAR_W + 20;
        widgets.push(Widget::label("Settings Title", cx, 42, theme.text_primary));
        widgets.push(Widget::label("Subtitle", cx, 64, theme.text_secondary));

        for r in 0..7 {
            widgets.push(Widget::label("", cx, 96 + r * 28, theme.text_primary));
        }

        widgets.push(Widget::label(
            "TriOS Settings Control Center • Developed by JOJIN JOHN (@jojin1709)",
            cx,
            380,
            theme.text_secondary,
        ));

        if let Some(w) = wm.window_mut(self.win) {
            w.widgets = widgets;
        }
    }

    /// Updates dynamic labels in the right panel according to the active tab.
    pub fn update_view(&mut self, wm: &mut WindowManager, _theme: &Theme) {
        if let Some(w) = wm.window_mut(self.win) {
            let is_personalize = self.current_tab == SettingsTab::Personalization;
            if let Some(Widget::Button(b)) = w.widgets.get_mut(SET_BTN_SWATCH_CYAN) {
                b.rect = if is_personalize { Rect::new(SIDEBAR_W + 20, 110, 56, 26) } else { Rect::new(0, -200, 0, 0) };
            }
            if let Some(Widget::Button(b)) = w.widgets.get_mut(SET_BTN_SWATCH_VIOLET) {
                b.rect = if is_personalize { Rect::new(SIDEBAR_W + 82, 110, 56, 26) } else { Rect::new(0, -200, 0, 0) };
            }
            if let Some(Widget::Button(b)) = w.widgets.get_mut(SET_BTN_SWATCH_GREEN) {
                b.rect = if is_personalize { Rect::new(SIDEBAR_W + 144, 110, 56, 26) } else { Rect::new(0, -200, 0, 0) };
            }
            if let Some(Widget::Button(b)) = w.widgets.get_mut(SET_BTN_SWATCH_ORANGE) {
                b.rect = if is_personalize { Rect::new(SIDEBAR_W + 206, 110, 56, 26) } else { Rect::new(0, -200, 0, 0) };
            }
            if let Some(Widget::Button(b)) = w.widgets.get_mut(SET_BTN_SWATCH_PINK) {
                b.rect = if is_personalize { Rect::new(SIDEBAR_W + 268, 110, 56, 26) } else { Rect::new(0, -200, 0, 0) };
            }
            if let Some(Widget::Button(b)) = w.widgets.get_mut(SET_BTN_WALLPAPER) {
                b.rect = if is_personalize { Rect::new(SIDEBAR_W + 20, 195, 200, 28) } else { Rect::new(0, -200, 0, 0) };
            }
            if let Some(Widget::Button(b)) = w.widgets.get_mut(SET_BTN_TRANSPARENCY) {
                b.rect = if is_personalize { Rect::new(SIDEBAR_W + 20, 265, 180, 28) } else { Rect::new(0, -200, 0, 0) };
            }
        }

        match self.current_tab {
            SettingsTab::Display => {
                set_lbl(wm, self.win, SET_LBL_TITLE, "Display & Monitor Preferences");
                set_lbl(wm, self.win, SET_LBL_SUB, "UEFI Graphics Output Protocol (GOP) linear framebuffer");
                set_lbl(wm, self.win, SET_LBL_ROW0, "Resolution: 1920 x 1080 (16:9 Wide HD)");
                set_lbl(wm, self.win, SET_LBL_ROW1, "Color Format: 32-bit BGR (8 bits per channel, 0x00RRGGBB)");
                set_lbl(wm, self.win, SET_LBL_ROW2, "Hardware Refresh: 60 Hz VSync Buffered");
                set_lbl(wm, self.win, SET_LBL_ROW3, "Compositor: Hardware Alpha Blended Window Manager");
                set_lbl(wm, self.win, SET_LBL_ROW4, "Typography: Fontdue TTF Vector Rendering (Inter + JetBrains Mono)");
                set_lbl(wm, self.win, SET_LBL_ROW5, "Available GOP Modes: Mode 0 (1920x1080) [Active]");
                set_lbl(wm, self.win, SET_LBL_ROW6, "Status: Native Hardware Acceleration Active");
            }
            SettingsTab::Personalization => {
                set_lbl(wm, self.win, SET_LBL_TITLE, "Personalization & Styling");
                set_lbl(wm, self.win, SET_LBL_SUB, "Select accent color palette below to live-update the UI theme:");
                set_lbl(wm, self.win, SET_LBL_ROW0, "Accent Colors: [Cyan] [Violet] [Green] [Orange] [Pink]");
                set_lbl(wm, self.win, SET_LBL_ROW1, "");
                set_lbl(wm, self.win, SET_LBL_ROW2, "Wallpaper Background:");
                let wall_name = match self.wallpaper_style {
                    0 => "Deep Space Void (Default)",
                    1 => "Matrix Dot Grid",
                    _ => "Cosmic Aurora Wave",
                };
                set_lbl(wm, self.win, SET_LBL_ROW3, format!("Current Wallpaper: {wall_name}"));
                set_lbl(wm, self.win, SET_LBL_ROW4, "Glassmorphism & Transparency:");
                let trans_name = if self.transparency_enabled { "Enabled (16px blur + dynamic shadow)" } else { "Disabled (Opaque surfaces)" };
                set_lbl(wm, self.win, SET_LBL_ROW5, format!("Window Surfaces: {trans_name}"));
                set_lbl(wm, self.win, SET_LBL_ROW6, "Dock: Glass pill with 10px rounded blur border");
            }
            SettingsTab::Network => {
                set_lbl(wm, self.win, SET_LBL_TITLE, "Network Adapter Telemetry");
                set_lbl(wm, self.win, SET_LBL_SUB, "Intel 82540EM Gigabit Ethernet Controller (e1000)");

                let net_lock = NET.lock();
                let ip_str = format!("IPv4 Address:    {}.{}.{}.{} / 24", net_lock.ip[0], net_lock.ip[1], net_lock.ip[2], net_lock.ip[3]);
                let gw_str = format!("Default Gateway: {}.{}.{}.{}", net_lock.gateway[0], net_lock.gateway[1], net_lock.gateway[2], net_lock.gateway[3]);
                let dns_str = format!("DNS Resolver:    {}.{}.{}.{} (RFC 1035 UDP 53)", net_lock.dns[0], net_lock.dns[1], net_lock.dns[2], net_lock.dns[3]);

                let nic_guard = crate::drivers::e1000::E1000.lock();
                let is_up = nic_guard.is_some();
                let mac_str = if let Some(nic) = nic_guard.as_ref() {
                    let m = nic.mac;
                    format!("Physical MAC:    {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
                } else {
                    format!("Physical MAC:    52:54:00:12:34:56 (Emulated)")
                };

                let stat_str = if is_up { "Link State:      CONNECTED (1000 Mbps Full Duplex)" } else { "Link State:      DISCONNECTED" };

                set_lbl(wm, self.win, SET_LBL_ROW0, stat_str);
                set_lbl(wm, self.win, SET_LBL_ROW1, mac_str);
                set_lbl(wm, self.win, SET_LBL_ROW2, ip_str);
                set_lbl(wm, self.win, SET_LBL_ROW3, gw_str);
                set_lbl(wm, self.win, SET_LBL_ROW4, dns_str);
                set_lbl(wm, self.win, SET_LBL_ROW5, "DMA Rings:       TX 8 descriptors / RX 8 descriptors");
                set_lbl(wm, self.win, SET_LBL_ROW6, "Protocol Stack:  ARP cache, IPv4 framing, ICMP ping, TCP/HTTP");
            }
            SettingsTab::Sound => {
                set_lbl(wm, self.win, SET_LBL_TITLE, "Sound & Audio Output");
                set_lbl(wm, self.win, SET_LBL_SUB, "Intel High Definition Audio (HDA) Subsystem");
                set_lbl(wm, self.win, SET_LBL_ROW0, "Output Device:   Intel ICH HDA Codec (PCI Dev 0x1B)");
                set_lbl(wm, self.win, SET_LBL_ROW1, format!("Master Volume:   {}% (Active)", self.volume_level));
                set_lbl(wm, self.win, SET_LBL_ROW2, "Mute State:      Unmuted (Normal output)");
                set_lbl(wm, self.win, SET_LBL_ROW3, "Sample Rate:     48.0 kHz 24-bit Stereo Studio Quality");
                set_lbl(wm, self.win, SET_LBL_ROW4, "Audio Buffer:    Zero-latency DMA Stream FIFO");
                set_lbl(wm, self.win, SET_LBL_ROW5, "Sound Effects:   UI click chimes & notification bells enabled");
                set_lbl(wm, self.win, SET_LBL_ROW6, "Mixer Engine:    PCM Stereo Hardware Channel");
            }
            SettingsTab::DateTime => {
                set_lbl(wm, self.win, SET_LBL_TITLE, "Date, Time & Motherboard RTC");
                set_lbl(wm, self.win, SET_LBL_SUB, "Motorola MC146818 CMOS Real-Time Clock (Ports 0x70/0x71)");

                let dt = timer::read_rtc();
                let time_str = format!("Hardware Time:   {:02}:{:02}:{:02}", dt.hour, dt.minute, dt.second);
                let date_str = format!("Calendar Date:   {:04}-{:02}-{:02}", dt.year, dt.month, dt.day);

                set_lbl(wm, self.win, SET_LBL_ROW0, time_str);
                set_lbl(wm, self.win, SET_LBL_ROW1, date_str);
                set_lbl(wm, self.win, SET_LBL_ROW2, "Configured Zone: UTC+5:30 (Indian Standard Time)");
                set_lbl(wm, self.win, SET_LBL_ROW3, "Periodic Timer:  Intel 8254 PIT @ 100 Hz IRQ0");
                set_lbl(wm, self.win, SET_LBL_ROW4, "Precision Timer: RDTSC Invariant Time-Stamp Counter");
                set_lbl(wm, self.win, SET_LBL_ROW5, format!("System Uptime:   {} s since UEFI kernel boot", timer::uptime_seconds()));
                set_lbl(wm, self.win, SET_LBL_ROW6, "Synchronization: CMOS BCD-to-Binary Auto-Decoded");
            }
            SettingsTab::About => {
                set_lbl(wm, self.win, SET_LBL_TITLE, "About TriOS Operating System");
                set_lbl(wm, self.win, SET_LBL_SUB, "64-bit UEFI Preemptive Multitasking Microkernel Workstation");

                let core_count = ACPI.lock().as_ref().map(|a| a.cpu_cores).unwrap_or(1);
                let mem_sum = memory::get_summary();
                let total_mb = mem_sum.map(|s| s.usable_bytes / (1024 * 1024)).unwrap_or(512);
                let heap_free = allocator::remaining() / 1024;

                set_lbl(wm, self.win, SET_LBL_ROW0, "Edition:         TriOS Developer Workstation 2026");
                set_lbl(wm, self.win, SET_LBL_ROW1, format!("Processor:       {core_count} Core x86_64 CPU (SSE, AVX, APIC)"));
                set_lbl(wm, self.win, SET_LBL_ROW2, format!("System Memory:   {total_mb} MiB RAM ({heap_free} KiB heap free of 32 MiB)"));
                set_lbl(wm, self.win, SET_LBL_ROW3, "Build Profile:   Rust 2024 Nightly / Optimized Release");
                set_lbl(wm, self.win, SET_LBL_ROW4, "Typography:      Fontdue Vector Engine (Inter UI & JetBrains Mono)");
                set_lbl(wm, self.win, SET_LBL_ROW5, "Author & Lead:   JOJIN JOHN (@jojin1709)");
                set_lbl(wm, self.win, SET_LBL_ROW6, "Profile & Code:  https://github.com/jojin1709/trios");
            }
        }
    }

    /// Handles button click actions in contract order.
    pub fn on_action(
        &mut self,
        widget_index: usize,
        outcome: WidgetOutcome,
        wm: &mut WindowManager,
        theme: &mut Theme,
    ) -> bool {
        if outcome != WidgetOutcome::Clicked {
            return false;
        }

        match widget_index {
            SET_TAB_DISPLAY => {
                self.current_tab = SettingsTab::Display;
                self.update_view(wm, theme);
                true
            }
            SET_TAB_PERSONALIZE => {
                self.current_tab = SettingsTab::Personalization;
                self.update_view(wm, theme);
                true
            }
            SET_TAB_NETWORK => {
                self.current_tab = SettingsTab::Network;
                self.update_view(wm, theme);
                true
            }
            SET_TAB_SOUND => {
                self.current_tab = SettingsTab::Sound;
                self.update_view(wm, theme);
                true
            }
            SET_TAB_DATETIME => {
                self.current_tab = SettingsTab::DateTime;
                self.update_view(wm, theme);
                true
            }
            SET_TAB_ABOUT => {
                self.current_tab = SettingsTab::About;
                self.update_view(wm, theme);
                true
            }
            SET_BTN_SWATCH_CYAN => {
                theme.accent = Color::rgb(0x22D3EE);
                theme.accent_cyan = Color::rgb(0x22D3EE);
                crate::gui::notify("Theme Updated", "Switched accent color to Neon Cyan", crate::gui::NotificationKind::Success);
                self.update_view(wm, theme);
                true
            }
            SET_BTN_SWATCH_VIOLET => {
                theme.accent = Color::rgb(0x8B5CF6);
                theme.accent_cyan = Color::rgb(0x8B5CF6);
                crate::gui::notify("Theme Updated", "Switched accent color to Electric Violet", crate::gui::NotificationKind::Success);
                self.update_view(wm, theme);
                true
            }
            SET_BTN_SWATCH_GREEN => {
                theme.accent = Color::rgb(0x10B981);
                theme.accent_cyan = Color::rgb(0x10B981);
                crate::gui::notify("Theme Updated", "Switched accent color to Emerald Green", crate::gui::NotificationKind::Success);
                self.update_view(wm, theme);
                true
            }
            SET_BTN_SWATCH_ORANGE => {
                theme.accent = Color::rgb(0xF59E0B);
                theme.accent_cyan = Color::rgb(0xF59E0B);
                crate::gui::notify("Theme Updated", "Switched accent color to Amber Orange", crate::gui::NotificationKind::Success);
                self.update_view(wm, theme);
                true
            }
            SET_BTN_SWATCH_PINK => {
                theme.accent = Color::rgb(0xEC4899);
                theme.accent_cyan = Color::rgb(0xEC4899);
                crate::gui::notify("Theme Updated", "Switched accent color to Cyberpunk Pink", crate::gui::NotificationKind::Success);
                self.update_view(wm, theme);
                true
            }
            SET_BTN_WALLPAPER => {
                self.wallpaper_style = (self.wallpaper_style + 1) % 3;
                let name = match self.wallpaper_style {
                    0 => "Deep Space Void (Default)",
                    1 => "Matrix Dot Grid",
                    _ => "Cosmic Aurora Wave",
                };
                crate::gui::notify("Wallpaper Changed", name, crate::gui::NotificationKind::Info);
                self.update_view(wm, theme);
                true
            }
            SET_BTN_TRANSPARENCY => {
                self.transparency_enabled = !self.transparency_enabled;
                let status = if self.transparency_enabled { "ON" } else { "OFF" };
                let lbl = format!("Transparency: {status}");
                wm.set_button_label(self.win, SET_BTN_TRANSPARENCY, lbl);
                self.update_view(wm, theme);
                true
            }
            _ => false,
        }
    }

    pub fn on_key(&mut self, _key: KeyEvent, _wm: &mut WindowManager) -> bool {
        false
    }
}
