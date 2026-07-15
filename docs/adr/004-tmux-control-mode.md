# ADR 004: tmux control mode — scope, layering, and PR slices

- Status: **PROPOSED** (Josh un-gated the work 2026-07-14: "start tmux control mode"). This
  ADR fixes the layering and slice plan; the engine-parser slices (1–3) are unambiguous and
  proceed immediately. The two decisions that want Josh's confirmation are flagged under
  "Open questions" (build-gate default; viewer ownership split with app-tails) — they do not
  block the parser work.
- Date: 2026-07-14
- Thread: vt-tails (VT engine) · succeeds T5 · Spec: `docs/threads/status/vt-tails.md`
- Upstream: Ghostty `2da015cd6`, `src/terminal/tmux/` (4,363 LoC) + `src/termio/stream_handler.zig`
- Confidence: **high** on the engine/app layering and the parser slices; **medium** on the
  viewer ownership boundary (needs an app-tails coordination call).

## Context

tmux control mode (`tmux -CC`) is the last open item in the Terminal/VT-engine section of
`docs/feature-coverage.md` (everything else is `[x]`/`[—]`). In control mode, tmux emits a
line-oriented control protocol on the pty instead of a normal screen; a supporting terminal
parses it and renders tmux windows/panes as **native** tabs/splits rather than tmux's own
text UI.

Upstream splits this across four files (`src/terminal/tmux/`, 4,363 LoC total):

| File          | LoC   | Role                                                                                                                                                      | Layer            |
| ------------- | ----- | --------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------- |
| `control.zig` | 839   | Control-mode **Parser**: byte stream → structured `Notification`s (`%begin`/`%end`/`%output`/`%window-add`/`%layout-change`/…). Transport-agnostic, pure. | **VT engine**    |
| `layout.zig`  | 638   | tmux **layout** string parser (`Layout` tree: window/pane geometry). Pure.                                                                                | **VT engine**    |
| `output.zig`  | 590   | `%output` payload parsing + command **encoding** (bytes to send back to tmux). Pure.                                                                      | **VT engine**    |
| `viewer.zig`  | 2,283 | **Viewer**: maps parsed notifications → native surfaces (tabs/panes), owns per-window `Terminal`s, drives the app.                                        | **app / termio** |

Entry is already seamed on our side: the DCS `\ePtmux;<escaped>\e\\` sequence parses to
`dcs::Command::TmuxRaw` (`crates/qwertty-term-vt/src/dcs.rs`) — currently a drop. Upstream
gates the whole feature behind a compile-time `terminal.options.tmux_control_mode` build flag
and the termio `stream_handler` owns the `?*Viewer`, creating it on the `.tmux .enter`
notification and feeding subsequent control bytes to the `ControlParser`.

## Decision

**Port the three pure protocol parsers into `crates/qwertty-term-vt` (vt-tails); leave the
Viewer + termio/app wiring to app-tails/termio.** The engine exposes a transport-agnostic
`tmux::ControlParser` that turns pty bytes into an owned `Notification` enum, plus `Layout`
parsing and `output` encode/parse. The consumer (termio/app) decides how to connect, creates
surfaces, and renders — exactly upstream's split (`control.zig` is "fully agnostic to how the
data is received and sent"; `viewer.zig` is the app-facing half).

### Layering

```text
pty bytes ──▶ Stream/DCS (TmuxRaw seam) ──▶ tmux::ControlParser ──▶ Notification
                                                   │                     │
                                          layout::Layout          output::{parse,encode}
                                                   ▼                     ▼
                                    ┌─────────────── app-tails / termio ───────────────┐
                                    │  tmux::Viewer: Notification → native surfaces,    │
                                    │  per-window Terminal, tab/split management        │
                                    └───────────────────────────────────────────────────┘
```

vt-tails owns everything above the dashed line; app-tails/termio owns the Viewer and the
`stream_handler`-equivalent wiring. Coordinate the Viewer boundary via app-tails' Inbox.

### PR slices (each gated, each with ported upstream inline tests)

1. **`tmux::ControlParser`** (`control.zig`) — the notification state machine
   (idle/notification/block, `%begin…%end` blocks, `max_bytes` broken-state guard) and the
   `Notification` enum. Unit-tested against upstream's inline tests. Foundational; no oracle
   (control mode is not a VT reply the libghostty-vt differential harness models — unit tests
   are the referee, like XTGETTCAP/DECRQSS).
2. **`tmux::layout::Layout`** (`layout.zig`) — layout-string parser + tree. Pure, unit-tested.
3. **`tmux::output`** (`output.zig`) — `%output` parse + command encode. Pure, unit-tested.
4. **Wire the `TmuxRaw` DCS seam** → feed control bytes into `ControlParser`, expose the
   `Notification` stream on the engine's reply/event surface for the consumer to drain
   (additive, mirrors how clipboard/notification events are seamed today). Corpus/round-trip
   tests for the DCS entry.
5. **Viewer + termio wiring** — app-tails/termio (native surfaces). NOT vt-tails; routed via
   Inbox once slices 1–4 land.

### What we port faithfully vs defer

- Faithful: the parser state machines, `Notification`/`Layout` shapes, `max_bytes` guard,
  and byte-exact command encoding (verify against `~/local/ghostty` `2da015cd6`, cite file:line).
- Zig-port hazards apply (AGENTS.md): `assert`-in-ReleaseSafe (no side effects in
  `debug_assert!`), truncation semantics, zero-capacity guards. `control.zig` uses
  `oniguruma` regex — port those matchers to our regex stack or hand-rolled scanners; note any
  divergence.
- Deferred (Josh's call, not this thread): the native Viewer UX (how tmux windows map to
  qwertty-term tabs vs splits) is an app-design decision for app-tails.

## Open questions (need Josh / app-tails, do not block slices 1–3)

1. **Build-gate default.** Upstream compiles tmux control mode behind
   `tmux_control_mode`. Do we ship it always-on, behind a Cargo feature, or behind a TOML
   config key? Recommendation: always-compiled (it's pure Rust, no heavy deps beyond a regex
   crate we already have), runtime-inert until a `\ePtmux;` entry arrives — simplest and
   matches how the rest of the engine ships. Confirm.
2. **Viewer ownership.** Slice 5 (native surfaces) is app-tails territory. Confirm app-tails
   takes it once the engine parsers land, or whether vt-tails carries a headless reference
   consumer (betamax-style) for testing.

## Consequences

- Closes the last VT-engine feature-coverage item once slices 1–4 land; slice 5 is app-side.
- ~2,067 LoC of pure parser port into vt-tails, unit-tested (no differential-oracle surface).
- New `tmux` sequence family → add fuzz-dictionary tokens for `\ePtmux;` + control-mode lines.
