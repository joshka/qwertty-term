//! Terminal emulation core, ported from Ghostty (`~/local/ghostty/src/terminal/`).
//!
//! This crate is the crown jewel of the rewrite: parser, stream handler, terminal state
//! machine, screen, and the page-based scrollback memory model. It stays dependency-light,
//! sync, and runtime-free so it can be embedded, fuzzed, and published independently.
//!
//! Port order and design constraints are defined in `docs/rewrite-prompt.md` (Phase 1).
//! The Zig source is the spec; every ported module ports its inline tests.

pub mod apc;
pub mod charsets;
pub mod color;
pub mod csi;
pub mod dcs;
pub mod formatter;
pub mod highlight;
pub mod kitty;
pub mod modes;
pub mod osc;
pub mod page;
pub mod pagelist;
pub mod parser;
pub mod point;
pub mod screen;
pub mod sgr;
pub mod snapshot;
pub mod stream;
pub mod tabstops;
pub mod terminal;
pub mod unicode;
pub mod utf8_decoder;

/// Crate version, exposed for the differential harness's report headers.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
