# dense_cells / print-path port gaps vs upstream perf sprint

> **EXECUTED (2026-07-13).** This was the *work plan*; the gaps below have since
> been closed. The wide-class `print_slice` fill (gap #1's missing half), the
> bulk style-only run fill (#2), and per-run `release_multiple` in clear paths
> (#4) all shipped. Result: `dense_cells` went from a 2.29× loss to a **0.64–0.78×
> whole-app win** vs Ghostty main (`docs/benchmarks/vtebench-baseline.md`). The
> wide fill also narrowed the CJK/wide *engine* gap from ~7× to ~2.6× — but note
> the `unicode` 0.50× whole-app number is a render-pipeline artifact, not an
> engine lead (engine-only Ghostty is still faster on wide; see
> `docs/analysis/stream-throughput-vs-upstream.md`). The "Partial/Absent" status
> column below is the *pre-work* state, retained for the file:line map; don't read
> it as current.

Recorded 2026-07-11 (T1, sub-agent deep-read; upstream `~/local/ghostty` at pin
`2da015cd6`, port at main `eee56f7f26e5`). Companion to
`upstream-perf-1.3.1-to-main.md` — this is the file:line-level gap map for the four
print/style commits, i.e. the work plan for the `dense_cells`/`medium_cells`/
`sync_medium_cells` vtebench losses vs Ghostty main.

**Pin context:** `47e26df60` and `cee35cabf` landed inside PR #13220 whose merge IS
our pin, so the port already reflects them. `cb2d78587` and `8d663a76e` are same-day
follow-ups AFTER the pin — porting them needs verification against
`~/local/ghostty-main`, but both are pure perf (ref-count batching / vector scans),
no observable-semantics change.

| #   | upstream commit | optimization                                                                                                                | port today                                                                                                                                                               | status                                                                                                                                                                                                                                                                           |
| --- | --------------- | --------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | `47e26df60`     | batch printable runs into direct row fills (narrow **and** wide classes)                                                    | `terminal/print.rs:133` `print_slice`, `:150` `print_slice_fast_narrow`, `:219` `print_slice_fill_narrow`; scan side `stream.rs:133` SWAR + `:656` `dispatch_ground_run` | **Partial** — narrow class fully present (guards mirrored, batched row-fill, once-per-row dirty/cursor); **wide class absent**: `print.rs:211` returns 0 for width≠1 so CJK/emoji fall back to per-codepoint `print` (`print.rs:142`). Upstream got 3.4x CJK from the wide fill. |
| 2   | `cb2d78587`     | vectorized eligibility + simple-cell scans; **bulk style-only run fill** (one `releaseMultiple`/`useMultiple` pair per run) | eligibility scan scalar `print.rs:230–254`; simple-cell scan scalar `print.rs:303–310`; style fixup **per cell** `print.rs:331–349`                                      | **Absent.** `ref_set.rs:410` `use_multiple` / `:470` `release_multiple` already exist (used once at `terminal/mod.rs:1459`) — wiring them into the print fill is the biggest styled-redraw lever (upstream +21% TUI redraw).                                                     |
| 3   | `cee35cabf`     | skip style-map update when SGR leaves style unchanged                                                                       | `terminal/mod.rs:1540–1543` equality short-circuit before `manual_style_update`                                                                                          | **Present.**                                                                                                                                                                                                                                                                     |
| 4   | `8d663a76e`     | release style refs per RUN in clearCells                                                                                    | `page/page_impl.rs:1225–1231` releases **per cell**                                                                                                                      | **Absent** — group consecutive equal `style_id` runs, one `release_multiple` per run (upstream 2.1x on styled-paint+ED2).                                                                                                                                                        |

Suggested PR slicing (each with vtebench dense/medium/sync_medium + corpus + release
lane evidence):

1. **clear_cells per-run release** (#4) — smallest, self-contained in
   `page_impl.rs`, exercised by every ED2/EL under style.
2. **bulk style-only fill in print_slice_fill_narrow** (#2's third change) — the
   dense_cells lever; the two vector scans can follow separately if profiling still
   shows the scalar scans (Rust autovectorization may already cover part).
3. **wide-class print_slice fill** (#1's missing half) — the utf8-mixed/CJK lever;
   largest (needs spacer head/tail pair fills + right-edge handling mirrored from
   `printSliceFill(.wide)`).

DOOM-fire note: its stream is `▀` (U+2580, >0xFF, narrow) with bg/fg SGR churn —
today it takes the >0xFF eligibility path per `print_slice_fast_narrow`'s clustering
guards; the bulk style-only fill (#2) is the item most likely to move it.
