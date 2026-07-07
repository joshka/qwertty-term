//! Renderer core, ported from Ghostty (`~/local/ghostty/src/renderer/`).
//!
//! This is renderer chunk R0: the foundation every other renderer chunk
//! imports. It provides:
//!
//! - [`size`]: screen/cell/grid geometry, padding-balance math, and
//!   coordinate conversion between surface/terminal/grid spaces.
//! - [`cursor`]: cursor style resolution (focus/blink/preedit priority).
//! - [`row`]: row background-extension heuristics.
//! - [`options`] / [`backend`]: construction-option and GPU-backend stubs,
//!   filled in by later chunks (fonts, apprt, threading, GPU backends).
//! - [`snapshot`]: the [`snapshot::RenderSnapshot`] trait — the contract
//!   between `ghostty-vt` and any renderer backend — plus
//!   [`snapshot::FullSnapshot`], a full-copy implementation backed by
//!   `ghostty-vt`'s existing `Terminal::snapshot_window`.
//!
//! This crate depends on `ghostty-vt` (read-only use of its snapshot APIs)
//! and never the reverse.
//!
//! See `docs/analysis/renderer-r0.md` for the maintainer-grade survey of the
//! Zig source this ports (`src/renderer/{size,State,cursor,row,Options,
//! backend}.zig`, commit `2da015cd6`) and the design rationale for the
//! `RenderSnapshot` trait, including the precise list of what a future
//! dirty-row implementation will need from `ghostty-vt`.

pub mod backend;
pub mod cursor;
pub mod options;
pub mod row;
pub mod size;
pub mod snapshot;
