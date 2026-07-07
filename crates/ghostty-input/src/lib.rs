//! Input encoding primitives, ported from Ghostty (`~/local/ghostty/src/input/`).
//!
//! This crate turns key/mouse/paste *events* into the exact PTY bytes Ghostty would send.
//! It is deliberately freestanding: it does not depend on `ghostty-vt`. Callers (e.g. the
//! `spike` window/engine) read whatever terminal-mode state they need (cursor key mode,
//! kitty keyboard flags, mouse tracking mode, bracketed paste, ...) and pass it in as plain
//! parameters via each module's `Options`/config struct.
//!
//! Port order and scope are defined in `docs/rewrite-prompt.md`. Every ported module ports
//! its inline Zig tests 1:1 (see each module's doc comment for the Zig-vs-Rust test count).
//!
//! ## Modules
//!
//! - [`mouse`] — mouse action/button/momentum model (port of `input/mouse.zig`).
//! - [`key`] — the `Key` enum, `KeyEvent`, `effectiveMods` (port of `input/key.zig`).
//! - [`key_mods`] — the `Mods` bitmask, remapping, macOS option-as-alt (port of
//!   `input/key_mods.zig`, `input/keyboard.zig`, `input/config.zig`).
//! - [`function_keys`] — the PC-style function key table (port of `input/function_keys.zig`).
//! - [`kitty_keymap`] — the kitty keyboard protocol functional-key table (port of
//!   `input/kitty.zig`).
//! - [`paste`] — bracketed-paste wrapping + control-char stripping (port of `input/paste.zig`).
//! - [`mouse_encode`] — X10/UTF8/SGR/urxvt/SGR-pixels mouse report encoding (port of
//!   `input/mouse_encode.zig`).
//! - [`key_encode`] — key event encoding. Currently implements the kitty-protocol `CSI…u`
//!   path in full; the legacy (non-kitty) encoder is a narrow placeholder seam, ported in a
//!   later chunk (see that module's doc comment).

pub mod function_keys;
pub mod key;
pub mod key_encode;
pub mod key_mods;
pub mod kitty_keymap;
pub mod mouse;
pub mod mouse_encode;
pub mod paste;

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
