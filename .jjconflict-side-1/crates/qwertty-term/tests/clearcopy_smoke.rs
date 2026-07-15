//! Windowed `selection-clear-on-copy` smoke: proves the config key clears the
//! selection after an *explicit* copy but not after copy-on-select.
//!
//! Launched with `copy-on-select = true` + `selection-clear-on-copy = true`. The
//! app drags a selection (copy-on-select copies it, selection stays visible),
//! then invokes an explicit Copy (`copy_to_clipboard`), which clears it —
//! matching upstream, where clear-on-copy excludes copy-on-select.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default:
//!
//! ```sh
//! cargo test -p qwertty-term --test clearcopy_smoke -- --ignored --nocapture
//! ```

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + child"]
fn windowed_selection_clear_on_copy() {
    let dir = std::env::temp_dir().join(format!(
        "qwertty-term-clearcopy-smoke-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp config dir");
    std::fs::write(
        dir.join("config.toml"),
        "copy-on-select = true\nselection-clear-on-copy = true\n",
    )
    .expect("write temp config.toml");

    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_CLEARCOPY", "1")
        .env("QWERTTY_TERM_CONFIG_DIR", &dir)
        .env("QWERTTY_TERM_COMMAND", "sleep 60")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        status.success(),
        "windowed clear-copy smoke failed (exit {:?}): selection-clear-on-copy did not \
         clear on explicit copy (or wrongly cleared on copy-on-select). See stderr.",
        status.code(),
    );
}
