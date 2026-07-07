//! Frontend shell for the ghostty-rs spike, now running on the `ghostty-vt`
//! engine.
//!
//! The terminal state machine, grid, and scrollback all live in the
//! `ghostty-vt` crate. This crate is just the two frontends (a crossterm
//! terminal-hosted mode and an egui native window) plus the thin [`Engine`]
//! adapter that bridges them to `ghostty-vt` (feed pty bytes, drain replies,
//! snapshot the grid for rendering).

mod engine;

pub use engine::{
    CellStyle, CellWidth, CursorStyle, Engine, MouseTracking, SnapshotCell, SnapshotColor,
    SnapshotCursor, SnapshotRow, SnapshotUnderline, SnapshotWindow,
};
pub use ghostty_vt::color::Rgb;
pub use ghostty_vt::snapshot::Snapshot;
