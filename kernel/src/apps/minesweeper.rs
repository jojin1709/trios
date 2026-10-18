//! TriOS Desktop Minesweeper Game ("TriSweeper").
//!
//! A classic 9x9 retro puzzle game featuring:
//! - 9x9 grid with 10 hidden mines
//! - First-click protection (mines are placed after the first click)
//! - Recursive zero-neighbor flood fill
//! - HUD with live mine counter, timer, reset face button, and Dig/Flag mode toggle
//! - Clean win and loss condition handling
//!
//! Developed by JOJIN JOHN (@jojin1709) for TriOS.

use alloc::format;
use alloc::string::String;

use crate::event::{KeyCode, KeyEvent};
use crate::gui::widgets::Widget;
use crate::gui::{Theme, WidgetOutcome, WindowManager};
use crate::perf;
use crate::timer;

// ---------------------------------------------------------------------------
// Widget Indices (Creation Order Contract)
// ---------------------------------------------------------------------------

pub const MS_LABEL_MINES: usize = 0;
pub const MS_BTN_FACE: usize = 1;
pub const MS_LABEL_TIME: usize = 2;
pub const MS_BTN_MODE: usize = 3;
pub const MS_LABEL_STATUS: usize = 4;
pub const MS_GRID_START: usize = 5;
pub const MS_GRID_COUNT: usize = 81; // 9 * 9 = 81 buttons

// ---------------------------------------------------------------------------
// Layout Metrics
// ---------------------------------------------------------------------------

const WIN_W: i32 = 300;
const WIN_H: i32 = 376;

const GRID_COLS: usize = 9;
const GRID_ROWS: usize = 9;
const TOTAL_MINES: usize = 10;

const CELL_SIZE: i32 = 28;
const GRID_X0: i32 = (WIN_W - (GRID_COLS as i32 * CELL_SIZE)) / 2; // (300 - 252) / 2 = 24
const GRID_Y0: i32 = 82;

// ---------------------------------------------------------------------------
// Game State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellState {
    Hidden,
    Revealed,
    Flagged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameStatus {
    Ready,
    Playing,
    Won,
    Lost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickMode {
    Dig,
    Flag,
}

#[derive(Debug)]
pub struct Minesweeper {
    pub win: u64,
    pub status: GameStatus,
    pub mode: ClickMode,
    pub mines: [bool; MS_GRID_COUNT],
    pub states: [CellState; MS_GRID_COUNT],
    pub neighbors: [u8; MS_GRID_COUNT],
    pub remaining_flags: i32,
    pub revealed_count: usize,
    pub elapsed_seconds: u32,
    pub last_tick_sec: u64,
}

impl Minesweeper {
    /// Builds the Minesweeper window and 9x9 grid buttons in contract order.
    pub fn new(wm: &mut WindowManager, theme: &Theme) -> Self {
        let win = wm.create_window("TriSweeper", 350, 90, WIN_W, WIN_H);
        if let Some(w) = wm.window_mut(win) {
            // 0: Mine Counter Label
            w.widgets.push(Widget::label("Mines: 10", 24, 42, theme.text_primary));
            // 1: Reset Face Button
            w.widgets.push(Widget::button(120, 36, 60, 24, ":-)"));
            // 2: Timer Label
            w.widgets.push(Widget::label("Time: 0s", 216, 42, theme.text_primary));
            // 3: Mode Toggle Button
            w.widgets.push(Widget::button(24, 60, 100, 18, "[Mode: Dig]"));
            // 4: Status footer
            w.widgets.push(Widget::label(
                "Click: Dig | 'F': Mode | 'R': Reset",
                24,
                348,
                theme.text_secondary,
            ));

            // 5..85: 9x9 Grid Buttons
            for r in 0..GRID_ROWS {
                for c in 0..GRID_COLS {
                    let x = GRID_X0 + (c as i32 * CELL_SIZE);
                    let y = GRID_Y0 + (r as i32 * CELL_SIZE);
                    w.widgets.push(Widget::button(x, y, CELL_SIZE, CELL_SIZE, ""));
                }
            }
        }

        Self {
            win,
            status: GameStatus::Ready,
            mode: ClickMode::Dig,
            mines: [false; MS_GRID_COUNT],
            states: [CellState::Hidden; MS_GRID_COUNT],
            neighbors: [0; MS_GRID_COUNT],
            remaining_flags: TOTAL_MINES as i32,
            revealed_count: 0,
            elapsed_seconds: 0,
            last_tick_sec: 0,
        }
    }

    /// Resets the game to initial fresh state.
    pub fn reset(&mut self, wm: &mut WindowManager) {
        self.status = GameStatus::Ready;
        self.mines = [false; MS_GRID_COUNT];
        self.states = [CellState::Hidden; MS_GRID_COUNT];
        self.neighbors = [0; MS_GRID_COUNT];
        self.remaining_flags = TOTAL_MINES as i32;
        self.revealed_count = 0;
        self.elapsed_seconds = 0;

        wm.set_button_label(self.win, MS_BTN_FACE, String::from(":-)"));
        wm.set_label(self.win, MS_LABEL_MINES, String::from("Mines: 10"));
        wm.set_label(self.win, MS_LABEL_TIME, String::from("Time: 0s"));
        wm.set_label(
            self.win,
            MS_LABEL_STATUS,
            String::from("Click: Dig | 'F': Mode | 'R': Reset"),
        );

        for i in 0..MS_GRID_COUNT {
            wm.set_button_label(self.win, MS_GRID_START + i, String::new());
        }
    }

    /// Initializes mines randomly, ensuring `safe_idx` and its neighbors are never mines.
    fn init_mines(&mut self, safe_idx: usize) {
        let (safe_r, safe_c) = (safe_idx / GRID_COLS, safe_idx % GRID_COLS);
        let mut prng = perf::tsc_now() ^ (timer::ticks() << 16) ^ 0x5DEECE66D;
        let mut placed = 0;

        while placed < TOTAL_MINES {
            // Xorshift PRNG step
            prng ^= prng << 13;
            prng ^= prng >> 17;
            prng ^= prng << 5;

            let idx = (prng as usize) % MS_GRID_COUNT;
            let (r, c) = (idx / GRID_COLS, idx % GRID_COLS);

            // Keep safe cell and its immediate neighbors mine-free
            let is_near_safe = (r as isize - safe_r as isize).abs() <= 1
                && (c as isize - safe_c as isize).abs() <= 1;

            if !self.mines[idx] && !is_near_safe {
                self.mines[idx] = true;
                placed += 1;
            }
        }

        // Calculate neighbor numbers
        for r in 0..GRID_ROWS {
            for c in 0..GRID_COLS {
                let idx = r * GRID_COLS + c;
                if self.mines[idx] {
                    continue;
                }
                let mut count = 0;
                for dr in -1..=1 {
                    for dc in -1..=1 {
                        if dr == 0 && dc == 0 {
                            continue;
                        }
                        let nr = r as isize + dr;
                        let nc = c as isize + dc;
                        if nr >= 0 && nr < GRID_ROWS as isize && nc >= 0 && nc < GRID_COLS as isize {
                            let n_idx = (nr as usize) * GRID_COLS + (nc as usize);
                            if self.mines[n_idx] {
                                count += 1;
                            }
                        }
                    }
                }
                self.neighbors[idx] = count;
            }
        }
    }

    /// Toggles between Dig and Flag modes.
    pub fn toggle_mode(&mut self, wm: &mut WindowManager) {
        self.mode = match self.mode {
            ClickMode::Dig => ClickMode::Flag,
            ClickMode::Flag => ClickMode::Dig,
        };
        let label = match self.mode {
            ClickMode::Dig => "[Mode: Dig]",
            ClickMode::Flag => "[Mode: Flag]",
        };
        wm.set_button_label(self.win, MS_BTN_MODE, String::from(label));
    }

    /// Reveals a cell, performing recursive flood fill on zero neighbors.
    fn reveal_cell(&mut self, idx: usize, wm: &mut WindowManager) {
        if self.states[idx] != CellState::Hidden {
            return;
        }

        self.states[idx] = CellState::Revealed;
        self.revealed_count += 1;

        if self.mines[idx] {
            // Hit a mine!
            self.game_over(false, wm);
            return;
        }

        let n = self.neighbors[idx];
        let label = if n > 0 {
            format!("{n}")
        } else {
            String::from(".")
        };
        wm.set_button_label(self.win, MS_GRID_START + idx, label);

        // Flood fill if 0 neighbors
        if n == 0 {
            let r = idx / GRID_COLS;
            let c = idx % GRID_COLS;
            for dr in -1..=1 {
                for dc in -1..=1 {
                    if dr == 0 && dc == 0 {
                        continue;
                    }
                    let nr = r as isize + dr;
                    let nc = c as isize + dc;
                    if nr >= 0 && nr < GRID_ROWS as isize && nc >= 0 && nc < GRID_COLS as isize {
                        let n_idx = (nr as usize) * GRID_COLS + (nc as usize);
                        if self.states[n_idx] == CellState::Hidden {
                            self.reveal_cell(n_idx, wm);
                        }
                    }
                }
            }
        }

        // Check victory (all non-mine cells uncovered)
        if self.revealed_count == MS_GRID_COUNT - TOTAL_MINES {
            self.game_over(true, wm);
        }
    }

    /// Toggles a flag on a hidden cell.
    fn toggle_flag(&mut self, idx: usize, wm: &mut WindowManager) {
        match self.states[idx] {
            CellState::Hidden => {
                self.states[idx] = CellState::Flagged;
                self.remaining_flags -= 1;
                wm.set_button_label(self.win, MS_GRID_START + idx, String::from("F"));
            }
            CellState::Flagged => {
                self.states[idx] = CellState::Hidden;
                self.remaining_flags += 1;
                wm.set_button_label(self.win, MS_GRID_START + idx, String::new());
            }
            CellState::Revealed => {}
        }
        wm.set_label(
            self.win,
            MS_LABEL_MINES,
            format!("Mines: {}", self.remaining_flags),
        );
    }

    /// Concludes game with victory or defeat.
    fn game_over(&mut self, won: bool, wm: &mut WindowManager) {
        if won {
            self.status = GameStatus::Won;
            wm.set_button_label(self.win, MS_BTN_FACE, String::from("B-)"));
            wm.set_label(
                self.win,
                MS_LABEL_STATUS,
                String::from("Victory! All safe cells cleared! [R]"),
            );
        } else {
            self.status = GameStatus::Lost;
            wm.set_button_label(self.win, MS_BTN_FACE, String::from("X-("));
            wm.set_label(
                self.win,
                MS_LABEL_STATUS,
                String::from("BOOM! Game Over! Hit [R] to retry"),
            );
            // Expose all mines
            for i in 0..MS_GRID_COUNT {
                if self.mines[i] {
                    wm.set_button_label(self.win, MS_GRID_START + i, String::from("*"));
                }
            }
        }
    }

    /// Handles button click actions in contract order.
    pub fn on_action(
        &mut self,
        widget_index: usize,
        outcome: WidgetOutcome,
        wm: &mut WindowManager,
    ) -> bool {
        if outcome != WidgetOutcome::Clicked {
            return false;
        }

        if widget_index == MS_BTN_FACE {
            self.reset(wm);
            return true;
        }

        if widget_index == MS_BTN_MODE {
            self.toggle_mode(wm);
            return true;
        }

        if widget_index >= MS_GRID_START && widget_index < MS_GRID_START + MS_GRID_COUNT {
            if self.status == GameStatus::Won || self.status == GameStatus::Lost {
                return false;
            }

            let cell_idx = widget_index - MS_GRID_START;

            // First click safety
            if self.status == GameStatus::Ready {
                self.init_mines(cell_idx);
                self.status = GameStatus::Playing;
            }

            if self.mode == ClickMode::Flag {
                self.toggle_flag(cell_idx, wm);
            } else {
                if self.states[cell_idx] != CellState::Flagged {
                    self.reveal_cell(cell_idx, wm);
                }
            }
            return true;
        }

        false
    }

    /// Handles hotkeys ('F' = toggle mode, 'R' = reset).
    pub fn on_key(&mut self, event: KeyEvent, wm: &mut WindowManager) -> bool {
        if !event.pressed {
            return false;
        }
        match event.code {
            KeyCode::Char('f') | KeyCode::Char('F') => {
                self.toggle_mode(wm);
                true
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.reset(wm);
                true
            }
            _ => false,
        }
    }

    /// 1 Hz timer tick update.
    pub fn on_tick(&mut self, wm: &mut WindowManager) -> bool {
        if self.status != GameStatus::Playing {
            return false;
        }

        let sec = timer::ticks() / timer::TICKS_PER_SECOND;
        if sec > self.last_tick_sec {
            self.last_tick_sec = sec;
            self.elapsed_seconds += 1;
            wm.set_label(
                self.win,
                MS_LABEL_TIME,
                format!("Time: {}s", self.elapsed_seconds),
            );
            return true;
        }
        false
    }
}
