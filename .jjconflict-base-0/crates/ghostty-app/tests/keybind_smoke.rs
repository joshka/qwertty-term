//! Windowed keybind smoke: proves the minimal `text:` keybind subset works
//! end-to-end in a real window — a seeded `shift+enter=text:...` binding fires in
//! the real `keyDown:` path (before the encoder) and sends its literal bytes to
//! the focused pane's pty, while a plain enter still falls through to the encoder
//! (CR) and submits the line.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session —
//! it builds a live `NSApplication` + Metal renderer and spawns a real shell — so
//! it is `#[ignore]`d by default (headless CI has no windowserver). Run it from a
//! logged-in desktop session:
//!
//! ```sh
//! cargo test -p ghostty-app --test keybind_smoke -- --ignored --nocapture
//! ```
//!
//! Under the hood it launches the app binary with `GHOSTTY_APP_SMOKE_KEYBIND=1`,
//! which seeds `shift+enter=text:zzKBMARKERzz`, drives synthetic Shift+Return then
//! plain Return through the real window key path, and asserts the marker reached
//! the pty (exit 0 pass / 1 fail). See `app::run` / `AppDelegate::run_keybind_smoke`.
//! The maintainer's exact `\x1b\r` bytes are unit-tested in `keybind.rs`.

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `ghostty-app` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_ghostty-app");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + a shell"]
fn windowed_text_keybind_round_trip() {
    let status = Command::new(BIN)
        .arg("--window")
        .env("GHOSTTY_APP_SMOKE_KEYBIND", "1")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("GHOSTTY_APP_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch ghostty-app binary");

    assert!(
        status.success(),
        "windowed keybind smoke failed (exit {:?}): the seeded shift+enter `text:` \
         binding did not send its literal bytes to the focused pane's pty via the \
         real keyDown path. See the FAIL line in the app's stderr.",
        status.code(),
    );
}
