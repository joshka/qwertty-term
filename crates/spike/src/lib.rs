//! Frontend shell for the qwertty-term spike, now running on the `qwertty-term-vt`
//! engine.
//!
//! The terminal state machine, grid, and scrollback all live in the
//! `qwertty-term-vt` crate. This crate is just the two frontends (a crossterm
//! terminal-hosted mode and an egui native window) plus the thin [`Engine`]
//! adapter that bridges them to `qwertty-term-vt` (feed pty bytes, drain replies,
//! snapshot the grid for rendering).

mod engine;

pub use engine::{
    CellStyle, CellWidth, CursorStyle, Engine, MouseTracking, SnapshotCell, SnapshotColor,
    SnapshotCursor, SnapshotRow, SnapshotUnderline, SnapshotWindow,
};
pub use qwertty_term_vt::color::Rgb;
pub use qwertty_term_vt::snapshot::Snapshot;
pub use qwertty_term_vt::terminal::Colors;
