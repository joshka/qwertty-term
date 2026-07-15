//! Unit tests for the tmux control-mode session driver. These drive a
//! [`TmuxSession`] with decoded [`Notification`]s (the same shape the engine's
//! DCS `1000p` seam surfaces) and assert the [`SessionUpdate`] it produces:
//! outgoing control-pty commands, the reconcile plan on a tree change, and pane
//! `%output` reaching the right pane `Terminal`.

use super::*;

use crate::tmux_reconcile::ReconcileOp;

// ---- notification builders (mirror tmux_viewer::tests) ---------------------

fn block_end(s: &str) -> Notification {
    Notification::BlockEnd(s.as_bytes().to_vec())
}

fn session_changed(id: usize) -> Notification {
    Notification::SessionChanged {
        id,
        name: b"session".to_vec(),
    }
}

fn output(pane_id: usize, data: &str) -> Notification {
    Notification::Output {
        pane_id,
        data: data.as_bytes().to_vec(),
    }
}

fn layout_change(window_id: usize, layout: &str) -> Notification {
    Notification::LayoutChange {
        window_id,
        layout: layout.as_bytes().to_vec(),
        visible_layout: layout.as_bytes().to_vec(),
        raw_flags: b"*".to_vec(),
    }
}

fn commands_blob(update: &SessionUpdate) -> Vec<u8> {
    update.commands.concat()
}

fn bytes_contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Drive a session from `Enter` to steady state with a single window whose
/// layout is `layout`, returning `(session, last_update)`. Mirrors
/// `tmux_viewer::tests::viewer_with_window`, but through the session reducer so
/// the outgoing commands + reconcile plan are exercised too.
fn session_with_window(layout: &str) -> (TmuxSession, SessionUpdate) {
    let mut s = TmuxSession::new(Colors::default());

    // Enter: nothing to do yet (Viewer created; StartupBlock).
    let u = s.ingest([Notification::Enter]);
    assert!(u.is_empty(), "enter alone produces nothing");

    // Startup block -> StartupSession.
    assert!(s.ingest([block_end("")]).is_empty());

    // Session id -> queues display-message (version) + list-windows.
    let u = s.ingest([session_changed(0)]);
    assert!(bytes_contains(&commands_blob(&u), b"display-message"));
    assert!(u.plan.is_none());

    // Version response -> list-windows command emitted.
    let u = s.ingest([block_end("3.5a")]);
    assert!(bytes_contains(&commands_blob(&u), b"list-windows"));

    // Window list -> a reconcile plan appears (tree changed) + first capture.
    let line = format!("$0 @0 83 44 {layout}");
    let u = s.ingest([block_end(&line)]);
    assert!(u.plan.is_some(), "window list yields a reconcile plan");
    (s, u)
}

// ---- tests -----------------------------------------------------------------

#[test]
fn immediate_exit_signals_teardown() {
    let mut s = TmuxSession::new(Colors::default());
    let u = s.ingest([Notification::Exit]);
    assert!(u.exit);
    assert!(u.plan.is_none());
    assert!(s.is_defunct());
    // A defunct session ignores further input.
    assert!(s.ingest([Notification::Exit]).is_empty());
}

#[test]
fn single_window_creates_one_tab() {
    // A single pane 0 window.
    let (s, u) = session_with_window("b7dd,83x44,0,0,0");
    let plan = u.plan.expect("plan");

    // Exactly one CreateTab (window 0), no removals.
    assert!(plan.ops.contains(&ReconcileOp::CreateTab { window_id: 0 }));
    assert!(
        !plan
            .ops
            .iter()
            .any(|o| matches!(o, ReconcileOp::RemoveTab { .. }))
    );

    // The window's split tree is a single mapped leaf for pane 0.
    let s0 = s.surface_of(0).expect("pane 0 has a surface");
    let tree = plan
        .ops
        .iter()
        .find_map(|o| match o {
            ReconcileOp::SetSplitTree { window_id: 0, tree } => Some(tree),
            _ => None,
        })
        .expect("a SetSplitTree for window 0");
    assert_eq!(tree.surfaces(), vec![s0]);
}

#[test]
fn split_window_builds_a_two_pane_tree() {
    // A vertical split: pane 0 over pane 1.
    let (s, u) = session_with_window("027b,83x44,0,0[83x20,0,0,0,83x23,0,21,1]");
    let plan = u.plan.expect("plan");

    let s0 = s.surface_of(0).unwrap();
    let s1 = s.surface_of(1).unwrap();
    assert_ne!(s0, s1);

    let tree = plan
        .ops
        .iter()
        .find_map(|o| match o {
            ReconcileOp::SetSplitTree { window_id: 0, tree } => Some(tree),
            _ => None,
        })
        .expect("a SetSplitTree for window 0");
    // Two leaves in layout order.
    assert_eq!(tree.surfaces(), vec![s0, s1]);
}

#[test]
fn output_reaches_the_right_pane_terminal_via_surface() {
    let (mut s, _u) = session_with_window("b7dd,83x44,0,0,0");

    // Feed the pane-0 capture queue to steady state (4 captures + list-panes).
    for _ in 0..5 {
        s.ingest([block_end("")]);
    }

    // Live %output routes into pane 0's terminal.
    let u = s.ingest([output(0, "hello-pane-0")]);
    assert!(u.is_empty(), "plain output produces no command/plan/exit");

    // Resolve pane 0's terminal through the native SurfaceId → pane map and
    // read its screen text back (the smoke's readback path).
    let s0 = s.surface_of(0).unwrap();
    let text = s
        .pane_terminal(s0)
        .expect("surface 0 resolves to a pane terminal")
        .plain_string();
    assert!(
        text.contains("hello-pane-0"),
        "pane 0 screen should show the routed output, got {text:?}"
    );

    // Output for an untracked pane is dropped silently.
    assert!(s.ingest([output(999, "ignored")]).is_empty());
}

#[test]
fn layout_change_replaces_the_tree_and_reuses_surviving_surface() {
    let (mut s, _u) = session_with_window("b7dd,83x44,0,0,0");
    for _ in 0..5 {
        s.ingest([block_end("")]);
    }
    let s0 = s.surface_of(0).unwrap();

    // A layout change splitting window 0 into panes 0 and 2.
    let u = s.ingest([layout_change(0, "e07b,83x44,0,0[83x22,0,0,0,83x21,0,23,2]")]);
    let plan = u.plan.expect("layout change yields a plan");

    // No tab create/remove — same window id; just a replaced tree.
    assert!(!plan.ops.iter().any(|o| matches!(
        o,
        ReconcileOp::CreateTab { .. } | ReconcileOp::RemoveTab { .. }
    )));
    // Pane 0 keeps its surface; pane 2 is new.
    assert_eq!(s.surface_of(0).unwrap(), s0, "surviving pane reused");
    let s2 = s.surface_of(2).unwrap();
    assert_ne!(s2, s0);

    let tree = plan
        .ops
        .iter()
        .find_map(|o| match o {
            ReconcileOp::SetSplitTree { window_id: 0, tree } => Some(tree),
            _ => None,
        })
        .unwrap();
    assert_eq!(tree.surfaces(), vec![s0, s2]);
}

#[test]
fn exit_after_a_session_signals_teardown_without_a_plan() {
    let (mut s, _u) = session_with_window("b7dd,83x44,0,0,0");
    let u = s.ingest([Notification::Exit]);
    assert!(u.exit);
    assert!(u.plan.is_none(), "exit tears down wholesale, no plan");
    assert!(s.is_defunct());
}

#[test]
fn commands_are_emitted_in_order_across_a_batch() {
    // Feeding the whole startup handshake as one batch still yields the version
    // query first, then (after its response) list-windows — order preserved.
    let mut s = TmuxSession::new(Colors::default());
    let u = s.ingest([
        Notification::Enter,
        block_end(""),      // StartupBlock -> StartupSession
        session_changed(0), // -> display-message queued + sent
        block_end("3.5a"),  // version response -> list-windows sent
    ]);
    assert_eq!(u.commands.len(), 2, "version query then list-windows");
    assert!(bytes_contains(&u.commands[0], b"display-message"));
    assert!(bytes_contains(&u.commands[1], b"list-windows"));
}

#[test]
fn send_keys_maps_surface_to_pane_and_emits_send_keys() {
    let (mut s, _u) = session_with_window("b7dd,83x44,0,0,0");
    // Drain the pane-0 capture queue (4 captures + list-panes) so the command
    // queue is idle and send-keys emits immediately.
    for _ in 0..5 {
        s.ingest([block_end("")]);
    }

    let s0 = s.surface_of(0).expect("pane 0 has a surface");
    let cmds = s.send_keys(s0, b"hi\r");
    let blob = cmds.concat();
    assert!(
        bytes_contains(&blob, b"send-keys -t %0 -H 68 69 d\n"),
        "unexpected send-keys bytes: {:?}",
        String::from_utf8_lossy(&blob)
    );

    // An unbound surface id resolves to no pane, so no command is produced.
    assert!(
        s.send_keys(crate::splits::SurfaceId(4242), b"x").is_empty(),
        "input for an unbound surface should yield no command"
    );
}

#[test]
fn split_pane_maps_surface_to_pane_and_emits_split_window() {
    // A 1x2 split so pane 1 has a real surface to target.
    let (mut s, _u) = session_with_window("027b,83x44,0,0[83x20,0,0,0,83x23,0,21,1]");
    // Drain the capture queue for both new panes so the queue is idle.
    for _ in 0..20 {
        if s.ingest([block_end("")]).commands.is_empty() {
            break;
        }
    }

    let s1 = s.surface_of(1).expect("pane 1 has a surface");
    // Horizontal, after: `-h`.
    let blob = s.split_pane(s1, true, false).concat();
    assert!(
        bytes_contains(&blob, b"split-window -t %1 -h\n"),
        "unexpected split bytes: {:?}",
        String::from_utf8_lossy(&blob)
    );

    // An unbound surface id resolves to no pane, so no command is produced.
    assert!(
        s.split_pane(crate::splits::SurfaceId(4242), true, false)
            .is_empty(),
        "split for an unbound surface should yield no command"
    );
}

#[test]
fn kill_pane_maps_surface_to_pane() {
    let (mut s, _u) = session_with_window("b7dd,83x44,0,0,0");
    for _ in 0..5 {
        s.ingest([block_end("")]);
    }
    let s0 = s.surface_of(0).expect("pane 0 has a surface");

    let blob = s.kill_pane(s0).concat();
    assert!(
        bytes_contains(&blob, b"kill-pane -t %0\n"),
        "unexpected kill bytes: {:?}",
        String::from_utf8_lossy(&blob)
    );
    assert!(
        s.kill_pane(crate::splits::SurfaceId(4242)).is_empty(),
        "kill for an unbound surface should yield no command"
    );
}

#[test]
fn select_pane_maps_surface_and_ingest_surfaces_tmux_focus() {
    // A 1x2 split so panes 0 and 1 both have surfaces.
    let (mut s, _u) = session_with_window("027b,83x44,0,0[83x20,0,0,0,83x23,0,21,1]");
    for _ in 0..20 {
        if s.ingest([block_end("")]).commands.is_empty() {
            break;
        }
    }
    let s0 = s.surface_of(0).expect("pane 0 surface");
    let s1 = s.surface_of(1).expect("pane 1 surface");

    // App→tmux: focusing pane 1 emits `select-pane -t %1`.
    let blob = s.select_pane(s1).concat();
    assert!(
        bytes_contains(&blob, b"select-pane -t %1\n"),
        "unexpected select bytes: {:?}",
        String::from_utf8_lossy(&blob)
    );
    // That set the active pane optimistically, so tmux's echo is a no-op (no
    // focus surfaced back to the app).
    let u = s.ingest([Notification::WindowPaneChanged {
        window_id: 0,
        pane_id: 1,
    }]);
    assert_eq!(u.focus, None, "app-initiated select must not bounce focus");

    // tmux→app: tmux moves the active pane to 0 on its own → focus surfaces s0.
    let u = s.ingest([Notification::WindowPaneChanged {
        window_id: 0,
        pane_id: 0,
    }]);
    assert_eq!(
        u.focus,
        Some(s0),
        "a tmux-initiated active-pane change surfaces the app focus target"
    );

    // Unbound surface → no select command.
    assert!(s.select_pane(crate::splits::SurfaceId(4242)).is_empty());
}

#[test]
fn new_window_is_bare_and_session_scoped() {
    // Fresh idle session so new-window emits immediately (no pane target).
    let (mut s, _u) = session_with_window("b7dd,83x44,0,0,0");
    for _ in 0..5 {
        s.ingest([block_end("")]);
    }
    let blob = s.new_window().concat();
    assert!(
        bytes_contains(&blob, b"new-window\n"),
        "unexpected new-window bytes: {:?}",
        String::from_utf8_lossy(&blob)
    );
}
