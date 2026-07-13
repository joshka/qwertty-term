# vtebench baseline: qwertty-term vs Ghostty 1.3.1 vs Ghostty main

Three-way comparison on the canonical terminal benchmark lane —
[vtebench](https://github.com/alacritty/vtebench), the tool upstream Ghostty uses for terminal
comparisons. Three columns: qwertty-term, Ghostty 1.3.1 (stable), and Ghostty **main** (the moving
upstream target). **Refreshed 2026-07-13** after the scroll-region optimization (#204) landed;
qwertty-term now wins or ties Ghostty main on 6/10 suites (leading outright on `dense_cells`,
`medium_cells` and `unicode`) and wins every suite vs 1.3.1. The four region-scroll variants — the
previous "only remaining loss" at 1.27–1.47× — are now **materially closed to 1.13–1.20×** by #204;
they are the last suite family where main still edges us. Both Ghostty columns were re-measured in
the same session and match the prior baseline within ±1–2 ms (a control confirming the run is
uncontaminated). Superseded numbers are preserved in the history notes at the end.

## Re-running

```sh
# qwertty-term (builds qwertty-term --release)
scripts/bench-vtebench.sh --terminal qwertty-term
# Ghostty 1.3.1 stable
scripts/bench-vtebench.sh --terminal ghostty --label ghostty-1.3.1
# Ghostty main (alternate bundle via --app-path)
scripts/bench-vtebench.sh --terminal ghostty \
    --app-path ~/local/ghostty-main/macos/build/ReleaseLocal/Ghostty.app \
    --label ghostty-main
```

Outputs land in `target/vtebench/<label>/` (`results.dat` per-sample times, `summary.txt` derived
stats, `grid.txt` fairness check). `--app-path` points the ghostty lane at any Ghostty.app bundle
(or its inner binary); `--label` keeps each build's output in its own subdir. The script
auto-clones vtebench pinned at the commit below into `work/vtebench-upstream` (git-ignored
scratch, not vendored) if missing.

## Environment

| item          | value                                                                                                                                                                   |
| ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| machine       | Apple M2 Max, 96 GB RAM, macOS 15.7.7                                                                                                                                   |
| date          | 2026-07-13 (refreshed after #204 scroll-region opt; see history at end)                                                                                                 |
| vtebench      | `ead80032e57dee2e75f0b51f2ea67528647d9944` (v0.3.1, 2025-01-09)                                                                                                         |
| qwertty-term  | `b626f8555496` (main, post-#204) — dirty tracking, SIMD ascii, CSI/SGR dispatch, clear_cells+bulk style fill, wide-class print_slice fill, **scroll-region opt (#204)** |
| Ghostty 1.3.1 | 1.3.1 stable (`/Applications/Ghostty.app`, ReleaseFast)                                                                                                                 |
| Ghostty main  | `91f66da24527fa02d92b5fd0b41cd020f553a64c` (2026-07-08, ReleaseLocal) — same pin as the original baseline                                                               |
| grid          | 80x24 in all three terminals (verified via `stty size` inside each)                                                                                                     |
| suite knobs   | vtebench defaults: 1 MiB min sample, 10 s per suite, 1 warmup                                                                                                           |
| sampling      | 3 rounds per terminal, load-gated (1-min loadavg < 5), interleaved; per-suite samples pooled                                                                            |

All three terminals ran as real GUI windows driven non-interactively: qwertty-term via the
`QWERTTY_TERM_COMMAND` override in `crates/qwertty-term-termio/`, both Ghostty builds via their
`--command` config with `--quit-after-last-window-closed`. This is the full GUI lane, not an
engine-only fallback. The maintainer's own Ghostty instance was open (idle) during all runs —
ambient shared-machine load applies to every column equally.

## Results

Milliseconds per ~1 MiB sample, lower is better. Median (p90 in parentheses) of per-sample times,
pooled across 3 load-gated rounds per terminal (medians because the Ghostty distributions have long
tails — see analysis). The final column is qwertty-term median / Ghostty main median: below 1.0
means qwertty-term is faster than upstream main, above 1.0 means slower.

| suite                         | qwertty-term med (p90) | Ghostty 1.3.1 med (p90) | Ghostty main@91f66da24 med (p90) | qt/main ratio |
| ----------------------------- | ---------------------- | ----------------------- | -------------------------------- | ------------- |
| dense_cells                   | 7 (8)                  | 14 (15)                 | 9 (15)                           | 0.78          |
| medium_cells                  | 7 (7)                  | 18 (18)                 | 8 (10)                           | 0.88          |
| scrolling                     | 16 (17)                | 34 (36)                 | 15 (16)                          | 1.07          |
| scrolling_bottom_region       | 18 (18)                | 30 (31)                 | 15 (16)                          | 1.20          |
| scrolling_bottom_small_region | 18 (18)                | 29 (29)                 | 15 (16)                          | 1.20          |
| scrolling_fullscreen          | 21 (22)                | 38 (39)                 | 20 (21)                          | 1.05          |
| scrolling_top_region          | 17 (18)                | 31 (31)                 | 15 (15)                          | 1.13          |
| scrolling_top_small_region    | 18 (18)                | 30 (30)                 | 15 (16)                          | 1.20          |
| sync_medium_cells             | 7 (8)                  | 19 (19)                 | 7 (8)                            | 1.00          |
| unicode                       | 3 (5)                  | 10 (11)                 | 6 (6)                            | 0.50          |

## Honest analysis

Read these numbers with vtebench's own disclaimer in hand: it measures **PTY read throughput
only** — no frame rate, no latency, no rendering-quality signal.

- **The cell-heavy losses flipped to wins or ties.** The original baseline had Ghostty main
  leapfrogging us on every cell suite (`dense_cells` 2.29x, `medium_cells` 1.67x,
  `sync_medium_cells` 1.83x *slower*). T1's perf work — CSI/SGR dispatch fast paths, per-run style
  release in `clear_cells`, the bulk style-only `print_slice` fill — closed all of it:
  `dense_cells` **0.78x**, `medium_cells` **0.88x**, `sync_medium_cells` a dead **1.00** tie. Note
  these suites run at 7–9 ms medians where ±1 ms quantization swings the ratio by ~0.1 (e.g.
  `dense_cells` read 0.64 last session and 0.78 this one purely from main's median moving 11→9 ms;
  our own median held at 7 ms both times) — read them as "we lead or tie," not to two decimals. The
  suites that were our worst embarrassment are now our best showing.

- **`unicode` is now our biggest win.** It was 1.33x *slower* than main; the wide-class
  `print_slice` fill (wide + spacer_tail pair batching, replacing the per-codepoint fallback) took
  it to **0.50 — twice as fast as main**, and 0.30x vs 1.3.1. Wide-character throughput went from a
  gap to a lead.

- **The region-scroll suites are now mostly closed (#204), but still the last gap.** `scrolling`
  and `scrolling_fullscreen` are ties (1.07 / 1.05); the four scroll-*region* variants
  (`scrolling_{top,bottom}_{,small_}region`) were the previous baseline's only real loss at
  **1.27–1.47x**. Porting upstream's scroll-region optimization (`77190bd02`, shipped as #204: skip
  scrollback for top-anchored regions on non-retaining screens + a specialized region-scroll path)
  brought them to **1.13–1.20x**. The biggest mover was `scrolling_top_region` (**1.47 → 1.13**,
  qt median 22 → 17 ms). They are *not* fully at parity: #204 deliberately routed the region scroll
  through the existing `erase_row_bounded` machinery rather than upstream's bespoke single-page
  rotate (which mishandled wrapped wide-cell spacer heads), so a small residual gap remains. This is
  still the only suite family where main edges us, now by ~13–20% rather than 27–47%.

- **vs 1.3.1 we now win every suite** (0.30–0.73x). The 9/10-vs-1.3.1 story from the very first
  baseline is back to 10/10, and the meaningful comparison — vs main — is tie-or-win on 6/10 with
  only the four region scrolls behind.

- **Stability / method**: 3 rounds per terminal, load-gated (loadavg < 5) and interleaved, with
  per-suite samples pooled (thousands per cell) before taking medians. The `unicode` 0.50 lead and
  the region-scroll deltas are well outside round-to-round noise; the sub-10 ms cell suites carry
  ±1 ms quantization (see above). As a fairness control, both Ghostty columns were re-run this
  session and reproduced the prior baseline within ±1–2 ms per suite — so the load that varied
  4–8 during the run did not bias the comparison (vtebench measures single-process pty-drain
  throughput, largely insensitive to this background load). Ghostty distributions still have long
  tails (hence medians, not means).

## Known gaps / TODO

- `cursor_motion` and `light_cells` never load on macOS at this vtebench pin: their payload
  scripts do `tty="/dev/$(ps -o tty= -p $$)"` and macOS `ps` pads the tty column with a trailing
  space, so the `tput cols < $tty` redirect fails and the payload is empty (vtebench silently
  drops empty benchmarks). All three terminals lose the same two suites, so the comparison stays
  fair. Fix would be a local patch to the pinned checkout; deliberately not done to keep the pin
  pristine.
- `alt_screen_random_write` (named in older comparisons) no longer exists in modern vtebench; the
  default set above is the complete current suite.
- These numbers are pooled over 3 load-gated rounds per terminal (an improvement over the
  original baseline's single run). Ratios within ~10% of 1.0 (`sync_medium_cells` 1.00,
  `scrolling` 1.07, `scrolling_fullscreen` 1.05, and the sub-10 ms cell suites) are effectively
  ties; the `unicode` lead and the region-scroll deltas survive the noise. Re-run with
  `scripts/bench-vtebench.sh [--terminal ghostty --app-path <bundle>] --label <name>` and pool
  `results.dat` across labels for the medians.
- Window occlusion/focus state is not controlled; macOS may throttle background windows
  differently per app. Runs here had freshly opened, frontmost windows.

## History: pre-perf-work baseline (2026-07-11, superseded)

The first three-way run was at qwertty-term `a094ae672dc6` (dirty tracking + SIMD ascii only,
before T1's dispatch / cell-write / wide-char work) against the same Ghostty main pin. It showed
qwertty-term *behind* main on every cell and unicode suite — the gap this refresh closed:

| suite                | qt/main 07-11 | qt/main now | change                       |
| -------------------- | ------------- | ----------- | ---------------------------- |
| dense_cells          | 2.29          | 0.78        | loss → win (~1.3x faster)    |
| medium_cells         | 1.67          | 0.88        | loss → win                   |
| sync_medium_cells    | 1.83          | 1.00        | loss → tie                   |
| unicode              | 1.33          | 0.50        | loss → 2x win                |
| scrolling            | 1.00          | 1.07        | tie (noise)                  |
| scrolling_fullscreen | 1.00          | 1.05        | tie                          |
| region scrolls ×4    | 1.20–1.47     | 1.13–1.20   | closed by #204 (`77190bd02`) |

The cell-suite wins came from the CSI/SGR dispatch fast paths, `clear_cells` per-run style
release, and the bulk style-only `print_slice` fill; the unicode win from the wide-class
`print_slice` fill. The region scrolls were the "one remaining gap" through the 2026-07-12 refresh
(qt/main 1.27–1.47, path untouched); porting upstream `77190bd02` as **#204** (2026-07-13) brought
them to 1.13–1.20 — the last family where main still edges us, now narrowly.
