//! GUI toolkit — theme, widgets and the window manager (Phase 3).
//!
//! Layering, bottom-up:
//!
//! ```text
//! theme.rs    colors (one table, no hex literals elsewhere)
//! widgets.rs  Label/Button/Panel/ProgressBar/Slider — draw + pointer
//! wm.rs       Window, WindowManager, Rect — z-order, chrome, drag/resize,
//!             hit-testing, composition (desktop -> windows -> hint bar
//!             -> cursor)
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

pub mod theme;
pub mod widgets;
pub mod wm;

// The public surface main.rs builds with. The full flat contract (every
// widget type, Rect, Window, WidgetAction) is intentionally *not*
// re-exported yet — re-export only what's consumed, so dead-code warnings
// keep working as a "is this pullin' its weight?" test. Future phases
// (taskbar, dialogs, text fields) extend this list as they land.
pub use theme::Theme;
pub use widgets::{Widget, WidgetOutcome};
pub use wm::WindowManager;
