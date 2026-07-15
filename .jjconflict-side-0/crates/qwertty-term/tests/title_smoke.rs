//! Windowed tab-title smoke: proves OSC 0/2 titles reach each tab's window
//! title (= its native tab-bar label) live and per-tab, that title changes
//! propagate, and that a cleared title falls back to the ghost emoji after
//! the 500ms grace period (upstream `SurfaceView_AppKit.swift:286-291`).
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term --test title_smoke -- --ignored --nocapture
//! ```
//!
//! Under the hood it launches the app binary with `QWERTTY_TERM_SMOKE_TITLE=1`;
//! the app feeds OSC 2 sequences straight into two tabs' engines and asserts
//! each tab's `NSWindow` title (exit 0 pass / 1 fail). See
//! `AppDelegate::run_title_smoke`.

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + shells"]
fn windowed_osc_tab_titles() {
    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_TITLE", "1")
        // Hermetic panes: a real login shell (or the user's prompt config)
        // emits its own OSC titles over the pty and races the smoke's
        // synthetic ones — spawn a quiet placeholder instead.
        .env("QWERTTY_TERM_COMMAND", "sleep 60")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    assert!(
        status.success(),
        "windowed tab-title smoke failed (exit {:?}): an OSC 0/2 title did not \
         reach the owning tab's window title, leaked across tabs, or the \
         ghost-emoji fallback misbehaved. See the FAIL line in the app's stderr.",
        status.code(),
    );
}
