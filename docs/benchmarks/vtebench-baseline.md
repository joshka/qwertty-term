# vtebench baseline: qwertty-term vs Ghostty 1.3.1 vs Ghostty main

Three-way comparison on the canonical terminal benchmark lane —
[vtebench](https://github.com/alacritty/vtebench), the tool upstream Ghostty uses for terminal
comparisons. This extends the earlier two-way baseline (qwertty-term vs Ghostty 1.3.1) with a
third column for **Ghostty main**, to see whether upstream's in-flight perf work has moved the
picture. It has — dramatically (see analysis).

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

| item          | value                                                                                          |
| ------------- | ---------------------------------------------------------------------------------------------- |
| machine       | Apple M2 Max, 96 GB RAM, macOS 15.7.7                                                          |
| date          | 2026-07-11                                                                                     |
| vtebench      | `ead80032e57dee2e75f0b51f2ea67528647d9944` (v0.3.1, 2025-01-09)                                |
| qwertty-term  | `a094ae672dc6` (main) + this bench-lane change, `--release`; incl. dirty tracking + SIMD ascii |
| Ghostty 1.3.1 | 1.3.1 stable (`/Applications/Ghostty.app`, ReleaseFast)                                        |
| Ghostty main  | `91f66da24527fa02d92b5fd0b41cd020f553a64c` (2026-07-08, ReleaseLocal)                          |
| grid          | 80x24 in all three terminals (verified via `stty size` inside each)                            |
| suite knobs   | vtebench defaults: 1 MiB min sample, 10 s per suite, 1 warmup                                  |

All three terminals ran as real GUI windows driven non-interactively: qwertty-term via the
`QWERTTY_TERM_COMMAND` override in `crates/qwertty-term-termio/`, both Ghostty builds via their
`--command` config with `--quit-after-last-window-closed`. This is the full GUI lane, not an
engine-only fallback. The maintainer's own Ghostty instance was open (idle) during all runs —
ambient shared-machine load applies to every column equally.

## Results

Milliseconds per ~1 MiB sample, lower is better. Median of per-sample times from one full run of
each terminal, with p90 in parentheses (medians because the Ghostty distributions have long tails
— see analysis). The final column is qwertty-term median / Ghostty main median: below 1.0 means
qwertty-term is faster than upstream main, above 1.0 means slower.

| suite                         | qwertty-term med (p90) | Ghostty 1.3.1 med (p90) | Ghostty main@91f66da24 med (p90) | qt/main ratio |
| ----------------------------- | ---------------------- | ----------------------- | -------------------------------- | ------------- |
| dense_cells                   | 16 (16)                | 11 (15)                 | 7 (13)                           | 2.29          |
| medium_cells                  | 10 (10)                | 15 (17)                 | 6 (8)                            | 1.67          |
| scrolling                     | 15 (16)                | 34 (38)                 | 15 (16)                          | 1.00          |
| scrolling_bottom_region       | 18 (18)                | 29 (29)                 | 15 (16)                          | 1.20          |
| scrolling_bottom_small_region | 18 (18)                | 29 (30)                 | 15 (16)                          | 1.20          |
| scrolling_fullscreen          | 20 (21)                | 42 (133)                | 20 (20)                          | 1.00          |
| scrolling_top_region          | 22 (22)                | 31 (36)                 | 15 (15)                          | 1.47          |
| scrolling_top_small_region    | 18 (18)                | 29 (30)                 | 15 (16)                          | 1.20          |
| sync_medium_cells             | 11 (11)                | 18 (20)                 | 6 (7)                            | 1.83          |
| unicode                       | 8 (8)                  | 9 (10)                  | 6 (6)                            | 1.33          |

## Honest analysis

Read these numbers with vtebench's own disclaimer in hand: it measures **PTY read throughput
only** — no frame rate, no latency, no rendering-quality signal.

- **Upstream main rewrote the story.** Against the 1.3.1 stable column, qwertty-term still wins
  9/10 (losing only `dense_cells`) — that was the earlier baseline's headline. Against **main**,
  that lead is gone: qwertty-term now ties on the two scrolling suites (`scrolling`,
  `scrolling_fullscreen`) and loses every other suite. Ghostty's recent perf work landed hard —
  main is 1.5x–2.7x faster than its own 1.3.1 release on `dense_cells`, `medium_cells`,
  `sync_medium_cells`, and the region-scroll suites. The comparison we should be tracking is now
  qwertty-term vs main, and on that scoreboard we are behind.

- **The scrolling gap closed.** At 80x24, 1.3.1 was slow on scrolling (34–42 ms medians, with a
  133 ms p90 tail on `scrolling_fullscreen`); main brought those down to 15–20 ms, landing on top
  of qwertty-term (both ~15 ms on plain `scrolling`, both ~20 ms on `scrolling_fullscreen`). So
  the six-way scrolling sweep where we previously led 0.68–0.92x vs 1.3.1 is now a tie-or-loss vs
  main: even ties on two, and 1.20–1.47x *slower* on the four region-scroll variants. Upstream's
  scroll-region handling in particular pulled ahead of ours.

- **`dense_cells` moved the wrong way for us.** It was our one loss vs 1.3.1 (1.36x then); vs main
  it is 2.29x. This is the most render-shaped suite (every cell rewritten with heavy per-cell
  SGR), and per-cell write cost remains qwertty-term's per-byte bottleneck. main also widened its
  lead on the other cell-heavy suites (`medium_cells` 1.67x, `sync_medium_cells` 1.83x). These are
  the suites to target next.

- **qwertty-term's own numbers include the new fast paths.** This re-run is on `a094ae672dc6`,
  which lands dirty tracking and the SIMD ascii fast path — both absent from the earlier baseline.
  Our medians did shift a little from that older run (e.g. `dense_cells` 15→16, `scrolling`
  17→15), but not enough to keep pace with main's jump. Dirty tracking should matter most at large
  grids and low-churn workloads, neither of which this 80x24 full-suite lane stresses; a
  large-window variant is the obvious follow-up to show what it buys.

- **Stability**: qwertty-term run-to-run spread was tight (per-suite stddev 0.5–2.5 ms). Ghostty
  main was similarly flat (0.6–3.1 ms). Ghostty 1.3.1 was the noisy one — `scrolling_fullscreen`
  stddev 48 ms, a long-tailed p90 of 133 ms — which is why medians (not means) are reported for
  all columns. Same idle background Ghostty instance during every run.

## Known gaps / TODO

- `cursor_motion` and `light_cells` never load on macOS at this vtebench pin: their payload
  scripts do `tty="/dev/$(ps -o tty= -p $$)"` and macOS `ps` pads the tty column with a trailing
  space, so the `tput cols < $tty` redirect fails and the payload is empty (vtebench silently
  drops empty benchmarks). All three terminals lose the same two suites, so the comparison stays
  fair. Fix would be a local patch to the pinned checkout; deliberately not done to keep the pin
  pristine.
- `alt_screen_random_write` (named in older comparisons) no longer exists in modern vtebench; the
  default set above is the complete current suite.
- Single-run medians per terminal; the lane is cheap (~2 min per terminal), so gather 3+ runs
  before drawing conclusions finer than ~10%. The qt-vs-main deltas above are large enough
  (1.2x–2.3x) to survive that noise, but the two `1.00` ties are within it.
- Window occlusion/focus state is not controlled; macOS may throttle background windows
  differently per app. Runs here had freshly opened, frontmost windows.
