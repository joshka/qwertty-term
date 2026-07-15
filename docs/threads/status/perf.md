# perf status

- **Current item:** Session 1 — **region-scroll fast path shipped as PR #266** (open for
  visibility; gate + oracle + Miri + fuzz all green; whole-app A/B done). **Awaiting Josh** on
  the frozen-pin decision for the bottom-region suites (see below). See `## Session 1`.
- **Last merged:** none yet (PR #266 open — <https://github.com/joshka/qwertty-term/pull/266>)
- **ESCALATION (Josh):** closing the two `scrolling_bottom_*` suites needs upstream 77190bd02's
  **change 1** (skip scrollback creation on no-scrollback screens), which is a **semantic**
  change vs our frozen pin `2da015cd6` (fails the differential oracle — proven). Options:
  (a) **pin bump** to ≥ 77190bd02 (moves the frozen oracle forward), or (b) a separate PR
  optimizing the scrollback-creating `cursor_scroll_above` path without changing its result.
  Which do you want? Details in `docs/analysis/scroll-region-opt.md`.
- **Last merged:** (none yet this thread; succeeds T1's engine-perf lane — #227 last)
- **Blockers:** none
- **Claims:** (2026-07-14, PR: cursor_scroll_region_up port) `crates/qwertty-term-vt/src/`
  `terminal/mod.rs` (index() region routing), `screen/mod.rs` (new
  `cursor_scroll_region_up`), `pagelist/mod.rs` (new `shift_tracked_pins_region_up`),
  `page/page_impl.rs` (new `rotate_rows_once_left`). All were vt-tails territory; vt-tails
  CLOSED (no claims). Additive new fns + one routing rewrite in index(). Drop on merge.
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
- 2026-07-14: **session 1 CLOSEOUT — respawn to continue.** State: PR #266 OPEN (not
  self-merged — engine hot-path change worth a look + a Josh pin-bump decision is attached;
  see Blockers/ESCALATION above). A fresh session resumes from this status file + the spec:
  (1) if #266 CI is green and Josh is OK, self-merge it per policy; (2) the bottom-region
  suites are Josh-gated (pin bump vs. optimize cursor_scroll_above — his call); (3) the next
  unblocked perf lane is the wide/CJK engine gap (unicode engine-only ~2.6× behind upstream —
  SIMD UTF-8 decode / tighter wide-print; T1's deferred items in `docs/analysis/perf.md`), or
  optimizing the scrollback-creating `cursor_scroll_above` path (bottom-region, semantics-
  preserving alternative to change 1). Bench harness + method: `docs/analysis/scroll-region-opt.md`.
