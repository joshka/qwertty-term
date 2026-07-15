# Print/dispatch profiling — the pipeline is scan-bound, not decode-bound

Commit-stamped record (frozen pin `77190bd02`, M2 Max, release) of a
profile-first pass over the whole `qwertty-term-vt` stream→print pipeline,
undertaken to decide the next perf lever between the two open candidates:
**(a) SIMD NEON UTF-8 decode** and **(b) a print-side lever**. The profile
**retires (a)** and **promotes a concrete, representative (b)**.

## Method

`crates/vt-diff/examples/profile_streams.rs` feeds 8 MiB synthesized/tiled
payloads through `Stream::feed`. Two handlers isolate the cost split:

- **NOOP** (`NoopHandler`) — decodes + dispatches but does no terminal print
  work, so its throughput is the **decode+dispatch ceiling**.
- **FULL** (`TerminalHandler`) — the whole path including print/execute.

`print% = (1/FULL − 1/NOOP) / (1/FULL)` is the fraction of full-pipeline time
spent *past* decode+dispatch (i.e. in print/execute/terminal work).

Line-level attribution: `samply record --save-only` → parse the Firefox-format
profile for per-frame self-time → resolve module-relative addresses through a
`dsymutil` dSYM with `atos`. Machine was contended (loadavg ~4, WindowServer
~47%); **absolute MiB/s are directional, ratios and self-time %s hold** (before
and after are equally loaded).

## Finding 1 — every stream is print/execute-bound, not decode-bound

NOOP-vs-FULL sweep (20 iters each):

| stream    | NOOP   | FULL  | print% |
| --------- | -----: | ----: | -----: |
| ascii     | 1623.5 | 618.2 | 62%    |
| sgr       | 205.4  | 114.2 | 44%    |
| utf8      | 855.1  | 306.5 | 64%    |
| cjk       | 1305.2 | 592.1 | 55%    |
| dense     | 427.6  | 134.0 | 69%    |
| erase     | 1079.4 | 388.9 | 64%    |
| redraw    | 1078.5 | 459.7 | 57%    |
| scrolling | 846.8  | 65.7  | 92%    |

**Decode+dispatch is at most ~56% of any stream, and for realistic app streams
(dense/erase/redraw/scrolling) print/execute is 57–92%.** No full-pipeline
workload is limited by decode. The decode-only ceiling itself is already high
(ascii 1623, cjk 1305, mixed-utf8 855 MiB/s) because the scalar decode is
already SWAR-optimized: a `u64`-at-a-time ASCII run scan (`ascii_non_esc_run`)
plus a branchy well-formed-multibyte fast path (`decode_wellformed_multibyte`),
with the Höhrmann DFA only as an ill-formed fallback.

### Consequence: the NEON UTF-8 decode lever is retired (on evidence)

A NEON decoder would raise the NOOP ceiling that **nothing in the full pipeline
is hitting**. It would help only a hypothetical decode-only embedded consumer,
and even there only mixed-utf8 (855) sits notably below the ascii/cjk ceilings —
and mixed decode is dominated by *mode-switching* between the ASCII SWAR path and
the multibyte path, which SIMD does not fix cleanly. Against that narrow,
speculative benefit stands the **highest differential risk in the codebase**
(variable-length UTF-8 decode + error-replacement + cross-chunk partial state).
**Not worth it.** (This confirms and hardens the prior thread's post-#277 note
that decode is no longer the cjk bottleneck.)

## Finding 2 — the hot print work is read-only find-first scans

Line-level self-time on the **real** vtebench payloads (tiled to 8 MiB;
`file:benchmarks/{light,medium}_cells/benchmark`), which are exactly the
competitive scoreboard inputs:

| line                  | what                                    | light_cells | medium_cells |
| --------------------- | --------------------------------------- | ----------: | -----------: |
| `stream.rs:955`       | `decode_until_control_seq` (decode)     | 13.3%       | 15.2%        |
| `stream.rs:957`       | `dispatch_ground_run` (C0-split + emit) | 20.8%       | 20.6%        |
| `print.rs:246–247`    | `print_slice_fill` **run_len scan**     | ~9.6%       | ~8.4%        |
| `print.rs:373/377`    | `print_slice_fill` **simple-cell scan** | ~13%        | ~8.4%        |
| `print.rs:401/403`    | `print_slice_fill` narrow fill (writes) | ~4.2%       | ~4.2%        |
| `screen mod.rs:1247…` | scroll-region up (light/medium scroll)  | ~12%        | ~18%         |

`print_slice_fill::<narrow>` is the dominant single function (~20–27% total on
both real payloads and the synthetic redraw), and its two hottest lines are both
**read-only find-first scans**:

- **run_len scan** (`print.rs:244–269`): find the longest prefix of `cps`
  (`u32`) that is same-width-class-batchable. In the dominant narrow,
  non-grapheme case this reduces to *find the first cp not in `[0x10, 0xFF]`* —
  a clean range scan over `u32` (4 lanes per NEON vector).
- **simple-cell scan** (`print.rs:369–378` and the style-run scan `430–438`):
  find the run of destination cells whose `cval() & SIMPLE_MASK` equals the
  expected value — a masked-compare find-first over `u64` cells (2 lanes per
  NEON vector).

Both are the **exact shape of the already-shipped `apc_scan_prefix_neon` lever**
(#289): a bounded find-first primitive with a scalar fallback, `cfg`-gated, and
differential+fuzz-checked. They are read-only (no cell writes, no style-refcount
mutation), so a wrong length is caught immediately by the differential oracle
(wrong cells filled) — a **much lower risk profile than the writes/refcount path**
that earlier flagged item (3) as higher-risk.

## Finding 3 — run lengths are NEON-favorable (representative-workload check)

NEON find-first only wins when runs are long enough to amortize vector setup
(the hash_map lesson: measure the *representative* workload, not a churn
microbench). Printable-run-length distribution of the real payloads:

| payload      | mean | median | p90 | ≥16 cells |
| ------------ | ---: | -----: | --: | --------: |
| light_cells  | 35.9 | 28.5   | 67  | 76%       |
| medium_cells | 28.3 | 31     | 45  | 67%       |
| dense_cells  | 28.2 | 27     | 61  | 66%       |

Runs are long (median ~28–31), squarely in the NEON-favorable regime: a 4-lane
`u32` run_len scan amortizes over ~28 elements (~2–4× on that loop), and a
2-lane `u64` cell scan over the same (~2×). Short-run mixed content would erase
the win, but the competitive cell benchmarks do not have short runs.

## Decision & plan

**Pursue the print-scan lever, not the decode lever.** It targets the actual
bottleneck (print/dispatch), is representative (hot on the real scoreboard
payloads with long runs), and is low-risk (read-only find-first, APC-scan
precedent). It also satisfies the prior thread's own gate for touching the print
side: *"only pursue with fresh line-level profiling showing a concrete hot
spot."*

One optimization per PR, full rigor (equivalence tests + differential vs the
`77190bd02` oracle + Miri on the unsafe cell scan + parser/resize fuzz +
before/after criterion numbers on a representative microbench):

- **PR-1 — run_len narrow prescan** (`print.rs:246`, `u32`, safe slice, 4-lane):
  NEON-skip the `[0x10, 0xFF]` prefix, scalar loop continues for the
  `>0xFF`/grapheme/width-checked tail. Lowest risk (no unsafe), highest lane
  count, hottest single print line.
- **PR-2 — simple-cell scan** (`print.rs:372` + style-run scan `430`, `u64`,
  2-lane): NEON masked-compare find-first over the destination cells. Needs Miri
  on the pointer walk.

Before PR-1, add a representative criterion bench (`benches/`) with the measured
run-length distribution so the before/after is honest.

## PR-1 — run_len narrow prescan (`latin1_narrow_prefix`) — SHIPPED

The narrow `run_len` loop (`print.rs`) now vector-skips the leading run of
Latin-1 printables `[0x10, 0xFF]` via `latin1_narrow_prefix`, a read-only
find-first prescan built on the exact `apc_scan_prefix` shape: NEON on aarch64
(4 `u32` lanes per `vld1q_u32`, `vcgeq`/`vcleq`/`vand`, horizontal `vminvq_u32`
to detect an out-of-range lane), a scalar-fallback `0` on every other target
(the existing scalar loop then does all the work), `cfg(not(miri))`. The scalar
per-codepoint checks are unchanged and resume at the boundary the prescan lands
on, so the batching decision — and thus the screen output — is identical; the
prescan can only *undercount* (never over-count past the first non-Latin-1 cp),
which the caller tolerates by construction.

**Numbers** (M2 Max, release, best-of-5–6 interleaved A/B, machine contended →
directional; every stream measured both binaries back-to-back at matched load):

| stream (real vtebench payload / synthetic) | base MiB/s | new MiB/s | delta     |
| ------------------------------------------ | ---------: | --------: | --------: |
| ascii (synthetic bulk)                     | ~618       | ~690      | +11–12%   |
| redraw (synthetic bulk)                    | ~466       | ~512      | +9.8%     |
| **light_cells** (real)                     | ~370       | ~400      | +7.6–8.0% |
| **medium_cells** (real)                    | ~328       | ~348      | +5.3–6.3% |
| **dense_cells** (real)                     | ~339       | ~357      | +5.2%     |

A full-pipeline win of **+5–8% on the competitive cell payloads** from PR-1
alone (the simple-cell scan is PR-2). Deltas held with A/B order swapped.

**Verification:** `latin1_narrow_prefix` boundary tests (never over-counts with
an out-of-range cp at every offset around each 4-lane edge; short/empty/leading-
oor; aarch64 fast-path-engages); the existing `print_slice_differential_fuzz_vs_
print` (print vs print_slice, rich alphabet) green; full `vt-diff --features
reference` **0-divergence** vs the `77190bd02` oracle (corpus + afl + generative
sweep + hand + formatter); workspace tests + release lane (5×) + paranoid lane
(1634) green; Miri clean on the print path (scalar fallback); parser fuzz clean;
fmt/clippy/check clean.
