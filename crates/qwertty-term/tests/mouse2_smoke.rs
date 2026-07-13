//! Windowed mouse slice-2 smoke: proves middle-click **primary-paste** (a
//! middle-click pastes the current selection into the pane) and
//! **focus-follows-mouse** (hovering a pane focuses it).
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term --test mouse2_smoke -- --ignored --nocapture
//! ```

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + child"]
fn windowed_mouse_slice2() {
    // Enable focus-follows-mouse via a temp config.
    let dir =
        std::env::temp_dir().join(format!("qwertty-term-mouse2-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp config dir");
    std::fs::write(dir.join("config.toml"), "focus-follows-mouse = true\n")
        .expect("write temp config.toml");

    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_MOUSE2", "1")
        .env("QWERTTY_TERM_CONFIG_DIR", &dir)
        // A `cat` child echoes the pasted selection deterministically.
        .env("QWERTTY_TERM_COMMAND", "cat")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        status.success(),
        "windowed mouse2 smoke failed (exit {:?}): middle-click primary-paste or \
         focus-follows-mouse did not behave. See the FAIL line in the app's stderr.",
        status.code(),
    );
}
