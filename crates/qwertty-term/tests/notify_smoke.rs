//! Windowed desktop-notification smoke: proves OSC 9 (iTerm2) and OSC 777
//! (rxvt) desktop-notification escapes are parsed by the VT engine, drained by
//! the app, gated by `desktop-notifications`, rate-limited, and delivered to
//! the notification seam.
//!
//! Real macOS delivery via `UNUserNotificationCenter` needs a signed app bundle
//! (see `docs/adr/0003-desktop-notifications-bundle.md`); this smoke asserts the
//! full end-to-end plumbing up to the delivery seam (the app records the last
//! delivered `(title, body)` for observation).
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term --test notify_smoke -- --ignored --nocapture
//! ```

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + child"]
fn windowed_desktop_notifications() {
    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_NOTIFY", "1")
        // A long-lived, quiet child keeps the surface alive without emitting its
        // own escapes that could race the fed OSC 9/777 sequences.
        .env("QWERTTY_TERM_COMMAND", "sleep 60")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    assert!(
        status.success(),
        "windowed notify smoke failed (exit {:?}): an OSC 9/777 desktop notification did \
         not reach the delivery seam. See the FAIL line in the app's stderr.",
        status.code(),
    );
}
