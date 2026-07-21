//! Runs the headless tmux control-mode smoke as a cargo test.
//!
//! The smoke itself (`smoke_tmux::run`) has existed for a while, but it was only
//! reachable through the `--tmux-smoke` CLI flag — nothing in CI or the test
//! suite ever invoked it, so it only ran when someone remembered to. That is a
//! large part of why tmux regressions surfaced in manual testing rather than in
//! the suite. Wiring it in here makes `cargo test --workspace` (and therefore
//! CI) exercise the control-mode parse → Viewer → reconcile path on every run.
//!
//! Note this smoke drives a *synthetic* tmux server (it feeds hand-built
//! control-mode bytes into a real `Engine` + `TmuxSession`). It proves the
//! parse/reconcile logic, not our behaviour against a real `tmux -CC` — that is
//! `tmux_real.rs`'s job.

#[test]
fn tmux_control_mode_smoke() {
    if let Err(e) = qwertty_term::smoke_tmux::run() {
        panic!("tmux control-mode smoke failed: {e}");
    }
}
