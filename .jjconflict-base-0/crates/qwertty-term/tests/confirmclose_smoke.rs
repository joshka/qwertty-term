//! Windowed confirm-close smoke: proves `confirm-close-surface` gates closing a
//! surface/tab/window on whether a process is running — decided by the OSC 133
//! shell-integration prompt state (the cursor being at a prompt vs mid-command
//! output), matching upstream. Absence of shell integration errs toward
//! confirming.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term --test confirmclose_smoke -- --ignored --nocapture
//! ```

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + child"]
fn windowed_confirm_close_surface() {
    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_CONFIRMCLOSE", "1")
        // A long-lived, quiet child keeps the surface alive without emitting its
        // own escapes that could race the fed OSC 133 marks. Uses the default
        // config (confirm-close-surface = true).
        .env("QWERTTY_TERM_COMMAND", "sleep 60")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    assert!(
        status.success(),
        "windowed confirm-close smoke failed (exit {:?}): confirm-close-surface did not \
         gate the close on the running-process (OSC 133) state. See the FAIL line in the \
         app's stderr.",
        status.code(),
    );
}
