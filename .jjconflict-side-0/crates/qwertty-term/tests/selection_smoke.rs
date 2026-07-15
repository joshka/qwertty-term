//! Windowed selection-gestures smoke: proves the mouse selection gestures work
//! through the real window event path — double-click word (upstream boundary
//! set), triple-click line, single-click clear, cell drag with the 60%
//! threshold, shift-click extend, and drag-past-top-edge viewport autoscroll
//! that extends the selection into scrollback and stops on release.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term --test selection_smoke -- --ignored --nocapture
//! ```
//!
//! Under the hood it launches the app binary with
//! `QWERTTY_TERM_SMOKE_SELECTION=1`; the app feeds a deterministic screen,
//! synthesizes mouse NSEvents through `sendEvent` (real hit-testing → the pane
//! view's mouse methods → the gesture state machine), and asserts the engine's
//! selection text after each gesture (exit 0 pass / 1 fail). See
//! `AppDelegate::run_selection_smoke`.

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + shell"]
fn windowed_selection_gestures() {
    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_SELECTION", "1")
        // Safety net: if the smoke sequence somehow stalls, don't hang the
        // suite (the gesture gaps scale with the OS double-click interval).
        .env("QWERTTY_TERM_SMOKE_MS", "30000")
        .status()
        .expect("failed to launch qwertty-term binary");

    assert!(
        status.success(),
        "windowed selection-gestures smoke failed (exit {:?}): a mouse selection \
         gesture (double/triple click, drag, shift-extend, or edge autoscroll) \
         did not produce the expected selection. See the FAIL line in the app's \
         stderr.",
        status.code(),
    );
}
