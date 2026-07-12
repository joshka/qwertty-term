# Upstream perf sprint, 1.3.1 → main@91f66da24

Recorded 2026-07-11 (T1), against upstream checkout `~/local/ghostty`. The three-way
vtebench table (`docs/benchmarks/vtebench-baseline.md`) shows Ghostty main is 1.5–2.7×
faster than its own 1.3.1 on the cell-heavy and region-scroll suites — enough to flip
our 9/10-wins scoreboard to tie-or-lose. That jump is a July 2026 perf sprint of public
commits (all Mitchell, PRs #13209–#13245). This doc names them and maps each onto our
backlog so we mirror deliberately instead of rediscovering.

**Pin note:** our semantics reference is pinned at `2da015cd6`, which IS the merge of
batch #13220 — the reference we ported against already contains that batch's semantics
(not its speed). Everything after it (#13226, #13227, #13231, #13237, #13245) is
post-pin; porting those needs source-verification against `~/local/ghostty-main`.

## The commits

| PR     | commit      | what it does                                                                                                  | our analog / owner                                                                                                              |
| ------ | ----------- | ------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| #13209 | `2f0e6659d` | pipeline pty reads to overlap parsing with draining (25–55% IO)                                               | termio read loop — app crate (T4 territory, inbox them)                                                                         |
| #13220 | `47e26df60` | batch printed codepoint runs into direct row fills (~85% of plain-text wall time was per-cp `Terminal.print`) | T1 `dense_cells`/`utf8-mixed` items — our SWAR fast path is the same idea for ascii; theirs also covers the row-fill write side |
| #13220 | `253e4f9c3` | bulk-parse CSI parameter bytes at the slice level                                                             | T1 `sgr/cursor dispatch` item                                                                                                   |
| #13220 | `1a88f3622` | dispatch CSI finals directly from stream fast paths (skip `[Option<Action>;3]`-style materialization)         | T1 `sgr/cursor dispatch` item                                                                                                   |
| #13220 | `cee35cabf` | skip style map update when SGR leaves style unchanged                                                         | T1 `dense_cells` item (style churn)                                                                                             |
| #13226 | `cb2d78587` | fill style-only cell runs in bulk in `printSliceFill`                                                         | T1 `dense_cells`                                                                                                                |
| #13226 | `8d663a76e` | release style refs per run instead of per cell in `clearCells`                                                | T1 `dense_cells` / clear paths                                                                                                  |
| #13226 | `300f42c7a` | handle CSI entry bytes inline in `consumeUntilGround`                                                         | T1 `sgr/cursor dispatch`                                                                                                        |
| #13226 | `083d9709b` | decode ASCII inline in the SIMD scan for ESC                                                                  | T1 — extends our SWAR scan                                                                                                      |
| #13227 | `446f80f4e` | render-state update: ~2.7–11× less terminal-lock hold per frame                                               | renderer snapshot path (T2 territory, inbox)                                                                                    |
| #13231 | `77190bd02` | scroll-region optimizations (incl. no scrollback for top-anchored regions on non-retaining screens)           | T1 — explains our 1.20–1.47× region-scroll losses                                                                               |
| #13237 | `bb0ac4c72` | don't bridge pty reads while the parser is idle                                                               | termio (T4)                                                                                                                     |
| #13245 | `896aca499` | return free-listed page memory to the OS                                                                      | memory hygiene, not throughput (T5/T1 later)                                                                                    |

## Reading order for the T1 items

1. `47e26df60` + `cb2d78587` + `cee35cabf` + `8d663a76e` — the dense/medium/sync_cells
   package. Target files upstream: `src/terminal/Terminal.zig` (`print`,
   `printSliceFill`, `clearCells`), stream fast paths.
2. `253e4f9c3` + `1a88f3622` + `300f42c7a` — the CSI/SGR dispatch package
   (`src/terminal/stream.zig`, `Parser.zig`).
3. `77190bd02` — scroll regions (`src/terminal/Terminal.zig` `index()`/region moves).
4. `083d9709b` — ASCII-in-ESC-scan (our `simd` module analog).

Each port keeps our differential-corpus gate: semantics frozen, corpus + release lane
referee, upstream `file:line` cited in the PR.
