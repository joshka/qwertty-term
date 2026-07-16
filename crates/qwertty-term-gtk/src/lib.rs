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
//! `epoxy` + `glow`), and its `render` callback draws the **real terminal**:
//! it captures the live vt screen, rebuilds the frame through the font grid +
//! `Engine<OpenGL>`, and presents it into the GLArea framebuffer via the
//! on-screen present seam (`GpuBackend::present` +
//! `Engine::<OpenGL>::draw_and_present` — a `glBlitFramebuffer` of the engine
//! target onto the GLArea FBO). The per-surface core lives in [`surface`]; it is
//! fed by a pty running the user's shell (interactive [`run`]) or by known bytes
//! (the headless [`run_text_smoke`] proof). Keyboard input is the next chunk.
//!
//! Headless validation (Xvfb + Mesa llvmpipe): [`run_smoke`] proves the GL
//! plumbing (clear + center-pixel readback); [`run_text_smoke`] proves the
//! terminal render (feed text → present → readback asserts glyph ink).
//!
//! The whole crate is `#[cfg(target_os = "linux")]`; on other targets it is an
//! empty shell so the workspace still builds on macOS with no GTK dependency.

#[cfg(target_os = "linux")]
mod app;
#[cfg(target_os = "linux")]
mod input;
#[cfg(target_os = "linux")]
mod mouse;
#[cfg(target_os = "linux")]
mod surface;

#[cfg(target_os = "linux")]
pub use app::{
    CLEAR_COLOR, HeaderbarOutcome, ResizeSmokeOutcome, SmokeOutcome, TabLifecycleOutcome,
    TextSmokeOutcome, run, run_headerbar_smoke, run_resize_smoke, run_smoke,
    run_tab_lifecycle_smoke, run_text_smoke,
};
