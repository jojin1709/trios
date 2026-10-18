//! TriOS GUI Web Browser Application ("Browser").
//!
//! Provides an interactive desktop web browser featuring:
//! - URL address bar with 'Go' button and Enter key navigation
//! - Integration with DNS resolver and bare-metal HTTP/1.0 client
//! - HTML tag tokenizer, entity decoder, and block-level text formatter
//! - Smooth scrolling content viewport with line counters and status indicators
//!
//! Developed by JOJIN JOHN (@jojin1709) for TriOS.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::clip;
use crate::event::{KeyCode, KeyEvent};
use crate::gui::widgets::Widget;
use crate::gui::{Theme, WidgetOutcome, WindowManager};

// ---------------------------------------------------------------------------
// Widget Indices (Creation Order Contract)
// ---------------------------------------------------------------------------

/// The URL input field where the user types target URLs.
pub const BR_URL: usize = 0;
/// The "Go" button that triggers web page retrieval.
pub const BR_GO: usize = 1;
/// The status indicator label (shows connection, status codes, and payload bytes).
pub const BR_STATUS: usize = 2;
/// First label index of the content display rows.
pub const BR_ROWS: usize = 3;
/// Number of text display rows visible in the viewport.
pub const BR_ROW_COUNT: usize = 16;
/// Footer label displaying scroll position and navigation hints.
pub const BR_FOOTER: usize = BR_ROWS + BR_ROW_COUNT;

// ---------------------------------------------------------------------------
// Layout Metrics
// ---------------------------------------------------------------------------

const WIN_W: i32 = 540;
const WIN_H: i32 = 376;
const ROW_Y0: i32 = 88;
const ROW_H: i32 = 14;
const ROW_CLIP: usize = 62;

// ---------------------------------------------------------------------------
// Browser State
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct Browser {
    /// Window ID registered with the WindowManager.
    pub win: u64,
    /// Current URL target.
    pub url: String,
    /// Status message displayed under the address bar.
    pub status: String,
    /// Parsed lines of content from the retrieved web page.
    pub lines: Vec<String>,
    /// First visible line index in the scrollable viewport.
    pub scroll: usize,
}

impl Browser {
    /// Builds the Browser window and its widgets in contract order.
    pub fn new(wm: &mut WindowManager, theme: &Theme) -> Self {
        let win = wm.create_window("Web Browser", 240, 60, WIN_W, WIN_H);
        if let Some(w) = wm.window_mut(win) {
            // Widget 0: URL TextInput
            let mut input = Widget::text_input(
                12,
                38,
                440,
                22,
                "http://example.com",
            );
            if let Widget::TextInput(ref mut t) = input {
                t.value = String::from("http://example.com");
                t.cursor = t.value.len();
            }
            w.widgets.push(input);
            // Widget 1: "Go" Button
            w.widgets.push(Widget::button(460, 38, 68, 22, "Go"));
            // Widget 2: Status Label
            w.widgets.push(Widget::label(
                "Ready | Enter URL and click Go or press Enter",
                12,
                66,
                theme.text_secondary,
            ));
            // Widgets 3..18: 16 Content Rows
            for i in 0..BR_ROW_COUNT {
                w.widgets.push(Widget::label(
                    "",
                    14,
                    ROW_Y0 + (i as i32) * ROW_H,
                    theme.text_primary,
                ));
            }
            // Widget 19: Footer Label
            w.widgets.push(Widget::label(
                "Type URL & press Enter | ArrowUp/Down to scroll",
                12,
                WIN_H - 24,
                theme.text_secondary,
            ));
        }

        let mut browser = Self {
            win,
            url: String::from("http://example.com"),
            status: String::from("Ready | Click Go or press Enter to fetch"),
            lines: Vec::new(),
            scroll: 0,
        };

        // Initialize welcome splash content
        browser.lines.push(String::from("=== TriOS Web Browser v1.0 ==="));
        browser.lines.push(String::from("Developer: JOJIN JOHN (@jojin1709)"));
        browser.lines.push(String::new());
        browser.lines.push(String::from("Features:"));
        browser.lines.push(String::from(" - Full RFC 793 Transmission Control Protocol (TCP) stack"));
        browser.lines.push(String::from(" - Bare-metal HTTP/1.0 client with automated URL parsing"));
        browser.lines.push(String::from(" - RFC 1035 UDP DNS resolution integration"));
        browser.lines.push(String::from(" - Real-time HTML tokenizer & text formatting engine"));
        browser.lines.push(String::new());
        browser.lines.push(String::from("Quick Navigation:"));
        browser.lines.push(String::from(" * Press Enter or click 'Go' to fetch 'http://example.com'"));
        browser.lines.push(String::from(" * Use ArrowUp / ArrowDown keys to scroll contents"));

        browser.paint(wm);
        browser
    }

    /// Fetches the specified URL via HTTP GET and formats the response.
    pub fn navigate(&mut self, url: &str, wm: &mut WindowManager) {
        crate::serial_println!("[browser] navigating to '{url}'...");
        self.url = String::from(url);
        wm.set_label(self.win, BR_STATUS, format!("Connecting to {url}..."));
        wm.set_label(self.win, BR_ROWS, String::from("Fetching page over TCP stream..."));

        match crate::net::http::http_get(url) {
            Ok(resp) => {
                let status_msg = format!(
                    "HTTP/1.0 {} {} ({} B, {})",
                    resp.status_code, resp.status_text, resp.body.len(), resp.content_type
                );
                self.status = status_msg.clone();
                wm.set_label(self.win, BR_STATUS, status_msg);

                let raw_text = core::str::from_utf8(&resp.body).unwrap_or("(non-utf8 payload)");
                self.lines = parse_html_to_lines(raw_text);
                self.scroll = 0;
            }
            Err(err) => {
                let err_msg = format!("Navigation Error: {err}");
                self.status = err_msg.clone();
                wm.set_label(self.win, BR_STATUS, err_msg);
                self.lines.clear();
                self.lines.push(format!("Failed to retrieve {url}"));
                self.lines.push(format!("Reason: {err}"));
                self.lines.push(String::new());
                self.lines.push(String::from("Check network cable, DNS settings, or host URL."));
                self.scroll = 0;
            }
        }

        self.paint(wm);
    }

    /// Handles pointer click events in the Browser window.
    pub fn on_action(
        &mut self,
        widget_index: usize,
        outcome: WidgetOutcome,
        wm: &mut WindowManager,
    ) -> bool {
        if outcome != WidgetOutcome::Clicked {
            return false;
        }

        if widget_index == BR_GO {
            let target = wm
                .text_value(self.win, BR_URL)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| self.url.clone());
            self.navigate(target.trim(), wm);
            return true;
        }

        false
    }

    /// Handles keyboard events when the Browser window or address bar is focused.
    pub fn on_key(&mut self, event: KeyEvent, wm: &mut WindowManager) -> bool {
        if !event.pressed {
            return false;
        }

        match event.code {
            KeyCode::Enter => {
                let target = wm
                    .text_value(self.win, BR_URL)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| self.url.clone());
                self.navigate(target.trim(), wm);
                true
            }
            KeyCode::ArrowDown => {
                if self.scroll + 1 < self.lines.len() {
                    self.scroll += 1;
                    self.paint(wm);
                    true
                } else {
                    false
                }
            }
            KeyCode::ArrowUp => {
                if self.scroll > 0 {
                    self.scroll -= 1;
                    self.paint(wm);
                    true
                } else {
                    false
                }
            }
            KeyCode::Home => {
                self.scroll = 0;
                self.paint(wm);
                true
            }
            KeyCode::End => {
                self.scroll = self.lines.len().saturating_sub(BR_ROW_COUNT);
                self.paint(wm);
                true
            }
            _ => false,
        }
    }

    /// Repaints all visible text rows and the scroll indicator footer.
    fn paint(&self, wm: &mut WindowManager) {
        for i in 0..BR_ROW_COUNT {
            let line_idx = self.scroll + i;
            let text = if let Some(line) = self.lines.get(line_idx) {
                clip(line, ROW_CLIP)
            } else {
                String::new()
            };
            wm.set_label(self.win, BR_ROWS + i, text);
        }

        let total = self.lines.len();
        let current_top = if total == 0 { 0 } else { self.scroll + 1 };
        let current_bot = (self.scroll + BR_ROW_COUNT).min(total);
        let footer = format!(
            "Lines {current_top}-{current_bot} of {total} | ArrowUp/Down to scroll"
        );
        wm.set_label(self.win, BR_FOOTER, footer);
    }
}

// ---------------------------------------------------------------------------
// HTML Tokenizer & Line Formatter
// ---------------------------------------------------------------------------

/// Tokenizes HTML markup and converts it into cleanly formatted plain-text lines.
pub fn parse_html_to_lines(html: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut in_script_or_style = false;
    let mut active_block_tag = String::new();

    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];

        // 1. Tag parsing
        if ch == '<' {
            i += 1;
            let mut tag = String::new();
            while i < chars.len() && chars[i] != '>' && !chars[i].is_whitespace() {
                tag.push(chars[i].to_ascii_lowercase());
                i += 1;
            }
            // Skip remaining tag attributes until '>'
            while i < chars.len() && chars[i] != '>' {
                i += 1;
            }
            if i < chars.len() && chars[i] == '>' {
                i += 1;
            }

            if tag == "script" || tag == "style" {
                in_script_or_style = true;
                active_block_tag = tag;
            } else if in_script_or_style && tag == format!("/{active_block_tag}") {
                in_script_or_style = false;
            } else if !in_script_or_style {
                // Formatting block-level elements
                if tag == "p" || tag == "/p" || tag == "br" || tag == "div" || tag == "/div"
                    || tag == "h1" || tag == "/h1" || tag == "h2" || tag == "/h2" || tag == "h3"
                    || tag == "/h3" || tag == "li" || tag == "/li" || tag == "tr" || tag == "/tr"
                    || tag == "hr" || tag == "title" || tag == "/title"
                {
                    if !current_line.trim().is_empty() {
                        lines.push(current_line.trim().into());
                        current_line.clear();
                    }
                    if tag == "li" {
                        current_line.push_str(" * ");
                    } else if tag == "h1" {
                        current_line.push_str("# ");
                    } else if tag == "h2" {
                        current_line.push_str("## ");
                    } else if tag == "h3" {
                        current_line.push_str("### ");
                    } else if tag == "title" {
                        current_line.push_str("[Page Title] ");
                    } else if tag == "hr" {
                        lines.push(String::from("--------------------------------------------------"));
                    }
                }
            }
            continue;
        }

        // 2. Ignore interior of script and style blocks
        if in_script_or_style {
            i += 1;
            continue;
        }

        // 3. HTML Entity decoding
        if ch == '&' {
            let rem_len = (chars.len() - i).min(8);
            let entity: String = chars[i..i + rem_len].iter().collect();
            if entity.starts_with("&nbsp;") {
                current_line.push(' ');
                i += 6;
                continue;
            } else if entity.starts_with("&lt;") {
                current_line.push('<');
                i += 4;
                continue;
            } else if entity.starts_with("&gt;") {
                current_line.push('>');
                i += 4;
                continue;
            } else if entity.starts_with("&amp;") {
                current_line.push('&');
                i += 5;
                continue;
            } else if entity.starts_with("&quot;") {
                current_line.push('"');
                i += 6;
                continue;
            }
        }

        // 4. Whitespace and character accumulation
        if ch == '\n' || ch == '\r' {
            if !current_line.trim().is_empty() {
                lines.push(current_line.trim().into());
                current_line.clear();
            }
        } else if ch == '\t' || ch.is_whitespace() {
            if !current_line.ends_with(' ') && !current_line.is_empty() {
                current_line.push(' ');
            }
        } else {
            current_line.push(ch);
            if current_line.len() >= ROW_CLIP - 2 {
                lines.push(current_line.clone());
                current_line.clear();
            }
        }
        i += 1;
    }

    if !current_line.trim().is_empty() {
        lines.push(current_line.trim().into());
    }

    if lines.is_empty() {
        lines.push(String::from("(empty document)"));
    }

    lines
}
