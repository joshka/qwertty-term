# vt-tails status

- **Current item:** **REOPENED for tmux control mode** (Josh un-gated 2026-07-14). Scoping
  done → ADR 004 written + slice plan. Next: **slice 1 — port `tmux::ControlParser`
  (`control.zig`, 839 LoC) + inline tests.** (This session is recycling after the ADR; a
  fresh session executes the slices with full context — boot from ADR 004 + this file.)
- **Last merged:** #250 (VT-tail closeout). VT-completeness tail is all `[x]`/`[—]` except tmux.
- **Blockers:** none (slices 1–3 are unambiguous; open questions in ADR 004 don't block them).
- **Claims:** none.
- **Inbox:** (other threads append requests here; owner triages into backlog)

## Mission (current: tmux control mode — ADR 004)

Port the **pure tmux protocol parsers** into `crates/qwertty-term-vt` (vt-tails). The native
Viewer + termio wiring is app-tails/termio (ADR 004 slice 5). Verify against `~/local/ghostty`
@ `2da015cd6`, cite file:line. tmux control mode is NOT a libghostty-vt differential-oracle
surface → **unit tests are the referee** (like XTGETTCAP/DECRQSS). Zig-port hazards apply.

### tmux backlog (ADR 004 slices — vt-tails owns 1–4; app-tails owns 5)

1. **`tmux::ControlParser`** — `control.zig` (839 LoC): idle/notification/block state machine,
   `%begin…%end` blocks, `max_bytes` broken-state guard, `Notification` enum. Uses `oniguruma`
   regex upstream → port matchers to our regex stack or hand-rolled scanners. Port inline tests.
   **← START HERE.**
2. **`tmux::layout::Layout`** — `layout.zig` (638 LoC): layout-string parser + tree. Pure.
3. **`tmux::output`** — `output.zig` (590 LoC): `%output` parse + command encode. Pure.
4. **Wire the `TmuxRaw` DCS seam** (`dcs.rs` already parses `\ePtmux;…`, currently dropped) →
   feed control bytes to `ControlParser`, expose the `Notification` stream on the engine's
   event surface (additive, like clipboard/notification seams). DCS entry tests + fuzz tokens.
5. **Viewer + termio wiring** (`viewer.zig`, 2,283 LoC) — **app-tails/termio, NOT vt-tails.**
   Route via app-tails Inbox once 1–4 land.

Open questions (ADR 004, need Josh/app-tails; do NOT block 1–3): build-gate default
(recommend: always-compiled, runtime-inert); Viewer ownership split.

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
  engine parsers = vt-tails, Viewer = app-tails; 5 PR slices). Recycling after the ADR so a
  fresh session executes slice 1 (`ControlParser`) with full context budget.
