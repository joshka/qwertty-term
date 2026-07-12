//! Windowed bell smoke: proves a terminal BEL (`0x07`) fires the configured
//! `bell-features` — specifically the `title` indicator (a 🔔 prefix on the
//! tab/window title) — and that refocusing the window clears it. The audible
//! (`system`) and dock-attention (`attention`) features are fire-and-forget
//! AppKit side effects, so the smoke asserts the deterministic title path.
//!
//! Like the other windowed smokes this needs a real GUI (windowserver) session,
//! so it is `#[ignore]`d by default. Run it from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term --test bell_smoke -- --ignored --nocapture
//! ```
//!
//! Under the hood it launches the app binary with `QWERTTY_TERM_SMOKE_BELL=1`;
//! the app feeds a BEL into the focused pane's engine, ticks, and asserts the
//! tab bell indicator appears then clears on refocus (exit 0 pass / 1 fail).
//! See `AppDelegate::run_bell_smoke`.

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + shell"]
fn windowed_bell_title_indicator() {
    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_BELL", "1")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term binary");

    assert!(
        status.success(),
        "windowed bell smoke failed (exit {:?}): a BEL did not set the tab's bell \
         title indicator, or refocusing the window did not clear it. See the FAIL \
         line in the app's stderr.",
        status.code(),
    );
}
