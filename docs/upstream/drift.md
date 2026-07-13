# Upstream drift watch

Tracks upstream Ghostty commits landed since the port's pin, classified by whether the port
should mirror them. Maintained by thread T8. Re-run each session: `git -C ~/local/ghostty fetch`
then diff `origin/main` against the pin for the ported subsystems.

- **Port pin:** `2da015cd6` (2026-07-06)
- **Upstream main at last scan:** `a3ac713b7` (2026-07-12, drift pass 2)
- **Last scan:** 2026-07-12 (T8, drift pass 2 — incremental `a887df42c..a3ac713b7`)
- **Range (cumulative, pin → pass-1 scan `a887df42c`):** 102 non-merge commits upstream since the
  pin; 92 unique touch a ported subsystem (`src/terminal/` 80, `src/renderer/` 9, `src/termio/` 5,
  `src/font/` 3, `src/Surface.zig` 1, `src/input*` 0). See the classification below.

## Drift pass 2 (`a887df42c..a3ac713b7`, 2026-07-12) — CLEAN, no must-mirror items

Only 2 non-merge commits upstream since pass 1; 1 touches a ported subsystem:

- `9659167ec` — `terminal/search`: reuse viewport fingerprint storage. **irrelevant (perf-only,
  no behavior change)** — retains the fingerprint's backing slice instead of reallocating per
  update; skips allocation for unchanged viewports. Pure allocation-reuse refactor of
  `search/viewport.zig`; nothing to mirror for correctness. Low-priority perf-mirror candidate
  for whoever ports viewport search (not on a vtebench hot path). No Inbox line filed.
- `a3ac713b7` — "Update VOUCHED list (#13309)". **irrelevant** — upstream contributor-governance
  file, not code.

No new Inbox lines filed this pass. Cumulative classification (pass 1) unchanged below.

Classification: **mirror** = a bug fix in logic the port has (likely) already ported — replicate
it. **feature** = new functionality the port lacks — owning thread's backlog, not urgent.
**irrelevant** = Zig-build/style/comment-only, perf-only with no behavior change, reverted-to-zero,
or a bug class the Rust type system already precludes.

All `file:line` references below are upstream paths at `a887df42c`; verify against the port's
corresponding Rust module before acting. Every item was read from the actual git diff, not memory.

## Headline: two of our four upstream findings are now resolved upstream

The drift scan directly re-dispositioned the `docs/upstream/` issue drafts (see
`findings-status.md`):

- **Issue 4 (OSC `color_operation` request-list leak)** — **fixed upstream** in `14c829883`
  (2026-07-07). The exact `osc.zig` reset hunk we described now deinits `v.requests`. Do not
  file; instead **port the fix** to the Rust `osc` parser reset (Inbox → T1/T5 below).
- **Issue 3 (OSC 4/10/11/12/21 color queries get no lib-vt reply)** — **implemented upstream**
  in the same commit `14c829883` ("report OSC color queries in lib-vt"), covering the xterm
  color queries and Kitty OSC 21. Do not file; this is now a feature the port lacks at the
  lib layer (Inbox → T5 below).
- **Issue 1 (`highlight.Flattened.init` compile bugs)** — re-verified **still present** on
  `a887df42c` (`highlight.zig:146/151/158`). Remains a live, fileable finding.
- **Issue 2 (`max_scrollback` header says "lines", is bytes)** — re-verified still present
  (`include/ghostty/vt/terminal.h:187`), but already a duplicate of upstream discussion
  \#12769. Do not file a new report.

## `src/terminal/` → `qwertty-term-vt` (T1 perf / T5 features) — 26 mirror items

Data-structure / allocator correctness (owner T1):

- `fedd42e8d` — offset hash map used tombstones with unbounded probe length → O(n) lookup/OOM
  under churn; backward-shift deletion. (`hash_map.zig`)
- `7e14347c1` — offset map allowed 100% load factor; missing-key lookup could scan whole map;
  +20% headroom & rehash (superseded by `fedd42e8d`). (`hash_map.zig:76-820`)
- `65f953e8e` — moving map entries via clobbering insert could exhaust headroom without rehash;
  no-clobber insert + rehash-on-exhaustion. (`hash_map.zig:425-490`)
- `e44f5cb0f` — `RefCountedSet.lookupContext` had no zero-capacity guard → `table[hash & 0]` read
  OOB. (`ref_counted_set.zig:499`) → **DONE (T1 #59)** — confirmed latent in the port; guard +
  `set_zero_capacity` test added.
- `8307349ec` — `increaseCapacity` doubled a dimension to grow it; doubling zero stays zero →
  breaks the growth contract. (`PageList.zig:3299`) → **DONE (T1 #59)** — confirmed latent;
  fixed + `grow_tests`.
- `b953bb346` — `BitmapAllocator` sized chunk region from the wrong variable → over/under-reserve
  → OOB alloc. (`bitmap_allocator.zig:222`) → **No work needed (T1)** — the port is already
  correct (`page/bitmap.rs:66` sizes the slab as `aligned_chunk_count * CHUNK`, a deliberate
  deviation from upstream's buggy `aligned_cap * chunk_size`).

PageList / Pin bounds and integer overflow (owner T1):

- `f1a5fab45` — `page_serial_min` floor advanced on reuse/erase but generations aren't monotonic
  across splits → could reject live pages. (`PageList.zig:3543,4697`)
- `d6e24d985` — Pin vertical/wrap movement assumed uniform page width → OOB / wrong row crossing
  into a narrower page. (`PageList.zig:5389-5450`)
- `0ff4e41b2` — `Pin.leftWrap/rightWrap` mishandled exact-multiple-of-width offsets → invalid
  pin. (`PageList.zig:5389,5416`)
- `30b42f42a` — `pointFromPin` accumulated scrollback rows into an unchecked `u32` → trap past
  2^32 rows. (`PageList.zig:4068`)
- `e6e4a9fdc` — `Cell.screenPoint` accumulated rows in a narrow `CellCountInt` → overflow past
  65,535 scrollback rows. (`PageList.zig:5564`)
- `afbf5ba15` — prompt-scroll delta negation trapped at `minInt(isize)`; fixed via `@abs`.
  (`PageList.zig:2723`)
- `c753fe4a4` — row-scroll delta negation trapped at `minInt(isize)`; fixed via `@abs`.
  (`PageList.zig:2518`)
- `0aaedf436` — `setCursorPos` origin-mode margin add unchecked → overflow before clamp.
  (`Terminal.zig:2070`)
- `0cb004734` — `clearCells` indexed `slice[0]`/`[len-1]` unconditionally → panic on empty/no-op
  clear. (`Screen.zig:1374`)

Selection / reflow width bugs (owner T5) — a cluster where end-pin columns were built from the
desired/global width instead of the owning page's own width:

- `b6f34be44` — `Selection.topLeft/bottomRight` swapped columns without clamping to the corner
  page's own width. (`Selection.zig:155-190`)
- `607160657` — `Screen.clone` fallback selection pins built from desired/global width, not the
  cloned node's width. (`Screen.zig:530-560`)
- `a9f5b7eba` — `Selection.containedRowCached` endpoints from desired width, not the owning page.
  (`Selection.zig:326-395`)
- `a55850c98` — `cursorCellEndOfPrev` set prev row's end column from desired width, not that
  page's own width. (`Screen.zig:651-656`)
- `fa8cae88b` — `selectLine` set end-of-prev-row pin x from the *next* page's width. (`Screen.zig:2809`)
- `0c299000f` — `Screen.select` released tracked pins unconditionally before reassigning →
  `select(self.selection)` double-frees aliased pins. (`Screen.zig:2624-2650`)

Search subsystem (owner T1/T5):

- `5d8eb78b7` — `PageListSearch.feed` left stale pin x/y crossing pages → OOB on a narrower page.
  (`search/pagelist.zig`)
- `5bc6588e4` — `SlidingWindow` search underflowed computing the end offset for an empty needle.
  (`search/sliding_window.zig`)
- `627518447` — search cached dims invalidated only on the feed path → selecting a cached match
  after resize could UAF. (`search/screen.zig`)

Stream / OSC (owner T5, except the leak which is T1):

- `b287f6d1a` — grapheme-break replay `assert` fires legitimately after toggling mode 2027
  mid-stream. (`Terminal.zig:952-965`)
- `14c829883` — `osc.Parser.reset()` missing `.color_operation` cleanup → leaked request list.
  **This is our issue 4 — port the fix.** (`osc.zig:405-411`)

**Cross-check against project memory:** `e44f5cb0f`/`8307349ec` are the same zero-capacity /
growth-doubling class as the RefCountedSet rehash-threshold bug already found in the port
(`zig-port-numeric-semantics` memo); `b287f6d1a` is the Zig-`assert`-always-evaluates hazard
(`zig-port-assert-side-effects` memo). Highest priority to check those three spots in the port
first — it may share the latent bug.

## `src/renderer/` → `qwertty-term-renderer` (T2) — 0 open (1 verified inapplicable)

- `d34b54e9b` — renderer-state mutex is unfair; the termio parse thread's tight relock loop can
  starve the renderer thread in `updateFrame`. Cross-territory — call sites also live in
  `termio/Exec.zig` (T4). (`renderer/State.zig:33-108`, `generic.zig:1170`, `termio/Exec.zig:1486`)
  → **DOES NOT STRUCTURALLY APPLY (2026-07-13, T2): no code change.** Upstream's bug needs the
  renderer to hold the shared state mutex across the *entire* `updateFrame`; the port renders off
  an owned `FullSnapshot` and holds the `Arc<Mutex<Engine>>` only for the microsecond
  `snapshot_window_tracking` copy (`qwertty-term/src/app.rs:658-666`), releasing it before the
  heavy `update_frame`/`draw_and_present`. Residual risk (unfair `std::sync::Mutex`, no
  `parking_lot`) is theoretical and empirically absent — DOOM-fire, the exact heavy-output
  trigger, is visibly smooth on main. Full analysis + options (do-nothing → `parking_lot`
  eventual fairness → full `lockDemand` port) in `docs/analysis/renderer-mutex-starvation.md`.

## `src/termio/` + `src/Surface.zig` → `qwertty-term-termio` (T4) — 0 open (both mirrored/inapplicable)

- `d34b54e9b` — see the renderer item above; the starvation fix's call sites also live in
  `termio/Exec.zig:1486,1776`. → **NO-OP for the port (2026-07-13, T2 analysis; T4 notified):**
  the parse-loop `yieldToDemand` half is unnecessary given the render side's snapshot boundary
  (per the renderer item). No change to `qwertty-term-termio`. See the T4 inbox note.
- `bb0ac4c72` — the pipelined pty gather stage slept the full ~1.2ms poll timeout even after the
  parse stage went idle → doubled frame latency for request/response-style apps; fixed with a
  self-pipe wake. (`termio/Exec.zig:1363-1630`) **MIRRORED (T4): `qwertty-term-termio/src/exec.rs`
  `Pipeline.idle_read`/`idle_write` + `bridging`, parse-idle early-deliver + self-pipe wake in
  `gather_main`, wake-write in the parse loop.**

Note the `bed47168c` → `bb0ac4c72` → `60121a039` sequence: `bed47168c` shrank non-Darwin pty
read-ahead but was reverted by `60121a039` (~20% Linux throughput regression), net zero change.
`bb0ac4c72` is a separate, surviving fix (the mirror item above).

## `src/font/` → `qwertty-term-font` (T2) — 0 open (1 verified already-mirrored)

- `dac341cad` — cursor sprites used `cell_height` instead of `cursor_height` → the
  `adjust-cursor-height` config had no effect. (`font/sprite/Face.zig:205-260`)
  → **ALREADY MIRRORED (2026-07-13, T2): no code change needed.** The port applies *both*
  halves of the upstream fix: height selection (`cursor_height` for rect/hollow-rect/bar,
  `cell_height` otherwise) in `qwertty-term-sprite/src/lib.rs::render` and the
  `(cell_height − draw_height)/2` re-centering in `canvas.rs::into_glyph`. This crate was
  ported from a post-`dac341cad` upstream, so it never carried the regression. Added
  regression tests (`cursor_height_tests` in `lib.rs`) to lock both halves — the exact
  case that "went unnoticed for so long" upstream now fails loudly if a draw-path refactor
  reverts it.

## `src/input*` → `qwertty-term-input` (T3)

No upstream commits touch these paths in the range — no drift.

## Feature backlog (not urgent; owning thread's radar)

1. **Scrollback compression (LZ4)** — ~19 commits. Compresses offscreen scrollback pages while
   not viewed: standalone LZ4 codec, compressed-page boundary in `PageList`, idle-time debounced
   scheduler exposed through `libghostty-vt`. Large; no equivalent infra in the port. Owner T1
   (core) + T2 (renderer scheduling).
2. **PageList generation-renewal + staleness detection** — ~14 commits. A generation/epoch
   primitive on `PageList` mutation, consumed by search/render to detect stale cached references.
   Port lacks this infra. Owner T5.
3. **OSC color-query reporting in lib-vt** (`14c829883`, `0a410f18e`) — **our issue 3**; surfaces
   OSC 4/10/11/12 and Kitty OSC 21 color queries through `libghostty-vt`'s `write_pty` effect.
   Owner T5.
4. **Clipboard protocol-neutral rewrite** (`634ef7198`) — OSC 52 replaced by a generic MIME
   clipboard-write callback / C-ABI. Owner T5.
5. **Perf backlog, no behavior change** — vectorized APC payload scan (`8c523ed03`), bulk APC
   slice dispatch (`f6f79acce`). Owner T1, low priority.

## Performance forensics: v1.3.1 → `91f66da24` (T1 mirror-target list)

Answers the orchestrator's PRIORITY request (main is 1.5–2.7× faster than 1.3.1 across vtebench).

**The biggest wins are already inside our pin.** `2da015cd6`'s own subject is *"terminal: various
VT processing optimizations (~1.5x to ~6x throughput increase)"* — the pin's tree already contains
the 2026-07-06 VT batch (`47e26df60` printSlice 5.7×/2.4×, `1a88f3622` CSI dispatch fast paths,
`253e4f9c3` bulk CSI param parse, `cee35cabf` SGR no-op skip) and the termio IO-pipeline overlap
(`2f0e6659d`). **So most of the 1.3.1→main delta lives in the pinned commit.** T1's first move is
therefore **verification, not mirroring**: confirm `qwertty-term-vt` actually reproduces printSlice
/ the CSI stream fast paths / SGR-eql skip that ship *inside* `2da015cd6`. If the Rust rewrite of
that squash didn't reproduce those mechanics, that gap dwarfs everything below.

Perf commits **ahead** of the pin (`2da015cd6..91f66da24`), ranked by likely vtebench impact:

- `cb2d78587` (vt / cell-write, **high**) — vectorize `printSliceFill` scans + bulk style-id run
  fill. Portable to `qwertty-term-vt`; needs real SIMD masked compares. Directly on the
  dense_cells / medium_cells hot path.
- `446f80f4e` (renderer / lock, **high**) — `RenderState.update`: chunk-iterate, masked compares,
  begin/endUpdate split (2.7–11× less lock hold). Portable across vt + renderer, but the lock-hold
  win only materializes if the port mirrors upstream's renderer-holds-terminal-lock threading
  model. Drives the sync_medium / frame path.
- `77190bd02` (vt / scroll, **high**) — in-place region scroll, skip scrollback for top-anchored
  regions (1.05–1.49× on the scrolling suite). Portable to `qwertty-term-vt`.
- `300f42c7a` (vt / parser, **med**) — inline "ESC [" + csi_entry byte into `consumeUntilGround`.
  Portable, but depends on the in-pin CSI fast-path scaffolding already existing in the port.
- `083d9709b` (simd, **med**) — fuse ASCII widening into the ESC scan, skip simdutf for pure ASCII.
  C++/Highway-specific — needs a Rust SIMD reimplementation, not a line port.
- `8d663a76e` (vt / cell-write, **med**) — release style refs per run (not per cell) in
  `clearCells`. Portable; compounds with `cb2d78587` on erase/redraw.

The `896aca499`/`16e4b5e98`/`b953bb346`/`8307349ec` cluster is page-memory/allocator hardening
(RSS + correctness, not throughput) — low mirror priority for perf, though `b953bb346`/`8307349ec`
are also correctness mirror items above.

## Method notes

- Upstream history in this range is linear (no divergent branches), so path-hit counts double-count
  the 6 commits that touch two subsystems.
- The classification above is drift pass 1 (baseline). Each later pass only scans
  `<last-scan-hash>..origin/main` (pass 2: `a887df42c..a3ac713b7`, clean — see header).
- Re-pin proposal (backlog item) is not yet warranted at 102 commits, but the 26 terminal mirror
  items and the "verify the pin's own perf squash reproduced" finding argue for scheduling the
  port's correctness-mirror sweep before drift compounds further.
