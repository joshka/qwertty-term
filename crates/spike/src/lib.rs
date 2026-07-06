//! Rust proof-of-concept for a focused slice of Ghostty's VT core.
//!
//! Start with [`Terminal`] for the emulator state machine and grid. The crate
//! root intentionally stays small and re-exports the public API from the
//! modules that own each concept.

mod cell;
mod color;
mod mode;
mod osc;
mod parser;
mod screen;
mod style;
mod terminal;

pub use cell::Cell;
pub use color::{AnsiColor, Color};
pub use mode::{CursorShape, MouseTracking};
pub use screen::{Cursor, ScreenKind};
pub use style::Style;
pub use terminal::Terminal;
