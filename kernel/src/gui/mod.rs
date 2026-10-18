//! GUI toolkit — theme, widgets and the window manager (Phase 3).
//!
//! Layering, bottom-up:
//!
//! ```text
//! theme.rs    colors (one table, no hex literals elsewhere)
//! widgets.rs  Label/Button/Panel/ProgressBar/Slider/TextInput — draw +
//!             pointer (+ text-key) handlers
//! wm.rs       Window, WindowManager, Rect — z-order, chrome, drag/resize,
//!             hit-testing, keyboard-focus routing, composition (desktop
//!             -> windows -> hint bar -> cursor)
//! ```
//!
//! Upper layers (main.rs) only ever say `gui::Theme`, `gui::WindowManager`
//! and `gui::Widget` — the flat re-exports below are that contract.
//!
//! ### Rendering model
//!
//! One frame = `wm.compose(&mut graphics, &theme)` followed by
//! `graphics.flush()` (Phase 2's double buffer, unchanged). The composer
//! rebuilds the whole backbuffer every frame — windows are cheap to draw
//! (a few hundred `fill_rect`s), the flush is one `memcpy`, and at 100 Hz
//! that's well within budget (the Phase 2 splash proved it). Partial
//! redraws are a Phase 4+ optimisation, not a correctness need.

pub mod icons;
pub mod theme;
pub mod widgets;
pub mod wm;

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

#[allow(unused_imports)]
pub use icons::Icon;
pub use theme::Theme;
pub use widgets::{Widget, WidgetOutcome};
pub use wm::{WidgetAction, WindowManager};

/// Notification severity kind for desktop toasts.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Info,
    Success,
    Warning,
    Error,
}

/// A desktop toast notification (Phase 18).
#[derive(Debug, Clone)]
pub struct ToastNotification {
    pub title: String,
    pub body: String,
    pub kind: NotificationKind,
    pub remaining_ticks: u32,
    pub initial_ticks: u32,
}

static TOAST_QUEUE: Mutex<Vec<ToastNotification>> = Mutex::new(Vec::new());

/// Public desktop notification API: fires a toast stacked at the top-right of the desktop.
pub fn notify(title: impl Into<String>, body: impl Into<String>, kind: NotificationKind) {
    let mut q = TOAST_QUEUE.lock();
    if q.len() >= 3 {
        q.remove(0);
    }
    q.push(ToastNotification {
        title: title.into(),
        body: body.into(),
        kind,
        remaining_ticks: 400, // 4.0 seconds at 100 Hz
        initial_ticks: 400,
    });
}

/// Ticks active toasts and returns the current list for composition.
pub fn tick_and_get_toasts() -> Vec<ToastNotification> {
    let mut q = TOAST_QUEUE.lock();
    for t in q.iter_mut() {
        if t.remaining_ticks > 0 {
            t.remaining_ticks -= 1;
        }
    }
    q.retain(|t| t.remaining_ticks > 0);
    q.clone()
}
