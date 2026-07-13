//! Windowed progress-bar smoke: proves OSC 9;4 (ConEmu) progress reports drive
//! the in-surface progress bar — parsed by the VT engine, drained by the app,
//! gated by `progress-style`, and tracked as a derived display state (fraction
//! + category) that clears on `remove`.
//!
//! The bar itself is a `CALayer` over the terminal content (upstream renders it
//! as a SwiftUI overlay, not a Metal draw); this smoke asserts the state
//! pipeline behind it via the app's `surface_progress` accessor.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term --test progress_smoke -- --ignored --nocapture
//! ```

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + child"]
fn windowed_osc9_4_progress_bar() {
    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_PROGRESS", "1")
        // A long-lived, quiet child keeps the surface alive without emitting its
        // own escapes that could race the fed OSC 9;4 reports.
        .env("QWERTTY_TERM_COMMAND", "sleep 60")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    assert!(
        status.success(),
        "windowed progress smoke failed (exit {:?}): an OSC 9;4 progress report did not \
         drive the expected progress-bar state. See the FAIL line in the app's stderr.",
        status.code(),
    );
}
