# vt-tails status

- **Current item:** **tmux control mode — vt-tails engine work COMPLETE (slices 1–4 all
  merged).** The DCS `1000p` seam feeds the `ControlParser` and surfaces a `Notification`
  stream via `TerminalHandler::take_tmux_notifications()`. Only **slice 5 (native Viewer,
  ~2,283 LoC) remains — app-tails territory** (Inbox-routed in `app-tails.md`). Nothing left
  for vt-tails on tmux unless the Viewer needs an engine accessor (route back via Inbox).
- **Last merged:** #271 (slice 4 — DCS seam). #261/#259/#257 (slices 3/2/1 parsers), #255
  (ADR 004), VT-completeness tail (#241/#244/#249/#250) — all on main.
- **Blockers:** none. vt-tails' tmux backlog is drained; slice 5 is app-tails.
- **Claims:** none.
- **Inbox:** (other threads append requests here; owner triages into backlog)

## Mission (current: tmux control mode — ADR 004)

Port the **pure tmux protocol parsers** into `crates/qwertty-term-vt` (vt-tails). The native
Viewer + termio wiring is app-tails/termio (ADR 004 slice 5). Verify against `~/local/ghostty`
@ `2da015cd6`, cite file:line. tmux control mode is NOT a libghostty-vt differential-oracle
surface → **unit tests are the referee** (like XTGETTCAP/DECRQSS). Zig-port hazards apply.

### tmux backlog (ADR 004 slices — vt-tails owns 1–4; app-tails owns 5)

1. ✅ **`tmux::ControlParser`** (`control.rs`) — MERGED #257. 26 tests.
2. ✅ **`tmux::layout::Layout`** (`layout.rs`) — MERGED #259. 24 tests.
3. ✅ **`tmux::output`** (`output.rs`) — MERGED #261. 23 tests.
4. ✅ **Wire the DCS `1000p` seam → `Notification` stream** — MERGED #271: `dcs::Command::Tmux`
   plus `TerminalHandler::{tmux, take_tmux_notifications}`; `feature-coverage` tmux → `[~]`.
5. **Viewer + termio wiring** (`viewer.zig`, 2,283 LoC) — **app-tails/termio, NOT vt-tails.**
   Route via app-tails Inbox once slice 4 lands.

Open questions — RESOLVED (Josh confirmed 2026-07-14): always-compiled/runtime-inert;
Viewer = app-tails. See ADR 004 "Resolution".

### Slice 4 design (for the fresh session — verify vs `~/local/ghostty` `2da015cd6`)

The `ControlParser`/`layout`/`output` modules exist in `crates/qwertty-term-vt/src/tmux/`.
Slice 4 connects the DCS state machine to `ControlParser` and surfaces its notifications.

**Upstream flow** (`src/terminal/dcs.zig`): the `ControlParser` lives *inside* the DCS
`State.tmux` payload. `hook` on `ESC P 1000 p` (`dcs.zig:53-73`) → `Command.tmux = .enter`.
Each subsequent body byte: `put` feeds `tmux.put(byte)`; a produced notification →
`Command.tmux = notification` (`dcs.zig:130-132`). `unhook` (ST) → `Command.tmux = .exit`
(`dcs.zig:168-170`). `stream_handler.zig:393` dispatches: `.enter` makes the Viewer, `.exit`
frees it, else feeds the Viewer.

**Our current seam** (`crates/qwertty-term-vt/src/dcs.rs`): `Handler::hook` already returns
`Command::TmuxRaw(TmuxEvent::Enter)` on `[1000]p` (dcs.rs ~194-202); `put`'s `State::Tmux`
arm drops bytes (TODO ~99-101); `unhook` returns `TmuxRaw(TmuxEvent::Exit)` (~139-141).
`enum Command` has `TmuxRaw(TmuxEvent)` (~265); `enum TmuxEvent { Enter, Exit }` (~273).

**Do:**

1. Replace `Command::TmuxRaw(TmuxEvent)` with `Command::Tmux(crate::tmux::Notification)`
   (my `Notification` already has `Enter`/`Exit` variants for exactly this). Delete `TmuxEvent`.
2. Add a `tmux_parser: Option<ControlParser>` field to `dcs::Handler` (our `State` is a unit
   enum, so the parser lives on the Handler — the Rust analog of upstream's `State.tmux`
   payload). `hook` `[1000]p`: `self.tmux_parser = Some(ControlParser::new())`, return
   `Command::Tmux(Notification::Enter)`. `put`/`State::Tmux`: `match self.tmux_parser.as_mut()?
   .put(byte) { Ok(Some(n)) => Some(Command::Tmux(n)), Ok(None) => None, Err(BufferOverflow)
   => None }` (broken parser then drops; document the divergence from upstream's error
   propagation — we don't want a panic path). `unhook`/`State::Tmux`: `self.tmux_parser =
   None`, return `Command::Tmux(Notification::Exit)`.
3. `stream.rs` dcs dispatch (~1056): add `dcs::Command::Tmux(n) => self.handler.tmux(n)`. Add a
   `pending_tmux: Vec<Notification>` field to `TerminalHandler` + a `fn tmux(&mut self, n)` that
   pushes, and a `pub fn take_tmux_notifications(&mut self) -> Vec<Notification>` drain accessor
   (additive event seam, mirroring `pending_clipboard`/`take_clipboard`). The app-tails Viewer
   drains it. Enter/Exit flow through too so the consumer can create/tear-down.
4. Tests: a stream/dcs test feeding `\eP1000p` then `%output %1 hi\n` … then ST, asserting the
   drained notifications are `[Enter, Output{1,"hi"}, Exit]`. Port `dcs.zig`'s
   "tmux enter and implicit exit" test. Add fuzz-dict tokens. NOT a differential-oracle surface
   (lib-vt tmux is build-gated off in our reference) → unit tests are the referee.
5. Update `feature-coverage.md` line "tmux control mode" from `[ ]` → `[~]` (engine parsers +
   DCS wiring done; native Viewer = app-tails slice 5) OR `[x]` with a note that the Viewer is
   app-tails — pick per how you read the checkbox; leave an Inbox note in `app-tails.md` handing
   off slice 5 (Viewer) with the `take_tmux_notifications` seam + the `tmux::{ControlParser,
   Layout, Variable, ...}` API.

After slice 4: vt-tails' tmux work is COMPLETE; only slice 5 (app-tails Viewer) remains.

## Completed (VT-completeness tail — CLOSED before the tmux reopen)

- #241 XTGETTCAP full terminfo table (268 caps + TN/Co/RGB); #244 XTWINOPS extra-param guard
  with title-stack verified no-op; #249 six VT config-toggle engine seams; #250 closeout
  (port-status recert, tertiary-DA parity corpus). DECRQSS + OSC 21 confirmed at parity.
  Full detail: PR bodies + `docs/port-status.md` recertification note (2026-07-14).

## Log

- 2026-07-14: session 1 — VT-completeness tail: audit (3 agents) → shipped #241, #244, #249,
  #250 (all self-merged gate-green). VT section → all `[x]`/`[—]` except tmux. Recertified
  port-status. jj lesson saved to memory ([[jj-new-before-next-pr]]).
- 2026-07-14: Josh un-gated tmux control mode. Scoped upstream `src/terminal/tmux/` (4,363
  LoC: control 839 / layout 638 / output 590 / viewer 2,283). Wrote **ADR 004** (layering:
  engine parsers = vt-tails, Viewer = app-tails; 5 PR slices). #255 merged.
- 2026-07-14: Josh confirmed ADR recommendations → ADR 004 ACCEPTED (always-compiled/
  runtime-inert; Viewer = app-tails). Slice 1 — ported `control.zig` → `src/tmux/control.rs`
  (`ControlParser` state machine + `Notification` enum); the oniguruma matchers are hand-rolled
  byte scanners (no regex dep). 26 tests (18 ported + 8 edge: idle/broken/exit/overflow/greedy
  splits). #257 merged.
- 2026-07-14: slice 2 — ported `layout.zig` → `src/tmux/layout.rs`: recursive-descent
  `Layout::parse` (pane / `{}`-horizontal / `[]`-vertical tree via a byte-offset scanner) +
  `parse_with_checksum` + `Checksum` (u16 rotate-add, 4-hex-digit). Rust ownership replaces
  upstream's arena. 24 tests (all ported: nesting, every syntax error, checksum vectors incl.
  tmux's `bb62`). #259 merged.
- 2026-07-14: slice 3 — ported `output.zig` → `src/tmux/output.rs`. Zig's comptime
  `FormatStruct`/`parseFormatStruct` become a runtime port: a 32-variant `Variable` enum
  (with `name`/`kind`/`parse`), a `Value` enum (Bool/Usize/Str), `format(vars, delim)` →
  request string, and `parse_format(vars, s, delim) -> Vec<Value>` positionally aligned with
  the vars. Zig's per-variable InvalidCharacter/Overflow collapse to one parse-failure
  (parseFormatStruct did the same); MissingEntry/ExtraEntry/FormatError preserved. 23 tests.
  (`tmux::ParseError` re-export dropped — `layout` and `output` both define one; use the
  module-qualified names.) #261 merged.
- 2026-07-14: slices 1–3 all merged (the three pure tmux parsers on main). **Recycling** with
  the detailed slice-4 (DCS-seam) design above — context is long and slice 4 is integration
  work (dcs.rs state machine) that deserves a fresh session. **Respawn to continue:** read
  `docs/adr/004-tmux-control-mode.md` + this status file and execute slice 4.
- 2026-07-14: Josh committed to **full tmux (slices 4+5)** + asked to recycle into a fresh
  session. Recorded the decision in ADR 004; routed **slice 5 (native Viewer, ~2,283 LoC)** to
  app-tails' Inbox with the engine API surface. Cleaned the vt-tails workspace (forgot all
  merged `vt-tails/*` bookmarks → dangling pre-merge commits auto-abandoned; working copy reset
  to empty-on-main). Workspace KEPT (purpose: slice 4). **Fresh session:** `cd work/vt-tails &&
  claude` → read ADR 004 + this file → do slice 4.
- 2026-07-14: **slice 4 MERGED (#271)** — done by a parallel background agent in `work/vt-slice4`
  (spawned alongside an app-tails PR-VT-toggles agent). `dcs::Command::Tmux(Notification)`
  replaces the old `TmuxRaw`; `hook`/`put`/`unhook` drive an on-Handler `ControlParser`;
  `stream::TerminalHandler` gains `pending_tmux` + `take_tmux_notifications()`. Documented
  divergence: `BufferOverflow` in the put arm → `None` (parser self-breaks; no panic path).
  Gate green + CI (one unrelated termio timing flake re-run green). **vt-tails' entire tmux
  engine port is COMPLETE (slices 1–4).** Only slice 5 (app-tails Viewer) remains.
