# T1 — Performance thread

**Model:** Opus · **Wave:** 1 · **Workspace:** `work/t1` · **Status:** `status/t1.md`
**Territory:** `crates/qwertty-term-vt` (perf-motivated changes), `crates/vt-diff`
(harness), `scripts/bench-*`, `docs/benchmarks/`. Renderer perf items require a file-claim
(T2 owns the renderer in Wave 1). Rules: `docs/threads/README.md`.

## Mission

Take qwertty-term from "wins 9/10 vtebench suites" to **unambiguously the fastest** and
keep it there: every whole-app suite ≥ parity vs Ghostty main, engine-level ratio band
raised from today's 0.49–0.93× toward ≥0.9×, and a standing regression fence so wins
can't silently rot. Semantics are FROZEN — the differential corpus, fuzz targets, and
release lane referee every change. Every PR carries before/after numbers.

## Context you inherit (read first)

- `docs/benchmarks/vtebench-baseline.md` + the three-way update (in flight in `work/bench3`
  when this thread starts — integrate or rerun as your first status check).
- simd-perf landed the SWAR ascii fast path (382→550 MiB/s, 0.79–0.93×) and left a
  profile + explicit next steps in its commit ("Perf: plain-text fast path…", see its PR/
  describe body). Prior profile: dense_cells = RefCountedSet churn (upstream-identical,
  frozen); sgr/cursor = `[Option<Action>;3]` parser dispatch; utf8-mixed = per-run print
  setup re-firing at wide-char boundaries.
- Field regression: **DOOM-fire ~820fps → ~760fps**, confirmed on idle machine, never
  bisected. Suspects (in order): lig-engine run-cache key (`Vec<char>` alloc per run
  lookup per frame), dirty-tracking snapshot read+clear overhead in all-dirty workloads,
  SWAR scan on escape-dense streams.

## Backlog (ordered; check off in status file, sizes are effort not LoC)

- [ ] **DOOM-fire bisect + fix** (M, FIRST): reproduce the fps measure (find/recreate the
      doom-fire bench used previously — likely a shell script + the app; make it a
      repeatable `scripts/bench-doomfire.sh` while you're there), bisect across the recent
      main commits (lig-engine → dirty-tracking → search → simd), name the culprit with
      numbers, fix without losing that feature's win. Likely fix if run-cache: key by
      precomputed hash + bucket compare, zero allocs on lookup. Evidence: fps restored
      ≥ old baseline, culprit's own tests still green.
- [ ] **dense_cells to parity** (M/L): the one vtebench loss (1.36×). Path: match
      upstream's `printString` batch structure (`src/terminal/Terminal.zig`) more closely —
      hoist style/hyperlink/width checks per-run, batch cell writes; the style-churn
      algorithm itself is upstream-identical, do NOT change ref-counting semantics.
      Evidence: vtebench dense_cells ratio ≤1.0 vs Ghostty main, differential green.
- [ ] **sgr/cursor dispatch** (M/L): internal `Parser` fast path that avoids materializing
      `[Option<Action>;3]` for the single-action common case; public API unchanged (keep
      the array API as a wrapper). Mirror upstream's inlined csi_entry/csi_param bulk
      consume where it applies. Target: sgr-heavy + cursor-heavy from ~0.5× to ≥0.75×.
- [ ] **utf8-mixed print-batch carry** (M): carry mode/charset checks across wide-char
      boundaries so short-run setup stops re-firing (simd-perf report item 1). Target
      ≥0.75× from 0.54×.
- [ ] **CVDisplayLink pacing** (S/M, FILE-CLAIM on renderer/present): replace timer tick;
      match upstream's update throttling. Evidence: no dropped-frame regression in the
      smokes, doom-fire fps unchanged-or-better, battery/idle CPU note.
- [ ] **Perf regression fence** (S): `scripts/bench-quick.sh` (engine streams + doom-fire,
      <60s) with recorded thresholds; document "run before merging perf-sensitive PRs" in
      benchmarks doc; wire into CI later if T8 lands a runner that can.
- [ ] **Refresh three-way table** (S, recurring): after each landed win, rerun
      `scripts/bench-vtebench.sh` lanes and update the baseline doc — the scoreboard is
      the thread's public output.

## Method rules

Profile before optimizing (samply/Instruments; name top-5 with %). One optimization per
PR with its numbers. Anything that changes bytes-on-the-wire or cell state is out —
that's T5's territory or a bug. Fuzz targets (`parser`, `resize`) get a 3-minute foreground
pass when the parser/stream is touched. `unsafe` needs Miri on the touched module +
a comment citing why safe code can't do it.

## Definition of done

vtebench: no suite where Ghostty (1.3.1 or main) beats qwertty-term by >5%. Engine band
≥0.75× everywhere, ≥0.9× on ascii/scroll. DOOM-fire ≥ the old 820fps baseline. Fence
script exists and is documented. Then this thread converts to maintenance (rerun after
big landings) and its seat goes to a Wave-2 thread.
