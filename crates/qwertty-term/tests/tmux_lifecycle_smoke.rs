//! GUI smoke for the tmux control-mode **tab lifecycle**: `tmux -CC`, then Cmd-T,
//! then close the new tab.
//!
//! This covers the gap between our two other tmux tests. `tmux_smoke` drives a
//! synthetic control-mode server, and `tmux_real` drives a real `tmux -CC` but
//! only through the Viewer/session — neither has native tabs, so neither can see
//! the AppKit tab layer where these bugs actually lived. This one runs the real
//! app and invokes the app's *own* entry points (`new_tab_in` for Cmd-T,
//! `close_tab_confirmed` for a tab close), so the command queue, focus sync and
//! window/tab-group behaviour are all exercised.
//!
//! It asserts what a user would actually see after closing the second tab:
//!
//! - exactly one tmux tab survives (closing one tab must not tear down the rest);
//! - the raw `tmux -CC` control tab is **not** on screen. AppKit surfaces a
//!   sibling tab when one closes, and the control window is in that tab group, so
//!   it can reappear behind our backs — leaving the user staring at the control
//!   surface (grid painting suppressed: stale text, no prompt) instead of their
//!   shell;
//! - the surviving pane still has content (the prompt is there).
//!
//! The app prints a `TMUXSTATE` dump at each step, so a failure shows the whole
//! observable state (per tab: tmux-managed, visible, and each pane's visible
//! text) rather than just a boolean.
//!
//! Skipped when tmux isn't installed. Runs in background mode (see
//! `background_mode` in `app.rs`) so it does not steal keyboard focus.

#![cfg(target_os = "macos")]

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_qwertty-term");

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run one lifecycle scenario end to end. `scenario` selects it
/// (`1` = Cmd-T then close the new tab, `closeall` = close every tmux tab).
///
/// `tmux -CC` is launched *from a shell that outlives it* (`; exec /bin/sh -i`),
/// matching how a user types it: when tmux exits the shell owns the pty again.
/// Running tmux as the pty command directly would leave nothing behind and hide
/// the whole post-exit behaviour this asserts.
fn run_scenario(scenario: &str) {
    if !tmux_available() {
        eprintln!("skipping tmux lifecycle smoke: tmux not installed");
        return;
    }
    let sock = std::env::temp_dir().join(format!(
        "qwertty-life-{}-{scenario}.sock",
        std::process::id()
    ));
    let kill = |s: &std::path::Path| {
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(s)
            .arg("kill-server")
            .output();
    };
    kill(&sock);

    let status = Command::new(BIN)
        .arg("--window")
        .env("QWERTTY_TERM_SMOKE_TMUXLIFE", scenario)
        .env(
            "QWERTTY_TERM_COMMAND",
            format!(
                "tmux -S {} -CC new-session; exec /bin/sh -i",
                sock.display()
            ),
        )
        // Safety net: the smoke exits itself; don't hang the suite if it stalls.
        .env("QWERTTY_TERM_SMOKE_MS", "30000")
        .status()
        .expect("failed to launch qwertty-term binary");

    kill(&sock);
    let _ = std::fs::remove_file(&sock);

    assert!(
        status.success(),
        "tmux lifecycle smoke `{scenario}` failed (exit {:?}). See the FAIL line \
         and the TMUXSTATE dumps in the app's stderr for the full per-tab state.",
        status.code(),
    );
}

#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + real tmux"]
fn windowed_tmux_tab_lifecycle() {
    run_scenario("1");
}

/// Closing every tmux tab must land the user somewhere usable: the control
/// surface restored, able to paint, and the shell that launched `tmux -CC`
/// prompting again. Regression test for "closing the tabs gets back to tmux -CC
/// with no control" — the control surface came back with grid painting still
/// suppressed and the shell's prompt swallowed.
#[test]
#[ignore = "needs a GUI (windowserver) session: builds a real NSApplication + Metal renderer + real tmux"]
fn windowed_tmux_close_all_tabs_restores_usable_shell() {
    run_scenario("closeall");
}
