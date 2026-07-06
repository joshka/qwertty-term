//! Terminal emulation core, ported from Ghostty (`~/local/ghostty/src/terminal/`).
//!
//! This crate is the crown jewel of the rewrite: parser, stream handler, terminal state
//! machine, screen, and the page-based scrollback memory model. It stays dependency-light,
//! sync, and runtime-free so it can be embedded, fuzzed, and published independently.
//!
//! Port order and design constraints are defined in `docs/rewrite-prompt.md` (Phase 1).
//! The Zig source is the spec; every ported module ports its inline tests.

pub mod page;
pub mod parser;
pub mod unicode;
pub mod utf8_decoder;

/// Crate version, exposed for the differential harness's report headers.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
