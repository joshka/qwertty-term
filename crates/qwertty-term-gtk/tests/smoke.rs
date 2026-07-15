//! Headless GTK smoke test: drive the `adw::Application` for one frame under a
//! display server (Xvfb or headless Wayland) with Mesa software GL and assert
//! the GLArea realized a GL context and rendered the clear frame without a GL
//! error. Skips cleanly when no display is available so display-less CI lanes
//! (e.g. the offscreen GL-readback lane) don't see a false failure.
//!
//! The crate is Linux-only; this whole test file is gated accordingly.
#![cfg(target_os = "linux")]

#[test]
fn headless_smoke_renders_clear_frame() {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("no DISPLAY/WAYLAND_DISPLAY set; skipping GTK headless smoke test");
        return;
    }

    let outcome = qwertty_term_gtk::run_smoke();
    eprintln!("smoke outcome: {outcome}");

    assert!(
        outcome.realized,
        "GLArea did not realize a GL context (realize_error={:?})",
        outcome.realize_error
    );
    assert!(
        outcome.realize_error.is_none(),
        "GLArea realize reported a context error: {:?}",
        outcome.realize_error
    );
    assert!(outcome.rendered, "the render callback never fired");
    assert_eq!(
        outcome.gl_error, 0,
        "GL error after clear: 0x{:04x}",
        outcome.gl_error
    );
    assert!(
        outcome.is_ok(),
        "framebuffer did not hold the clear color: {:?}",
        outcome.center_pixel
    );
}
