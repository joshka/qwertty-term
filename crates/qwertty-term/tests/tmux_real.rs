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
//! It asserts the two invariants that make panes render correctly, which is
//! where both garbling bugs lived:
//!
//! 1. **tmux lays out at *our* control client's grid.** The session is created
//!    at a deliberately different size, so this proves we actively drive tmux
//!    (via `refresh-client -C`) rather than passively matching it.
//! 2. **Every pane terminal is sized to the pane rect tmux reports.** Checked
//!    for the initial window *and after a split*, which is where the reflow bug
//!    lived (only newly-created panes were being sized).
//!
//! It also fails on an unexpected control-session exit — the shape of the
//! DCS-break class of bug.
//!
//! Skipped (not failed) when tmux isn't installed, so it stays CI-portable, and
//! it cleans up its own socket so a run leaves nothing behind.

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use qwertty_term::engine::Engine;
use qwertty_term::termio::TabIo;
use qwertty_term::tmux_session::TmuxSession;
use qwertty_term_vt::terminal::Colors;

const COLS: u16 = 80;
const ROWS: u16 = 24;
/// The session is created at a size deliberately *different* from our control
/// client's, so the test proves we actively drive tmux to our grid rather than
/// passively lucking into a match. A control client owns the window size; if
/// tmux keeps the session's own size instead, every pane terminal is sized to a
/// grid the UI never draws at — which is exactly what renders panes garbled.
const SESSION_COLS: u16 = 140;
const SESSION_ROWS: u16 = 40;
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

/// Run a `tmux` CLI command against our private socket. We use `-S <path>`
/// rather than `-L <name>` so the socket lives at a path we chose and can
/// delete — `-L` leaves its socket file behind in tmux's shared tmp dir after
/// `kill-server`, which would litter one file per CI run.
fn tmux(sock: &std::path::Path, args: &[&str]) -> bool {
    Command::new("tmux")
        .arg("-S")
        .arg(sock)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Kill the server and remove its socket file, so a run leaves nothing behind.
fn cleanup(sock: &std::path::Path) {
    tmux(sock, &["kill-server"]);
    let _ = std::fs::remove_file(sock);
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

/// tmux must lay the window out at *our* control client's grid. A control client
/// drives the window size; if tmux keeps the session's own size, the pane
/// terminals are sized to a grid the UI doesn't draw at (garbled panes).
fn assert_layout_matches_client_grid(session: &TmuxSession, ctx: &str) {
    let viewer = session.viewer();
    assert!(!viewer.windows().is_empty(), "{ctx}: no windows");
    for w in viewer.windows() {
        assert_eq!(
            (w.width, w.height),
            (COLS as usize, ROWS as usize),
            "{ctx}: tmux laid window @{} out at {}x{}, but our control client is \
             {COLS}x{ROWS} — tmux is not tracking the client size",
            w.id,
            w.width,
            w.height,
        );
    }
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
    let sock = std::env::temp_dir().join(format!("qwertty-it-{}.sock", std::process::id()));
    // Make sure we start clean and always tear the server down.
    cleanup(&sock);

    // Spawn a real `tmux -CC` through the app's own pty spawn path.
    // SAFETY: this test binary is single-threaded at this point (one #[test] in
    // this file), so no other thread can be reading the environment concurrently.
    unsafe {
        std::env::set_var(
            "QWERTTY_TERM_COMMAND",
            format!(
                "tmux -S {} -CC new-session -x {SESSION_COLS} -y {SESSION_ROWS}",
                sock.display()
            ),
        );
    }

    let engine = Arc::new(Mutex::new(Engine::new(COLS as usize, ROWS as usize)));
    let io = TabIo::spawn(Arc::clone(&engine), COLS, ROWS, CELL_W, CELL_H, None)
        .expect("spawn tmux -CC on a pty");
    let mut session = TmuxSession::new(Colors::default());
    // Declare our grid, exactly as the app does on the control-mode Enter. tmux
    // must lay out at this size, not the session's own.
    for cmd in session.set_client_size(COLS as usize, ROWS as usize) {
        io.write(&cmd);
    }

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
        assert_layout_matches_client_grid(&session, "initial window");
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

    cleanup(&sock);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
