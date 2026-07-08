//! Windowed synthetic-input smoke: proves a real launched window is *typable*
//! end-to-end (frontmost/key → `keyDown:` → `NSTextInputClient`/encode → PTY →
//! engine → screen), guarding the "I can see tabs but can't type" regression.
//!
//! This must run in a real GUI (windowserver) session — it builds a live
//! `NSApplication`, a Metal renderer, and delivers synthetic `NSEvent`
//! keystrokes through the AppKit responder chain — so it is `#[ignore]`d by
//! default (headless CI has no windowserver). Run it explicitly from a logged-in
//! desktop session:
//!
//! ```sh
//! cargo test -p qwertty-term-app --test typing_smoke -- --ignored --nocapture
//! ```
//!
//! Under the hood it launches the app binary with
//! `QWERTTY_TERM_APP_SMOKE_TYPE="echo <marker>\n"`; the app types that through the
//! real keyDown path, waits for the shell round-trip, asserts the marker shows
//! up in the engine's screen text, and exits 0 (pass) / 1 (fail). See
//! `app::run` / `AppDelegate::run_type_smoke`.

#![cfg(target_os = "macos")]

use std::process::Command;

/// The compiled `qwertty-term-app` binary under test (Cargo sets `CARGO_BIN_EXE_*`).
const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term-app");

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer"]
fn windowed_typing_round_trip() {
    // A distinctive marker so the assertion can't be satisfied by shell noise.
    let marker = "zz-typing-smoke-marker";
    let script = format!("echo {marker}\\n");

    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_APP_SMOKE_TYPE", &script)
        // Assert on the *presented* IOSurface pixels, not just the engine's text
        // buffer: this catches the "window shows only the theme background, zero
        // glyphs" bug (presentation geometry / contentsScale / re-present),
        // which the engine-text-only check could not see.
        .env("QWERTTY_TERM_APP_ASSERT_PRESENT", "1")
        // Safety net: if the synthetic-input path somehow never fires, don't
        // hang the test suite — the app's own check timer exits well before
        // this, but a stuck run should still die.
        .env("QWERTTY_TERM_APP_SMOKE_MS", "10000")
        .status()
        .expect("failed to launch qwertty-term-app binary");

    assert!(
        status.success(),
        "windowed typing smoke failed (exit {:?}): the launched window did not \
         round-trip typed input through keyDown -> PTY -> engine AND present it. \
         Either the 'renders but can't type' regression (check app activation / \
         first-responder wiring) or the 'blank window' regression (check \
         IOSurfaceLayer contentsScale / the present path).",
        status.code(),
    );
}
