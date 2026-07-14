# vt-tails status

- **Current item:** XTWINOPS extra-params guard — PR up (#TBD). Next: config-toggle gaps
  (KAM/scrollback/image-storage/title-report) + OSC 21/DECRQSS checkbox verification.
- **Last merged:** #241 (XTGETTCAP full terminfo table), 2026-07-14.
- **Blockers:** none
- **Claims:** none
- **Inbox:** (other threads append requests here; owner triages into backlog)

## Mission

Drive the Terminal/VT-engine tail of `docs/feature-coverage.md` (lines 33–38) to green.
Differential oracle (`vt-diff` + `--features reference`) is the referee. Territory:
`crates/qwertty-term-vt` + `crates/vt-diff`. Do NOT touch the app crate (app-tails owns it) —
expose engine setters/accessors additively; app-tails wires config keys. Coordinate via Inbox.

Succeeds T5 (CLOSED). Audit findings (2026-07-14, three Explore agents):

- **XTGETTCAP** — was the real gap: only 6 hardcoded caps vs upstream's full 268 + TN/Co/RGB.
  DONE this session (full port from `ghostty.zig`, faithful `\E`/`^X`/`%`-verbatim encoding).
- **DECRQSS** — already at FULL parity (SGR/DECSCUSR/DECSTBM/DECSLRM). Stale checkbox only.
- **XTWINOPS / title stack** — near parity. Title push/pop are correct no-op seams (upstream
  lib-vt has no title stack; it lives in apprt). ONE real gap: ops 14/16/18/21 don't guard on
  `params.len()==1` (upstream ignores extra params). Small fix + corpus case. → PR2.
- **OSC 21** kitty color protocol — already fully implemented (#28, set/reset/query 8-bit).
  Verify + checkbox.
- **Config toggles**: `enquiry-response` ✓ setter, `osc-color-report-format` ✓ setter (both
  #35). `scrollback-limit` — engine `PageList::init(max_size)` already supports it. KAM mode 2
  (`vt-kam-allowed`), `title-report`, `image-storage-limit` — need audit-3 detail (agent still
  running at handoff); verify engine seam exists or add setter.
- **tmux** control mode — DEFERRED, Josh-gated, do NOT start.

## Log

- 2026-07-14: session 1 start — workspace `vt-tails` off main. Read AGENTS.md, threads/README,
  T5 handoff. Ran 3 parallel audit agents (XTWINOPS/title, XTGETTCAP/DECRQSS, OSC21/toggles).
- 2026-07-14: PR2 (#TBD) — XTWINOPS report ops 14/16/18/21 now gated on `params.len()==1`
  (upstream `stream.zig:2003-2030` ignores extra params); unit test + differential corpus
  case `xtwinops_size/title_extra_params_ignored` (agrees vs reference oracle). Title stack
  22/23 confirmed correct as upstream's apprt-level no-op. feature-coverage L33 → [x].
  (Note: jj reused PR1's change-id after the squash-merge fetch → divergence; rebuilt PR2
  as a fresh change off main, abandoned the divergent copy — recovered losslessly.)
- 2026-07-14: PR1 (#241, MERGED) — ported the full ghostty terminfo capability table into `terminfo.rs`
  (268 caps + TN/Co/RGB specials), byte-faithful `xtgettcapMap` encoding
  (`\E`→ESC, leading `^X`→ctrl, `%`-param strings verbatim); TN stays `qwertty-term`.
  Generator `crates/qwertty-term-vt/scripts/gen_terminfo.py`. 11 unit tests. Gate green
  (check/clippy/fmt/1542 lib debug+release/paranoid/vt-diff corpus). feature-coverage L29/L34
  → [x]. XTGETTCAP is termio-layer (no differential oracle) → unit-tested, no corpus.
