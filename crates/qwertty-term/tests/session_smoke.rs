//! Windowed window-session smoke: proves the `window-save-state` content-restore
//! path. The app can capture a live tab's split tree + per-pane cwd into a
//! serializable `WindowSession`, round-trip it through JSON, and rebuild it into
//! a fresh tab — both a single pane and a full multi-pane tree (structure +
//! per-split ratios).
//!
//! This is the app-visible restore path — the serializable model is unit-tested
//! in `session.rs`, and this smoke drives the live capture/restore. Wiring the
//! JSON into macOS's `NSWindowRestoration` `NSCoder` (so a genuine quit+relaunch
//! replays it) is the remaining OS step, only exercisable by a real relaunch.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default:
//!
//! ```sh
//! cargo test -p qwertty-term --test session_smoke -- --ignored --nocapture
//! ```

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + child"]
fn windowed_window_session() {
    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_SESSION", "1")
        .env("QWERTTY_TERM_COMMAND", "sleep 60")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    assert!(
        status.success(),
        "windowed session smoke failed (exit {:?}): capture/round-trip/restore of the \
         window-session tree did not behave as expected. See the app's stderr.",
        status.code(),
    );
}
