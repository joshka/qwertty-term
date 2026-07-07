//! Differential-testing harness for the Ghostty Rust rewrite.
//!
//! Feeds identical byte streams to (a) the Zig-built `libghostty-vt`
//! reference terminal and (b) the pure-Rust `ghostty-vt` port, then diffs
//! observable state (screen text, cursor). The [`Oracle`] trait is the
//! common interface both sides implement.
//!
//! The reference side requires the `reference` cargo feature and the
//! Zig-built static library; see the crate README.

mod oracle;
pub use oracle::{CursorPos, Oracle, ScreenDump, normalize_screen_text};

mod rust_engine;
pub use rust_engine::RustTerminal;

#[cfg(feature = "reference")]
pub mod ffi;

#[cfg(feature = "reference")]
mod reference;
#[cfg(feature = "reference")]
pub use reference::ReferenceTerminal;
