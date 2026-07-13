//! Windowed selection-config smoke: proves the `selection-word-chars` and
//! `click-repeat-interval` config keys (#30) are wired into the live gesture
//! layer.
//!
//! Launched with a temp config setting `selection-word-chars = " -"` (so the
//! hyphen is a word boundary) and `click-repeat-interval = 1234`. The app then
//! double-clicks the middle of "beta-gamma" and asserts the selection is "beta"
//! — the inverse of the default-config selection smoke, where the hyphen is not
//! a boundary and the word is "beta-gamma" — and asserts the resolved click
//! interval matches the config.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default:
//!
//! ```sh
//! cargo test -p qwertty-term --test wordchars_smoke -- --ignored --nocapture
//! ```

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + child"]
fn windowed_selection_word_chars() {
    let dir = std::env::temp_dir().join(format!(
        "qwertty-term-wordchars-smoke-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp config dir");
    std::fs::write(
        dir.join("config.toml"),
        "selection-word-chars = \" -\"\nclick-repeat-interval = 1234\n",
    )
    .expect("write temp config.toml");

    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_WORDCHARS", "1")
        .env("QWERTTY_TERM_CONFIG_DIR", &dir)
        .env("QWERTTY_TERM_COMMAND", "sleep 60")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        status.success(),
        "windowed wordchars smoke failed (exit {:?}): selection-word-chars or \
         click-repeat-interval did not take effect. See the app's stderr.",
        status.code(),
    );
}
