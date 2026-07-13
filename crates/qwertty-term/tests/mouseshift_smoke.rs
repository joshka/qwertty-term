//! Windowed `mouse-shift-capture` smoke: proves the config key (#30) gates
//! whether shift overrides mouse reporting for selection.
//!
//! Launched with a temp config setting `mouse-shift-capture = always`. The app
//! enables mouse reporting (`CSI ?1000h`) and shift-drags over a word: with
//! shift captured by the program, no selection is made. A control drag with
//! reporting off still selects, proving the selection machinery is live and it
//! was the config gating shift.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default:
//!
//! ```sh
//! cargo test -p qwertty-term --test mouseshift_smoke -- --ignored --nocapture
//! ```

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + child"]
fn windowed_mouse_shift_capture() {
    let dir = std::env::temp_dir().join(format!(
        "qwertty-term-mouseshift-smoke-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp config dir");
    std::fs::write(
        dir.join("config.toml"),
        "mouse-shift-capture = \"always\"\n",
    )
    .expect("write temp config.toml");

    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_MOUSESHIFT", "1")
        .env("QWERTTY_TERM_CONFIG_DIR", &dir)
        .env("QWERTTY_TERM_COMMAND", "sleep 60")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        status.success(),
        "windowed mouse-shift smoke failed (exit {:?}): mouse-shift-capture did not \
         gate the shift-over-reporting selection override. See the app's stderr.",
        status.code(),
    );
}
