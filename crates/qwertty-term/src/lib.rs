//! `qwertty-term`: the native macOS AppKit host for the qwertty-term terminal
//! (renderer chunk R5).
//!
//! Binary crate with a thin library face so the platform-independent logic
//! (config, input translation, tab registry, menu action model, font-size
//! state, grid geometry, OSC 7 pwd inheritance) is unit-testable off the main
//! thread and the macOS shell (`app`, `view`, `clipboard`, `smoke`) layers on
//! top.
//!
//! Architecture and the AppKit object graph are documented in
//! `docs/analysis/renderer-r5.md`.

pub mod bell;
pub mod config;
pub mod context_menu;
pub mod engine;
pub mod font_size;
pub mod frame_dump;
pub mod geometry;
pub mod gesture;
pub mod input;
pub mod keybind;
pub mod menu;
pub mod paste;
pub mod quickterm;
pub mod scroll;
pub mod search;
pub mod searchkeys;
pub mod selection;
pub mod splitkeys;
pub mod splits;
pub mod tabkeys;
pub mod tabs;
pub mod theme;

// The real terminal IO stack binding (M2 chunk E). `qwertty-term-termio` is POSIX
// (rustix/libc fork+pty), so gate on unix; the app itself is macOS-only.
#[cfg(unix)]
pub mod termio;

// macOS-only: the font grid + AppKit shell + render presentation path.
#[cfg(target_os = "macos")]
pub mod app;
#[cfg(target_os = "macos")]
pub mod clipboard;
#[cfg(target_os = "macos")]
pub mod font;
#[cfg(target_os = "macos")]
pub mod search_overlay;
#[cfg(target_os = "macos")]
pub mod smoke;
#[cfg(target_os = "macos")]
pub mod splitview;
#[cfg(target_os = "macos")]
pub mod view;
