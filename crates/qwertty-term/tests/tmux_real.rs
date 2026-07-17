//! Integration test against a **real `tmux -CC`** — the coverage gap that let
//! tmux regressions reach manual testing instead of the suite.
//!
//! `smoke_tmux` (see `tmux_smoke.rs`) drives a *synthetic* control-mode server:
//! it proves our parser/Viewer/reconciler agree with bytes we hand-wrote. It
//! cannot catch a disagreement with what tmux actually emits, nor a break in the
//! live pty path. Every tmux bug found by hand so far lived in exactly that gap:
//! the DCS passthrough terminating on real UTF-8 payload bytes, and pane
//! terminals not being resized to the layout tmux reports (garbled panes).
//!
//! This test spawns a real `tmux -CC` on a real pty via [`TabIo`] — the same
//! spawn path the app uses — feeds its output through a real [`Engine`], and
//! drives the resulting notifications through a real [`TmuxSession`], writing
//! the session's commands back to the control pty. That is the app's whole pump
//! loop minus AppKit.
//!
//! The invariant it asserts is the one that makes panes render correctly:
//! **every pane terminal is sized to the pane rect tmux reports in its layout**.
//! It checks that both for the initial window and *after a split*, which is
//! where the reflow bug lived (only newly-created panes were being sized).
//!
//! Skipped (not failed) when tmux isn't installed, so it stays CI-portable.

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use qwertty_term::engine::Engine;
use qwertty_term::termio::TabIo;
use qwertty_term::tmux_session::TmuxSession;
use qwertty_term_vt::terminal::Colors;

const COLS: u16 = 80;
const ROWS: u16 = 24;
/// Cell pixel size handed to the pty; only the cols/rows matter to tmux.
const CELL_W: u32 = 10;
const CELL_H: u32 = 20;

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run a `tmux` CLI command against our private socket.
fn tmux(sock: &str, args: &[&str]) -> bool {
    Command::new("tmux")
        .args(["-L", sock])
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Pump engine → session → control-pty until `done` holds, or time out.
/// This mirrors the app's per-tick drain (`take_tmux_notifications` → `ingest`
/// → write `commands` back to the same pty; control mode is in-band).
fn pump_until(
    engine: &Arc<Mutex<Engine>>,
    io: &TabIo,
    session: &mut TmuxSession,
    secs: u64,
    mut done: impl FnMut(&TmuxSession) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        let notes = {
            let mut e = engine.lock().expect("engine lock");
            e.take_tmux_notifications()
        };
        if !notes.is_empty() {
            let update = session.ingest(notes);
            for cmd in &update.commands {
                io.write(cmd);
            }
            assert!(
                !update.exit,
                "tmux control session exited unexpectedly — the control stream \
                 broke (e.g. the DCS was terminated mid-session)"
            );
        }
        if done(session) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

/// Every tracked pane's terminal must be sized to the pane rect tmux reports.
/// A mismatch is exactly what renders a pane garbled: its content wraps at the
/// terminal's width while the UI draws it at the layout's width.
fn assert_panes_match_layout(session: &TmuxSession, ctx: &str) {
    let viewer = session.viewer();
    let mut checked = 0;
    for w in viewer.windows() {
        for r in viewer.pane_rects(w.id) {
            let pane = viewer
                .pane(r.pane_id)
                .unwrap_or_else(|| panic!("{ctx}: pane %{} in layout but untracked", r.pane_id));
            let pages = &pane.terminal().screen().pages;
            assert_eq!(
                (pages.cols() as usize, pages.rows() as usize),
                (r.width, r.height),
                "{ctx}: pane %{} terminal is {}x{} but tmux's layout says {}x{}",
                r.pane_id,
                pages.cols(),
                pages.rows(),
                r.width,
                r.height,
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "{ctx}: no panes to check");
}

fn pane_count(session: &TmuxSession) -> usize {
    session.viewer().pane_count()
}

#[test]
fn real_tmux_panes_track_layout_across_split() {
    if !tmux_available() {
        eprintln!("skipping real-tmux test: tmux not installed");
        return;
    }
    let sock = format!("qwertty-it-{}", std::process::id());
    // Make sure we start clean and always tear the server down.
    tmux(&sock, &["kill-server"]);

    // Spawn a real `tmux -CC` through the app's own pty spawn path.
    // SAFETY: this test binary is single-threaded at this point (one #[test] in
    // this file), so no other thread can be reading the environment concurrently.
    unsafe {
        std::env::set_var(
            "QWERTTY_TERM_COMMAND",
            format!("tmux -L {sock} -CC new-session -x {COLS} -y {ROWS}"),
        );
    }

    let engine = Arc::new(Mutex::new(Engine::new(COLS as usize, ROWS as usize)));
    let io = TabIo::spawn(Arc::clone(&engine), COLS, ROWS, CELL_W, CELL_H, None)
        .expect("spawn tmux -CC on a pty");
    let mut session = TmuxSession::new(Colors::default());

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // 1. Come up: control mode must parse against *real* tmux bytes and the
        //    reconcile must produce a window with a pane that has content.
        let up = pump_until(&engine, &io, &mut session, 15, |s| {
            pane_count(s) >= 1
                && s.viewer()
                    .pane(0)
                    .map(|p| !p.terminal().plain_string().trim().is_empty())
                    .unwrap_or(false)
        });
        assert!(
            up,
            "real tmux -CC never reached a pane with content — the control-mode \
             path is broken against real tmux"
        );
        assert_panes_match_layout(&session, "initial window");

        // 2. Split: tmux re-lays-out, so the *existing* pane must be resized too,
        //    not just the new one. This is the reflow bug's exact shape.
        assert!(tmux(&sock, &["split-window", "-h"]), "split-window failed");
        let split = pump_until(&engine, &io, &mut session, 15, |s| pane_count(s) >= 2);
        assert!(split, "split never reconciled into a second pane");
        // Let the follow-up layout/capture traffic settle.
        pump_until(&engine, &io, &mut session, 2, |_| false);
        assert_panes_match_layout(&session, "after split-window -h");
    }));

    tmux(&sock, &["kill-server"]);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
