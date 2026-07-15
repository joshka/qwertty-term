# APC bulk-slice dispatch (kitty-graphics parser throughput)

Commit-stamped record of the APC/kitty-graphics parser throughput pass on the
`qwertty-term-vt` stream path. Ports upstream Ghostty's two post-pin APC perf
commits (both after our frozen pin `77190bd02`):

- **`f6f79acce`** "terminal: dispatch APC string bytes in bulk slices" — the
  foundational change (~25× on upstream's isolated APC-parser bench).
- **`8c523ed03`** "terminal: vectorize APC payload scanning" — a SIMD scan on
  top of the bulk path (a further ~1.69× upstream). *(Tracked as PR-2.)*

## Why APC was slow (profile-first)

Before this pass, every byte of an APC string (e.g. a kitty-graphics image
transmission, which can be megabytes of base64) was dispatched individually
through five layers: the VT state-machine table (`Parser::next`), the `Emitted`
materialization, the `Handler::apc_put` trait call, `apc::Handler::feed`, and a
per-byte `Vec::push` into the protocol buffer.

An APC-heavy stream generator (`profile_streams kitty`: well-formed kitty
transmit commands with 4 KiB payloads, mirroring upstream's `ghostty-gen
+kitty` corpus) confirmed the path was parser-APC-bound:

    stream    MiB/s (M2 Max, release, machine contended — read the ratio)
    kitty     ~42        <- APC path, on par with our slowest stream (sgr)
    ascii     ~280       <- for scale (bulk narrow print already landed)

The noop-handler ceiling for the kitty stream was only ~54 MiB/s, i.e. even the
per-byte *parser/dispatch* cost (before any buffer work) dominates — exactly
what a bulk path eliminates.

## The change (PR-1, `f6f79acce` port)

A bulk fast path in `Stream::feed`, alongside the existing CSI fast paths in the
non-ground loop: when the parser is in `SosPmApcString`, `consume_apc_string`
scans the longest run of apc_put bytes and dispatches it as a single
`Handler::apc_put_slice(&[u8])` instead of one `apc_put(u8)` per byte.

- **Scan boundary** (`stream.rs::consume_apc_string`): stop at the first byte
  that is not an apc_put byte in the parse table — `0x18` (CAN), `0x1A` (SUB),
  `0x1B` (ESC), or `0x80..=0xFF`. This is the exact complement of the
  `SosPmApcString` `ApcPut` transitions in `parser::table`
  (`0x00..=0x17`, `0x19`, `0x1C..=0x7F`). The terminating byte is left for the
  per-byte path, which performs the state transition (and emits `apc_end`) as
  today. The parser state is intentionally not stepped for the run: apc_put
  bytes are `SosPmApcString -> SosPmApcString` self-transitions with no
  collect/param side effects, so scan + one dispatch is byte-for-byte
  equivalent to feeding each through `Parser::next`.
- **Trait seam** (`Handler::apc_put_slice`): default loops `apc_put`, so any
  handler that only implements `apc_put` is unaffected; `TerminalHandler`
  overrides it to `apc::Handler::feed_slice`. Mirrors the established
  `print_slice` pattern.
- **Handler bulk-append** (`apc::Handler::feed_slice`): resolves the identify
  state machine byte-by-byte (only the first few bytes of a command), then
  appends the remainder of the run to the recognized protocol's buffer with a
  single `extend_from_slice`, replicating the per-byte kitty `in_data`/
  `max_bytes` and glyph `max_bytes` semantics exactly at slice granularity.

Unlike upstream, the fast path is unguarded: our `Handler` has no per-byte
`vtRaw`/inspector hook (the CSI fast paths above it are likewise unconditional),
so batching is transparent — there is no consumer that must still see per-byte
`apc_put`.

## Numbers (engine-only, `profile_streams kitty`, M2 Max, release)

Measured on a **contended** machine (loadavg ~8, WindowServer busy) — both
before and after are equally contended, so the ratio holds; absolute MiB/s to be
refreshed on a quiet box.

| path                           | before    | after      | change  |
| ------------------------------ | --------- | ---------- | ------- |
| kitty (stream -> APC -> parse) | ~42 MiB/s | ~294 MiB/s | **~7×** |

Our ~7× is over the whole-terminal path (it also parses + executes the kitty
command on each `apc_end`), versus upstream's ~25× on an isolated APC-parser
bench that skips image decode/storage; the whole-path number is the honest one
for our stack.

## The SIMD scan (PR-2, `8c523ed03` port)

After PR-1, `consume_apc_string`'s payload-boundary scan is a per-byte match
loop. PR-2 prescans it a vector at a time (`apc_scan_prefix` / `_neon`): on
`aarch64`, 16 bytes per `vld1q_u8`, testing each lane for CAN/SUB/ESC or
`>= 0x80` (`vceqq_u8` × 3 `| vcgeq_u8`), horizontal-OR via `vmaxvq_u8`; the
scalar loop then pinpoints the exact boundary in the first vector that contains
a non-apc_put byte. NEON is mandatory in the ARMv8-A baseline, so there is no
runtime feature detection — a `cfg(target_arch = "aarch64")` gate with a scalar
fallback (return 0) on every other target. The block is `cfg(not(miri))` so Miri
exercises the scalar path; its correctness on hardware is proved by
`apc_vector_boundaries_match_scalar` (control byte before/on/after every 16-byte
edge) plus the feed-vs-next fuzz differential.

Additional gain, same generator + machine, back-to-back at matched load:

| build            | kitty MiB/s | vs PR-1 |
| ---------------- | ----------- | ------- |
| PR-1 (scalar)    | ~294        | —       |
| PR-1 + SIMD scan | ~338–347    | ~+15%   |

Distributions are tight and non-overlapping, so the ~15% is real (not noise),
though the absolute figures still want a quiet box.

## Verification

- **Direct equivalence** (the primary guarantee for this change):
  `apc::feed_slice_matches_feed` + `_at_max_bytes` (bulk vs per-byte `feed`
  across every slice split, incl. the `max_bytes` cap boundary),
  `stream::apc_bulk_slice_matches_scalar` (bulk `feed` vs per-byte `next`,
  incl. 7-byte chunking and CAN/SUB/ESC terminators), and
  `stream::apc_vector_boundaries_match_scalar` (SIMD boundary vs scalar with a
  control byte before/on/after every 16-byte vector edge — PR-2).
- **Differential** vs the `77190bd02` reference oracle
  (`vt-diff --features reference`): corpus + afl + generative sweep + hand
  differential all green, zero divergences.
- **Fuzz**: `parser` target section 3 (`Stream::feed` fast path vs byte-at-a-
  time `Stream::next`, screen-state differential), seeded with an APC corpus
  file — ~730k runs / 3 min each PR, no divergence/crash. This is the on-
  hardware check of the active NEON path (PR-2).
- **Miri** (`cargo +nightly miri test -p qwertty-term-vt --lib apc`): clean; the
  NEON block is `cfg(not(miri))` so Miri validates `feed_slice`'s slicing and the
  scalar boundary scan.
- Full gate: check (0 warnings), workspace tests, release + paranoid lanes,
  fmt, clippy.
