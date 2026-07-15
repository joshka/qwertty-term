# Scroll-region fast path: `cursor_scroll_region_up` (port of upstream 77190bd02)

Commit-stamped analysis, 2026-07-14 (perf thread). Ported against upstream Ghostty at our
frozen pin `2da015cd6`; the divergence referee is `cargo test -p vt-diff --features reference`.

## Context

The three-way vtebench scoreboard (`docs/benchmarks/vtebench-baseline.md`) left the four
region-scroll suites (`scrolling_{top,bottom}_{,small_}region`) as the last family where
Ghostty **main** still edged qwertty-term (1.13–1.20×). #204 had already closed the worst of
it (1.27–1.47 → 1.13–1.20) by porting **part 1** of upstream `77190bd02`, but routed the
region scroll through the existing `PageList.eraseRowBounded` machinery rather than upstream's
bespoke single-page rotate (**part 2**, `Screen.cursorScrollRegionUp`). The residual overhead
was that generic path's per-scroll bookkeeping.

## What upstream 77190bd02 actually contains (two changes)

1. **`no_scrollback` scrollback-skip.** `index()` routed *any* top-anchored (`top == 0`)
   full-width region through `cursorScrollAbove()` (which creates scrollback). 77190bd02 skips
   that on screens that don't retain scrollback (the alt screen), using the in-place region
   scroll instead — each avoided scroll saves a `PageList.grow()` + amortized page pruning
   (a 512 KB memset per recycled page). Wins `scrolling_bottom_*` (1.05–1.49×) and alt
   full-screen scrolling (1.25×).
2. **`cursorScrollRegionUp`.** A specialized in-place rotate for the region-scroll hot path
   (cursor on the bottom row of a full-width region), replacing `eraseRowBounded`'s per-scroll
   Point→Pin resolution, cursor re-resolution, and `manualStyleUpdate`. Wins `scrolling_top_*`
   (1.23–1.24×).

## The frozen-pin constraint: only change 2 is portable

`77190bd02` landed **~12 hours after** our frozen pin `2da015cd6` (both 2026-07-06). Our
reference oracle (libghostty-vt) is built at the pin and therefore does **not** have change 1.

Change 1 is a **semantic** change relative to the pin, not a pure optimization: on a
no-scrollback screen with a top-anchored region whose bottom is above the last row, the pin's
`cursorScrollAbove` pushes the scrolled-out rows into (transient) scrollback, and the reference
retains them. The generative differential sweep caught this immediately — porting change 1
produced 122 divergences (rows discarded by the in-place path that the oracle keeps, e.g. a
`\x1b[1;2r` region scrolled on a `max_scrollback=0` screen: ref `A\nB\nC\nD` + 2 scrollback
rows vs ours `C\nD` + 0). Adopting it would require a **pin bump**, which the mission
explicitly excludes ("port the ideas as new perf work, NOT a re-pin").

**We port only change 2**, and only for its semantics-preserving domain:

- `top != 0`, full-width, **zero blank** → `cursor_scroll_region_up` (the fast path). This is
  the exact domain the old `eraseRowBounded` routing covered; the result is bit-identical.
- `top == 0` full-width → unchanged (`cursor_scroll_above`, matching the pinned oracle).
- left/right margins **or a non-zero SGR-bg blank** → unchanged (`scroll_up(1)`). The fast
  in-place rotate does not fill the blank and, critically, handles a wide-spacer-head at the
  region boundary differently from `scroll_up` for a non-zero blank — the differential sweep
  flagged exactly this (the reference kept a trailing spacer-head blank cell after `世世` that
  the fast path dropped). Restricting the fast path to a zero blank keeps it bit-identical to
  `eraseRowBounded` and leaves the non-zero case on the proven `scroll_up`.

## Implementation

- `page/page_impl.rs`: `Page::rotate_rows_once_left` (mirror of the existing `_right`).
- `pagelist/mod.rs`: `PageList::shift_tracked_pins_region_up` — the viewport-cache + tracked-pin
  shift for an in-page region scroll, skipping the cursor pin (which stays on the region
  bottom), mirroring the in-page block of `eraseRowBounded`.
- `screen/mod.rs`: `Screen::cursor_scroll_region_up` (single-page fast path: clear top row +
  `rotate_rows_once_left` + pin shift + direct cursor-pointer refresh) and
  `cursor_scroll_region_up_slow` (cross-page: reuse `eraseRowBounded`, restore the cursor pin).
  The erased row is cleared with a bulk `write_bytes` zero when it has no managed memory and no
  kitty placeholder (bit-identical to `clear_cells` in that case), else via `clear_cells`
  (releasing managed memory + recomputing row flags) — matching `eraseRowBounded` exactly.
- `terminal/mod.rs`: `index()` routes the `top != 0`, full-width, zero-blank case to the new
  fast path.

The win over `eraseRowBounded`: no `self.pin(pt)` Point→Pin walk (uses `cursor.page_pin`), no
`cursor.y -= 1; cursor_down(1)` re-resolution, no `manual_style_update` (the cursor pin never
changes page so its style ref stays valid), and a bulk zero fill instead of a per-cell clear.

## Numbers (engine-only, `profile_streams scroll-region`, M2 Max, release, medians)

| grid   | before (eraseRowBounded) | after (cursor_scroll_region_up) | change |
| ------ | ------------------------ | ------------------------------- | ------ |
| 80×24  | ~48.5 MiB/s              | ~70.8 MiB/s                     | +46%   |
| 120×40 | ~45.5 MiB/s              | ~63.0 MiB/s                     | +38%   |

Whole-app vtebench A/B (same machine + session, clean-main parent `6dc92761` vs this change,
per-sample medians; the machine had GUI/WindowServer contention that inflates the render-heavy
region suites *equally on both builds* — so read the ratio, not the absolute ms):

| suite                                | main med | this med | ratio                 |
| ------------------------------------ | -------- | -------- | --------------------- |
| scrolling_top_region                 | 87.0 ms  | 54.0 ms  | **0.62**              |
| scrolling_top_small_region           | 74.0 ms  | 68.0 ms  | 0.92                  |
| scrolling_bottom_region              | 74.0 ms  | 71.0 ms  | 0.96 (unchanged path) |
| scrolling_bottom_small_region        | 76.5 ms  | 71.0 ms  | 0.93 (unchanged path) |
| scrolling_fullscreen                 | 67.0 ms  | 63.0 ms  | 0.94 (unchanged path) |
| scrolling / dense / medium / unicode | —        | —        | 0.86–1.00 (flat)      |

`scrolling_top_region` (the suite change 2 targets) is ~1.6× faster whole-app; the top_small
and the unchanged bottom/fullscreen suites are flat within noise. No regression. The absolute
region-scroll ms are ~3–4× the 2026-07-13 scoreboard purely from current machine GUI load
(both builds show it), not a code change — re-run the three-way scoreboard on a quiet machine
before refreshing the published table.

## Follow-up: the bottom-region suites remain (frozen-pin blocker)

`scrolling_bottom_region` / `_small_region` are `top == 0` regions on the alt screen; closing
their ~1.20× gap needs **change 1**, which is post-pin semantics. Options are (a) a pin bump to
`>= 77190bd02` (Josh's call — moves the frozen oracle forward), or (b) optimizing the
scrollback-creating `cursor_scroll_above` path itself without changing its result (separate,
larger PR). Flagged in `docs/threads/status/perf.md`.
