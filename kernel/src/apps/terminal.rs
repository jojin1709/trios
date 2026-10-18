//! Phase 4 (cycle 2) — the Terminal: the "Terminal" window.
//!
//! ### What it is
//!
//! A single-line command prompt over the same ramfs the explorer and
//! editor use — the third proof that one [`FileSystem`] can back three
//! UIs. Output lands in a scrolling transcript ([`TERM_LINE_COUNT`] rows
//! showing the *tail* of [`Self::scrollback`], oldest falling off the
//! top), typed with the keyboard and submitted with Enter.
//!
//! ### The widget-index contract
//!
//! Creation order *is* the address space (see [`super`]): [`TERM_LINE_COUNT`]
//! transcript labels first, then the prompt at [`TERM_INPUT`] — which is
//! why [`Terminal::on_key`] reads and clears exactly that index.
//!
//! ### Why commands return `Vec<String>` instead of printing
//!
//! [`Terminal::execute`] is pure: command + `&FileSystem` in, output lines
//! out. The caller echoes, appends, repaints and logs — one path for
//! every command, and `execute` can be reasoned about (and later tested)
//! without a window manager in hand. Multi-line output (`cat`, `ls`,
//! `help`) is just more lines in the returned vector; oversized lines are
//! clipped at paint time, never at build time.
//!
//! ### Failure is output, not a panic
//!
//! The ramfs is live and user-driven: files vanish between `ls` and
//! `cat`, paths can name a directory, `write` can target the root. Every
//! one of those answers with a line of text (`file not found: <path>`,
//! `cannot write <path>`, ...) — this module never unwraps user input.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::clip;
use crate::event::{KeyCode, KeyEvent};
use crate::fs::FileSystem;
use crate::gui::widgets::Widget;
use crate::gui::{Theme, WidgetOutcome, WindowManager};

// ---------------------------------------------------------------------------
// Widget indices (creation order — see module docs)
// ---------------------------------------------------------------------------

/// First of the [`TERM_LINE_COUNT`] transcript row labels.
pub const TERM_LINES: usize = 0;
/// How many scrollback lines fit in the window at once.
pub const TERM_LINE_COUNT: usize = 14;
/// The prompt `TextInput` — read (and cleared) on Enter.
pub const TERM_INPUT: usize = TERM_LINES + TERM_LINE_COUNT;

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

const WIN_W: i32 = 300;
const WIN_H: i32 = 246;
/// First transcript row's y, and the row pitch (8 px glyphs + 5 px gap).
const LINE_Y: i32 = 32;
const LINE_H: i32 = 13;
/// Characters of a transcript line before clipping (35 x 8 = 280 px plus
/// the 12 px left margin = 292 px, inside the 300 px window).
const LINE_CLIP: usize = 35;
/// Transcript rows are user-driven and therefore unbounded; keep the last
/// this many lines so a command loop cannot eat the 16 MiB heap.
const MAX_SCROLLBACK: usize = 200;

// ---------------------------------------------------------------------------
// Terminal
// ---------------------------------------------------------------------------

/// The "Terminal" window's state: its transcript and (implicit) prompt.
///
/// Build with [`Terminal::new`] (via [`super::Apps::new`]); the prompt is
/// a widget, not a field here — the WM owns its contents and hands them
/// to [`Terminal::on_key`] on Enter.
#[derive(Debug)]
pub struct Terminal {
    /// The "Terminal" window's id — the dispatcher's routing key.
    pub win: u64,
    /// Every line ever echoed or printed (capped at [`MAX_SCROLLBACK`]);
    /// the last [`TERM_LINE_COUNT`] are on screen.
    scrollback: Vec<String>,
}

impl Terminal {
    /// Creates the "Terminal" window with every widget pushed in contract
    /// order (see module docs), seeds the transcript with the startup
    /// banner and paints it — the first frame already shows a live prompt.
    pub fn new(wm: &mut WindowManager, theme: &Theme) -> Self {
        let win = wm.create_window("Terminal", 490, 24, WIN_W, WIN_H);
        if let Some(w) = wm.window_mut(win) {
            for i in 0..TERM_LINE_COUNT {
                w.widgets.push(Widget::label(
                    "",
                    12,
                    LINE_Y + i as i32 * LINE_H,
                    theme.text_primary,
                ));
            }
            w.widgets.push(Widget::text_input(
                12,
                216,
                276,
                22,
                "command... (try 'help')",
            ));
        }
        let mut this = Self {
            win,
            scrollback: Vec::new(),
        };
        this.push(String::from("TriOS Terminal - type 'help'"));
        this.refresh(wm);
        this
    }

    /// Handles one finished pointer interaction inside the "Terminal"
    /// window.
    ///
    /// Nothing to handle: labels don't capture the pointer and a
    /// `TextInput` reports no [`WidgetOutcome`] (typing *is* the
    /// interaction), so no action can ever name one of our widgets. The
    /// method exists to satisfy [`super::Apps::on_action`]'s contract and
    /// always reports "nothing visible changed".
    pub fn on_action(
        &mut self,
        _widget_index: usize,
        _outcome: WidgetOutcome,
        _wm: &mut WindowManager,
    ) -> bool {
        false
    }

    /// Handles one key press while the "Terminal" window is focused.
    ///
    /// Enter submits: read the [`TERM_INPUT`] value, clear the field,
    /// log the command, echo it, run it and repaint the transcript.
    /// Every other key (printables, Backspace, ...) is the focused
    /// field's — `TextInput::handle_key` consumes it in `wm.handle_key`,
    /// which `main.rs` calls right after this returns.
    ///
    /// Returns whether the transcript visibly changed: a blank Enter
    /// touches nothing and reports `false`.
    pub fn on_key(&mut self, event: KeyEvent, wm: &mut WindowManager, fs: &mut FileSystem) -> bool {
        if !event.pressed || event.code != KeyCode::Enter {
            return false;
        }
        let raw = wm.text_value(self.win, TERM_INPUT).unwrap_or_default();
        let command = raw.trim();
        if command.is_empty() {
            // Nothing typed: the field is already empty, so there is
            // nothing to clear, log or repaint.
            return false;
        }
        set_field(wm, self.win, TERM_INPUT, "");
        serial_println!("[apps  ] term: {command}");

        self.push(format!("> {command}"));
        if command == "clear" {
            // "clear" means clear — including its own echo, which is
            // already in the buffer above.
            self.scrollback.clear();
            self.refresh(wm);
            return true;
        }
        for line in self.execute(command, fs) {
            self.push(line);
        }
        self.refresh(wm);
        true
    }

    // -- helpers -----------------------------------------------------------

    /// Runs `command`, returning its output lines (never its side effects:
    /// writing to the fs is the one exception). See the module docs for
    /// why this is pure. `command` is expected trimmed and non-empty —
    /// [`Terminal::on_key`] guarantees both.
    fn execute(&self, command: &str, fs: &mut FileSystem) -> Vec<String> {
        let mut out = Vec::new();
        let (name, rest) = split_first(command).unwrap_or((command, ""));
        match name {
            "help" => {
                out.push(String::from("help - show this list"));
                out.push(String::from("ls [dir] - list a directory"));
                out.push(String::from("cat <path> - print a file"));
                out.push(String::from("write <path> <text> - write a file"));
                out.push(String::from("echo <text> - print text"));
                out.push(String::from("clear - clear scrollback"));
                out.push(String::from("mem - free heap bytes"));
                out.push(String::from("uptime - seconds since boot"));
                out.push(String::from("elfinfo <path> - inspect ELF executable"));
                out.push(String::from("elfrun <path> - load and execute ELF binary"));
                out.push(String::from("disks - list SATA storage controllers"));
                out.push(String::from("gpt - list GPT partitions on disk"));
                out.push(String::from("fatls [dir] - list FAT volume directory"));
                out.push(String::from("netinfo - show network adapter & IP stats"));
                out.push(String::from("arp - display ARP cache table"));
                out.push(String::from("ping <ip> - send ICMP Echo Request"));
                out.push(String::from("date - calendar date & time from RTC"));
                out.push(String::from("dns <host> - resolve hostname via UDP"));
                out.push(String::from("tcp <ip> <port> - test TCP 3-way handshake"));
                out.push(String::from("curl <url> - HTTP/1.0 web client request"));
                out.push(String::from("acpi - inspect ACPI tables & power config"));
                out.push(String::from("poweroff - ACPI S5 shutdown computer"));
                out.push(String::from("reboot - hardware restart computer"));
                out.push(String::from("apps: Ctrl+1=Files, Ctrl+2=Editor, Ctrl+3=Term, Ctrl+4=Browser"));
            }
            "ls" => {
                let dir = if rest.is_empty() {
                    "/"
                } else {
                    split_first(rest).map_or(rest, |(d, _)| d)
                };
                if fs.exists(dir) && fs.read(dir).is_none() && !dir.ends_with('/') && fs.read_bytes(dir).is_some() {
                    // Regular file: print itself
                    out.push(String::from(dir));
                } else if !fs.exists(dir) {
                    out.push(format!("file not found: {dir}"));
                } else {
                    let entries = fs.list(dir);
                    if entries.is_empty() {
                        out.push(format!("(empty) {dir}"));
                    }
                    for entry in entries {
                        out.push(format!(
                            "{}{}",
                            entry.name,
                            if entry.is_dir { "/" } else { "" }
                        ));
                    }
                }
            }
            "cat" => {
                if rest.is_empty() {
                    out.push(String::from("usage: cat <path>"));
                } else {
                    let path = split_first(rest).map_or(rest, |(p, _)| p);
                    match fs.read(path) {
                        // A missing path, binary file, and a directory all answer
                        // `None` — neither is text, neither panics.
                        None => {
                            if fs.exists(path) {
                                out.push(format!("binary file or directory: {path}"));
                            } else {
                                out.push(format!("file not found: {path}"));
                            }
                        }
                        Some("") => out.push(String::from("(empty)")),
                        Some(text) => {
                            for line in text.split('\n') {
                                out.push(String::from(line));
                            }
                        }
                    }
                }
            }
            "write" => {
                if rest.is_empty() {
                    out.push(String::from("usage: write <path> <text...>"));
                } else {
                    // Split the path off the *original* remainder so the
                    // text keeps its internal spacing (splitting on
                    // whitespace would collapse "a  b" to "a b").
                    let (path, text) = split_first(rest).unwrap_or((rest, ""));
                    if fs.write(path, text) {
                        out.push(format!("wrote {path} ({} bytes)", text.len()));
                    } else {
                        out.push(format!("cannot write {path}"));
                    }
                }
            }
            "echo" => out.push(String::from(rest)),
            "mem" => out.push(format!(
                "heap free: {} bytes",
                crate::allocator::remaining()
            )),
            "uptime" => out.push(format!("uptime: {} s", crate::timer::uptime_seconds())),
            "elfinfo" => {
                if rest.is_empty() {
                    out.push(String::from("usage: elfinfo <path>"));
                } else {
                    let path = split_first(rest).map_or(rest, |(p, _)| p);
                    match fs.read_bytes(path) {
                        None => out.push(format!("file not found: {path}")),
                        Some(bytes) => {
                            match crate::fs::elf::parse_header(bytes) {
                                Ok(hdr) => {
                                    out.push(format!("ELF64 entry: {:#x}", hdr.entry_point));
                                    out.push(format!("program headers: count={}", hdr.ph_num));
                                    for i in 0..hdr.ph_num {
                                        if let Some(ph) = crate::fs::elf::parse_program_header(bytes, &hdr, i) {
                                            out.push(format!(
                                                "PH#{i}: type={} vaddr={:#x} memsz={}",
                                                ph.p_type, ph.p_vaddr, ph.p_memsz
                                            ));
                                        }
                                    }
                                }
                                Err(err) => out.push(format!("not an ELF binary: {err:?}")),
                            }
                        }
                    }
                }
            }
            "elfrun" => {
                if rest.is_empty() {
                    out.push(String::from("usage: elfrun <path>"));
                } else {
                    let path = split_first(rest).map_or(rest, |(p, _)| p);
                    match fs.read_bytes(path) {
                        None => out.push(format!("file not found: {path}")),
                        Some(bytes) => {
                            match crate::fs::elf::load_executable(bytes) {
                                Ok(loaded) => {
                                    let entry = loaded.entry_point;
                                    let tid = crate::sched::spawn_elf(
                                        path,
                                        entry,
                                        loaded.image,
                                    );
                                    out.push(format!("spawned ELF thread #{tid} (entry={entry:#x})"));
                                }
                                Err(err) => out.push(format!("ELF load error: {err:?}")),
                            }
                        }
                    }
                }
            }
            "gpt" | "partitions" => {
                match crate::fs::gpt::scan_partitions(crate::drivers::ahci::read_sector) {
                    Ok((_hdr, parts)) => {
                        out.push(format!("GPT valid: {} partition(s)", parts.len()));
                        for (i, p) in parts.iter().enumerate() {
                            out.push(format!(
                                "#{i}: '{}' LBA {}..{} ({} KB)",
                                p.name,
                                p.starting_lba,
                                p.ending_lba,
                                (p.ending_lba - p.starting_lba + 1) / 2
                            ));
                        }
                    }
                    Err(err) => out.push(format!("GPT scan error: {err}")),
                }
            }
            "fatls" => {
                let dir = if rest.is_empty() { "/" } else { rest };
                match crate::fs::gpt::scan_partitions(crate::drivers::ahci::read_sector) {
                    Ok((_, parts)) => {
                        if let Some(boot) = parts.first() {
                            match crate::fs::fat::FatFileSystem::mount(boot.starting_lba, &crate::drivers::ahci::read_sector) {
                                Ok(fat_fs) => {
                                    match fat_fs.list_dir(dir, &crate::drivers::ahci::read_sector) {
                                        Ok(entries) => {
                                            out.push(format!("FAT [{dir}] ({} entries):", entries.len()));
                                            for e in entries {
                                                out.push(format!(
                                                    "{}{}",
                                                    e.name,
                                                    if e.is_dir { "/" } else { "" }
                                                ));
                                            }
                                        }
                                        Err(err) => out.push(format!("fatls error: {err}")),
                                    }
                                }
                                Err(err) => out.push(format!("mount error: {err}")),
                            }
                        } else {
                            out.push(String::from("no partitions found"));
                        }
                    }
                    Err(err) => out.push(format!("disk error: {err}")),
                }
            }
            "disks" => {
                let lock = crate::drivers::ahci::AHCI_CONTROLLER.lock();
                match lock.as_ref() {
                    Some(ctrl) => {
                        out.push(format!(
                            "AHCI SATA BAR5={:#x} (active drives: {})",
                            ctrl.bar5_phys,
                            ctrl.active_drives.len()
                        ));
                        for port in &ctrl.active_drives {
                            out.push(format!("  Port #{port}: SATA Hard Disk [Ready]"));
                        }
                    }
                    None => out.push(String::from("AHCI storage controller not initialized")),
                }
            }
            "netinfo" | "ifconfig" => {
                let nic_guard = crate::drivers::e1000::E1000.lock();
                match nic_guard.as_ref() {
                    Some(nic) => {
                        let mac = nic.mac;
                        let link = if nic.is_link_up() { "UP" } else { "DOWN" };
                        out.push(format!("eth0: Intel 82540EM [Link {link}]"));
                        out.push(format!(
                            "  MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                        ));
                        let net = crate::net::NET.lock();
                        out.push(format!(
                            "  IP:  {}.{}.{}.{}/24",
                            net.ip[0], net.ip[1], net.ip[2], net.ip[3]
                        ));
                        out.push(format!(
                            "  GW:  {}.{}.{}.{}",
                            net.gateway[0], net.gateway[1], net.gateway[2], net.gateway[3]
                        ));
                        out.push(format!(
                            "  TX:  {} pkts ({} B)",
                            nic.packets_tx, nic.bytes_tx
                        ));
                        out.push(format!(
                            "  RX:  {} pkts ({} B)",
                            nic.packets_rx, nic.bytes_rx
                        ));
                    }
                    None => out.push(String::from("no network interface card detected")),
                }
            }
            "arp" => {
                let net = crate::net::NET.lock();
                if net.arp_cache.is_empty() {
                    out.push(String::from("ARP cache is empty"));
                } else {
                    out.push(format!("ARP cache ({} entries):", net.arp_cache.len()));
                    for entry in &net.arp_cache {
                        out.push(format!(
                            "{}.{}.{}.{} at {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                            entry.ip[0], entry.ip[1], entry.ip[2], entry.ip[3],
                            entry.mac[0], entry.mac[1], entry.mac[2], entry.mac[3], entry.mac[4], entry.mac[5]
                        ));
                    }
                }
            }
            "ping" => {
                if rest.is_empty() {
                    out.push(String::from("usage: ping <ip> (e.g. ping 10.0.2.2)"));
                } else {
                    let target_str = split_first(rest).map_or(rest, |(p, _)| p);
                    let mut ip_parts = [0u8; 4];
                    let mut count = 0;
                    let mut valid = true;
                    for p in target_str.split('.') {
                        if count < 4 {
                            if let Ok(val) = p.parse::<u8>() {
                                ip_parts[count] = val;
                                count += 1;
                            } else {
                                valid = false;
                                break;
                            }
                        } else {
                            valid = false;
                            break;
                        }
                    }
                    if !valid || count != 4 {
                        out.push(format!("invalid IPv4 address: {target_str}"));
                    } else {
                        out.push(format!("PING {}.{}.{}.{}: 32 data bytes", ip_parts[0], ip_parts[1], ip_parts[2], ip_parts[3]));

                        // Send ARP request to populate ARP cache if not already present
                        let _ = crate::net::send_arp_request(ip_parts);
                        for _ in 0..10_000 {
                            crate::net::poll_network();
                        }

                        // Clear old ping replies with seq 1
                        {
                            let mut net = crate::net::NET.lock();
                            net.ping_replies.retain(|r| r.seq != 1);
                        }

                        let start_ticks = crate::timer::ticks();
                        match crate::net::send_icmp_ping(ip_parts, 1) {
                            Ok(()) => {
                                let mut reply = None;
                                for _ in 0..600_000 {
                                    crate::net::poll_network();
                                    let net = crate::net::NET.lock();
                                    if let Some(pos) = net.ping_replies.iter().position(|r| r.seq == 1 && r.from_ip == ip_parts) {
                                        reply = Some(net.ping_replies[pos].clone());
                                        break;
                                    }
                                    core::hint::spin_loop();
                                }
                                let elapsed = crate::timer::ticks().saturating_sub(start_ticks);
                                let ms = elapsed * 10;
                                if let Some(r) = reply {
                                    out.push(format!(
                                        "64B from {}.{}.{}.{}: t={}ms",
                                        r.from_ip[0], r.from_ip[1], r.from_ip[2], r.from_ip[3],
                                        if ms == 0 { 1 } else { ms }
                                    ));
                                } else {
                                    out.push(format!("ping timeout: no reply from {target_str}"));
                                }
                            }
                            Err(err) => {
                                out.push(format!("ping send error: {err}"));
                            }
                        }
                    }
                }
            }
            "date" => {
                let dt = crate::timer::read_rtc();
                out.push(format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                    dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
                ));
            }
            "dns" | "nslookup" => {
                if rest.is_empty() {
                    out.push(String::from("usage: dns <hostname> (e.g. dns router)"));
                } else {
                    let hostname = split_first(rest).map_or(rest, |(h, _)| h);
                    out.push(format!("resolving {hostname}..."));
                    match crate::net::resolve_dns(hostname) {
                        Ok(ip) => {
                            out.push(format!(
                                "{hostname} -> {}.{}.{}.{}",
                                ip[0], ip[1], ip[2], ip[3]
                            ));
                        }
                        Err(e) => {
                            out.push(format!("dns error: {e}"));
                        }
                    }
                }
            }
            "tcp" => {
                if rest.is_empty() {
                    out.push(String::from("usage: tcp <ip> <port> (e.g. tcp 10.0.2.2 80)"));
                } else {
                    let (ip_str, port_str) = split_first(rest).unwrap_or((rest, "80"));
                    let port: u16 = port_str.trim().parse().unwrap_or(80);
                    let mut ip_parts = [0u8; 4];
                    let mut count = 0;
                    for p in ip_str.split('.') {
                        if count < 4 {
                            if let Ok(val) = p.parse::<u8>() {
                                ip_parts[count] = val;
                                count += 1;
                            }
                        }
                    }
                    if count != 4 {
                        out.push(format!("invalid IPv4: {ip_str}"));
                    } else {
                        out.push(format!(
                            "connecting to {}.{}.{}.{}:{}...",
                            ip_parts[0], ip_parts[1], ip_parts[2], ip_parts[3], port
                        ));
                        match crate::net::tcp::TcpStream::connect(ip_parts, port) {
                            Ok(sock) => {
                                out.push(format!("TCP connected on local port {sock}!"));
                                crate::net::tcp::TcpStream::close(sock);
                                out.push(String::from("TCP stream closed cleanly"));
                            }
                            Err(e) => {
                                out.push(format!("TCP connect error: {e}"));
                            }
                        }
                    }
                }
            }
            "curl" | "fetch" => {
                if rest.is_empty() {
                    out.push(String::from("usage: curl <url> (e.g. curl http://example.com/ or curl 10.0.2.2:8000)"));
                } else {
                    let url = split_first(rest).map_or(rest, |(u, _)| u);
                    out.push(format!("fetching {url}..."));
                    match crate::net::http::http_get(url) {
                        Ok(resp) => {
                            out.push(format!("HTTP/1.0 {} {}", resp.status_code, resp.status_text));
                            out.push(format!("Content-Type: {} ({} bytes)", resp.content_type, resp.body.len()));
                            if let Ok(body_str) = core::str::from_utf8(&resp.body) {
                                let mut lines_shown = 0;
                                for line in body_str.lines() {
                                    if lines_shown >= 6 {
                                        out.push(format!("... ({} bytes total)", resp.body.len()));
                                        break;
                                    }
                                    if !line.trim().is_empty() {
                                        out.push(String::from(line));
                                        lines_shown += 1;
                                    }
                                }
                            } else {
                                out.push(format!("(binary payload: {} bytes)", resp.body.len()));
                            }
                        }
                        Err(e) => {
                            out.push(format!("curl error: {e}"));
                        }
                    }
                }
            }
            "acpi" => {
                let guard = crate::acpi::ACPI.lock();
                match guard.as_ref() {
                    Some(info) => {
                        out.push(format!("RSDP: {:#x}", info.rsdp_phys));
                        out.push(format!(
                            "OEM: {}{}{}{}{}{}",
                            info.oem_id[0] as char, info.oem_id[1] as char, info.oem_id[2] as char,
                            info.oem_id[3] as char, info.oem_id[4] as char, info.oem_id[5] as char
                        ));
                        out.push(format!("PM1a_CNT: {:#x} (FADT={})", info.pm1a_cnt, info.has_fadt));
                        out.push(format!("CPU Cores (MADT): {}", info.cpu_cores));
                        out.push(format!("S5 TypeA={:#x} TypeB={:#x}", info.slp_typa, info.slp_typb));
                    }
                    None => out.push(String::from("ACPI subsystem not initialized")),
                }
            }
            "poweroff" | "shutdown" => {
                out.push(String::from("shutting down system..."));
                crate::acpi::poweroff();
            }
            "reboot" | "restart" => {
                out.push(String::from("rebooting system..."));
                crate::acpi::reboot();
            }
            "" => {}
            other => out.push(format!("unknown command: {other} - try 'help'")),
        }
        out
    }

    /// Appends one line to the transcript, dropping the oldest line once
    /// the cap in [`MAX_SCROLLBACK`] is exceeded.
    fn push(&mut self, line: String) {
        self.scrollback.push(line);
        if self.scrollback.len() > MAX_SCROLLBACK {
            self.scrollback.remove(0);
        }
    }

    /// Repaints the [`TERM_LINE_COUNT`] transcript rows from the tail of
    /// [`Self::scrollback`]: newest at the bottom, older lines scrolling
    /// off the top, short transcripts leaving blank rows above.
    fn refresh(&self, wm: &mut WindowManager) {
        let start = self.scrollback.len().saturating_sub(TERM_LINE_COUNT);
        for i in 0..TERM_LINE_COUNT {
            let text = self
                .scrollback
                .get(start + i)
                .map(|line| clip(line, LINE_CLIP))
                .unwrap_or_default();
            wm.set_label(self.win, TERM_LINES + i, text);
        }
    }
}

// ---------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------

/// Splits `s` on its first whitespace run: `("ls", "/notes")` for
/// `"ls /notes"`, `None` when `s` holds no whitespace. The remainder is
/// left-trimmed so callers can treat it as "everything after the word".
fn split_first(s: &str) -> Option<(&str, &str)> {
    let i = s.find(char::is_whitespace)?;
    Some((&s[..i], s[i..].trim_start()))
}

/// Writes `value` into the text field at `index` (caret to the end).
///
/// The WM exposes no public setter for a field's value — it owns *focus*,
/// not content — so this reaches through [`WindowManager::window_mut`].
/// A closed window or stale index is a silent no-op: Esc can close the
/// window mid-batch, and that must never panic.
fn set_field(wm: &mut WindowManager, win: u64, index: usize, value: &str) {
    if let Some(w) = wm.window_mut(win)
        && let Some(Widget::TextInput(t)) = w.widgets.get_mut(index)
    {
        t.value = String::from(value);
        t.cursor = t.value.len();
    }
}
