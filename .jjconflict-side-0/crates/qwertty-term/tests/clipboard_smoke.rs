//! Windowed clipboard-hardening smoke: proves `clipboard-paste-protection`
//! gates an unsafe (multiline) paste behind confirmation — a declined paste
//! never reaches the pty, a confirmed one does — and that typing clears the
//! selection (`selection-clear-on-typing`).
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term --test clipboard_smoke -- --ignored --nocapture
//! ```
//!
//! Under the hood it launches the app binary with
//! `QWERTTY_TERM_SMOKE_CLIPBOARD=1` against a `cat` child (so pastes echo
//! deterministically and no shell enables bracketed paste), and asserts the
//! paste-protection + selection-clear behavior (exit 0 pass / 1 fail). See
//! `AppDelegate::run_clipboard_smoke`.

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + child"]
fn windowed_clipboard_hardening() {
    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_CLIPBOARD", "1")
        // A `cat` child echoes pasted stdin and doesn't enable bracketed paste,
        // making the paste-protection assertions deterministic.
        .env("QWERTTY_TERM_COMMAND", "cat")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    assert!(
        status.success(),
        "windowed clipboard smoke failed (exit {:?}): paste-protection did not gate an \
         unsafe paste, or typing did not clear the selection. See the FAIL line in the \
         app's stderr.",
        status.code(),
    );
}
