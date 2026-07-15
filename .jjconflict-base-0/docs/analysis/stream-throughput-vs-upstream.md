# Stream throughput: qwertty-term-vt vs upstream Zig (2026-07-07)

> **PARTIALLY SUPERSEDED (2026-07-13).** These are the *pre-optimization* engine
> numbers, taken before the M1 perf levers (`docs/analysis/perf.md`) and the
> later wide-class `printSliceFill` work landed. The wide fill **narrowed** the
> CJK/wide gap substantially, but did **not** flip it to a lead. Spot re-measured
> engine-only on 2026-07-13 (`ghostty-bench +terminal-stream` at `38e49a232`,
> ReleaseFast, vs our engine, one pass of the vtebench `unicode` wide payload at
> 80×24): **Ghostty ~790 MiB/s vs qwertty-term ~300 MiB/s — upstream's wide
> engine is still ~2.6× faster.** The `unicode` 0.50× *whole-app* win in
> `docs/benchmarks/vtebench-baseline.md` is a render-pipeline artifact (our
> renderer is decoupled from the pty drain; Ghostty's backpressures it), **not**
> an engine lead. So the direction of these old ratios still holds — upstream's
> wide engine leads — the magnitude just shrank from ~7× to ~2.6×. The remaining
> gap (SIMD UTF-8 decode, tighter wide-print) is a real T1 opportunity.

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

| Workload       | upstream Zig (ReleaseFast) | qwertty-term (release) | ratio     |
| -------------- | -------------------------- | ---------------------- | --------- |
| ASCII `a`      | ~0.07 s (~900 MB/s)        | ~0.165 s (~406 MB/s)   | Zig ~2.3x |
| CJK `U+4E2D`   | ~0.07 s (~950 MB/s)        | ~0.515 s (~130 MB/s)   | Zig ~7x   |
| NFD `a`+U+0301 | ~11.6 s (~5.7 MB/s)        | ~11.5 s (~5.8 MB/s)    | parity    |

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

> **Update 2026-07-13:** the CJK gap has since narrowed substantially. Two
> landings: (1) the wide-class `print_slice` fill (batched `(wide, spacer_tail)`
> pairs) removed the per-wide-cell print overhead, and (2) a scalar bulk
> multibyte UTF-8 decode (`decode_wellformed_multibyte`, perf.md Lever 5)
> replaced the per-byte Hoehrmann DFA on the wide stream. Engine-only on the
> vtebench `unicode` symbols payload (80×24), our full rate moved from ~300 to
> ~500 MiB/s (decode+dispatch alone ~770 MiB/s). Upstream's ~790 MiB/s full is
> still ahead — the remaining gap is split roughly evenly between decode
> (true SIMD, deferred) and the wide-print path — but it is no longer ~7× or
> even ~2.6×. Do not conflate this with the whole-app vtebench `unicode` number,
> which is render-pipeline dominated (see `docs/benchmarks/vtebench-baseline.md`).

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
