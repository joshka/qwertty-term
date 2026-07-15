//! Headless GTK **per-surface resize** proof: drive the `adw::Application` for
//! one frame under a display server (Xvfb) with Mesa software GL, build the
//! surface at the GLArea size, render, then `SurfaceState::resize` to a
//! different pixel size and render again — asserting the terminal **re-grids**
//! (a different `grid_size()`), the render target **re-sizes** with no GL error,
//! and (when a pty spawned) the `TIOCSWINSZ` reached the kernel pty.
//!
//! This is the resize analog of the text-render smoke, exercising the same
//! on-screen present seam at a second, smaller grid. Skips cleanly when no
//! display is available so display-less CI lanes don't see a false failure. The
//! crate is Linux-only; the whole file is gated.
#![cfg(target_os = "linux")]

#[test]
fn headless_smoke_resizes_surface() {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("no DISPLAY/WAYLAND_DISPLAY set; skipping GTK resize smoke test");
        return;
    }

    let outcome = qwertty_term_gtk::run_resize_smoke();
    eprintln!("resize-smoke outcome: {outcome}");

    assert!(
        outcome.realized,
        "GLArea did not realize a GL context (realize_error={:?})",
        outcome.realize_error
    );
    assert!(
        outcome.surface_init,
        "per-surface renderer core (engine+grid+terminal) failed to initialize",
    );
    assert!(
        outcome.rendered,
        "terminal render/present at the new size never ran or errored: {:?}",
        outcome.render_error
    );
    assert!(
        outcome.render_error.is_none(),
        "terminal render/present at the new size errored: {:?}",
        outcome.render_error
    );
    assert_eq!(
        outcome.gl_error, 0,
        "GL error after the post-resize present: 0x{:04x}",
        outcome.gl_error
    );

    // The core claim: the resize actually changed the grid (re-grid happened)
    // and a clean frame rendered at the new size.
    assert!(
        outcome.regridded(),
        "surface did not re-grid + render cleanly on resize: {outcome}",
    );
    // Halving an 800×600 default window must shrink the grid in both axes.
    assert!(
        outcome.resized_grid.0 < outcome.initial_grid.0
            && outcome.resized_grid.1 < outcome.initial_grid.1,
        "resized grid {:?} is not smaller than initial {:?}: {outcome}",
        outcome.resized_grid,
        outcome.initial_grid,
    );

    // If a pty spawned, the TIOCSWINSZ must have reached the kernel pty: its
    // winsize should match the resized grid. (No pty → the shell couldn't spawn
    // in this environment; the surface re-grid above is the load-bearing claim.)
    if let Some((cols, rows)) = outcome.pty_grid {
        assert_eq!(
            (cols as usize, rows as usize),
            outcome.resized_grid,
            "pty winsize {:?} does not match the resized grid {:?}: {outcome}",
            (cols, rows),
            outcome.resized_grid,
        );
    } else {
        eprintln!("resize-smoke: no pty spawned; skipped the TIOCSWINSZ propagation assertion");
    }
}
