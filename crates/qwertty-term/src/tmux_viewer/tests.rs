//! Unit tests for the headless tmux Viewer model. These mirror the inline
//! `test`s in upstream `viewer.zig` (immediate exit, session-changed reset,
//! initial capture flow, layout change, and the queue-gating of command
//! emission), plus qwertty-term-specific assertions for `%output` routing and
//! `list-panes` state parsing.

use super::*;

// ---- notification builders -------------------------------------------------

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

fn window_pane_changed(window_id: usize, pane_id: usize) -> Notification {
    Notification::WindowPaneChanged { window_id, pane_id }
}

// ---- action assertions -----------------------------------------------------

fn command_bytes(actions: &[Action]) -> Option<Vec<u8>> {
    actions.iter().find_map(|a| match a {
        Action::Command(b) => Some(b.clone()),
        _ => None,
    })
}

fn has_windows_changed(actions: &[Action]) -> bool {
    actions.iter().any(|a| matches!(a, Action::WindowsChanged))
}

fn has_exit(actions: &[Action]) -> bool {
    actions.iter().any(|a| matches!(a, Action::Exit))
}

fn has_command(actions: &[Action]) -> bool {
    actions.iter().any(|a| matches!(a, Action::Command(_)))
}

fn bytes_contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Drive a Viewer from `Enter` to steady state with a single window whose
/// layout is `layout` (with checksum), returning the Viewer. The window listing
/// response uses session `$0`, window `@0`, size `83x44`.
fn viewer_with_window(layout: &str) -> Viewer {
    let mut v = Viewer::new(Colors::default());
    // Startup block.
    let a = v.next(block_end(""));
    assert!(a.is_empty());
    // Session id -> queues tmux_version (display-message) + list-windows.
    let a = v.next(session_changed(0));
    assert!(bytes_contains(
        &command_bytes(&a).unwrap(),
        b"display-message"
    ));
    // Version response -> list-windows sent.
    let a = v.next(block_end("3.5a"));
    assert!(bytes_contains(&command_bytes(&a).unwrap(), b"list-windows"));
    // Window list -> parse, WindowsChanged + first capture-pane.
    let line = format!("$0 @0 83 44 {layout}");
    let a = v.next(block_end(&line));
    assert!(has_windows_changed(&a));
    assert!(has_command(&a));
    v
}

// ---- tests -----------------------------------------------------------------

#[test]
fn immediate_exit() {
    let mut v = Viewer::new(Colors::default());
    let a = v.next(Notification::Exit);
    assert!(has_exit(&a));
    assert!(v.is_defunct());
    // A defunct viewer ignores all further input.
    let a = v.next(Notification::Exit);
    assert!(a.is_empty());
}

#[test]
fn startup_advances_through_states() {
    let mut v = Viewer::new(Colors::default());
    // StartupBlock: only a block advances us; other notifications are ignored.
    assert!(v.next(Notification::SessionsChanged).is_empty());
    assert!(v.next(block_end("")).is_empty());
    // StartupSession: session-changed records the id and queues startup commands.
    let a = v.next(session_changed(42));
    assert_eq!(v.session_id(), 42);
    assert!(bytes_contains(
        &command_bytes(&a).unwrap(),
        b"display-message"
    ));
}

#[test]
fn initial_flow_builds_tree_and_routes_output() {
    // Vertical split of two panes: pane 0 (83x20) over pane 1 (83x23).
    let layout = "027b,83x44,0,0[83x20,0,0,0,83x23,0,21,1]";
    let mut v = viewer_with_window(layout);

    assert_eq!(v.windows().len(), 1);
    assert_eq!(v.windows()[0].id, 0);
    assert_eq!(v.pane_count(), 2);
    assert!(v.pane(0).is_some());
    assert!(v.pane(1).is_some());
    assert_eq!(v.tmux_version(), b"3.5a");

    // Pane geometry is flattened from the layout tree.
    let rects = v.pane_rects(0);
    assert_eq!(rects.len(), 2);
    assert_eq!(
        rects[0],
        PaneRect {
            pane_id: 0,
            x: 0,
            y: 0,
            width: 83,
            height: 20
        }
    );
    assert_eq!(
        rects[1],
        PaneRect {
            pane_id: 1,
            x: 0,
            y: 21,
            width: 83,
            height: 23
        }
    );

    // Drain the capture-pane queue. Order: pane 0 {H,V primary; H,V alt}, then
    // pane 1, then a single list-panes (pane_state). The visible-primary
    // captures are applied to the pane's terminal.
    // H0(primary): front was emitted by viewer_with_window; feed its response.
    let a = v.next(block_end("")); // H0 primary response -> V0 primary command
    assert!(bytes_contains(&command_bytes(&a).unwrap(), b"-t %0"));
    assert!(!bytes_contains(&command_bytes(&a).unwrap(), b"-a "));

    let a = v.next(block_end("visible-zero")); // V0 primary -> H0 alt command
    assert!(bytes_contains(&command_bytes(&a).unwrap(), b"-t %0"));
    assert!(bytes_contains(&command_bytes(&a).unwrap(), b"-a "));
    assert!(
        v.pane(0)
            .unwrap()
            .terminal()
            .plain_string()
            .contains("visible-zero")
    );

    let a = v.next(block_end("")); // H0 alt -> V0 alt
    assert!(bytes_contains(&command_bytes(&a).unwrap(), b"-a "));
    let a = v.next(block_end("")); // V0 alt -> H1 primary
    assert!(bytes_contains(&command_bytes(&a).unwrap(), b"-t %1"));
    let a = v.next(block_end("")); // H1 primary -> V1 primary
    assert!(bytes_contains(&command_bytes(&a).unwrap(), b"-t %1"));
    v.next(block_end("visible-one")); // V1 primary -> H1 alt
    assert!(
        v.pane(1)
            .unwrap()
            .terminal()
            .plain_string()
            .contains("visible-one")
    );
    let _ = v.next(block_end("")); // H1 alt -> V1 alt
    let a = v.next(block_end("")); // V1 alt -> list-panes command
    assert!(bytes_contains(&command_bytes(&a).unwrap(), b"list-panes"));

    // Respond to list-panes for pane 0; state is parsed and stored.
    let panes_line = "%0;5;7;1;block;;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;";
    let a = v.next(block_end(panes_line));
    // Queue is now empty: no further command.
    assert!(!has_command(&a));
    assert_eq!(
        v.pane(0).unwrap().state(),
        Some(&PaneState {
            cursor_x: 5,
            cursor_y: 7,
            cursor_visible: true,
            cursor_shape: "block".to_string(),
            alternate_on: false,
        })
    );

    // Live %output is routed to the right pane's terminal.
    let a = v.next(output(0, " live"));
    assert!(a.is_empty());
    assert!(
        v.pane(0)
            .unwrap()
            .terminal()
            .plain_string()
            .contains("live")
    );

    // %output for an untracked pane is dropped.
    let a = v.next(output(999, "ignored"));
    assert!(a.is_empty());

    // Exit tears down.
    let a = v.next(Notification::Exit);
    assert!(has_exit(&a));
    assert!(v.is_defunct());
}

#[test]
fn session_changed_resets_state() {
    let layout = "027b,83x44,0,0[83x20,0,0,0,83x23,0,21,1]";
    let mut v = viewer_with_window(layout);
    assert_eq!(v.pane_count(), 2);
    assert_eq!(v.tmux_version(), b"3.5a");

    // A new session resets everything but preserves the version, emits an
    // empty-windows signal, and restarts list-windows.
    let a = v.next(session_changed(2));
    assert!(has_windows_changed(&a));
    assert!(bytes_contains(&command_bytes(&a).unwrap(), b"list-windows"));
    assert_eq!(v.session_id(), 2);
    assert_eq!(v.windows().len(), 0);
    assert_eq!(v.pane_count(), 0);
    assert_eq!(v.tmux_version(), b"3.5a");

    // The new session's window list rebuilds the tree.
    let a = v.next(block_end(
        "$2 @1 83 44 027b,83x44,0,0[83x20,0,0,0,83x23,0,21,1]",
    ));
    assert!(has_windows_changed(&a));
    assert_eq!(v.windows().len(), 1);
    assert_eq!(v.windows()[0].id, 1);
    assert_eq!(v.pane_count(), 2);
}

#[test]
fn window_add_refreshes_window_list() {
    let layout = "b7dd,83x44,0,0,0"; // single pane 0
    let mut v = viewer_with_window(layout);
    // Drain the pane-0 capture + state queue (4 captures + 1 list-panes).
    for _ in 0..5 {
        v.next(block_end(""));
    }
    // A window-add queues (and, since the queue is now empty, immediately
    // sends) a fresh list-windows.
    let a = v.next(Notification::WindowAdd { id: 1 });
    assert!(bytes_contains(&command_bytes(&a).unwrap(), b"list-windows"));
}

#[test]
fn layout_change_adds_and_prunes_panes() {
    let layout = "b7dd,83x44,0,0,0"; // single pane 0
    let mut v = viewer_with_window(layout);
    assert_eq!(v.pane_count(), 1);
    assert!(v.pane(0).is_some());

    // Drain the single-pane capture + state queue so the queue is empty.
    for _ in 0..5 {
        v.next(block_end(""));
    }

    // A layout change that splits window 0 into panes 0 and 2.
    let a = v.next(layout_change(0, "e07b,83x44,0,0[83x22,0,0,0,83x21,0,23,2]"));
    assert!(has_windows_changed(&a));
    // Queue was empty, so the first capture command for the new pane is emitted.
    assert!(bytes_contains(&command_bytes(&a).unwrap(), b"-t %2"));
    assert_eq!(v.pane_count(), 2);
    assert!(v.pane(0).is_some()); // reused
    assert!(v.pane(2).is_some()); // new
    assert!(v.pane(1).is_none());
}

#[test]
fn layout_change_does_not_emit_command_when_queue_busy() {
    let layout = "b7dd,83x44,0,0,0"; // single pane 0
    let mut v = viewer_with_window(layout);
    // Do NOT drain the capture queue: a command is still in flight.

    let a = v.next(layout_change(0, "e07b,83x44,0,0[83x22,0,0,0,83x21,0,23,2]"));
    assert!(has_windows_changed(&a));
    // A command is in flight, so no new command is emitted.
    assert!(!has_command(&a));
    assert_eq!(v.pane_count(), 2);
}

#[test]
fn layout_change_for_unknown_window_is_ignored() {
    let layout = "b7dd,83x44,0,0,0";
    let mut v = viewer_with_window(layout);
    for _ in 0..5 {
        v.next(block_end(""));
    }
    let before = v.pane_count();
    let a = v.next(layout_change(
        999,
        "e07b,83x44,0,0[83x22,0,0,0,83x21,0,23,2]",
    ));
    assert!(a.is_empty());
    assert_eq!(v.pane_count(), before);
    assert!(!v.is_defunct());
}

#[test]
fn exit_during_steady_state_becomes_defunct() {
    let layout = "b7dd,83x44,0,0,0";
    let mut v = viewer_with_window(layout);
    let a = v.next(Notification::Exit);
    assert!(has_exit(&a));
    assert!(v.is_defunct());
    // Ignored thereafter.
    assert!(v.next(output(0, "x")).is_empty());
}

#[test]
fn ignored_steady_state_notifications_are_noops() {
    let layout = "b7dd,83x44,0,0,0";
    let mut v = viewer_with_window(layout);
    for _ in 0..5 {
        v.next(block_end(""));
    }
    // These are explicitly ignored in steady state (no actions, no state change).
    assert!(v.next(Notification::SessionsChanged).is_empty());
    assert!(
        v.next(Notification::WindowRenamed {
            id: 0,
            name: b"x".to_vec()
        })
        .is_empty()
    );
    assert!(
        v.next(Notification::WindowPaneChanged {
            window_id: 0,
            pane_id: 0
        })
        .is_empty()
    );
    assert!(
        v.next(Notification::ClientDetached {
            client: b"/dev/tty".to_vec()
        })
        .is_empty()
    );
    assert_eq!(v.pane_count(), 1);
}

#[test]
fn layout_change_resizes_existing_pane_terminal() {
    // A split / close / window resize re-lays-out; an EXISTING pane's terminal
    // must follow its new layout size, not stay frozen at its create-time size
    // (the "contents don't resize in tmux mode" bug).
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    drain_to_idle(&mut v);
    let t = v.pane(0).unwrap().terminal();
    assert_eq!((t.screen().pages.cols(), t.screen().pages.rows()), (83, 44));

    // tmux re-lays-out the same pane at a smaller size (e.g. after a split).
    v.next(layout_change(0, &checksummed("40x30,0,0,0")));
    let t = v.pane(0).unwrap().terminal();
    assert_eq!(
        (t.screen().pages.cols(), t.screen().pages.rows()),
        (40, 30),
        "existing pane terminal must resize to the new layout size"
    );
}

// ---- send-keys (ADR 006 slice 5d input) ------------------------------------

/// A full checksummed layout string for `body` (mirrors the smoke helper).
fn checksummed(body: &str) -> String {
    let c = qwertty_term_vt::tmux::Checksum::calculate(body.as_bytes()).as_string();
    format!("{},{}", String::from_utf8_lossy(&c), body)
}

/// Feed empty block responses until the capture-pane/list-panes queue drains to
/// an idle steady state (no further command emitted).
fn drain_to_idle(v: &mut Viewer) {
    for _ in 0..100 {
        if !has_command(&v.next(block_end(""))) {
            return;
        }
    }
    panic!("command queue did not drain to idle");
}

#[test]
fn send_keys_emits_hex_send_keys_for_a_known_pane_when_idle() {
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    drain_to_idle(&mut v);
    // Idle queue: the send-keys command is emitted immediately. "hi\r" -> the
    // codepoints h(68) i(69) CR(d), each a hex arg to `send-keys -H`.
    let a = v.send_keys(0, b"hi\r");
    let cmd = command_bytes(&a).expect("send-keys command");
    assert!(
        bytes_contains(&cmd, b"send-keys -t %0 -H 68 69 d\n"),
        "unexpected send-keys bytes: {:?}",
        String::from_utf8_lossy(&cmd)
    );
}

#[test]
fn send_keys_defers_while_a_command_is_in_flight() {
    // A capture command is in flight straight after startup (queue non-empty).
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    // The send-keys is queued behind it, so nothing is emitted right now.
    assert!(!has_command(&v.send_keys(0, b"x")));
    // Draining the capture queue eventually emits the deferred send-keys, and
    // its %begin/%end reply correlates (queue advances, no panic).
    let mut saw = false;
    for _ in 0..100 {
        let a = v.next(block_end(""));
        match command_bytes(&a) {
            Some(cmd) if bytes_contains(&cmd, b"send-keys -t %0 -H 78\n") => {
                saw = true;
                break;
            }
            Some(_) => continue,
            None => break,
        }
    }
    assert!(saw, "deferred send-keys was never emitted");
}

#[test]
fn send_keys_ignores_unknown_pane_pre_steady_and_empty_input() {
    // Not yet in steady state: no command.
    let mut fresh = Viewer::new(Colors::default());
    assert!(fresh.send_keys(0, b"x").is_empty());
    // Steady state, but an unknown pane id or empty input yields no command.
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    drain_to_idle(&mut v);
    assert!(v.send_keys(999, b"x").is_empty());
    assert!(v.send_keys(0, b"").is_empty());
}

// ---- structural writes: split-window / new-window / kill-pane (slice 5e) ---

#[test]
fn split_window_emits_orientation_and_before_flags() {
    // Horizontal (left/right, `-h`), new pane after (no `-b`).
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    drain_to_idle(&mut v);
    let cmd = command_bytes(&v.split_window(0, true, false)).expect("split command");
    assert!(
        bytes_contains(&cmd, b"split-window -t %0 -h\n"),
        "unexpected split bytes: {:?}",
        String::from_utf8_lossy(&cmd)
    );

    // Vertical (top/bottom, `-v`), new pane before (`-b`).
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    drain_to_idle(&mut v);
    let cmd = command_bytes(&v.split_window(0, false, true)).expect("split command");
    assert!(
        bytes_contains(&cmd, b"split-window -t %0 -v -b\n"),
        "unexpected split bytes: {:?}",
        String::from_utf8_lossy(&cmd)
    );
}

#[test]
fn kill_pane_emits_targeted_kill() {
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    drain_to_idle(&mut v);
    let cmd = command_bytes(&v.kill_pane(0)).expect("kill command");
    assert!(
        bytes_contains(&cmd, b"kill-pane -t %0\n"),
        "unexpected kill bytes: {:?}",
        String::from_utf8_lossy(&cmd)
    );
}

#[test]
fn kill_window_emits_targeted_kill_and_queues_a_list_windows_refresh() {
    // A tab close on a tmux-managed tab (gap 1) → `kill-window -t @<w>`.
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    drain_to_idle(&mut v);
    let cmd = command_bytes(&v.kill_window(0)).expect("kill-window command");
    assert!(
        bytes_contains(&cmd, b"kill-window -t @0\n"),
        "unexpected kill-window bytes: {:?}",
        String::from_utf8_lossy(&cmd)
    );
    // tmux signals the window's removal with an undecoded
    // `%window-close`/`%unlinked-window-close`, so the write's %begin/%end reply
    // must queue a `list-windows` refresh to reconcile the tab away (I3).
    let refresh = command_bytes(&v.next(block_end(""))).expect("follow-up refresh");
    assert!(
        bytes_contains(&refresh, b"list-windows"),
        "kill-window must queue a list-windows refresh, got: {:?}",
        String::from_utf8_lossy(&refresh)
    );
}

#[test]
fn kill_pane_queues_a_list_windows_refresh_for_the_last_pane_collapse() {
    // kill-pane of a window's *last* pane closes the window (undecoded
    // `%window-close`), so kill-pane also queues a list-windows refresh so the
    // collapse-to-tab-close reconciles (gap 1 "collapse-to-tab-close").
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    drain_to_idle(&mut v);
    command_bytes(&v.kill_pane(0)).expect("kill-pane command");
    let refresh = command_bytes(&v.next(block_end(""))).expect("follow-up refresh");
    assert!(
        bytes_contains(&refresh, b"list-windows"),
        "kill-pane must queue a list-windows refresh, got: {:?}",
        String::from_utf8_lossy(&refresh)
    );
}

#[test]
fn kill_window_guards_pre_steady_and_unknown_window() {
    // Not yet in steady state: no command.
    let mut fresh = Viewer::new(Colors::default());
    assert!(fresh.kill_window(0).is_empty());
    // Steady state, but an unknown window id yields no command.
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    drain_to_idle(&mut v);
    assert!(v.kill_window(999).is_empty());
}

#[test]
fn detach_client_emits_bare_detach_and_guards_pre_steady() {
    // Orphan teardown (I1): closing the last tmux tab detaches the -CC client.
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    drain_to_idle(&mut v);
    let cmd = command_bytes(&v.detach_client()).expect("detach-client command");
    assert!(
        bytes_contains(&cmd, b"detach-client\n"),
        "unexpected detach-client bytes: {:?}",
        String::from_utf8_lossy(&cmd)
    );
    // Its %begin/%end reply carries nothing to apply and queues no follow-up
    // (unlike kill-*); tmux's own `%exit` drives teardown.
    assert!(
        v.next(block_end("")).is_empty(),
        "detach-client reply should queue nothing"
    );
    // Not yet in steady state: no command.
    let mut fresh = Viewer::new(Colors::default());
    assert!(fresh.detach_client().is_empty());
}

#[test]
fn new_window_emits_bare_new_window() {
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    drain_to_idle(&mut v);
    let cmd = command_bytes(&v.new_window()).expect("new-window command");
    assert!(
        bytes_contains(&cmd, b"new-window\n"),
        "unexpected new-window bytes: {:?}",
        String::from_utf8_lossy(&cmd)
    );
}

#[test]
fn structural_writes_thread_through_the_command_queue() {
    // A capture command is in flight straight after startup (queue non-empty),
    // so a split enqueues behind it and emits nothing right now — proving the
    // write correlates through the same in-flight queue as reads.
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    assert!(!has_command(&v.split_window(0, true, false)));
    // Draining the capture queue eventually emits the deferred split.
    let mut saw = false;
    for _ in 0..100 {
        let a = v.next(block_end(""));
        match command_bytes(&a) {
            Some(cmd) if bytes_contains(&cmd, b"split-window -t %0 -h\n") => {
                saw = true;
                break;
            }
            Some(_) => continue,
            None => break,
        }
    }
    assert!(saw, "deferred split-window was never emitted");
}

#[test]
fn structural_writes_guard_pre_steady_and_unknown_pane() {
    // Not yet in steady state: no command from any structural write.
    let mut fresh = Viewer::new(Colors::default());
    assert!(fresh.split_window(0, true, false).is_empty());
    assert!(fresh.kill_pane(0).is_empty());
    assert!(fresh.new_window().is_empty());
    // Steady state, but an unknown pane id yields no split/kill command.
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    drain_to_idle(&mut v);
    assert!(v.split_window(999, true, false).is_empty());
    assert!(v.kill_pane(999).is_empty());
}

// ---- focus sync: select-pane + active-pane tracking (slice 5e) -------------

#[test]
fn window_pane_changed_updates_active_pane() {
    let mut v = viewer_with_window(&checksummed("83x44,0,0,0"));
    drain_to_idle(&mut v);
    assert_eq!(
        v.active_pane(),
        None,
        "no active pane until tmux reports one"
    );
    v.next(window_pane_changed(0, 0));
    assert_eq!(v.active_pane(), Some(0), "tmux's active pane is tracked");
}

#[test]
fn select_pane_emits_targeted_select_and_sets_active_optimistically() {
    // A 1x2 split so panes 0 and 1 both exist.
    let mut v = viewer_with_window(&checksummed("83x44,0,0[83x20,0,0,0,83x23,0,21,1]"));
    drain_to_idle(&mut v);
    let cmd = command_bytes(&v.select_pane(1)).expect("select command");
    assert!(
        bytes_contains(&cmd, b"select-pane -t %1\n"),
        "unexpected select bytes: {:?}",
        String::from_utf8_lossy(&cmd)
    );
    // Active pane set optimistically so tmux's echoing %window-pane-changed is a
    // no-op (no focus bounce).
    assert_eq!(v.active_pane(), Some(1));
}

#[test]
fn select_pane_is_idempotent_and_guarded() {
    // Not in steady state: nothing.
    let mut fresh = Viewer::new(Colors::default());
    assert!(fresh.select_pane(0).is_empty());

    let mut v = viewer_with_window(&checksummed("83x44,0,0[83x20,0,0,0,83x23,0,21,1]"));
    drain_to_idle(&mut v);
    // Unknown pane → no command.
    assert!(v.select_pane(999).is_empty());
    // Selecting an already-active pane is a no-op (avoids redundant traffic).
    v.next(window_pane_changed(0, 1));
    assert_eq!(v.active_pane(), Some(1));
    assert!(
        v.select_pane(1).is_empty(),
        "selecting the already-active pane must emit nothing"
    );
}
