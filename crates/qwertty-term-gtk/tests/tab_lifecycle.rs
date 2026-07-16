//! Headless GTK **tab-lifecycle** proof: drive the tabbed `adw::Application`
//! under a display server (Xvfb) with Mesa software GL and exercise the
//! multi-surface tab model end-to-end — open two tabs, let each realize +
//! render its own `SurfaceState` + `Pty`, switch the selected page, then close
//! one tab — asserting the `AdwTabView` page count, per-tab surface
//! independence, that switching works, and that closing drops the count from 2
//! to 1.
//!
//! The tab model is driven directly (via the `AdwTabView` / per-tab `Shared`),
//! not by injected GTK key events, so it stays deterministic. Skips cleanly when
//! no display is available so display-less CI lanes don't see a false failure.
//! The crate is Linux-only; the whole file is gated.
#![cfg(target_os = "linux")]

#[test]
fn headless_tab_lifecycle_create_switch_close() {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("no DISPLAY/WAYLAND_DISPLAY set; skipping GTK tab-lifecycle test");
        return;
    }

    let outcome = qwertty_term_gtk::run_tab_lifecycle_smoke();
    eprintln!("tab-lifecycle outcome: {outcome}");

    assert!(
        outcome.realize_error.is_none(),
        "a tab GLArea realize reported a context error: {:?}",
        outcome.realize_error
    );
    assert_eq!(
        outcome.pages_after_two_opened, 2,
        "the TabView should hold 2 pages after opening two tabs"
    );
    assert!(
        outcome.distinct_surfaces,
        "the two tabs must own independent per-surface state (distinct Shared)"
    );

    // Each tab built its own SurfaceState, spawned its own Pty, and rendered.
    assert!(
        outcome.tab1_surface_init && outcome.tab1_pty && outcome.tab1_rendered,
        "tab 1 did not stand up an independent rendered surface: {outcome}"
    );
    assert!(
        outcome.tab2_surface_init && outcome.tab2_pty && outcome.tab2_rendered,
        "tab 2 did not stand up an independent rendered surface: {outcome}"
    );

    assert!(
        outcome.switch_selected_ok,
        "switching the selected tab page did not take effect"
    );
    assert_eq!(
        outcome.pages_after_close, 1,
        "closing one tab should drop the TabView to 1 page"
    );

    assert!(outcome.is_ok(), "tab lifecycle failed: {outcome}");
}
