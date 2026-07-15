//! Windowed command-finish smoke: proves `notify-on-command-finish` — OSC 133
//! `C`/`D` shell-integration marks are surfaced as command boundaries, the app
//! times the command from output-start to command-end, and delivers a
//! finish notification whose title reflects the exit status.
//!
//! Real macOS delivery via `UNUserNotificationCenter` needs a signed app bundle
//! (ADR 0003); this smoke asserts the full pipeline up to the delivery seam.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term --test notifycmd_smoke -- --ignored --nocapture
//! ```

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + child"]
fn windowed_notify_on_command_finish() {
    // A temp config enabling command-finish notifications unconditionally: mode
    // `always` (focus doesn't gate), the `notify` action (so it reaches the
    // observable delivery seam), and a 0s threshold (any duration notifies).
    let dir = std::env::temp_dir().join(format!(
        "qwertty-term-notifycmd-smoke-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp config dir");
    std::fs::write(
        dir.join("config.toml"),
        "notify-on-command-finish = \"always\"\n\
         notify-on-command-finish-action = \"notify\"\n\
         notify-on-command-finish-after = 0\n",
    )
    .expect("write temp config.toml");

    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_NOTIFYCMD", "1")
        .env("QWERTTY_TERM_CONFIG_DIR", &dir)
        // A long-lived, quiet child keeps the surface alive without emitting its
        // own escapes that could race the fed OSC 133 marks.
        .env("QWERTTY_TERM_COMMAND", "sleep 60")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        status.success(),
        "windowed notifycmd smoke failed (exit {:?}): OSC 133 command-finish did not \
         deliver the expected notification. See the FAIL line in the app's stderr.",
        status.code(),
    );
}
