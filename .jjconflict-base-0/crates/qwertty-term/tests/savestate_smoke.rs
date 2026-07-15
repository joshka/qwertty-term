//! Windowed window-save-state smoke: proves `window-save-state` gates macOS's
//! native window restoration — with `never`, windows are marked non-restorable
//! and the `NSQuitAlwaysKeepsWindows` user default is set false.
//!
//! This is slice 1 (the config-gating foundation). The actual tab/split/cwd
//! content restore rides on macOS `NSWindowRestoration` + `NSSecureCoding` and
//! is a separate follow-up.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default:
//!
//! ```sh
//! cargo test -p qwertty-term --test savestate_smoke -- --ignored --nocapture
//! ```

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + child"]
fn windowed_window_save_state() {
    let dir = std::env::temp_dir().join(format!(
        "qwertty-term-savestate-smoke-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp config dir");
    std::fs::write(dir.join("config.toml"), "window-save-state = \"never\"\n")
        .expect("write temp config.toml");

    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_SAVESTATE", "1")
        .env("QWERTTY_TERM_CONFIG_DIR", &dir)
        .env("QWERTTY_TERM_COMMAND", "sleep 60")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        status.success(),
        "windowed savestate smoke failed (exit {:?}): window-save-state=never did not mark \
         the window non-restorable or set NSQuitAlwaysKeepsWindows. See the app's stderr.",
        status.code(),
    );
}
