//! Headless GTK **headerbar + primary menu + title binding** proof: drive the
//! real tabbed `adw::Application` under a display server (Xvfb) with Mesa
//! software GL and assert the app chrome is wired end-to-end —
//!
//! - an `adw::HeaderBar` is present in the presented window tree, carrying a
//!   primary `gtk::MenuButton` with a non-empty primary menu model;
//! - the primary menu's **New Tab** action (`win.new-tab`), driven directly,
//!   creates a second tab (`AdwTabView.n_pages()` goes 1 → 2);
//! - feeding `ESC]0;hello BEL` (OSC 0) into the active tab's terminal propagates
//!   the title to both the active `AdwTabPage` title and the window title.
//!
//! The window/menu are driven directly (via the `gio::Action` + the per-tab
//! `Shared`), not by injected GTK key events, so it stays deterministic. Skips
//! cleanly when no display is available. The crate is Linux-only; the whole file
//! is gated.
#![cfg(target_os = "linux")]

#[test]
fn headless_headerbar_menu_and_title_binding() {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("no DISPLAY/WAYLAND_DISPLAY set; skipping GTK headerbar test");
        return;
    }

    let outcome = qwertty_term_gtk::run_headerbar_smoke();
    eprintln!("headerbar outcome: {outcome}");

    assert!(
        outcome.realize_error.is_none(),
        "a tab GLArea realize reported a context error: {:?}",
        outcome.realize_error
    );

    // Chrome present.
    assert!(
        outcome.has_headerbar,
        "the tabbed window must present an adw::HeaderBar: {outcome}"
    );
    assert!(
        outcome.has_menu_button,
        "the HeaderBar must carry a primary gtk::MenuButton: {outcome}"
    );
    assert!(
        outcome.menu_item_count > 0,
        "the primary MenuButton must carry a non-empty menu model: {outcome}"
    );

    // New Tab menu action opens a second tab.
    assert_eq!(
        outcome.pages_after_new_tab_action, 2,
        "the New Tab menu action should open a second tab: {outcome}"
    );

    // OSC 0/2 title binding reaches the tab page and the window title.
    assert_eq!(
        outcome.tab_title.as_deref(),
        Some("hello"),
        "the OSC 0 title should bind to the active tab page title: {outcome}"
    );
    assert_eq!(
        outcome.window_title.as_deref(),
        Some("hello"),
        "the OSC 0 title should bind to the window title: {outcome}"
    );

    assert!(outcome.is_ok(), "headerbar smoke failed: {outcome}");
}
