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
pub mod import;
pub mod input;
pub mod keybind;
pub mod menu;
pub mod notify;
pub mod paste;
pub mod progress;
pub mod quickterm;
pub mod resize_overlay;
pub mod scroll;
pub mod search;
pub mod searchkeys;
pub mod selection;
pub mod session;
pub mod splitkeys;
pub mod splits;
pub mod tabkeys;
pub mod tabs;
pub mod theme;
// Headless tmux control-mode Viewer model (ADR 006 / ADR 004 slice 5a). Pure,
// AppKit-free — consumes the engine's tmux notification stream. Platform-agnostic.
pub mod tmux_viewer;

// tmux Viewer layout → SplitTree converter + window → tab reconciler (ADR 006
// slice 5b — pure logic). Maps the Viewer's window/pane model onto native
// tab/split intent (Option (a)); creates no native surfaces. Platform-agnostic.
pub mod tmux_reconcile;

// tmux control-mode session driver (ADR 006 slice 5c). Owns a Viewer + a
// Reconciler and turns a drained notification stream into outgoing control-pty
// command bytes + a native-surface ReconcilePlan. Headless and platform-agnostic
// (the app's per-surface control-mode lifecycle wraps it).
pub mod tmux_session;

// Headless tmux control-mode smoke (ADR 006 slice 5c, `--tmux-smoke`). Drives a
// synthetic `tmux -CC` byte stream through a real Engine + TmuxSession and
// asserts the native tab/split reconciliation + %output routing. No GPU/AppKit,
// so it runs anywhere (unlike the Metal `smoke`).
pub mod smoke_tmux;

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
