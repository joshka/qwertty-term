//! Windowed quick-terminal smoke: proves the dropdown toggles in/out, lands at
//! the configured screen edge, and hosts a live shell.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session
//! (it creates a borderless key window on a real `NSScreen`), so it is
//! `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term --test quickterm_smoke -- --ignored --nocapture
//! ```
//!
//! Under the hood it launches the app binary with
//! `QWERTTY_TERM_SMOKE_QUICKTERM=1`; the app toggles the quick terminal in
//! (asserting visibility + window geometry + a shell that echoes input), out,
//! and back in (exit 0 pass / 1 fail). See `AppDelegate::run_quickterm_smoke`.

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + borderless key window"]
fn windowed_quick_terminal_toggle() {
    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_QUICKTERM", "1")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    assert!(
        status.success(),
        "windowed quick-terminal smoke failed (exit {:?}): the dropdown did not \
         toggle in/out, landed at the wrong position, or its shell wasn't live. \
         See the FAIL line in the app's stderr.",
        status.code(),
    );
}
