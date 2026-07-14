# vt-tails status

- **Current item:** **CLOSED — VT-engine completeness tail is fully green. The Terminal/VT
  section of `docs/feature-coverage.md` is all `[x]`/`[—]` except tmux (deferred, Josh-gated).**
- **Last merged:** #249 (config-toggle seams), 2026-07-14. Also #241 (XTGETTCAP), #244 (XTWINOPS).
- **Blockers:** none.
- **Claims:** none.
- **Inbox:** (other threads append requests here; owner triages into backlog)

## Handoff

### What shipped this thread (succeeding T5's lib-layer parity work)

- **#241 — XTGETTCAP full terminfo table.** Ported the entire ghostty terminfo Source
  (268 caps + `TN`/`Co`/`RGB` specials) into `crates/qwertty-term-vt/src/terminfo.rs`, with a
  byte-faithful `xtgettcapMap` value encoding (string caps with a `%` parameter returned
  verbatim in terminfo source form; otherwise `\E`→ESC and a leading `^X`→control byte). `TN`
  stays `qwertty-term` (trademark). Rows are generated from upstream by
  `crates/qwertty-term-vt/scripts/gen_terminfo.py`. XTGETTCAP is a termio-layer reply (the
  lib-vt oracle ignores DCS) so it's verified by 11 unit tests, not differential corpus.
- **#244 — XTWINOPS extra-param guard.** Report ops 14/16/18/21 t now require
  `params.len()==1` (upstream `stream.zig:2003-2030` drops extra-param report ops). Title
  stack 22/23 t confirmed a faithful apprt-level no-op (upstream lib-vt has no title-stack
  storage). Unit test + `corpus/xtwinops_size/title_extra_params_ignored` (agrees vs oracle).
- **#249 — VT config-toggle engine seams.** Added `TerminalHandler::set_title_reporting(bool)`
  (gates `CSI 21 t`; default true = oracle parity, app sets to config `title-report` which
  upstream defaults false per `Surface.zig:983`) and `Terminal::set_kitty_graphics_size_limit`
  (all screens; port of `Terminal.zig:3243`). The other four toggles already had seams:
  `set_enquiry_response`, `set_osc_color_report_format`, `Options::max_scrollback`,
  KAM mode 2 via `Terminal::modes`. Seam map handed to app-tails' Inbox (they wire the keys).
- **DECRQSS / OSC 21** — verified already at full parity (DECRQSS SGR/DECSCUSR/DECSTBM/DECSLRM
  #27; OSC 21 kitty color set/reset/query #28); corrected the stale feature-coverage checkboxes.
- **Tertiary DA parity** — reverified the stream-handler delta's flagged `CSI = c` divergence:
  our `DECRPTUI` reply agrees with the oracle; locked in via `reply_diffing/tertiary_da_probe`
  (in this closeout PR). Recertification note + totals appended to `docs/port-status.md`;
  `docs/analysis/stream-handler-delta.md` banner marks the old table historical.

### What remains (NOT this thread's — routed)

- **tmux control mode** — DEFERRED, Josh-gated. Do NOT start without Josh's call. Seamed in
  `dcs.rs` (`TmuxRaw`); `docs/analysis/stream-handler-delta.md` line 166.
- **App/renderer seams** flagged by the delta (mode-5 reverse redraw, 2026 sync-timer,
  1004/2048 initial reports on enable, linefeed mode 20, cursor-blink-12 config interplay) —
  these are app-tails / renderer territory, not VT-engine core.
- **#178 DECCOLM-with-prompt scrollback push** — T5 handed to T1 (needs a PageList
  `promptIterator`, T1 territory). Not reopened here.
- **Config-key wiring** for the six toggles — app-tails (Inbox note left in `app-tails.md`).

### How a fresh thread resumes

The tail is drained. If reopened, the only in-territory VT work would be optional harness
strengthening (more oracle dims / sweep vocabulary — low bug-yield per T5) or, on Josh's
go-ahead, tmux control mode. Verify against `~/local/ghostty` @ `2da015cd6`; the differential
oracle (`cargo test -p vt-diff --features reference`, ref lib per AGENTS.md) is the referee.

## Log

- 2026-07-14: session 1 — workspace `vt-tails` off main; read AGENTS.md, threads/README,
  T5 handoff; ran 3 parallel audit agents (XTWINOPS/title, XTGETTCAP/DECRQSS, OSC21/toggles).
- 2026-07-14: shipped #241 (XTGETTCAP), #244 (XTWINOPS), #249 (config-toggle seams) — each
  self-merged gate-green (own territory). feature-coverage VT section → all `[x]`/`[—]` bar tmux.
- 2026-07-14: jj hazard (recorded so it isn't repeated): twice I skipped `jj new` after a push,
  so the next PR's edits commingled into the just-pushed change; the squash-merge fetch then
  flagged divergence, and one rebase surfaced a stale-base revert of a sibling's `handoff.md`
  and a clobber of app-tails' real status file. All recovered losslessly (rebuild off
  main@origin, restore paths, resolve conflicts append-only). Lesson: **always `jj new
  main@origin` before starting the next PR's edits.**
- 2026-07-14: CLOSED — recertified `docs/port-status.md` (Terminal engine → only tmux remains;
  checklist recount 116 `[x]` / 16 `[~]` / 33 `[ ]` / 1 `[—]`). Respawn only for tmux (Josh's
  call) or optional harness work.
