# vt-tails status

- **Current item:** PR3 — VT config-toggle engine seams (title-report + image-storage-limit
  setters) + OSC 21/config-toggle checkboxes + app-tails Inbox coordination. Rebased onto
  current main; gate re-running. After this: tail drained (tmux Josh-gated) → closeout +
  port-status recert.
- **Last merged:** #241 (XTGETTCAP), #244 (XTWINOPS extra-params guard). Both on main.
- **Blockers:** none
- **Claims:** none
- **Inbox:** (other threads append requests here; owner triages into backlog)

## Mission

Drive the Terminal/VT-engine tail of `docs/feature-coverage.md` (lines 33–44) to green.
Differential oracle (`vt-diff` + `--features reference`) is the referee. Territory:
`crates/qwertty-term-vt` + `crates/vt-diff`. Do NOT touch the app crate (app-tails owns it) —
expose engine setters/accessors additively; app-tails wires config keys. Coordinate via Inbox.

Succeeds T5 (CLOSED). Audit findings (2026-07-14, three Explore agents):

- **XTGETTCAP** — was the real gap: 6 hardcoded caps vs upstream's full 268 + TN/Co/RGB.
  DONE (#241): full port from `ghostty.zig`, faithful `\E`/`^X`/`%`-verbatim encoding.
- **DECRQSS** — already FULL parity (SGR/DECSCUSR/DECSTBM/DECSLRM). Stale checkbox (fixed #241).
- **XTWINOPS / title stack** — DONE (#244): report ops 14/16/18/21 now gated on
  `params.len()==1`; title stack 22/23 confirmed correct as upstream's apprt-level no-op.
- **OSC 21** kitty color protocol — already fully implemented (#28); checkbox (PR3).
- **Config toggles** (PR3): `title-report` + `image-storage-limit` engine setters added;
  `enquiry-response`/`osc-color-report-format` (#35), `scrollback-limit`
  (`Options::max_scrollback`), `vt-kam-allowed` (mode 2 via `pub modes`) already present.
- **tmux** control mode — DEFERRED, Josh-gated, do NOT start.

## Log

- 2026-07-14: session 1 start — workspace `vt-tails` off main. Read AGENTS.md, threads/README,
  T5 handoff. Ran 3 parallel audit agents (XTWINOPS/title, XTGETTCAP/DECRQSS, OSC21/toggles).
- 2026-07-14: PR1 (#241, MERGED) — full ghostty terminfo capability table in `terminfo.rs`
  (268 caps + TN/Co/RGB), byte-faithful `xtgettcapMap` encoding; TN stays `qwertty-term`.
  Generator `scripts/gen_terminfo.py`; 11 unit tests. feature-coverage L29/L35 → [x].
- 2026-07-14: PR2 (#244, MERGED) — XTWINOPS ops 14/16/18/21 gated on `params.len()==1`
  (upstream `stream.zig:2003-2030`); unit test + differential corpus case
  `xtwinops_size/title_extra_params_ignored` (agrees vs reference). feature-coverage L33 → [x].
- 2026-07-14: PR3 — config-toggle engine seams: `set_title_reporting(bool)` (gates `CSI 21 t`,
  default true = oracle parity; app sets to config `title-report`, upstream default false per
  `Surface.zig:983`) + `Terminal::set_kitty_graphics_size_limit(usize)` (all screens, port of
  `Terminal.zig:3243`). Other four toggles already had seams. OSC 21 → checkbox. Appended
  seam-map Inbox note to app-tails.md. 2 new tests; reference corpus agrees. feature-coverage
  L37/L39 → [x].
- 2026-07-14: jj hazard note — after pushing PR2 I did NOT `jj new` before PR3 edits, so PR3
  commingled into the PR2 change; the squash-merge fetch then flagged divergence. Recovered by
  rebasing onto current main + resolving three doc/test conflicts (incl. discovering app-tails
  had created its own real status file — reconciled to append-only Inbox). No work lost.
