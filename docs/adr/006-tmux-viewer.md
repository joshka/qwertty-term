# ADR 006: tmux control-mode Viewer — architecture, UX mapping, and slices

- Status: **ACCEPTED** (Josh ratified 2026-07-15: **Option (a)** — tmux window
  -> native tab, tmux pane -> split within that tab). Slice 5a landed with this
  ADR; slices 5b+ are unblocked. The options + tradeoffs are retained below for
  the record.
- Date: 2026-07-15
- Thread: app-tails (app/termio) · succeeds ADR 004 slice 5 handoff
- Upstream: Ghostty `2da015cd6`, `src/terminal/tmux/viewer.zig` (2,283 LoC) +
  `src/termio/stream_handler.zig`
- Consumes (engine public API, do NOT modify): `qwertty-term-vt::tmux::{ControlParser,
  Notification, layout::Layout, output::{Variable, Value, format, parse_format}}`
  and `stream::TerminalHandler::take_tmux_notifications()`.

## Context

ADR 004 ported the three pure tmux protocol parsers into the engine (slices 1-3,
merged) and the DCS `1000p` seam that surfaces a `Notification` stream (slice 4).
The last piece — the **Viewer** (slice 5) — is app/termio territory: it maps the
notification stream to native surfaces so `tmux -CC` renders as native tabs and
splits instead of tmux's own text UI. Until it lands, tmux control mode is
parsed but not app-observable.

Upstream's `viewer.zig` is a single 2,283-LoC struct that mixes pure state
(session/window/pane tree, a command-queue correlation state machine, layout
application, `%output` routing) with the app-driving half (it hands the caller
`Action.command` / `Action.windows` / `Action.exit` and the caller wires those
to a real tmux pty and real surfaces). The pure half is unit-testable with no
windowing; the app half needs AppKit. This ADR splits them along that seam.

## Decision

### Architecture

The Viewer lives in the **app crate** (`crates/qwertty-term`), split into a
headless model (`tmux_viewer.rs`, slice 5a — landed) and a native binding
(slice 5b+, follow-ups). It is **not** in the engine crate (`qwertty-term-vt`
stays windowing-free and is only consumed, never modified) and not in termio
(the model owns per-pane `Terminal`s and is app-render-facing).

```text
pty bytes ─▶ engine Stream/DCS seam ─▶ TerminalHandler.pending_tmux
                                              │  take_tmux_notifications() -> Vec<Notification>
                                              ▼
                    ┌──────────────── crates/qwertty-term ────────────────┐
                    │  tmux_viewer::Viewer  (slice 5a — HEADLESS, tested)  │
                    │   • session -> windows -> panes tree                 │
                    │   • command-queue correlation state machine          │
                    │   • Layout -> pane geometry (PaneRect)               │
                    │   • %output -> per-pane Terminal (Stream::feed)      │
                    │   • next(Notification) -> Vec<Action>                │
                    │        Action = Exit | Command(bytes) | WindowsChanged│
                    └───────────────────────┬─────────────────────────────┘
                                            │ query: windows(), pane(), pane_rects()
                                            ▼
                    ┌──────── native binding (slice 5b+ — AppKit) ─────────┐
                    │   • WindowsChanged -> create/destroy NSWindow tabs +  │
                    │     SplitTree splits, bind each pane Terminal to a     │
                    │     Surface/renderer                                  │
                    │   • Command(bytes) -> write to the tmux control pty    │
                    │   • Exit  -> tear down surfaces; drop the Viewer      │
                    │   • focus / resize / keyboard input routing           │
                    └───────────────────────────────────────────────────────┘
```

**Lifecycle** (mirrors upstream `stream_handler.zig`: it owns a `?*Viewer`,
creates it on the tmux `.enter` notification and frees it on `.exit`). The app's
per-surface notification drain — wherever `take_tmux_notifications()` is polled
each frame, alongside `take_clipboard`/`take_bell`/`take_notification` — gains a
tmux branch:

- On `Notification::Enter`: construct a `Viewer` for that control-mode surface.
- For each subsequent notification: call `viewer.next(n)`, apply the returned
  `Action`s (send `Command` bytes to the tmux pty, reconcile surfaces on
  `WindowsChanged`, tear down on `Exit`).
- On `Notification::Exit` / `viewer.is_defunct()`: drop the Viewer and its
  surfaces.

**Per-pane `Terminal` ownership.** Each tmux pane owns an engine
`Stream<TerminalHandler>` (hence a `Terminal`), constructed via the engine's
public `Terminal::new(Options { cols, rows, .. })` sized from the pane's layout
node. `%output` bytes are fed straight through `Stream::feed`. This is the same
pattern as any app surface's terminal, so the renderer can snapshot a pane
terminal unchanged.

### The UX mapping (the open question for Josh)

How do tmux **windows** and **panes** map onto qwertty-term **surfaces**?

- A tmux *session* is the whole control-mode connection (one `tmux -CC`).
- A tmux *window* is a full-screen workspace; a session has many, one active.
- A tmux *pane* is a rectangular split within a window (the `Layout` tree).

qwertty-term surfaces: a native **window** contains **tabs**; each tab owns a
**`SplitTree`** of panes (`crates/qwertty-term/src/splits.rs`), each leaf a
`Surface` bound to one engine `Terminal`.

Options:

| Option                           | tmux window ->  | tmux pane ->        | Pros                                                                                          | Cons                                                                                                                 |
| -------------------------------- | --------------- | ------------------- | --------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| **(a) window->tab, pane->split** | a native tab    | a split in that tab | Natural mental model; tmux's own tab bar becomes the native tab bar; splits reuse `SplitTree` | tmux layouts are arbitrary binary geometry; must map `Layout` rects to `SplitTree` ratios (lossy for exotic layouts) |
| (b) flat: pane -> tab            | (dropped)       | a native tab        | Trivial mapping; no geometry translation                                                      | Loses tmux's window/pane hierarchy entirely; a 4-pane window becomes 4 unrelated tabs                                |
| (c) window -> native window      | a native window | a split             | Strong isolation per tmux window                                                              | tmux windows are cheap and numerous; spawning OS windows per window is heavy and unlike every other `-CC` client     |

**Recommendation: option (a)** — tmux window -> native tab, tmux pane -> split
within that tab. It is what users expect from a native tmux front-end, matches
the shape of `iTerm2`'s tmux integration, and reuses the existing `SplitTree`.
The one real cost is translating tmux's `Layout` geometry (absolute cell rects
per pane) into `SplitTree`'s recursive ratio splits; the headless model already
flattens the layout to `PaneRect`s (`Viewer::pane_rects`), so slice 5b builds a
rect-tree -> `SplitTree` converter. Upstream `viewer.zig` is agnostic here — it
emits an opaque `windows` action and leaves the surface mapping entirely to the
apprt (macOS `TerminalController`), so choosing (a) does not diverge from
upstream's engine-side contract.

**Ratified 2026-07-15 (Josh): Option (a).** Slice 5b builds the native surface
reconciliation against this mapping (tab per tmux window, `SplitTree` per window
from the model's `PaneRect`s).

### Headless vs AppKit boundary

**Pure logic (slice 5a — landed in this PR, `tmux_viewer.rs`):**

- the session/window/pane **state model** and the `next(Notification) ->
  Vec<Action>` **reducer**;
- the **command-queue** correlation state machine (`%begin`/`%end` block <-> the
  command that triggered it; one command in flight at a time);
- **layout application**: parse `%layout-change`/`list-windows` layouts via
  `tmux::layout::Layout`, flatten to per-pane geometry (`PaneRect`);
- **`%output` routing** to the correct pane's owned `Terminal` (`Stream::feed`);
- session-changed reset, window add/prune, defunct/exit handling;
- a **queryable model** (`windows()`, `pane()`, `pane_rects()`, `session_id()`,
  `tmux_version()`, `is_defunct()`).

**Needs AppKit (slice 5b+, follow-ups — NOT in this PR):**

- reconciling `WindowsChanged` into `NSWindow` tabs + `SplitTree` splits, binding
  each pane `Terminal` to a `Surface`/renderer;
- writing `Command(bytes)` to the tmux control pty (termio wiring);
- focus (which pane/tab is active), resize (propagating native size back to tmux
  via `refresh-client -C`/`resize-pane`), and keyboard/mouse input routing into
  the active pane;
- applying captured pane **content** (scrollback history, alternate screen) and
  **terminal state** (cursor position/shape, modes, scroll region, tab stops)
  to each pane `Terminal`. Slice 5a parses `list-panes` state into a queryable
  `PaneState` and feeds the visible primary-screen capture, but does not yet
  write scrollback/alternate captures or apply cursor/modes — those need engine
  screen-write paths that are not on the public API surface today.

### PR slice breakdown (the whole Viewer)

1. **5a — headless model** (landed): `tmux_viewer::Viewer`, unit-tested. No
   AppKit.
2. **5b — reconcile logic** (landed): `layout_to_split_tree` + `Reconciler` ->
   `ReconcilePlan` (`CreateTab`/`RemoveTab`/`SetSplitTree`) + `SplitTree::from_node`.
   Unit-tested. The **native application** of a plan (creating `NSWindow` tabs +
   display-only pane surfaces) is **5b-native**, deferred — see "5c
   surface-binding" below.
3. **5c — control-mode lifecycle** (landed, this PR): a new
   `Engine::take_tmux_notifications` drain plus `tmux_session::TmuxSession`.
   `Surface::pump` constructs the session on
   `Enter`, drains notifications each tick, writes `Command` bytes back to the
   control pty, and tears down on `Exit`; the `ReconcilePlan` rides out on
   `PumpResult`. The "make it live" slice. Headless `--tmux-smoke` verifies it.
   **5b-native** (turning the plan into real native tabs/display-only surfaces)
   is the remaining follow-up — see "5c surface-binding".
4. **5d — input + focus + resize**: route keyboard/mouse to the active pane
   (tmux `send-keys`), track active window/pane focus, propagate native resize to
   tmux.
5. **5e — capture-content fidelity**: apply scrollback history + alternate-screen
   captures and `list-panes` cursor/mode state to each pane `Terminal` (needs an
   engine public screen-write/mode-apply path — coordinate with vt-tails via
   Inbox if new engine API is required).
6. **5f — polish**: pane titles (`%window-renamed`), pane resize -> tmux
   `resize-pane`, detach/exit UX, and robustness for out-of-order startup
   notifications (upstream's noted TODO).

### 5c surface-binding (the display-only-surface design)

Slice 5c makes control mode **live**: a pane running `tmux -CC` now drives the
whole model + IO loop. What landed, and the deliberate seam left for the native
tab creation, is recorded here so reviewers see the design decision.

**Landed (live + headlessly verified):**

- `Engine::take_tmux_notifications()` exposes the engine's DCS `1000p`
  notification drain to the app (mirrors the other `take_*` event drains).
- `tmux_session::TmuxSession` (headless) owns a `Viewer` + a `Reconciler` and
  reduces a drained notification batch into a `SessionUpdate { commands, plan,
  exit }` via one `ingest` call. It also resolves a native `SurfaceId` back to
  the Viewer's pane `Terminal` (`pane_terminal`) — the render seam.
- `Surface` gains an `Option<TmuxSession>`. `Surface::pump` drains
  `take_tmux_notifications` alongside the existing per-tick drains, constructs
  the session on the `Enter` the DCS seam emits, feeds every notification,
  **writes the session's outgoing command bytes back to that same surface's
  pty** (control mode is in-band on the control pty), and drops the session on
  `Exit`. The reconcile `plan` + `exit` ride out on `PumpResult`; the controller
  applies them after the per-surface borrow drops
  (`Controller::apply_tmux_reconciles`).
- A headless smoke (`--tmux-smoke`) drives a synthetic `tmux -CC` byte stream
  through a real engine + session (a fake tmux server answers the session's
  commands with `%begin`/`%end` blocks over the same in-band stream) and asserts
  the native tabs, each window's split tree, and that `%output` reached the
  right pane `Terminal`. No GPU/window, so it runs anywhere.

**The novel piece — display-only pane surfaces (deferred to 5b-native):** a tmux
pane surface is **not** pty-backed. Its bytes arrive via `%output` from the
Viewer, and its `Terminal` is owned by the Viewer's `Pane`, not by a
`Surface { engine: Arc<Mutex<Engine>>, io: TabIo }`. Every existing `Surface`
field and `build_surface` assume a pty + a private engine, so a Viewer-backed
pane is a **new construction mode**. The chosen design (to implement in
5b-native, not blindly here where AppKit behaviour can't be exercised):

- Introduce a `Surface` source seam — the minimal form is a display-only
  surface that holds **no `TabIo`** and renders a `Terminal` it does **not**
  own. Two viable shapes: (a) the render path snapshots
  `controller.tmux_session(control_surface).pane_terminal(surface_id)` by
  reference each frame (no engine duplication; the Viewer stays the single
  owner/feeder of pane bytes — preferred, matches upstream's "the Viewer hands
  out surfaces"); or (b) each pane's engine becomes a shared
  `Arc<Mutex<Engine>>` that the session feeds instead of an owned `Stream`
  (simpler render path, but couples the headless Viewer to the app engine and
  duplicates the ownership). **(a) is preferred.**
- `Controller::apply_tmux_reconciles` then turns each `ReconcileOp` into native
  intent: `CreateTab` → spawn a display-only tab, `RemoveTab` → close it,
  `SetSplitTree` → set the tab's `SplitTree` (already the exact type the
  Reconciler emits) and bind each leaf to its pane `Terminal`;
  `dropped_surfaces` frees the renderer surfaces of closed panes. The control
  surface itself is hidden while its session is live (upstream behaviour).

Until 5b-native lands, `apply_tmux_reconciles` records each reconciliation (so a
live `tmux -CC` session is observable and the seam is explicit) and the pane
`Terminal`s are already populated by `%output` and reachable via
`TmuxSession::pane_terminal`. Focus / resize / input routing stay in 5d, and
capture-content fidelity in 5e, unchanged.

## Consequences

- Closes the last VT-engine feature (`tmux -CC`) at the app layer; slices 1-4
  (engine) are already merged.
- The engine crate is untouched — the app consumes only public tmux API.
- Slice 5a is ~600 LoC of pure Rust with 10 unit tests; it de-risks the large
  native slices by pinning the protocol state machine first.
- The `Action::WindowsChanged` signal deviates from upstream's
  `Action.windows([]Window)` slice (the model is queried instead of pushed) —
  documented in `tmux_viewer.rs`; it avoids threading borrowed slices through the
  reducer return in Rust.

## Open questions for Josh

1. **UX mapping — RESOLVED 2026-07-15: Option (a)** (tmux window -> native tab,
   tmux pane -> split). Slices 5b+ unblocked.
2. Should a tmux control-mode session open in the **current window as tabs**, or
   spawn a **dedicated native window**? (Recommendation: current window, tabs.)
3. On `%session-changed` the Viewer resets all surfaces (upstream behaviour) —
   acceptable, or should we preserve/animate? (Recommendation: accept upstream's
   reset for now.)
4. Slice 5e may need a new engine public API to apply captured cursor/mode state
   and scrollback to a `Terminal`. OK to route that request to vt-tails' Inbox
   when we get there?
