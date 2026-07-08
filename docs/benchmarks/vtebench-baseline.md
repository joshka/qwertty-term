# vtebench baseline: qwertty-term vs Ghostty

First recorded baselines for the canonical terminal benchmark lane —
[vtebench](https://github.com/alacritty/vtebench), the tool upstream Ghostty uses for terminal
comparisons. This is the scoreboard for upcoming perf work (dirty tracking, SIMD).

## Re-running

```sh
scripts/bench-vtebench.sh                    # qwertty-term (builds qwertty-term --release)
scripts/bench-vtebench.sh --terminal ghostty # real Ghostty.app for the A/B column
```

Outputs land in `target/vtebench/<terminal>/` (`results.dat` per-sample times, `summary.txt`
derived stats, `grid.txt` fairness check). The script auto-clones vtebench pinned at the commit
below into `work/vtebench-upstream` (git-ignored scratch, not vendored) if missing.

## Environment

| item        | value                                                             |
| ----------- | ----------------------------------------------------------------- |
| machine     | Apple M2 Max, 96 GB RAM, macOS 15.7.7                             |
| date        | 2026-07-08                                                        |
| vtebench    | `ead80032e57dee2e75f0b51f2ea67528647d9944` (v0.3.1, 2025-01-09)   |
| qwertty-term  | `bc762bf87115` (main) + this bench-lane change, `--release` build |
| Ghostty     | 1.3.1 stable (`/Applications/Ghostty.app`, ReleaseFast)           |
| grid        | 80x24 in both terminals (verified via `stty size` inside each)    |
| suite knobs | vtebench defaults: 1 MiB min sample, 10 s per suite, 1 warmup     |

Both terminals ran as real GUI windows driven non-interactively: qwertty-term via the
`QWERTTY_TERM_COMMAND` override in `crates/qwertty-term/src/termio.rs`, Ghostty via its `--command`
config with `--quit-after-last-window-closed`. This is the full GUI lane, not an engine-only
fallback.

## Results

Milliseconds per ~1 MiB sample, lower is better. Median of per-sample times from one full run of
each terminal (medians because Ghostty's distribution has a long tail — see analysis). Ratio is
qwertty-term time / Ghostty time: below 1.0 means qwertty-term is faster.

| suite                         | qwertty-term med (p90) | Ghostty med (p90) | ratio |
| ----------------------------- | -------------------- | ----------------- | ----- |
| dense_cells                   | 15 (15)              | 11 (25)           | 1.36  |
| medium_cells                  | 10 (10)              | 16 (47)           | 0.62  |
| scrolling                     | 17 (17)              | 25 (28)           | 0.68  |
| scrolling_bottom_region       | 19 (19)              | 23 (25)           | 0.83  |
| scrolling_bottom_small_region | 19 (19)              | 23 (24)           | 0.83  |
| scrolling_fullscreen          | 23 (23)              | 34 (35)           | 0.68  |
| scrolling_top_region          | 23 (23)              | 25 (25)           | 0.92  |
| scrolling_top_small_region    | 19 (19)              | 23 (24)           | 0.83  |
| sync_medium_cells             | 11 (11)              | 17 (18)           | 0.65  |
| unicode                       | 8 (8)                | 9 (10)            | 0.89  |

## Honest analysis

Read these numbers with vtebench's own disclaimer in hand: it measures **PTY read throughput
only** — no frame rate, no latency, no rendering-quality signal.

- **The headline flatters us architecturally.** qwertty-term applies pty output on the io-reader
  thread straight into the engine while the renderer redraws whole frames at its own cadence;
  there is little backpressure between parsing and presentation, so we can drain the pty at parse
  speed even when frames lag. Ghostty deliberately couples reads to its render loop. A pty-drain
  benchmark therefore rewards our decoupling without proving we present frames as well — the
  vt-diff throughput harness remains the parser-truth lane.
- **`dense_cells` is our one median loss (1.36x)** and the most render-shaped suite (every cell
  rewritten with heavy SGR per cell). Ghostty's median is faster but bimodal (p90 25 ms vs its
  11 ms median); qwertty-term is slower but flat (stddev 0.7 ms). Cell-write cost, not scroll
  handling, is our current per-byte bottleneck.
- **Scrolling did not turn out to be our weakest suite at this grid** (0.68–0.92x across all six
  scrolling variants) — at 80x24 the full-redraw renderer's per-frame cost is small enough that
  pty drain dominates. That will not hold at large grids, and the in-flight dirty-tracking chunk
  should move exactly these suites; re-run this lane when it lands (and consider a large-window
  variant).
- **Stability**: qwertty-term run-to-run spread was tight (per-suite stddev 0.2–1.2 ms). Ghostty
  showed a noisy first few minutes in one of two runs (dense/medium/scrolling means up to
  58–64 ms before settling); medians from its cleaner run are reported. The user's own Ghostty
  instance was running (idle) during all runs — shared-machine noise applies to both columns.

## Known gaps / TODO

- `cursor_motion` and `light_cells` never load on macOS at this vtebench pin: their payload
  scripts do `tty="/dev/$(ps -o tty= -p $$)"` and macOS `ps` pads the tty column with a trailing
  space, so the `tput cols < $tty` redirect fails and the payload is empty (vtebench silently
  drops empty benchmarks). Both terminals lose the same two suites, so the A/B stays fair. Fix
  would be a local patch to the pinned checkout; deliberately not done to keep the pin pristine.
- `alt_screen_random_write` (named in older comparisons) no longer exists in modern vtebench; the
  default set above is the complete current suite.
- Single-run medians per terminal; the lane is cheap (~3 min per terminal), so gather 3+ runs
  before drawing conclusions finer than ~10%.
- Window occlusion/focus state is not controlled; macOS may throttle background windows
  differently per app. Runs here had freshly opened, frontmost windows.
