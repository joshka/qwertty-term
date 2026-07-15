# perf status — 🗄️ ARCHIVED 2026-07-15

> **Thread retired.** The competitive-perf mission is complete: **all four vtebench
> region-scroll suites closed** (the last DoD gap) and the **wide/CJK engine path optimized end
> to end**. Shipped + merged: **#266** (region-scroll change 2), **#269** (change 1 + frozen-pin
> bump `2da015cd6`→`77190bd02`), **#277** (unchecked interior UTF-8 decode, cjk decode +41%),
> **#283** (wide-cell pair-write, cjk +4%). The `perf` jj workspace was forgotten + deleted and
> this session archived. **To resume perf work, spawn a fresh thread** off this file — the two
> remaining items are (1) the whole-app vtebench **scoreboard refresh** (the mission's final
> "Done" deliverable, BLOCKED only on a quiet machine — run `scripts/bench-vtebench.sh` across
> all three terminals when loadavg is below ~3 and WindowServer is idle, then refresh
> `docs/benchmarks/vtebench-baseline.md`) and (2) an optional **SIMD NEON decode** lever (no
> longer the cjk-full bottleneck post-#277 — a decode-heavy-embedded nicety for its own focused
> session). Oracle infra to keep: the installed `77190bd02` lib at `~/local/ghostty/zig-out/lib`
> and the `~/local/ghostty-pin77190` build worktree. Full history + resume detail below.

- **Current item:** 🗄️ **ARCHIVED** (see banner above). Thread retired after #266/#269/#277/#283
  all merged + the pin bump. Remaining backlog for a fresh thread (all gated or big-effort):
  - **(1) whole-app vtebench scoreboard refresh** — the mission's remaining "Done" deliverable;
    BLOCKED on a quiet machine (re-checked 2026-07-15 repeatedly: WindowServer 43–44%, loadavg
    ~5–6, Josh's interactive apps active → the render-heavy region suites are contended and
    would read 3–4× inflated on ALL builds; see the A/B caveat in
    `docs/analysis/scroll-region-opt.md`). Run `scripts/bench-vtebench.sh` across all three
    terminals (qt, ghostty-main, ghostty-1.3.1), 3 load-gated rounds each, when loadavg is below
    ~3 and WindowServer is idle; then refresh `docs/benchmarks/vtebench-baseline.md`.
  - **(2) SIMD NEON UTF-8 decode** — a decode lever, but NOTE post-#277 decode is NO LONGER the
    cjk bottleneck (noop ~1200 MiB/s > upstream's full pipeline; the full-pipeline cost is now
    print-bound). SIMD would raise decode-only throughput (matters for decode-heavy embedded
    consumers) but won't move cjk *full* much. `std::arch::aarch64` NEON is stable + no
    dependency; gate `cfg(target_arch="aarch64")` + scalar fallback. Large + differential-
    CRITICAL → its OWN focused session; lower priority now given the bottleneck moved to print.
  - **(3) print-side wide lever** (`print_slice_fill<WIDE>`, now ~70% of cjk) — #283 took the
    clean `/2` slice (+4%). What remains (the per-row simple-check read pass, the width lookup in
    run_len) is correctness-load-bearing / already-minimal → diminishing returns, higher risk.
    Only pursue with fresh line-level profiling showing a concrete hot spot.
  - **(4) font/sprite pin-delta verification** (routed to T2/sprite in `issues.md`).
- **Last merged:** **#283** (wide pair-write, `9e51aad3`); **#277** (unchecked interior UTF-8
  decode, `2708b267`); **#269** (change 1 + pin bump, `36256c78`); **#266** (change 2, `0fb53969`).
- **Blockers:** none. **Workspace:** forgotten + deleted (thread retired).

## Pin bump 2da015cd6 → 77190bd02 (Josh approved "fine to pin bump") — STATE

**Done (this session):** de-risked + built + code-ported the VT-engine half.

- Sized it: `2da015cd6..77190bd02` = **14 commits**, most already ported by T1 as new perf work
  (behavior-identical → oracle-neutral). Built the new-pin oracle at
  **`/Users/joshka/local/ghostty-pin77190/zig-out/lib`** (git worktree of `~/local/ghostty` at
  `77190bd02`; do NOT delete — the change-1 gate needs it). Against it, ONLY the change-1
  scroll-region divergences appear (259); curated corpus + afl + hand differential all green →
  **no other semantic delta for the vt engine**.
- Ported change 1 (commit `kwzluoswxpsu`): the `no_scrollback` gate in `index()`
  (`!no_scrollback || bottom==0`) AND `scroll_up`/CSI-S (`!no_scrollback || bottom==rows-1`),
  plus restored `cursor_scroll_region_up`'s non-zero-blank (`fill_cells`) branch to match
  upstream's full `cursorScrollRegionUp`. Result: **generative sweep 259→0 vs the 77190bd02
  oracle** (x2), differential + afl green, release lane + 1618 lib tests green. (Change 1's only
  observable difference — transient scrollback on a no-scrollback screen — is invisible to
  visible-grid tests, so all in-crate tests passed unchanged; it's user-visible-identical.)

**DONE (Josh authorized "merge 266 … and do the recommended steps"):**

1. ✅ **Shared oracle bumped.** Built libghostty-vt at `77190bd02` in a `~/local/ghostty`
   worktree (`~/local/ghostty-pin77190`) and installed the lib set into the default path
   `~/local/ghostty/zig-out/lib/` (old `2da015cd6`-era `.a` backed up to
   `zig-out/lib-backup-2da015cd6/`). The source checkout at `~/local/ghostty` (repro commit
   `38e49a232`, uncommitted files) was left untouched — only the built artifact in `zig-out`.
   Default `cargo test -p vt-diff --features reference` now runs the change-1 code GREEN with
   no env override. (To rebuild reproducibly: `cd ~/local/ghostty && git checkout 77190bd02 &&
   zig build -Demit-lib-vt=true -Doptimize=ReleaseFast`.)
2. ✅ **Authoritative pin docs bumped** to `77190bd02`: `AGENTS.md` (with a bump note),
   `docs/handoff.md` (build recipe), `crates/vt-diff/src/ffi.rs` (C-API source-of-truth). The
   226 historical per-file "ported from `2da015cd6`" provenance comments are left as-is (they
   record original port origin; the differential oracle is the authority).
3. ✅ **font/sprite tracked** in `docs/threads/status/issues.md` Inbox (3 cursor-height commits
   `cabbdee32`/`dac341cad`/`e8f3f6c43` owed a T2/sprite parity check).

Full analysis: `docs/analysis/scroll-region-opt.md`.

## Claims

- (2026-07-14, PR #266 + change-1 commit) `crates/qwertty-term-vt/src/`: `terminal/mod.rs`
  (index() + scroll_up region routing), `screen/mod.rs` (`cursor_scroll_region_up`),
  `pagelist/mod.rs` (`shift_tracked_pins_region_up`), `page/page_impl.rs`
  (`rotate_rows_once_left`). All were vt-tails territory; vt-tails CLOSED. Drop on merge.
- **Inbox:** (other threads append requests here; owner triages into backlog)

## Standing state (from orientation, 2026-07-14)

- **Competitive standing** (`docs/benchmarks/vtebench-baseline.md`, refreshed 2026-07-13 @
  post-#204, pre-#227): qwertty-term ties/wins 6/10 vs Ghostty main; wins all 10 vs 1.3.1.
  The ONLY remaining loss vs main is the **4 region-scroll suites at 1.13–1.20×**.
- **A/B target**: Ghostty main `91f66da24` (built at `~/local/ghostty-main`). Fetched
  upstream 2026-07-14: 112 commits since our A/B pin, but no major NEW cell-write perf work
  in src/terminal/simd (mostly search/correctness + `8c523ed03` APC SIMD scan). So the built
  A/B bundle remains a fair current comparison — no rebuild needed to measure the gap.
- **Why the region-scroll gap persists**: #204 (port of upstream `77190bd02`) deliberately
  routed region scroll through the existing `erase_row_bounded` machinery rather than
  upstream's bespoke single-page rotate (which mishandled wrapped wide-cell spacer heads).
  The residual ~13–20% is that generic-path overhead. Closing it = port the bespoke rotate
  with correct wide-spacer-head handling. Path lives in `terminal/mod.rs` (index()/CSI S),
  `screen/mod.rs`, `pagelist/resize.rs` — all now free (vt-tails CLOSED).
- **Not the target** (per DoD): the `unicode` engine gap (~2.6× behind engine-only) is a
  whole-app *render* artifact in vtebench (we show 0.50× = 2× ahead). Real engine work but
  invisible to the DoD; deferred behind the region-scroll win.

## Session 1 — region-scroll fast path (port of upstream cursorScrollRegionUp)

**Shipped (pending PR):** `cursor_scroll_region_up` — change 2 of upstream `77190bd02`. The
old `index()` region path used `erase_row_bounded` + a Point→Pin walk + `cursor_down(1)`
re-resolution + `manual_style_update` every scroll; the new fast path clears the top row +
`rotate_rows_once_left` + direct cursor-pointer refresh, using `cursor.page_pin` directly.
Engine-only (`profile_streams scroll-region`, M2 Max, release): **80×24 48.5→70.8 MiB/s
(+46%), 120×40 45.5→63.0 (+38%)**.

**KEY FROZEN-PIN FINDING (route to Josh):** `77190bd02` landed ~12h AFTER our pin
`2da015cd6`. It has TWO changes; only **change 2** (the bespoke rotate) is a pure perf port.
**Change 1** (skip scrollback creation for top-anchored regions on no-scrollback screens) is a
**semantic** change relative to our pin — the reference oracle retains the scrolled-out rows,
so porting it fails the differential (proved: it caused all 122 generative-sweep divergences;
dropping it → 0). So the two **bottom-region** suites (`scrolling_bottom_region` /
`_small_region`, top==0) CANNOT be closed without either (a) a **pin bump** to ≥ `77190bd02`
(moves the frozen oracle forward — Josh's call), or (b) a separate PR optimizing the
scrollback-creating `cursor_scroll_above` path without changing its result. The **top-region**
suites (`scrolling_top_*`, top!=0) ARE closed by this change. Full writeup:
`docs/analysis/scroll-region-opt.md`.

**Whole-app A/B (same machine+session, clean-main parent vs this change, medians):**
`scrolling_top_region` **87→54 ms (0.62×, ~1.6× faster)** — the suite change 2 targets;
top_small 0.92; unchanged bottom/fullscreen paths 0.93–0.96 (flat, as expected); dense/medium/
scrolling/unicode 0.86–1.00. NOTE: absolute region-scroll ms are ~3–4× the 2026-07-13
scoreboard purely from current machine GUI/WindowServer load (present on BOTH builds equally —
NOT a code regression); refresh the published three-way table on a quiet machine.

Gate: check/clippy/fmt clean; workspace tests + release lane + paranoid lane green (1545/1545);
`vt-diff --features reference` differential + corpus + afl + 20k generative sweep all green;
resize fuzz 83,117 runs no crash; Miri clean on the new unsafe (`index_region_scroll_fast_path`).
New tests: `hand_scroll_region_fast_path` (vt-diff, wide+deep), `index_region_scroll_fast_path`
(in-crate). Files: `page/page_impl.rs`, `pagelist/mod.rs`, `screen/mod.rs`, `terminal/mod.rs`
(+tests), `vt-diff/tests/differential.rs`, `docs/analysis/scroll-region-opt.md`.

## Log

- 2026-07-14: session 1 start — created `perf` workspace off main; read AGENTS.md,
  threads/README, vtebench-baseline, doomfire, T1 + vt-tails status. Confirmed vt-tails
  CLOSED (scroll-region files free). Fetched upstream ghostty (112 commits since A/B pin,
  no major new cell-write perf).
- 2026-07-14: shipped **PR #266** (region-scroll fast path). Profiled the region-scroll path
  (`profile_streams scroll-region`), ported upstream 77190bd02 change 2 (cursorScrollRegionUp),
  debugged the differential (found change 1 is post-pin semantics — 122→0 divergences by
  dropping it; then found the non-zero-blank wide-spacer-head divergence → restricted the fast
  path to zero blank). Full gate + oracle + Miri + resize-fuzz green; whole-app A/B vs
  clean-main parent shows `scrolling_top_region` 0.62× (~1.6× faster), no regression. CI
  running on #266 (markdownlint pass; Linux + macOS pending at handoff).
- 2026-07-14: session 1 — shipped #266 (change 2). Escalated the frozen-pin question for the
  bottom-region suites (change 1 is post-pin semantics).
- 2026-07-15: Josh approved the pin bump + "merge 266 and do the recommended steps." Executed:
  merged **#266** (change 2, `0fb53969`); sized the pin bump (14 commits, only change-1 VT
  divergences); ported **change 1** (index + scroll_up no_scrollback gates + non-zero-blank
  fill); **bumped the oracle** to `77190bd02` (built in `~/local/ghostty-pin77190`, installed
  the lib into the default path, old lib backed up to `zig-out/lib-backup-2da015cd6/`); bumped
  the authoritative pin docs (AGENTS.md / handoff.md / vt-diff ffi.rs); tracked the 3
  font/sprite cursor-height commits in `issues.md`; merged **#269** (change 1 + pin bump,
  `36256c78`). Verified green vs the new oracle: generative sweep 259→0, differential, corpus,
  afl, release + paranoid (1618), Miri, resize fuzz 76k. **All 4 region-scroll suites now
  addressed.** Next: quiet-machine vtebench scoreboard refresh; then the wide/CJK engine gap.
