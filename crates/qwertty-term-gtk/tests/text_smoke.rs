//! Headless GTK **terminal-render** proof: drive the `adw::Application` for one
//! frame under a display server (Xvfb) with Mesa software GL, feed known text
//! into the vt terminal, present it into the `GtkGLArea` via the on-screen
//! present seam (`Engine::<OpenGL>::draw_and_present` → `OpenGL::present`), then
//! read the presented framebuffer back and assert **real glyph ink** reached it
//! — the analog of the software/opengl headless readback tests, but through the
//! real GTK GLArea present path.
//!
//! Skips cleanly when no display is available so display-less CI lanes don't see
//! a false failure. The crate is Linux-only; the whole file is gated.
#![cfg(target_os = "linux")]

#[test]
fn headless_smoke_renders_terminal_glyphs() {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("no DISPLAY/WAYLAND_DISPLAY set; skipping GTK text-render smoke test");
        return;
    }

    let outcome = qwertty_term_gtk::run_text_smoke();
    eprintln!("text-smoke outcome: {outcome}");

    assert!(
        outcome.realized,
        "GLArea did not realize a GL context (realize_error={:?})",
        outcome.realize_error
    );
    assert!(
        outcome.surface_init,
        "per-surface renderer core (engine+grid+terminal) failed to initialize",
    );
    assert!(outcome.rendered, "the terminal render/present never ran");
    assert!(
        outcome.render_error.is_none(),
        "terminal render/present errored: {:?}",
        outcome.render_error
    );
    assert_eq!(
        outcome.gl_error, 0,
        "GL error after present/readback: 0x{:04x}",
        outcome.gl_error
    );

    // The core claim: the presented framebuffer carries glyph ink — not a clear
    // color, not an empty frame.
    assert!(
        outcome.glyphs_rendered(),
        "no glyph ink in the presented framebuffer: {outcome}",
    );
    // A meaningful amount of ink (a whole line of text), guarding against a
    // stray-pixel false positive.
    assert!(
        outcome.bright_pixels > 100,
        "too little glyph ink (bright_pixels={}); expected a line of text: {outcome}",
        outcome.bright_pixels,
    );
    // Exactly one cell-row band carries the text and the opposite band is blank
    // — proof it's a real single-line terminal render (orientation-agnostic).
    assert!(
        outcome.one_band_is_text(),
        "text/blank bands don't separate cleanly (top={}, bottom={}): {outcome}",
        outcome.top_band_bright,
        outcome.bottom_band_bright,
    );
}
