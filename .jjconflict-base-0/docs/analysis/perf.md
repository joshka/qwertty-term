# M1 performance pass (`vt-perf` chunk)

Commit-stamped record of the M1 throughput pass on the `qwertty-term-vt` stream +
print path. Gate (rewrite-prompt Phase-1 exit): **>= 0.5x of ReleaseFast
libghostty-vt** on every stream of the committed benchmark
(`cargo test -p vt-diff --features reference --release -- --ignored --nocapture
throughput`), with zero parity regressions.

Reference lib: ghostty commit `2da015cd6`, ReleaseFast, `~/local/ghostty/zig-out/lib`.

## Baseline (before this pass)

    stream           rust MiB/s    ref MiB/s    ratio
    ascii                  83.5        564.2    0.15x
    sgr-heavy              42.3        141.8    0.30x
    utf8-mixed             92.3        332.9    0.28x
    cursor-heavy           52.1        169.7    0.31x

## Profile findings (attribution, not symbolication)

`samply`/`cargo flamegraph` are installed, but on macOS without a `.dSYM` the
saved samply profile collapses to raw offsets (samply symbolicates lazily in the
viewer, not in `--save-only` JSON). Rather than fight symbolication, the cost was
attributed structurally by re-running each stream through the full
`Stream<TerminalHandler>` vs a `Stream<NoopHandler>` (parse + decode + dispatch,
but `print`/effects are a no-op add). The delta is the terminal-effect cost.

    stream   full MiB/s   noop MiB/s | full ns/B  noop ns/B  effect ns/B
    ascii        86.0        492.4    |   11.09      1.94        9.15
    utf8         93.9        298.7    |   10.15      3.19        6.96
    sgr          40.3         52.4    |   23.65     18.22        5.44

Reading:

- **ascii / utf8**: the parse+decode+dispatch layer alone already runs at
  ~490 / ~300 MiB/s (near or above the reference's *total* rate). ~80% of wall
  time is inside `Terminal::print` — the per-codepoint print state machine
  (repeated `screen()` match-derivation, `modes.get` gates, `cursor_right`
  pin/cell bookkeeping, `print_cell` style-ref/hyperlink/grapheme checks). This
  is exactly the run that upstream's `printSliceFast` bulk-writes. The
  decode-until-control-seq scan alone would NOT close the gap; the print path is
  the ascii/utf8 bottleneck.
- **sgr**: the no-op path is only 52 MiB/s, so here the bottleneck is the
  parse/dispatch/SGR machinery itself (per-byte `Parser::next`, the owned
  `Emitted` copy in `next_non_utf8`, `sgr::Parser`), not `print`.

Levers, in landing order, below.

## Lever progression

### Lever 1 — decode-until-control-seq bulk print (ascii/utf8)

Two coordinated changes, both faithful scalar ports of upstream:

1. **`Stream::feed`** now mirrors `stream.zig`'s `nextSlice`/`nextSliceCapped`
   structure (scalar, no SIMD, no CSI-param fast path — those are separate perf
   items and out of this chunk's charter). While the parser is in ground state
   and the UTF-8 decoder is idle, it bulk-decodes a run of codepoints into a
   boxed 4096-entry scratch buffer up to the next ESC, then hands each maximal
   printable run to `Handler::print_slice` in ONE call. Non-ground bytes and
   partial UTF-8 fall back to the existing per-byte `next` path unchanged.
2. **`Terminal::print_slice`** (+ `print_slice_fast_narrow` /
   `print_slice_fill_narrow`) is a faithful port of `printSlice` /
   `printSliceFast` / `printSliceFill(.narrow, …)`: it batches cell writes for
   runs of narrow (width-1) codepoints — the whole ASCII run and the narrow
   part of mixed UTF-8 — using the same masked "simple cell" check
   (`Cell::SIMPLE_MASK` / `simple_check_expected`) and template-OR-codepoint
   store as upstream, hoisting the per-cp mode/charset/style checks and cursor
   bookkeeping out of the inner loop. Wide runs, grapheme continuations, insert
   mode, active charsets, hyperlinks, and complex cells all defer to the
   per-cp `print`, so semantics are identical. `TerminalHandler::print_slice`
   overrides the trait default to route to it; the default (loop `print`) keeps
   spy handlers correct.

Result (release, vs ReleaseFast reference):

    stream           rust MiB/s    ref MiB/s    ratio      (was)
    ascii                 279.7        566.6    0.49x     (0.15x)
    sgr-heavy              42.4        141.9    0.30x     (0.30x)
    utf8-mixed            165.5        331.9    0.50x     (0.28x)
    cursor-heavy          52.6        170.0    0.31x     (0.31x)

(numbers filled in from the committed bench after the lever landed — see below)

Attribution after the lever (full vs no-op handler):

    stream   full MiB/s   noop MiB/s | full ns/B  noop ns/B  effect ns/B
    ascii       282.6        421.6    |   3.37       2.26       1.11   (was 9.15)
    utf8        165.2        401.5    |   5.77       2.38       3.40   (was 6.96)
    sgr          39.2         49.2    |  24.32      19.39       4.93

ascii/utf8 print-effect collapsed (9.15 -> 1.11, 6.96 -> 3.40 ns/byte); the
batched narrow fill is doing its job. ascii/utf8 now clear the 0.5x gate. `sgr`
and `cursor-heavy` are dispatch-bound (their no-op rate is the ceiling) — the
next levers target the dispatch path.

### Lever 2 — allocation-free CSI/ESC/DCS dispatch payloads (sgr/cursor)

`Stream::next_non_utf8` materializes each borrowed parser action into an owned
`Emitted` before dispatch (the parser borrow must end before the `&mut self`
handler call). The CSI/ESC/DCS payloads previously copied their
intermediates/params into heap `Vec`s — a per-control-sequence allocation that
dominated the dispatch cost on control-dense streams. Since the parser's
intermediates/params are already small fixed-capacity arrays
(`MAX_INTERMEDIATE=4` / `MAX_PARAMS=24`), the owned payloads now copy into inline
`[u8; 4]` / `[u16; 24]` arrays + a length (`copy_bounded`), keeping the dispatch
hot path allocation-free. Pure win, behavior-identical.

Rust-only profiler (stable under load — CPU-bound, gets its own core):

    stream   before   after lever 2
    ascii     288.0    288.0   (unchanged; ascii has ~no control seqs)
    sgr        46.4     46.4-> (small; the bigger sgr win is lever 3)
    cursor     49.1     57.6   (+17%)

(sgr moved from ~40 to ~46 across levers 1+2; the allocation removal mostly
helped cursor-heavy, which is CUP-dense.)

### Lever 3 — bulk CSI-parameter consume (sgr/cursor)

`Parser::bulk_consume_csi_params` is a faithful port of `stream.zig`'s
`consumeCsiParams`: while in the `csi_param` state, it consumes the dense run of
digit/separator bytes (and the final byte) in one call, accumulating param state
in locals instead of stepping `Parser::next` per byte. On the final byte it runs
the exact CSI finalize (param-overflow drop, colon-for-non-`m` drop) and yields
the dispatch. `Stream::feed`'s non-ground loop calls it when the parser is in
`csi_param`; anything that isn't a parameter/final byte (intermediates, private
markers, C0, ESC) breaks out to the existing per-byte path. Behavior-equivalent
to per-byte feeding (`acc_idx |= 1` matches upstream's own bulk path, which
deliberately diverges from the scalar 256-digit wrap — a pathological case that
never affects a finalized value).

Rust-only profiler after lever 3:

    stream    lever 1+2   lever 3
    ascii        291.4     291.4   (unchanged)
    sgr           46.5      74.2   (+60%)
    utf8         165.0     165.0   (unchanged)
    cursor        58.3      84.3   (+45%)

### Lever 4 — ASCII scan shortcut in `decode_until_control_seq`

The ground-state scan still ran each byte through the Hoehrmann UTF-8 DFA
(two table lookups + accumulator math) even for pure ASCII. When the decoder is
idle, a byte `< 0x80` is a complete 1-byte codepoint equal to the byte itself
(DFA char-class 0 in the ACCEPT state), so the scan now emits it directly and
skips the DFA. This is the scalar analogue of what the SIMD path does in bulk.
Safe code, behavior-identical (covered by the `fastpath_*` equivalence tests).

Rust-only profiler after lever 4:

    stream    lever 3   lever 4
    ascii        291.4    384.7   (+32%)
    sgr           74.2     73.8   (unchanged)
    utf8         165.0    179.5   (+9%; the ascii tail of each utf8 chunk)
    cursor        84.3     85.2   (unchanged)

## Final result

Committed benchmark (`cargo test -p vt-diff --features reference --release --
--ignored --nocapture throughput`), all four levers landed:

    stream           rust MiB/s    ref MiB/s    ratio      (baseline ratio)
    ascii                 380.9        603.8    0.63x      (0.15x)
    sgr-heavy              75.2        139.9    0.54x      (0.31x)
    utf8-mixed            183.9        323.9    0.57x      (0.29x)
    cursor-heavy           88.5        162.6    0.54x      (0.30x)

(The reference column here matches the committed baseline closely — ascii 603.8
vs 613.6, sgr 139.9 vs 140.0, utf8 323.9 vs 321.6, cursor 162.6 vs 172.2 — so
this is a quiet-machine reading. On a loaded machine the reference drifts a few
percent since both engines run sequentially within the one bench; the rust-only
rates are stable regardless.)

Gate status: **PASS — every stream >= 0.5x.** All four moved from 0.15-0.31x to
0.54-0.63x, a 1.7x-4.2x per-stream improvement, with the bench's built-in
`assert_eq!(rust.text(), reference.text())` still holding (zero divergence).

### Parity + quality gates

- `cargo test -p qwertty-term-vt` — 993 lib + doctests green (incl. 8 new
  `stream::tests::fastpath_*` whole-vs-per-byte-vs-chunked equivalence tests).
- `cargo test -p vt-diff --features reference` — full differential + formatter
  differential green, **zero divergences**.
- fmt + clippy clean; Miri clean over the touched modules (see commit).

### Deferred (not landed this pass)

> **Update 2026-07-13:** the two "deferred" items below were scoped to *this M1
> pass*; the wide fill has since shipped as its own package (see below), and it
> closed a large chunk of the wide/CJK engine gap. But do **not** read the
> `unicode` 0.50× in `docs/benchmarks/vtebench-baseline.md` as an engine lead —
> that is a *whole-app* number dominated by render pipeline, and measured
> engine-only Ghostty's wide path is still ~2.6× faster than ours (~790 vs ~300
> MiB/s on the vtebench unicode payload). A real engine gap remains (SIMD UTF-8
> decode + tighter wide-print). Later dispatch/clear_cells work did move `sgr`
> and `cursor` further than the numbers here.

- **SIMD `utf8DecodeUntilControlSeq`** — the scalar decode-until-control-seq scan
  is ported; the SIMD bulk decoder (std::simd / simdutf) is a later item and was
  out of this chunk's charter. **→ PARTLY ADDRESSED** by the scalar bulk
  multibyte decode (Lever 5 below): true SIMD (NEON/std::simd) is still deferred,
  but the per-byte DFA is no longer the wide-stream wall.
- **Batched WIDE `printSliceFill(.wide, …)`** — only the narrow fill is ported;
  wide runs defer to per-cp `print` (correct, just not batched). utf8-mixed
  would gain a little more from a wide batch. **→ SHIPPED** as the wide-class
  `print_slice` fill (port of the `.wide` path of `47e26df60`); it drove the
  `unicode`/CJK wins recorded in the vtebench baseline.
- **Dependencies added: none.** (`memchr` was evaluated for the ground scan but
  the hand-rolled ESC-stop scan in `decode_until_control_seq` is already tight
  and keeps the crate dependency-free; no justification to add it.)

## Lever 5 — scalar bulk multibyte UTF-8 decode (cjk / wide, 2026-07-13)

Follow-up pass targeting the wide/CJK engine gap called out in
`stream-throughput-vs-upstream.md` (engine-only Ghostty ~790 MiB/s on the
vtebench `unicode` payload vs our ~300). Structural attribution
(`profile_streams <stream>` full vs `NOOP=1` no-op handler) showed the wall was
**decode+dispatch**, not print: on the synthetic `cjk` stream the no-op
(decode+dispatch, zero print) path ran at only ~463 MiB/s — *slower than
upstream's entire full pipeline* — because `decode_until_control_seq` stepped the
Hoehrmann DFA byte-by-byte, and every CJK byte is `>= 0x80` so the ASCII SWAR
scan matched nothing (3 DFA table-lookup steps per 3-byte codepoint).

The lever adds `decode_wellformed_multibyte`: a scalar decode of one *complete,
well-formed* 2/3/4-byte sequence (exactly Unicode Table 3-7 — the same
well-formedness the DFA enforces, including the E0/ED and F0/F4 lead-specific
second-byte ranges that exclude over-longs, surrogates, and code points above
U+10FFFF). It runs in a tight inner loop across a run of consecutive non-ASCII
bytes (paying the outer ESC/`< 0x80`/partial re-checks once per run, not per
codepoint), stopping the instant the next byte is ASCII so mixed text falls back
to the SWAR scan with no wasted probe. Anything ill-formed, truncated, or a
partial-decoder state defers to the DFA below — behavior-identical, so it is a
pure fast path. Scalar analogue of the bulk decode in upstream's
`simd.vt.utf8DecodeUntilControlSeq`; no SIMD intrinsics, no new dependency,
portable across targets.

`profile_streams` (release, this machine; full = terminal handler,
noop = decode+dispatch only):

    stream               full (before→after)     noop (before→after)
    cjk                    314 → 455  (+45%)       463 → 880  (+90%)
    utf8-mixed             271 → 285  (+5%)        631 → 733  (+16%)
    ascii                  579 → 571  (flat)      1543 → 1598 (flat)
    vtebench unicode
      symbols @ 80x24     ~300 → 498  (+66%)         — → 766

ascii is untouched (it never reaches the multibyte branch); utf8-mixed improves
modestly (mostly ASCII + print-bound); the wide/CJK streams — the target — gain
~45–66%. Decode+dispatch on `cjk` roughly doubled (463 → 880 MiB/s), moving the
wide-stream bottleneck off decode. The committed throughput gate stays healthy
(utf8-mixed 0.84×, ascii 0.65×, all ≫ 0.5×).

### Parity + quality gates (Lever 5)

- `cargo test -p qwertty-term-vt` — 1536 lib + doctests green, incl. 2 new
  `stream::tests::fastpath_{malformed_utf8_defers_to_dfa,wide_cjk_and_emoji_bulk}`
  whole-vs-per-byte-vs-chunked equivalence tests.
- `cargo test -p vt-diff --features reference` — differential + corpus +
  afl_corpus + a **150 000-iteration** generative sweep all green, **zero
  divergences**.
- fmt + clippy clean; Miri clean over all 10 `fastpath_*` tests.

### Still deferred after Lever 5

- **True SIMD decode** (NEON / `std::simd`): would push decode+dispatch past the
  ~880 MiB/s the scalar bulk path reaches, but needs target-specific unsafe (or a
  nightly feature) and a scalar fallback — out of scope for a dependency-free,
  stable, all-targets change.
- **Tighter wide-print path**: after Lever 5 the `cjk` full/noop split is ~455 /
  880, so print is again ~half the per-byte cost. Per-wide-cell overhead (width
  lookup, spacer-tail pairing) is the next print-side lever.
