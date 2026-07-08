//! Windowed splits smoke: proves terminal splits work end-to-end in a real
//! window — split right + down into 3 panes, each with its own live shell and
//! isolated input, directional focus navigation, divider-driven per-pane resize,
//! and close-collapse.
//!
//! Like `typing_smoke`, this must run in a real GUI (windowserver) session — it
//! builds a live `NSApplication` + Metal renderer and spawns three real shells —
//! so it is `#[ignore]`d by default (headless CI has no windowserver). Run it
//! explicitly from a logged-in desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term-app --test splits_smoke -- --ignored --nocapture
//! ```
//!
//! Under the hood it launches the app binary with `QWERTTY_TERM_APP_SMOKE_SPLITS=1`
//! (+ `QWERTTY_TERM_APP_ASSERT_PRESENT=1` so each pane's presented frame is checked
//! for real ink). The app runs the whole split/focus/resize/close sequence and
//! exits 0 (pass) / 1 (fail). See `app::run` / `AppDelegate::run_splits_smoke`.

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term-app` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term-app");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + 3 shells"]
fn windowed_splits_lifecycle() {
    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_APP_SMOKE_SPLITS", "1")
        // Also assert each pane presented real ink in its own rect (not just that
        // the engines have text) — the per-pane presentation-geometry check.
        .env("QWERTTY_TERM_APP_ASSERT_PRESENT", "1")
        // Safety net: if the smoke sequence somehow stalls, don't hang the suite.
        .env("QWERTTY_TERM_APP_SMOKE_MS", "20000")
        .status()
        .expect("failed to launch qwertty-term-app binary");

    assert!(
        status.success(),
        "windowed splits smoke failed (exit {:?}): the app did not build 3 isolated \
         panes, walk focus directionally, resize adjacent panes on a divider move, \
         and collapse on close. See the FAIL line in the app's stderr for the exact \
         assertion.",
        status.code(),
    );
}
