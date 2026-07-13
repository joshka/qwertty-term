//! Windowed resize-overlay smoke: proves the `cols ⨯ rows` resize HUD
//! (`resize-overlay*`) shows the new grid size when the window is resized and
//! auto-clears after its configured duration.
//!
//! The overlay is an AppKit `NSTextField` over the terminal view (upstream draws
//! it as a SwiftUI overlay, never in the renderer); this smoke asserts the state
//! + text via the app's `tab_resize_overlay_text` accessor.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term --test resize_smoke -- --ignored --nocapture
//! ```

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + child"]
fn windowed_resize_overlay() {
    // A temp config forcing the overlay on every resize with a short lifetime so
    // the auto-clear assertion is quick.
    let dir =
        std::env::temp_dir().join(format!("qwertty-term-resize-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp config dir");
    std::fs::write(
        dir.join("config.toml"),
        "resize-overlay = \"always\"\nresize-overlay-duration = 400\n",
    )
    .expect("write temp config.toml");

    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_RESIZE", "1")
        .env("QWERTTY_TERM_CONFIG_DIR", &dir)
        .env("QWERTTY_TERM_COMMAND", "sleep 60")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        status.success(),
        "windowed resize smoke failed (exit {:?}): the resize overlay did not show the new \
         grid or did not auto-clear. See the FAIL line in the app's stderr.",
        status.code(),
    );
}
