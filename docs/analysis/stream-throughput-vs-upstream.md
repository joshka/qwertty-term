# Stream throughput: qwertty-term-vt vs upstream Zig (2026-07-07)

Apples-to-apples comparison of full stream→terminal-state throughput
(`Stream<TerminalHandler>::feed` vs upstream's `ghostty-bench terminal-stream`,
which is the model for our harness: read the data file in 64 KiB chunks, feed
every chunk through the full VT stream handler into real terminal state).

## Methodology

- Identical 64 MiB data files fed to both sides, lines of 100 codepoints
  terminated by CRLF: plain ASCII `a`, wide CJK `中`, and NFD `a`+U+0301
  (combining acute — exercises the grapheme-append path on every other
  codepoint).
- Terminal geometry 120×80 (upstream bench default) on both sides.
- Upstream: ghostty @ 38e49a232 (the spec checkout), `ghostty-bench` built
  with zig 0.15.2 and `-Doptimize=ReleaseFast` — this matters, see pitfall
  below. Run as `ghostty-bench +terminal-stream --data=<file>`.
- qwertty-term: `cargo build --release`, `slow_runtime_safety` off (the
  default), qwertty-term-vt @ e1f6f21b. Harness source lived in a scratch crate;
  it is ~30 lines mirroring upstream's `TerminalStream.step`.
- Best of 3 interleaved runs, macOS arm64 (M-series).

## Results

| Workload            | upstream Zig (ReleaseFast) | qwertty-term (release) | ratio      |
| ------------------- | -------------------------- | -------------------- | ---------- |
| ASCII `a`           | ~0.07 s (~900 MB/s)        | ~0.165 s (~406 MB/s) | Zig ~2.3×  |
| CJK `中`            | ~0.07 s (~950 MB/s)        | ~0.515 s (~130 MB/s) | Zig ~7×    |
| NFD `a`+U+0301      | ~11.6 s (~5.7 MB/s)        | ~11.5 s (~5.8 MB/s)  | parity     |

Interpretation:

- **Grapheme-append is at parity** — and is 2 orders of magnitude slower than
  plain prints *in both implementations*. The ~5.7 MB/s wall is the shared
  design cost (per-mark grapheme-map work), not a port regression.
- **ASCII gap (~2.3×)**: upstream uses SIMD UTF-8 decode plus bulk fast paths;
  our `Stream::feed` bulk scan is scalar. Already flagged as separate perf
  items in the `feed` doc comment (SIMD decode, CSI-param bulk path).
- **CJK gap (~7×)**: same decode gap amplified by the per-wide-char print
  path; the Zig side stays at memory-bandwidth-ish speed while ours drops to
  130 MB/s. The wide-char print path is the biggest known throughput gap.

## Pitfall: benching upstream

`zig build -Demit-bench` alone builds the *bench exe* ReleaseFast but the
terminal module at the default (Debug) optimize level, and upstream sets
`slow_runtime_safety = true` for Debug — that build ran the ASCII workload in
**29 s user time** (~400× off) because it runs the integrity scans we gate
behind the `slow_runtime_safety` Cargo feature (ADR 0001). Pass
`-Doptimize=ReleaseFast` for the whole tree when benchmarking upstream.

## Caveats

- The grapheme data is a single repeated cluster; real mixed text amortizes
  differently.
- Grapheme runs at ~15% relative noise due to background machine load; the
  ascii/cjk numbers were stable across interleaved repeats.
- Upstream's hyperlink/OSC-heavy workloads were not compared (blocked on the
  `start_hyperlink` rehash infinite-loop bug; see the fuzz-test comment in
  `terminal/tests.rs`).
