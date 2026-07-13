//! Windowed window-state smoke: proves the `window-width`/`window-height`
//! config keys size the *first* window's terminal grid to the requested cell
//! count (initial geometry override), end-to-end through
//! config → Controller → `setContentSize`.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term --test windowstate_smoke -- --ignored --nocapture
//! ```
//!
//! The harness writes a temp `config.toml` (pointed at by
//! `QWERTTY_TERM_CONFIG_DIR`) requesting a 100x30 cell window, launches the app
//! with `QWERTTY_TERM_SMOKE_WINDOWSTATE=1`, and asserts the live grid matches
//! (exit 0 pass / 1 fail). See `AppDelegate::run_windowstate_smoke`.

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + child"]
fn windowed_window_state_initial_geometry() {
    // A temp config dir requesting a specific initial window size in cells.
    let dir = std::env::temp_dir().join(format!(
        "qwertty-term-windowstate-smoke-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp config dir");
    std::fs::write(
        dir.join("config.toml"),
        "window-width = 100\nwindow-height = 30\n",
    )
    .expect("write temp config.toml");

    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_WINDOWSTATE", "1")
        .env("QWERTTY_TERM_CONFIG_DIR", &dir)
        // A long-lived child keeps the surface alive for the probe.
        .env("QWERTTY_TERM_COMMAND", "sleep 60")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        status.success(),
        "windowed window-state smoke failed (exit {:?}): the first window did not honor \
         the configured window-width/window-height. See the FAIL line in the app's stderr.",
        status.code(),
    );
}
