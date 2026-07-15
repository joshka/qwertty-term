//! Windowed mouse-behaviors smoke: proves the right-click context menu shows
//! the expected items for a pane (Paste / splits / Close, Copy only with a
//! selection) and that its Split Right / Close Pane items actually split and
//! collapse the tab.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term --test mouse_smoke -- --ignored --nocapture
//! ```
//!
//! Under the hood it launches the app binary with `QWERTTY_TERM_SMOKE_MOUSE=1`;
//! the app asserts the context-menu contents and invokes its actions (exit 0
//! pass / 1 fail). See `AppDelegate::run_mouse_smoke`.

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + shell"]
fn windowed_mouse_context_menu() {
    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_MOUSE", "1")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    assert!(
        status.success(),
        "windowed mouse smoke failed (exit {:?}): the right-click context menu items \
         were wrong, or Split Right / Close Pane did not split/collapse the tab. See \
         the FAIL line in the app's stderr.",
        status.code(),
    );
}
