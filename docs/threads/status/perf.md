# perf status — 🗄️ RECYCLED 2026-07-15 (print-scan lever COMPLETE — PR-1 + PR-2 merged)

> **RECYCLED — the print-scan lever is DONE (both PRs merged, verified ancestors of origin/main).**
> This session: profile-first pivot → retired the NEON UTF-8 decode lever on evidence (pipeline is
> print-bound, not decode-bound) → shipped both print-scan PRs. Merged: **#304** (`6f8735ef`, pivot
> docs), **#305** (`47f42f46`, PR-1 run_len NEON prescan `latin1_narrow_prefix`), **#306**
> (`30819e43`, mid status), **#307** (`6c113b04`, PR-2 cell-scan NEON `simple_cell_prefix`).
> **Full-pipeline wins (real vtebench cell payloads, compounding): light_cells ~+14%, medium_cells
> ~+11%, dense_cells ~+9%** vs the pre-session baseline (PR-1 +5–8%, PR-2 +4–6% on top); redraw
> ~+24% combined. All gates green each PR (0-divergence differential vs `77190bd02`, release +
> paranoid lanes, parser fuzz, Miri, boundary tests, cross-platform CI). Analysis + numbers:
> `docs/analysis/print-slice-scan.md`.
>
> **BACKLOG NOW DRAINED to the machine-blocked scoreboard.** The print-scan lever is fully mined
> (the remaining `print_slice_fill` fill-writes line is ~4% + write-side/higher-risk → not worth
> it). Decode retired. The only remaining perf deliverable is the **whole-app vtebench scoreboard
> refresh**, which needs a genuinely quiet box (never quieted this session). A fresh thread should:
> (a) poll `uptime`/`ps` and run the scoreboard when quiet (see the item below), or (b) if new
> upstream perf commits appear, port them. To resume, spawn off this file; `jj new main@origin`
> before any edits.
>
> ---
>
> **ACTIVE (respawn 2026-07-15, Opus).** Profile-first pass over the whole stream→print
> pipeline **retired the NEON UTF-8 decode lever on evidence** and **promoted a concrete,
> representative print-scan lever**. Full writeup + numbers: `docs/analysis/print-slice-scan.md`.
> Key data (M2 Max, release, machine contended → ratios/self-time hold):
> **every stream is print/execute-bound, not decode-bound** (print is 44–92% of full-pipeline
> time; decode+dispatch ≤56%). The scalar decode is already SWAR-optimized (u64 ASCII scan +
> multibyte fast path; DFA only for ill-formed), and its NOOP ceiling (ascii 1623, cjk 1305,
> mixed-utf8 855 MiB/s) is above what any FULL stream hits — so a NEON decoder helps only a
> speculative decode-only consumer at the codebase's highest differential risk. **Not worth it.**
> The hot print work is `print_slice_fill::<narrow>` (~20–27% of the real light/medium_cells
> vtebench payloads), whose two hottest lines are **read-only find-first scans**: the `run_len`
> scan (`print.rs:246`, u32, 4-lane) and the `simple-cell` scan (`print.rs:372`, u64, 2-lane) —
> the exact shape of the shipped `apc_scan_prefix_neon` lever (#289), low-risk (read-only, scalar
> fallback, cfg-gated, differential+fuzz catch any wrong length). Run lengths on the real payloads
> are long (median ~28–31 cells, 66–76% ≥16) → NEON-favorable (representative-workload checked, no
> hash_map churn-trap). **Plan:** PR-1 run_len narrow prescan (safe slice, hottest+lowest-risk),
> then PR-2 simple-cell scan (needs Miri on the pointer walk). Machine still not quiet for the
> scoreboard (WindowServer ~47%, loadavg >3, projclean+Ghostty.app running).
>
> ---
>
> **[archived recycle banner]** Session recycled after completing the hash_map backward-shift
> lever + fixing a fleet-wide
> stale-oracle bug. Both PRs MERGED to origin/main (verified ancestors): **#297** `0babde7a`
> (backward-shift deletion) and **#303** `d0a62c8c` (80% hyperlink load factor) — the full faithful
> port of the upstream hash_map cluster (`fedd42e8d` + `7e14347c1`; `65f953e8e` already present).
> Both oracle-neutral (**no pin bump** — proven, not assumed). Analysis:
> `docs/analysis/hash-map-backward-shift.md`. **To resume perf work, spawn a fresh thread off this
> file.** Oracle infra now CORRECT: `77190bd02` lib installed at `~/local/ghostty/zig-out/lib`
> (stale Jul-7 lib backed up to `lib-backup-stale-jul7-2da015cd6/`); `~/local/ghostty-pin77190`
> worktree intact. Backlog for the next thread is below.
>
> **PR-1 (backward-shift deletion) — MERGED `0babde7a` (#297).** Replaced tombstone deletion with
> backward-shift (Knuth §6.4-R) in `page::offset_map`. **Profile-first correction:** the first
> churn bench misled (same-key churn is cheap on raw tombstones → backward-shift looked slower);
> the *representative* workload is **sliding-window churn** (cells at *different* offsets), where
> tombstones accumulate — there backward-shift is **9.3× faster @50%, 18× @90%** (criterion
> `benches/hash_map.rs`), plus canonical lookups, at the cost of a narrow same-key-churn
> regression at high load (matches upstream's tradeoff). **Oracle-neutral** (internal algorithm
> swap): proved divergence-neutral (generative sweep 259 identical seeds w/ & w/o the change,
> **zero** hyperlink/grapheme) → **no pin bump**. Gates: unit 15/15 (+ oracle differential),
> release lane 1631, corpus/afl/hand/formatter differential green, Miri 15/15, resize fuzz 85 257
> runs clean, fmt/clippy/check clean.
>
> **PR-2 (80% hyperlink load factor, `7e14347c1`) — MERGED `d0a62c8c` (#303), ORACLE-NEUTRAL.**
> Defaulted const generic `OffsetHashMap<K, V, const MAX_LOAD: u8 = 100>` (hyperlink map = 80,
> grapheme stays 100), `layout_for_size` scaling + `max_load()` ceiling, `hyperlink_capacity()` →
> `max_load()`. **Evaluated the pin-bump question empirically: NO pin bump.** The full `vt-diff
> --features reference` suite is 0-divergence vs the (now-correct) `77190bd02` oracle — page growth
> is lossless, so 80%-vs-100% fill timing is invisible. Bounds the full-map probe cliffs (the map
> now operates at ≤80% fill). Gates: 1631 lib + release lane, differential 0-divergence, Miri
> 15/15, resize fuzz 87 561 runs clean, fmt/clippy. `65f953e8e` (no-clobber moves) already present.
> The full faithful port (backward-shift + load factor) is complete and cheap.
>
> **✅ Fixed a stale-oracle repo issue (fleet-wide).** The installed reference lib
> `~/local/ghostty/zig-out/lib/libghostty-vt.a` was the old Jul-7 `2da015cd6`-era artifact
> (the prior pin-bump install updated only symlinks/xcframework, not the `.a`/`.dylib`), leaving
> main's generative sweep red at **259 scrollback-class divergences** — orthogonal to hash_map.
> Installed the correct `77190bd02` build (already present un-installed in `~/local/ghostty-
> pin77190/zig-out/lib`) into the oracle path; **default `vt-diff --features reference` now passes
> at 0 divergences** — zero-divergence invariant restored for every thread. PR-1 re-verified 0
> against the correct oracle. issues.md item marked resolved.
>
> **Vibes scoreboard (Josh-requested, our numbers vs 2026-07-13 baseline):** 1-round, loadavg ~7,
> mean-not-median → DIRECTIONAL ONLY, not written to the baseline doc. No regression anywhere;
> region-scroll suites down ~1.5–1.7 ms (18→16.4) consistent with the shipped region-scroll
> levers. A real 3-round median refresh still wants a quiet box.

- **Current item:** none active — **print-scan lever COMPLETE**; recycled. Backlog:
  - **(DONE) print-scan NEON lever** — both PRs merged. **PR-1** run_len prescan `latin1_narrow_prefix`
    (u32, 4-lane, `#305` `47f42f46`); **PR-2** cell-scan `simple_cell_prefix` (u64, 2-lane, `#307`
    `6c113b04`). Read-only find-first over the `print_slice_fill` scans (the real print bottleneck).
    Compounding full-pipeline win on the real vtebench cell payloads (~+9–14%). Analysis
    `docs/analysis/print-slice-scan.md`. **Fully mined** — the residual fill-writes line (~4%) is
    write-side/higher-risk, not worth it.
  - **(DONE) hash_map backward-shift + load factor** — both PRs merged; full faithful port complete,
    oracle-neutral. Analysis `docs/analysis/hash-map-backward-shift.md`.
  - **(RETIRED) SIMD NEON UTF-8 decode** — profile shows decode is not the bottleneck (print is);
    would only lift a NOOP ceiling nothing hits, at max differential risk. Evidence in the analysis
    doc (`print-slice-scan.md` Finding 1). Superseded by the print-scan lever.
  - **(blocked) whole-app vtebench scoreboard refresh** — the mission's remaining "Done" deliverable;
    BLOCKED on a quiet machine (re-checked 2026-07-15: WindowServer 47%, loadavg 8.75 rising,
    mediaanalysisd 69%, Josh active on Firefox → the render-heavy region suites are contended and
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
- **Last merged:** **#307** (print cell-scan NEON prescan, `6c113b04`, +4–6% real cell payloads on
  top of PR-1); **#306** (mid status, `30819e43`); **#305** (print run_len NEON prescan, `47f42f46`,
  +5–8%); **#304** (profile-first pivot docs, `6f8735ef`); **#303** (80% hyperlink load factor,
  `d0a62c8c`); **#297** (backward-shift, `0babde7a`); **#289** (APC SIMD, `50e9814f`); **#287** (APC
  bulk dispatch, `8fa6772a`); **#283** (`9e51aad3`); **#277** (`2708b267`).
- **Blockers:** the **scoreboard refresh** remains machine-blocked — needs a genuinely quiet box
  (WindowServer idle, no sibling GUI app; re-checked end of this session: CGPDFService 91% +
  WindowServer 47% + projclean → still contended). **Workspace:** `work/perf` live; both PRs merged,
  nothing uncommitted of value.
- **NEXT (top unblocked, fully specced):** **PR-2 — simple-cell scan** (`print.rs` `print_slice_fill`
  simple-cell/style-run scans, `print.rs:372` + `430`). The second read-only find-first scan (~8–13%
  of the real cell payloads, next after PR-1's run_len). NEON masked-compare find-first over the
  destination `u64` cells: load 2 cells/`vld1q_u64`, `vand` with `Cell::SIMPLE_MASK`, `vceq` vs the
  splatted `check_expected`, find first non-matching lane. **Needs Miri on the pointer walk** (unlike
  PR-1 it reads `(*base_cell.add(i)).cval()` raw pointers, not a safe slice). Same rigor: boundary
  test at every 2-lane edge, differential 0-divergence vs `77190bd02`, parser+resize fuzz, before/
  after A/B. Design + context in `docs/analysis/print-slice-scan.md` ("Decision & plan").
  `jj new main@origin` first (PR-1 is merged) — do NOT stack on PR-1's change-id.

## Session — respawn 2026-07-15 part 4 (Opus) — profile-first pivot to print-scan

- Bootstrapped `work/perf` fresh (predecessor deleted). Read AGENTS.md, threads/README, this
  status, both method-template analysis docs (hash-map + apc). Pin `77190bd02` confirmed; oracle
  intact. Machine not quiet (WindowServer ~47%, loadavg >3) → scoreboard still blocked.
- **Profile-first over the whole pipeline** (`profile_streams` NOOP-vs-FULL sweep + samply
  line-level on real vtebench `light/medium_cells`). Two decisive findings: (1) **every stream is
  print/execute-bound, not decode-bound** → **retired the NEON UTF-8 decode lever** (it lifts a
  NOOP ceiling nothing hits, at max differential risk); (2) the hot print work is two **read-only
  find-first scans** in `print_slice_fill::<narrow>` (run_len `print.rs:246` u32; simple-cell
  `print.rs:372` u64) — representative (hot on the real scoreboard payloads), long runs
  (median ~28–31 → NEON-favorable), low-risk (APC-scan precedent). Wrote
  `docs/analysis/print-slice-scan.md`. Landed the analysis + pivot as **#304** (`6f8735ef`, doc-only,
  self-merged gate-green).
- **Shipped + self-merged PR-1** (`#305`, `47f42f46`, run_len narrow prescan `latin1_narrow_prefix`).
  A/B best-of-5: real payloads light_cells +7.6–8.0%, medium_cells +5.3–6.3%, dense_cells +5.2%
  (ascii/redraw +10–12% synthetic). Full gate green (differential 0-divergence, release+paranoid
  lanes, parser fuzz 1.04M, Miri scalar, boundary tests); CI green macOS (NEON) + Linux (scalar
  fallback); verified ancestor of origin/main. Signing was locked → pushed via
  `git push origin <hash>:refs/heads/<branch>` (jj push signing workaround).
- Note: hit the divergent-change hazard (#304 merged carrying change-id `sntkpyso`; the local twin
  held PR-1's code) — recovered by restoring from the explicit commit_id + abandoning the twin.
  Lesson (reinforces `jj-new-before-next-PR`): `jj new main@origin` immediately after pushing a PR,
  before touching the next one's files.
- **Recycled** here: PR-2 (simple-cell scan) is fully specced above + in the analysis doc. Context
  was long after the profiling + two PRs; a fresh session resumes PR-2 cheaply from this file.
- **Resumed on Josh's "pr 2?" nudge — shipped + self-merged PR-2** (`#307`, `6c113b04`,
  `simple_cell_prefix`). `Cell` is `#[repr(transparent)]` u64, so the NEON `u64` load reads exactly
  `cval()`; verified the layout before implementing. A/B (baseline = main WITH PR-1): incremental
  redraw +10–13%, light_cells +6.1–6.3%, medium_cells +5.0%, dense_cells +3.9%, ascii +7.5% —
  compounds with PR-1 to ~+14% light_cells vs pre-session. Full gate green (0-divergence, release +
  paranoid 1637, parser fuzz 734,860 runs, Miri clean on helpers + real print_slice integration
  tests, boundary tests, cross-platform CI). Verified ancestor of origin/main. **Both print-scan
  levers now done → backlog drained to the machine-blocked scoreboard. Recycled.**

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

## Session — respawn 2026-07-15 (Opus)

- Bootstrapped `work/perf` fresh (predecessor workspace was deleted; name was free). Read
  AGENTS.md, threads/README, this status file, `docs/analysis/perf.md` +
  `scroll-region-opt.md`. Confirmed pin at `77190bd02`, oracle infra intact.
- Machine check: loadavg **8.75/7.73/6.60** (rising) on 12 cores, WindowServer 47%,
  mediaanalysisd 69%, Josh active on Firefox → **scoreboard blocked** and no clean perf numbers
  obtainable. Won't publish contaminated numbers.
- Sibling scan: no thread names `perf` as a blocker; my Inbox empty. No cross-thread asks.
- **Upstream perf scan (bootstrap item 3):** `git -C ~/local/ghostty fetch` → 81 commits touch
  `src/simd`/`src/terminal` since the pin. Perf-relevant: `8c523ed03` (APC SIMD scan, +42%
  kitty-graphics — strongest net-new lever; our path is per-byte `ApcPut(u8)`), and the
  `hash_map.zig` backward-shift-deletion cluster `fedd42e8d`/`7e14347c1`/`65f953e8e` (~5.5% cell
  move, big + pin-bump-coupled). Rest are search/generation-marker correctness, not perf.
- Un-archived this file; recorded findings; presented Josh the go/hold decision on the APC
  vectorize vs. hold-for-scoreboard. Awaiting steer.

## Session — respawn 2026-07-15 cont. (Opus) — APC lever shipped

- Josh chose "start APC SIMD vectorize". Profile-first (`profile_streams kitty`, new APC/kitty
  stream generator) showed the path was parser-APC-bound (~42 MiB/s, per-byte `ApcPut(u8)`).
- Upstream scan found the real lever is **two** post-pin commits: `f6f79acce` (bulk-slice
  dispatch, ~25× upstream) then `8c523ed03` (SIMD on top, ~1.69×). Shipped both, split one per
  PR with before/after numbers per the perf-thread method.
- **#287** (bulk dispatch): kitty ~42 → ~294 MiB/s (~7× whole-path). Scalar; no unsafe.
  New `Handler::apc_put_slice` trait method (default loops `apc_put`), `Stream::consume_apc_string`,
  `apc::Handler::feed_slice`. Equivalence tests (bulk vs per-byte, every slice split + max_bytes),
  differential green, 730k-run fuzz. CI GREEN.
- **#289** (SIMD, stacked): ~294 → ~338–347 MiB/s (~+15%). `apc_scan_prefix_neon` (aarch64,
  `cfg(not(miri))`) + scalar fallback. Boundary test (control byte at every 16-byte edge), 384k-run
  fuzz with NEON active, Miri clean (scalar path), differential green.
- Both open for Josh (no self-merge authority); monitoring CI. Analysis:
  `docs/analysis/apc-bulk-dispatch.md`. Scoreboard still the only remaining "Done" item, still
  machine-blocked.

## Session — respawn 2026-07-15 part 3 (Opus) — APC merged, thread recycled

After shipping #287/#289, Josh granted self-merge + pin-bump authority + cleanup latitude.

- Merged both (rebase): #287 `8fa6772a`; retargeted + rebased #289 onto new main, merged `50e9814f`.
- Re-verified the rebased #289 combo (check/apc-tests/differential green) before merge.
- Confirmed both are ancestors of origin/main (merge-race check); remote branches auto-deleted.
- Scoreboard re-checked: loadavg eased to ~3.7 but WindowServer ~47% + a sibling ran the app; NOT run.
- Recycled: closeout above; workspace forgotten + deleted. Next: hash_map backward-shift lever.
