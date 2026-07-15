//! GTK4 + libadwaita host for the qwertty-term terminal — the **Linux windowed
//! app** (ADR 005 P4, slice 2).
//!
//! This crate is the Linux analog of the macOS AppKit host in `qwertty-term`.
//! It is strictly additive: it depends on the platform-free core crates and
//! never edits the macOS app or the renderer core.
//!
//! ## Scope of this scaffold (PR-B)
//!
//! An [`adw::Application`] opens an `adw::ApplicationWindow` whose child is a
//! [`gtk::GLArea`]. The GLArea realizes a GL 4.3 core context (loaded through
//! `epoxy` + `glow`), and its `render` callback clears the framebuffer to a
//! distinctive color. This proves the window/GL-hosting plumbing headless
//! (Xvfb + Mesa llvmpipe). The **actual terminal render** is deferred: it lands
//! once the generic on-screen present seam (`GpuBackend::present` +
//! `Engine::<OpenGL>::draw_and_present`, plan §2 / PR-A) is available. The
//! `render` callback in [`app`] marks exactly where that wiring slots in.
//!
//! The whole crate is `#[cfg(target_os = "linux")]`; on other targets it is an
//! empty shell so the workspace still builds on macOS with no GTK dependency.

#[cfg(target_os = "linux")]
mod app;

#[cfg(target_os = "linux")]
pub use app::{CLEAR_COLOR, SmokeOutcome, run, run_smoke};
