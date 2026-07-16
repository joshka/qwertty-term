# mirror-verify status

- **Current item:** **CLOSED — mission complete.** All 17 SHAs dispositioned: **16 VERIFIED**;
  **1 MIRRORED** (`a55850c98`). Gap-closure PR **#316 MERGED** (`bc7bf326`, tip of main;
  verified ancestor — no orphaning). Backlog drained. Respawn only if the 2 test-only
  follow-ups (`30b42f42a`, `e6e4a9fdc`) are wanted — code is already verified correct.
- **Last merged:** #316 — a55850c98 print-path OOB fix, plus 0aaedf436/afbf5ba15 regression
  tests and the 627518447 Phase-2 note; merged 2026-07-16.
- **Blockers:** none
- **Claims:** none (dropped on merge of #316).
- **Inbox:** (other threads append requests here; owner triages into backlog)

## Disposition table (17 SHAs across 5 clusters) — all dispositioned

Referee note: all 17 SHAs post-date the current differential oracle pin `77190bd02`, so the
oracle carries the *same pre-fix behavior* — differential green cannot referee these. They
are panic/overflow/assert/OOB fixes; the referee is upstream-fix reading + Rust code + a
targeted unit test (matching how the existing mixed-width-page regressions are guarded).

| Cluster | SHA       | Upstream fix (short)                          | Disposition                                     |
| ------- | --------- | --------------------------------------------- | ----------------------------------------------- |
| 1       | afbf5ba15 | scroll_prompt minimum delta (@abs)            | VERIFIED — `unsigned_abs` ops.rs:474; +test     |
| 1       | c753fe4a4 | scroll_delta_row minInt negation              | VERIFIED — `unsigned_abs` ops.rs:403+; test✓    |
| 1       | 30b42f42a | reject unrepresentable pin coords             | VERIFIED — `checked_add?` pin.rs:437+ (no test) |
| 1       | e6e4a9fdc | widen cell screen coords u16→u32              | VERIFIED — u32 accumulator ops.rs:63 (no test)  |
| 1       | 0aaedf436 | saturate origin cursor offsets                | VERIFIED — `saturating_add` mod.rs:880; +test   |
| 2       | fedd42e8d | backward-shift deletion in page maps          | VERIFIED — offset_map.rs:562 (#297); tests✓     |
| 2       | 65f953e8e | no-clobber insert + rehash (load factor)      | VERIFIED — no-clobber move (#303); superseded   |
| 3       | b6f34be44 | clamp mirrored selection corners              | VERIFIED — selection.rs:262 (#147); test✓       |
| 3       | 607160657 | clamp cloned selections to page width         | VERIFIED — mod.rs:330 (#150); test✓             |
| 3       | a9f5b7eba | clamp selection rows to page width            | VERIFIED — selection.rs:373 (#151); test✓       |
| 3       | a55850c98 | previous page width for cursor cells          | **MIRRORED** — print.rs OOB fix + test          |
| 3       | fa8cae88b | destination width for line selection          | VERIFIED — mod.rs:1965 (#153); test✓            |
| 3       | 0c299000f | preserve aliased selection pins (double-free) | VERIFIED — deinit_preserving (#81); test✓       |
| 4       | 5d8eb78b7 | search reset pins while feeding               | VERIFIED — pagelist.rs:106 (#142); test✓        |
| 4       | 5bc6588e4 | search ignore empty needles                   | VERIFIED — 3 sites (#134); test✓                |
| 4       | 627518447 | search reset cached results after resize      | VERIFIED-N/A — `ScreenSearch` unported (note)   |
| 5       | b287f6d1a | grapheme stored-boundary assert (mode 2027)   | VERIFIED — `let _ =` print.rs:648 (#76); test✓  |

**MIRRORED (a55850c98):** `clear_stale_spacer_head` (the port of upstream `cursorCellEndOfPrev`)
indexed the *global* `self.cols - 1` when clearing the previous row's stale spacer_head. If
incomplete reflow left the previous page narrower, that reads a cell outside the row's logical
width (raw pointer past `get_cells`' `size.cols` slice) and leaves the real stale spacer_head
behind — upstream panicked here in runtime-safety builds. Fix: use the reached page's own
`size.cols - 1`. Regression test `clear_stale_spacer_head_uses_previous_page_width` (proven
to fail on the reverted line).

**VERIFIED-N/A (627518447):** the whole `ScreenSearch` result cache (`screen.zig`) is
Phase-2 / unported, so the stale-flattened-result crash cannot occur today. Left a Phase-2
porting note in `search/mod.rs` so the fixed shape (reset before feed/reload/**select**) is
ported, not the pre-fix code.

**Test-only follow-ups (verified-correct code, upstream regression test not yet ported):**
`30b42f42a` (point_from_pin overflow-rejection — needs ~65k metadata-only pages) and
`e6e4a9fdc` (screenPoint > u16 — needs ~307 pages). Non-urgent; code is confirmed correct.

## Log

- 2026-07-15: session start — created workspace `work/mirror-verify`, read AGENTS.md +
  threads/README.md. Established all 17 SHAs post-date the current oracle pin `77190bd02`,
  so differential green cannot referee these (oracle carries the same pre-fix behavior);
  most are panic/overflow/assert fixes → verify by reading upstream fix + Rust path +
  targeted unit test. Mapped prior ports via `gh pr list`; 3 suspected gaps flagged above.
  vt-tails backlog drained (no active claims) → low collision risk on qwertty-term-vt.
- 2026-07-15: dispositioned all 17 (4 parallel read-only verify subagents + own review).
  16 VERIFIED; 1 MIRRORED (`a55850c98`: `clear_stale_spacer_head` OOB → own-page-width fix +
  `clear_stale_spacer_head_uses_previous_page_width` regression test, proven to fail on the
  reverted line). Also ported 2 missing upstream regression tests for verified-correct code
  (`0aaedf436`, `afbf5ba15`) + a Phase-2 porting note for `627518447` (`ScreenSearch`
  unported). Full gate green: check(0w)/workspace tests/release lane/paranoid lane/fmt/clippy/
  vt-diff(+`--features reference`, 0 divergences)/markdownlint. Pushing gap-closure PR.
