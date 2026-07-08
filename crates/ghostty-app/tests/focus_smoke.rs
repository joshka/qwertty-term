//! Windowed per-pane focus-reporting smoke: proves mode-1004 focus reporting is
//! routed per SURFACE, not per tab. Two panes run `cat -v` with mode 1004
//! enabled; switching pane focus must deliver `CSI I` (focus-in) to the
//! newly-focused pane's pty and `CSI O` (focus-out) to the previously-focused
//! pane's pty — each to its OWN pty.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p ghostty-app --test focus_smoke -- --ignored --nocapture
//! ```
//!
//! Under the hood it launches the app binary with `GHOSTTY_APP_SMOKE_FOCUS=1`.
//! The app splits into two panes, runs `cat -v` in each, enables mode 1004, then
//! focus-switches and asserts the focus-in/out bytes land at the right ptys
//! (exit 0 pass / 1 fail). See `app::run` / `AppDelegate::run_focus_smoke`.

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `ghostty-app` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_ghostty-app");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + 2 shells"]
fn windowed_per_pane_focus_reporting() {
    let status = Command::new(BIN)
        .arg("--window")
        .env("GHOSTTY_APP_SMOKE_FOCUS", "1")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("GHOSTTY_APP_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch ghostty-app binary");

    assert!(
        status.success(),
        "windowed focus-reporting smoke failed (exit {:?}): switching pane focus did \
         not deliver mode-1004 focus-in/out bytes to the correct per-surface ptys. \
         See the FAIL line in the app's stderr.",
        status.code(),
    );
}
