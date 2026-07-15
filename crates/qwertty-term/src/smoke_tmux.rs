//! Headless tmux control-mode smoke (ADR 006 slice 5c).
//!
//! `--tmux-smoke` runs this. It drives the **live** control-mode path end to end
//! without a window, a pty, or a Metal device: it feeds a synthetic `tmux -CC`
//! control-mode byte stream into a real `qwertty-term-vt` [`Engine`], drains the
//! decoded notifications exactly as [`Surface::pump`](crate::app) does, feeds
//! them to a [`TmuxSession`], applies the resulting [`ReconcilePlan`]s to a mock
//! native-tab set (the same ops the AppKit layer applies to real
//! [`NSWindow`](objc2_app_kit::NSWindow) tabs — slice 5b-native), and writes the
//! session's outgoing command bytes back into the engine, playing the role of
//! the tmux server. It then asserts:
//!
//! - the expected native **tabs** exist (one per tmux window);
//! - each window's **split tree** matches the tmux layout (window → tab, pane →
//!   split — Option (a));
//! - live `%output` bytes reached the **right pane `Terminal`** (verified by
//!   reading that pane's screen text back through
//!   [`TmuxSession::pane_terminal`]).
//!
//! Because the control-mode reconcile/routing layer needs no GPU, this smoke
//! runs anywhere (unlike `--offscreen-smoke`, which needs a Metal device). It
//! exits `0` on success, non-zero on failure, so it doubles as a gate.
//!
//! ## The fake tmux server
//!
//! Control mode is in-band: the client (our [`TmuxSession`]) sends commands
//! (`list-windows`, `capture-pane`, …) and tmux replies with `%begin`/`%end`
//! blocks on the same pty. Here [`FakeServer`] plays tmux: [`drive`] loops
//! draining engine notifications → `session.ingest` → and, for every command the
//! session emits, writes the matching `%begin`/`%end` response back into the
//! engine, running the whole command-queue handshake to a fixpoint. This mirrors
//! the real bytes a `tmux -CC` process would send, so the Viewer/Reconciler are
//! exercised through their genuine notification decode path, not fed
//! `Notification`s directly (that is what the unit tests do).

use std::collections::HashMap;

use qwertty_term_vt::tmux::Checksum;

use crate::engine::Engine;
use crate::splits::SplitTree;
use crate::tmux_reconcile::{ReconcileOp, ReconcilePlan};
use crate::tmux_session::TmuxSession;

/// The mock native-tab set the reconcile plan is applied to — the headless
/// analog of the AppKit tab registry + per-tab `SplitTree`s (slice 5b-native).
#[derive(Default)]
struct NativeTabs {
    /// Native tab window ids in creation order (each tmux window → one tab).
    order: Vec<usize>,
    /// Each present window's split tree (pane → split).
    trees: HashMap<usize, SplitTree>,
}

impl NativeTabs {
    /// Apply one reconcile plan, exactly as the native layer would: removals,
    /// then creations, then set each present window's split tree.
    fn apply(&mut self, plan: &ReconcilePlan) {
        for op in &plan.ops {
            match op {
                ReconcileOp::RemoveTab { window_id } => {
                    self.order.retain(|w| w != window_id);
                    self.trees.remove(window_id);
                }
                ReconcileOp::CreateTab { window_id } => {
                    if !self.order.contains(window_id) {
                        self.order.push(*window_id);
                    }
                }
                ReconcileOp::SetSplitTree { window_id, tree } => {
                    self.trees.insert(*window_id, tree.clone());
                }
            }
        }
    }
}

/// A synthetic tmux control-mode server: holds the current window model and
/// answers the client's commands with `%begin`/`%end` blocks.
struct FakeServer {
    /// The current windows: `(window_id, layout_body)` where `layout_body` is a
    /// checksum-less tmux layout string (the checksum is added on send).
    windows: Vec<(usize, String)>,
    /// Monotonic block sequence number for the `%begin`/`%end` metadata.
    seq: usize,
}

impl FakeServer {
    fn new() -> FakeServer {
        FakeServer {
            windows: Vec::new(),
            seq: 0,
        }
    }

    /// Write a raw control-mode line (a trailing `\n` is appended) into the
    /// engine's control-mode DCS stream.
    fn send_line(&self, engine: &mut Engine, line: &str) {
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\n');
        engine.write(&bytes);
    }

    /// Write a `%begin`/`%end` block whose payload is `payload` (each payload
    /// line already `\n`-terminated by the caller, or empty for an empty block).
    fn send_block(&mut self, engine: &mut Engine, payload: &str) {
        self.seq += 1;
        let n = self.seq;
        let mut blob = format!("%begin {n} {n} 0\n");
        blob.push_str(payload);
        blob.push_str(&format!("%end {n} {n} 0\n"));
        engine.write(blob.as_bytes());
    }

    /// A full checksummed layout string for `body` (e.g. `83x44,0,0,1`).
    fn layout(body: &str) -> String {
        let csum = Checksum::calculate(body.as_bytes()).as_string();
        format!("{},{}", String::from_utf8_lossy(&csum), body)
    }

    /// Respond to one command the session emitted, playing tmux.
    fn respond(&mut self, engine: &mut Engine, command: &[u8]) {
        if contains(command, b"display-message") {
            // The tmux version query.
            self.send_block(engine, "3.5a\n");
        } else if contains(command, b"list-windows") {
            // One line per window: `$0 @<id> 83 44 <layout>`.
            let mut payload = String::new();
            for (id, body) in &self.windows {
                payload.push_str(&format!("$0 @{} 83 44 {}\n", id, Self::layout(body)));
            }
            self.send_block(engine, &payload);
        } else if let Some((pane_id, text)) = parse_send_keys(command) {
            // Keyboard input (ADR 006 slice 5d). Ack the write with an empty
            // block, then echo the printable keys back as the pane's `%output`,
            // mimicking a shell echoing typed input — so the smoke can assert the
            // keystroke reached the right pane terminal end to end.
            self.send_block(engine, "");
            if !text.is_empty() {
                self.send_line(engine, &format!("%output %{pane_id} {text}"));
            }
        } else if contains(command, b"capture-pane") || contains(command, b"list-panes") {
            // Content-capture / pane-state: an empty block is a valid response
            // and keeps the command queue advancing (the live path we assert is
            // %output, not the initial capture).
            self.send_block(engine, "");
        }
        // Any other command: ignore (none are emitted by the Viewer today).
    }
}

/// Parse a `send-keys -t %<id> -H <hex…>` command into `(pane_id, text)`,
/// decoding the hex codepoints and keeping only printable ASCII (enough to
/// prove the input round-trip). Returns `None` for any other command.
fn parse_send_keys(command: &[u8]) -> Option<(usize, String)> {
    let s = std::str::from_utf8(command).ok()?.trim();
    let rest = s.strip_prefix("send-keys -t %")?;
    let (id_str, rest) = rest.split_once(' ')?;
    let pane_id: usize = id_str.parse().ok()?;
    let hexes = rest.strip_prefix("-H")?.trim();
    let mut text = String::new();
    for tok in hexes.split_whitespace() {
        if let Some(c) = u32::from_str_radix(tok, 16)
            .ok()
            .and_then(char::from_u32)
            .filter(|c| (' '..='~').contains(c))
        {
            text.push(c);
        }
    }
    Some((pane_id, text))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Drain the engine's tmux notifications, feed them to the session, apply any
/// reconcile plan to `native`, and answer every emitted command via `server` —
/// looping until the exchange reaches a fixpoint (no more notifications).
/// Returns whether the session signalled exit.
fn drive(
    engine: &mut Engine,
    session: &mut TmuxSession,
    native: &mut NativeTabs,
    server: &mut FakeServer,
) -> bool {
    let mut exited = false;
    // Bound the loop defensively so a protocol bug can't hang the smoke.
    for _ in 0..10_000 {
        let notifications = engine.take_tmux_notifications();
        if notifications.is_empty() {
            break;
        }
        let update = session.ingest(notifications);
        if let Some(plan) = &update.plan {
            native.apply(plan);
        }
        if update.exit {
            exited = true;
        }
        for command in &update.commands {
            server.respond(engine, command);
        }
    }
    exited
}

/// Run the headless control-mode smoke. `Ok(())` on success; `Err` with a
/// diagnostic on any assertion failure.
pub fn run() -> Result<(), String> {
    let mut engine = Engine::new(83, 44);
    let mut session = TmuxSession::new(qwertty_term_vt::terminal::Colors::default());
    let mut native = NativeTabs::default();
    let mut server = FakeServer::new();

    // The session opens with one window @1 holding one pane %1.
    server.windows.push((1, "83x44,0,0,1".to_string()));

    // --- Enter control mode + the startup handshake ------------------------
    // `\eP1000p` enters control mode (the engine emits Enter). tmux then sends
    // an initial block, then `%session-changed`, which kicks the Viewer's
    // version + list-windows handshake; `drive` runs it to completion.
    engine.write(b"\x1bP1000p");
    drive(&mut engine, &mut session, &mut native, &mut server);
    server.send_block(&mut engine, ""); // initial startup block
    server.send_line(&mut engine, "%session-changed $0 tmux-smoke");
    drive(&mut engine, &mut session, &mut native, &mut server);

    // Assert: exactly one native tab (window @1), a single-leaf tree for pane 1.
    if native.order != [1] {
        return Err(format!(
            "after startup expected one native tab for window 1, got {:?}",
            native.order
        ));
    }
    let s1 = session
        .surface_of(1)
        .ok_or("pane 1 was not assigned a surface")?;
    let tree1 = native.trees.get(&1).ok_or("window 1 has no split tree")?;
    if tree1.surfaces() != vec![s1] {
        return Err(format!(
            "window 1 tree should be a single leaf for pane 1's surface, got {:?}",
            tree1.surfaces()
        ));
    }

    // --- Live %output routes to the right pane terminal --------------------
    server.send_line(&mut engine, "%output %1 hello-from-pane-1");
    drive(&mut engine, &mut session, &mut native, &mut server);
    let text = session
        .pane_terminal(s1)
        .ok_or("surface for pane 1 did not resolve to a terminal")?
        .plain_string();
    if !text.contains("hello-from-pane-1") {
        return Err(format!(
            "pane 1's %output did not reach its terminal; screen = {text:?}"
        ));
    }

    // --- A %layout-change splits window @1 into panes %1 | %3 --------------
    let split_body = "83x44,0,0[83x22,0,0,1,83x21,0,23,3]";
    server.windows[0].1 = split_body.to_string();
    server.send_line(
        &mut engine,
        &format!(
            "%layout-change @1 {} {} *",
            FakeServer::layout(split_body),
            FakeServer::layout(split_body)
        ),
    );
    drive(&mut engine, &mut session, &mut native, &mut server);

    // The surviving pane 1 keeps its surface; pane 3 is new; the tree now holds
    // both, in layout order.
    if session.surface_of(1) != Some(s1) {
        return Err(
            "surviving pane 1 lost its stable surface across the layout change".to_string(),
        );
    }
    let s3 = session
        .surface_of(3)
        .ok_or("new pane 3 was not assigned a surface")?;
    let tree1 = native.trees.get(&1).ok_or("window 1 tree missing")?;
    if tree1.surfaces() != vec![s1, s3] {
        return Err(format!(
            "window 1 tree should now be panes 1|3, got {:?}",
            tree1.surfaces()
        ));
    }
    // Still exactly one tab (a split adds a pane, not a window/tab).
    if native.order != [1] {
        return Err(format!(
            "a layout split should not add a tab, tabs = {:?}",
            native.order
        ));
    }
    // Pane 1's earlier output survives the reflow (surface reuse, not recreation).
    let text = session
        .pane_terminal(s1)
        .ok_or("pane 1 terminal missing after split")?
        .plain_string();
    if !text.contains("hello-from-pane-1") {
        return Err("pane 1 lost its content across the layout change".to_string());
    }

    // Per-pane surface binding: this is exactly the seam the native render pass
    // (`Controller::render_tmux_panes`) depends on — each split-tree leaf's
    // `SurfaceId` must resolve to *its own* pane `Terminal`, so `%output` for one
    // pane never bleeds into another's surface. Route output to pane 3 and assert
    // it lands in s3's terminal and NOT in s1's. We snapshot through the same
    // `snapshot_window` call the render path uses, not just `plain_string`, so
    // the binding is exercised end to end.
    server.send_line(&mut engine, "%output %3 hello-from-pane-3");
    drive(&mut engine, &mut session, &mut native, &mut server);
    let term3 = session
        .pane_terminal(s3)
        .ok_or("pane 3 surface did not resolve to a terminal")?;
    if !term3.plain_string().contains("hello-from-pane-3") {
        return Err(format!(
            "pane 3's %output did not reach its own terminal; screen = {:?}",
            term3.plain_string()
        ));
    }
    // The render pass reads each pane via `snapshot_window(0)`; make sure that
    // call resolves a window sized to the pane (non-empty), not a panic/empty.
    if term3.snapshot_window(0).window.is_empty() {
        return Err("pane 3 snapshot_window returned no rows".to_string());
    }
    let term1 = session
        .pane_terminal(s1)
        .ok_or("pane 1 terminal missing")?
        .plain_string();
    if term1.contains("hello-from-pane-3") {
        return Err("pane 3's output bled into pane 1's surface/terminal".to_string());
    }

    // --- Input: send-keys routes typed bytes to the focused pane -----------
    // ADR 006 slice 5d: a display-only tmux pane has no pty, so a keystroke is
    // encoded and handed to `TmuxSession::send_keys`, which emits a `send-keys`
    // control command on the control pty. Drive it end to end: encode "hi\r"
    // for pane 1's surface, play tmux (ack + echo), and assert it landed in
    // pane 1's terminal (and not pane 3's).
    let cmds = session.send_keys(s1, b"hi\r");
    if cmds.is_empty() {
        return Err("send_keys produced no control command for pane 1".to_string());
    }
    let joined: Vec<u8> = cmds.concat();
    // The exact wire form: `send-keys -t %1 -H 68 69 d` (h, i, CR).
    if !contains(&joined, b"send-keys -t %1 -H 68 69 d") {
        return Err(format!(
            "unexpected send-keys command bytes: {:?}",
            String::from_utf8_lossy(&joined)
        ));
    }
    for cmd in &cmds {
        server.respond(&mut engine, cmd);
    }
    drive(&mut engine, &mut session, &mut native, &mut server);
    let text1 = session
        .pane_terminal(s1)
        .ok_or("pane 1 terminal missing after input")?
        .plain_string();
    if !text1.contains("hi") {
        return Err(format!(
            "typed input did not reach pane 1's terminal; screen = {text1:?}"
        ));
    }
    let text3 = session
        .pane_terminal(s3)
        .ok_or("pane 3 terminal missing after input")?
        .plain_string();
    if text3.contains("hi") {
        return Err("pane 1's input bled into pane 3's terminal".to_string());
    }
    // Input for an unbound surface id yields no command (defensive).
    if !session
        .send_keys(crate::splits::SurfaceId(9999), b"x")
        .is_empty()
    {
        return Err("send_keys for an unknown surface should yield no command".to_string());
    }

    // --- A 2nd %window-add @2 opens a 2nd native tab ----------------------
    server.windows.push((2, "83x44,0,0,2".to_string()));
    server.send_line(&mut engine, "%window-add @2");
    drive(&mut engine, &mut session, &mut native, &mut server);

    if native.order != [1, 2] {
        return Err(format!(
            "window-add @2 should create a 2nd native tab, tabs = {:?}",
            native.order
        ));
    }
    let s2 = session
        .surface_of(2)
        .ok_or("pane 2 (window 2) was not assigned a surface")?;
    let tree2 = native.trees.get(&2).ok_or("window 2 has no split tree")?;
    if tree2.surfaces() != vec![s2] {
        return Err(format!(
            "window 2 tree should be a single leaf for pane 2, got {:?}",
            tree2.surfaces()
        ));
    }
    // Window 1's split tree is untouched by the new window.
    let tree1 = native.trees.get(&1).ok_or("window 1 tree missing")?;
    if tree1.surfaces() != vec![s1, s3] {
        return Err("window 1 tree changed when window 2 was added".to_string());
    }

    // --- Exit control mode tears the session down --------------------------
    engine.write(b"\x1b\\");
    let exited = drive(&mut engine, &mut session, &mut native, &mut server);
    if !exited {
        return Err("ST (exit control mode) did not signal session teardown".to_string());
    }
    if !session.is_defunct() {
        return Err("session should be defunct after exit".to_string());
    }

    Ok(())
}
