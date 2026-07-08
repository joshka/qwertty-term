# Font shaping: Collection, resolver, run segmentation, cluster→cell mapping

Analysis of Ghostty's font-collection / codepoint-resolution / text-run /
shaping subsystem and the reduced port that lands as the M3 "first pixels"
font substrate: `Collection`, `CodepointResolver`, `Shaper` (rustybuzz), and a
`Grid` (SharedGrid-reduced).

- **Upstream reference:** commit `2da015cd6` (the designated port baseline).
  The reference tree read for this analysis is the vendored ghostty source at
  `fdbf9ff3a31d7531b691cb49c98fc465a1a503a0`, whose `src/font/` line counts
  match the plan exactly (`Collection.zig` 1523, `CodepointResolver.zig` 562,
  `SharedGrid.zig` 540, `shaper/run.zig` 409, `shape.zig` 102,
  `shaper/Cache.zig` 106), confirming it is the same font subsystem as
  `2da015cd6`.
- **Plan:** `docs/plans/m3-first-pixels.md`, decisions 1 (rustybuzz-first
  shaping), 7 (load-by-name discovery only), 8 (slotmap-style Collection index,
  not the packed u16 bitfield). These are LOCKED; this analysis documents how
  the reduced cut maps onto upstream, not whether to re-litigate them.
- **Scope of the port (this chunk):** single regular style, single primary
  font, sprite dispatch to `qwertty-term-sprite`, rustybuzz shaping of one run,
  cluster→cell mapping, glyph→atlas upload with a codepoint→glyph render cache.
  Everything the plan calls a "completeness pass" (font fallback search, style
  grouping beyond a single slot, ligature/emoji run splitting) is deferred and
  enumerated under "Deferrals" below.

## Collection: style grouping and the index model

`Collection.zig` is "a list of faces of different styles, ordered by priority
per style." It owns no search/rasterization logic (that is
`CodepointResolver`); it is purely a typed store of faces keyed by style and
priority.

### Style grouping (upstream)

Faces are stored in a `StyleArray = std.EnumArray(Style, SegmentedList(Entry))`
(`Collection.zig:697`) — one growable priority-ordered list **per style**.
`Style` is a 4-value enum (`main.zig:54`):

```zig
pub const Style = enum(u3) { regular = 0, bold = 1, italic = 2, bold_italic = 3 };
```

`add(face, .{ .style, .fallback, .size_adjustment })` appends to the list for
that style and returns an `Index { style, idx }` (the idx is the position in
that style's list). Priority is append order: the first face added for a style
is searched first. All faces in a collection share the same *size* (so a glyph
missing in one can be pulled from another interchangeably), and adding a
non-primary face rescales it to the primary via `scaleFactor` +
`size_adjustment` (`ic_width`/`ex_height`/`cap_height`/`none`) so fallback
glyphs visually match the primary's cap/ex/ideograph metrics.

### The index model (upstream packed bitfield) vs the port (slotmap)

Upstream `Index` (`Collection.zig:891`) is a `packed struct(u16)`:

```zig
pub const Index = packed struct(u16) {
    style: Style,          // u3
    idx: u13,              // 13 bits => up to 8192 faces per style
    // Special.start = maxInt(u13); Special.sprite = start (the sprite pseudo-font)
};
```

The top idx value (`u13` max) is reserved as `Special.sprite` — a
pseudo-index that means "this codepoint is drawn by the sprite subsystem, not a
real face." `Index.special()` returns `?Special`; `Index.int()` bit-casts to
u16 for hashing/equality. This packing exists because font indices are stored
per-cell all over the renderer and upstream wants them to be exactly 2 bytes
(there is an inline test asserting `@sizeOf(Index) == 2`).

**Decision 8 (locked): the Rust port uses a slotmap-style / arena index, not
the packed bitfield.** Rationale recorded here: Rust's idiom for "handle into
an arena" is a generational or plain arena index, and the packed-bitfield's
only motivation (2-byte per-cell storage) is a renderer-side concern that the
R4 cell engine can satisfy with its own compact representation if it needs to —
it does not need to leak into the font crate's public handle type. The port
therefore models the index as:

```rust
pub enum FontIndex {
    Face { style: Style, slot: usize },   // arena slot into the per-style list
    Sprite,                               // the sprite pseudo-font (== Special.sprite)
}
```

This preserves the two semantics that actually matter downstream: (1) the
4-style grouping (`Style` is ported as the same 4-value enum), and (2) the
"sprite is a special non-face index" distinction (`FontIndex::Sprite`, the
analog of `Index.initSpecial(.sprite)` / `Index.special()`). Equality and
hashing fall out of `#[derive(PartialEq, Eq, Hash)]` — the analog of upstream
hashing `Index.int()`. The reduced Collection stores a `regular` slot (always
present) plus reserved `Option` slots for `bold`/`italic`/`bold_italic` that
are wired but empty until the style chunk lands; `add` for a non-regular style
is accepted and stored but the reduced resolver never routes to it (it maps
every style request to regular, see below), matching upstream step 1/5's
"prefer a regular loaded font over a styled fallback."

## CodepointResolver: the 7-step resolution chain

`CodepointResolver.getIndex(cp, style, presentation) -> ?Index`
(`CodepointResolver.zig:120`) is the heart of "which font draws this
codepoint." Its own doc-comment enumerates the algorithm; the exact 7 steps
are:

1. **Disabled-style fallback.** If a non-regular style is requested but that
   style is disabled (`styles.get(style) == false`), restart at regular.
   Regular can never be disabled.
2. **Codepoint override.** If the user configured a codepoint→font override for
   this cp, honor it regardless of style/presentation.
3. **Sprite dispatch.** If sprite drawing is enabled and
   `sprite.hasCodepoint(cp, presentation)`, return `Index.initSpecial(.sprite)`.
   This is how box-drawing / block / braille / powerline / legacy-computing
   codepoints get routed to the procedural rasterizer instead of a font.
4. **Exact style+presentation match** among loaded faces, in priority order
   (`collection.getIndex(cp, style, p_mode)`). Presentation defaults come from
   the UCD `is_emoji_presentation` property when the caller passes `null`.
5. **Regular-style retry.** If not regular and step 4 missed, restart at
   regular before ever reaching for a fallback font (a regular glyph from a
   loaded font beats a new styled fallback font, because cross-font style
   swaps change glyph metrics/widths).
6. **Discovery fallback** (regular only): use platform font discovery to find a
   fallback face that has the codepoint, add it deferred, return its index.
7. **Any-presentation last resort.** Restart the whole process for a regular
   face satisfying ANY presentation for the codepoint; if that also fails,
   return `null`.

The function is documented as *infallible* — any internal error (allocation,
discovery failure) is swallowed and the resolver moves to the next method,
because "better to render something than nothing."

### What the reduced resolver implements

The reduced cut is single-font + sprite dispatch, so it collapses to steps
3 → 4 → notdef:

- **Step 1 (disabled-style):** trivially satisfied — the reduced resolver maps
  every style to regular (there is only a regular face), so a bold request
  resolves against the regular font. This is behaviorally the same as upstream
  when bold/italic faces aren't loaded (upstream step 5 would fall back to
  regular anyway).
- **Step 2 (codepoint override):** **deferred.** No `CodepointMap` in the
  reduced config surface.
- **Step 3 (sprite dispatch):** **implemented.** `qwertty_term_sprite::has_codepoint(cp)`
  is the analog of `sprite.hasCodepoint`. A hit returns `FontIndex::Sprite`.
  (The reduced sprite check is codepoint-only; upstream also gates on
  presentation for a few emoji-vs-text cases, deferred.)
- **Step 4 (exact match):** **implemented, reduced to the primary face.**
  `Face::glyph_index(cp).is_some()` on the single loaded face is the analog of
  `collection.getIndex` over a one-entry priority list.
- **Steps 5–7 (regular retry / discovery fallback / any-presentation):**
  **deferred.** There is no second face and no discovery, so a miss after step
  4 returns `None` (the caller then substitutes notdef / the replacement
  character, mirroring run.zig's `0xFFFD`→`' '` fallback chain but reduced to
  "return notdef glyph 0").

So the reduced resolver is: **sprite? → primary-has-it? → else None (notdef).**
The primary always has a glyph for space, so a run never fails to produce
*something*, preserving the upstream invariant that the notdef path is
reachable but rendering never aborts.

## run.zig: text-run segmentation responsibilities

`shaper/run.zig`'s `RunIterator.next()` (`run.zig:47`) walks a row's cells
left-to-right and emits `TextRun`s. A run is a maximal contiguous span of cells
that share a single font index and shaping context, so it can be handed to the
shaper as one unit. The breaks it enforces:

1. **Trailing-empty trim** — compute `max`, the index past the last non-empty
   cell, and never shape past it.
2. **Invisible-cell skip** — cells whose style has the `invisible` flag are
   skipped (no glyph).
3. **Spacer skip** — `spacer_head` / `spacer_tail` cells (the second half of a
   wide char, and wide-char padding) are `continue`d, not shaped; the wide
   glyph is emitted once for the head cell.
4. **Style-change split** (`run.zig:112-147`) — if the cell's style differs
   from the run's style (compared via `comparableStyle`, which ignores
   background color), the run breaks. This is why `>=` rendered with two
   differently-colored halves does not ligate into one glyph.
5. **"Bad ligature" split** (`run.zig:118-137`) — an *explicit* break between
   adjacent plain codepoints that commonly form unwanted ligatures: `f`+`l`,
   `f`+`i`, `s`+`t`. This prevents `fl`/`fi`/`st` from ligating in code.
6. **Cursor split** (`run.zig:188-209`) — the run breaks immediately before,
   exactly around, and after the cursor cell, so the cursor cell shapes in
   isolation.
7. **Font-change split** (`run.zig:215-255`) — `indexForCell` resolves the font
   index (via the grid/resolver) for each cell's grapheme; when the resolved
   index differs from the run's current font, the run breaks. Graphemes that
   need a font supporting *all* their codepoints are resolved by intersecting
   per-codepoint candidate fonts (`run.zig:318-395`).
8. **Presentation determination** (`run.zig:161-176`) — a leading `U+FE0E`
   (text) / `U+FE0F` (emoji) variation selector in a grapheme sets the run's
   presentation, which flows into font selection.

### What the reduced single-font cut keeps vs defers

- **Keeps: style boundaries only.** With a single regular font, the only run
  break that changes behavior is the **style change** (break 4) — cells with
  different styles must not shape together. The reduced port segments a line
  into runs at style boundaries; every run resolves to the same single font,
  so break 7 (font-change) never fires but is *structurally* honored (each run
  carries its `FontIndex`, and a sprite codepoint gets its own `Sprite` run,
  which is a font-change break in disguise: sprite cells cannot shape with
  text cells).
- **Keeps: sprite isolation.** A sprite codepoint (box-drawing etc.) resolves
  to `FontIndex::Sprite`, which differs from the text run's font index, so it
  breaks the run — matching upstream's break 7 for the sprite pseudo-font.
  Sprite runs are not sent to rustybuzz (codepoint == glyph, no shaping),
  mirroring `harfbuzz.zig:133` `run.font_index.special()` short-circuit.
- **Defers: font-fallback splitting** (break 7 for real fallback fonts) — there
  is no second font, so no fallback split.
- **Defers: bad-ligature splits** (break 5, `fl`/`fi`/`st`) — noted as a
  completeness item; the reduced test line avoids these pairs so the absence is
  observable-neutral. (The primary JetBrains Mono at default features does not
  ligate ASCII anyway, so the reduced ASCII 1:1 assertion holds without it.)
- **Defers: emoji-presentation splits** (break 8, `U+FE0E`/`U+FE0F`) — no
  variation-selector handling in the reduced cut.
- **Defers: cursor / selection / invisible splits** (breaks 2, 6) — these need
  terminal cell state (`terminal.page.Cell`) the font crate deliberately does
  not depend on; the reduced Shaper takes a plain `&str` per run, not a row of
  terminal cells.

## Cluster → cell mapping (the load-bearing semantics)

This is the piece the reduced Shaper must reproduce faithfully, because it
determines how shaped glyphs land on the terminal grid. Upstream's HarfBuzz
shaper (`shaper/harfbuzz.zig:130-259`) is the behavioral reference (the
CoreText shaper produces the same cell semantics via a different code path).

### Upstream mechanism

1. Each codepoint is pushed into the HB buffer with its **array index** as the
   HB cluster value (`harfbuzz.zig:286`, `addCodepoint`), and the buffer's
   cluster level is set to `characters` (`harfbuzz.zig:270`) so HB reports the
   minimum cluster per output glyph. A side table `codepoints[index] = {cp,
   cluster}` records the *original* terminal-cell cluster (the cell's X within
   the run) for each pushed codepoint.
2. After shaping, for each output glyph: `index = info.cluster` recovers the
   array index; `cluster = codepoints[index].cluster` recovers the **cell X**.
3. A glyph's cell X is `cell_offset.cluster`. When the cluster changes,
   `cell_offset` is reset to the new cluster and the current pen X
   (`run_offset.x`) — *conditionally*: only if this glyph is the *first
   codepoint in its cluster* AND the cluster is *not* "after a glyph from the
   current or a later cluster" (`harfbuzz.zig:177-226`). This condition is a
   ligature-detection heuristic: if the first codepoint of a cluster never
   appears as its own glyph (it fused into a ligature spanning a previous
   cluster), the following mark glyphs are positioned relative to the ligature,
   so the cell offset is NOT reset to the grid — which would misplace them. The
   `!is_after…` guard handles marks that come from a later cluster but render
   first (Chakma/Bengali reordering).
4. Positions are 26.6 fixed point under both FreeType and CoreText, so each is
   rounded to whole pixels via `(v + 0b100_000) >> 6` (add ½, arithmetic shift
   right by 6). `x_offset = run_offset.x - cell_offset.x + round(pos.x_offset)`;
   `y_offset = run_offset.y + round(pos.y_offset)`. Advances accumulate into
   `run_offset` and apply to the *next* glyph.
5. Output `Cell { x: cell_offset.cluster (cell X), x_offset, y_offset,
   glyph_index: info.codepoint }` (`shape.zig:41-58`). Note `glyph_index` here
   is the HB output glyph id (a *glyph*, not a codepoint).

The net effect for the common monospace cases:

- **Plain ASCII** (no ligature, 1 codepoint → 1 glyph, advance == cell width):
  each glyph gets its own cluster == its own cell X, `x_offset == 0`. **1:1
  codepoint↔glyph↔cell.**
- **A ligature** (N codepoints → 1 glyph): the glyph takes the cluster of the
  first codepoint (its cell X); the subsequent cells are covered by the single
  glyph's advance. Fewer glyphs than cells.
- **A wide char** (1 codepoint → 1 glyph, advance == 2 cells): one glyph at one
  cell X; the terminal already marked the second cell as a `spacer_tail`
  (skipped by run.zig), so the wide glyph occupies 2 cells with a single glyph.

### Reduced port mapping approach

The reduced Shaper takes a `&str` for one run and produces
`ShapedCell { cell_x, x_offset, y_offset, glyph_index }`. It reproduces the
upstream semantics without terminal-cell types:

- Push each `char` with `UnicodeBuffer::add(ch, cluster)` where `cluster` is a
  caller-supplied per-char cell-X (defaults to a running index for the common
  1-char-per-cell case), and `set_cluster_level(BufferClusterLevel::Characters)`
  — the exact analog of upstream's `characters` level + index-as-cluster.
- After `rustybuzz::shape`, walk `glyph_infos()`/`glyph_positions()` in
  lockstep. `info.cluster` is the original cluster we supplied (rustybuzz keeps
  the minimum cluster per glyph under `Characters` level, same as HB), which is
  directly the **cell X** — the reduced cut supplies cluster == cell X, so no
  side table indirection is needed (upstream's side table exists only because
  it passes the *array index* as the HB cluster to keep grapheme components
  distinct; the reduced cut has no multi-codepoint graphemes in scope, so
  cluster == cell X is exact).
- The cluster-reset condition is reduced to its common-case behavior: reset
  `cell_offset` to `(cluster, run_offset.x)` whenever the cluster advances
  (the full ligature/mark heuristic guard is only needed for complex-script
  mark positioning, which the reduced ASCII/CJK/em-dash/box scope never hits —
  documented as deferred; a ligature still maps correctly because its single
  glyph keeps the first cluster and later cells get no glyph of their own).
- Scale positions from font design units to pixels. rustybuzz returns positions
  in **font units** at `units_per_em` scale (unlike ghostty's HB which is
  configured at pixel ppem giving 26.6). The reduced Shaper scales by
  `px_per_em / units_per_em` and rounds to whole pixels (round-half-up), the
  Rust analog of upstream's `(v + ½) >> 6`.
- The result invariants the integration test pins: ASCII maps 1:1 (N chars → N
  glyphs → N cells, x_offset 0); a wide CJG char is one glyph whose advance is
  ~2 cells and which sits at one cell X (the caller marks the trailing cell as
  covered); a sprite codepoint bypasses shaping entirely.

## Glyph → atlas upload flow (SharedGrid render caching)

`SharedGrid.zig` is the render-caching layer between the resolver and the
atlas. The relevant flow (locking elided — the reduced Grid is single-threaded,
per the plan's "no locking"):

1. `getIndex(cp, style, p)` (`SharedGrid.zig:153`) — a cached wrapper over
   `resolver.getIndex`. Keyed by `{style, cp, presentation}`; caches even
   negative (null) results. On a hit it also preloads the face (unless it's a
   special/sprite index). **Port:** the reduced Grid caches
   `cp → Option<FontIndex>`.
2. `renderGlyph(index, glyph_index, opts)` (`SharedGrid.zig:255`) — the render
   cache. Keyed by `{index, glyph, opts}`. On a miss it (a) determines
   presentation to pick the atlas (`atlas_grayscale` for text, `atlas_color`
   for emoji), (b) applies emoji constraints if needed, (c) calls
   `resolver.renderGlyph(atlas, index, glyph_index, opts)`, (d) on
   `AtlasFull`, grows the atlas to 2× and retries. Returns a
   `Render { glyph, presentation }`.
3. `resolver.renderGlyph` → `face.renderGlyph` (`face/coretext.zig:547-566`) —
   this is where the bitmap becomes an atlas region: `atlas.reserve(w, h)` then
   `atlas.set(region, buf)`, returning a `Glyph { width, height, offset_x,
   offset_y, atlas_x, atlas_y }`. `offset_x` is left bearing (cell-left to
   ink-left); `offset_y` is `px_y + px_height` (cell-bottom to ink-top,
   baseline-relative). `atlas_x/atlas_y` are the reserved region's top-left in
   the atlas texture.

### Reduced port: `Grid`

The reduced `Grid` (SharedGrid-reduced, no locking) combines the two caches and
the atlas ownership:

- Owns a single grayscale `Atlas` (the plan's first-pixels target is a
  grayscale atlas; the color/emoji atlas is deferred with emoji presentation).
- `codepoint → FontIndex` cache (analog of `getIndex`'s cache).
- `(FontIndex, glyph_id) → CachedGlyph` render cache (analog of `renderGlyph`'s
  cache), where `CachedGlyph { atlas_x, atlas_y, width, height, offset_x,
  offset_y }` mirrors upstream's `Glyph`.
- On a render miss: for a face glyph, `face.rasterize(glyph_id)` (F5) →
  `atlas.reserve(w, h)` → `atlas.set` (or a zero-region for blank glyphs); for
  a sprite codepoint, `qwertty_term_sprite::render(cp, &sprite_metrics)` → same
  reserve/set. On `AtlasFull`, `atlas.grow(size*2)` and retry — the exact
  upstream escalation.
- Returns atlas coordinates + placement offsets. This is the "returning atlas
  coords" contract the plan asks for and the substrate R4's cell engine
  consumes.

The `modified`/`resized` atlas counters (from F1) are the renderer's
re-upload/realloc signal; the reduced Grid does not poll them itself (that is
the renderer's job) but preserves them by routing every write through the F1
`Atlas`.

## Shaper::Cache (upstream) — deferred

`shaper/Cache.zig` (106 lines) is an LRU over `TextRun.hash → []Cell` (shaped
run results), so an unchanged row's shaping is reused frame-to-frame. The
reduced cut **defers** this: it shapes on demand and relies on the per-glyph
render cache (which is where the expensive rasterization actually is) rather
than a run-level LRU. Flagged for the R4/renderer chunk to add once frame
pacing exists and run identity (hashing) is wired to real terminal rows.

## Test inventory: Zig vs reduced Rust

Upstream inline tests in the scope files and how the reduced port covers them:

- **`Collection.zig`** — inline tests: `init`, `add full` (CollectionFull at
  the 8192 cap), `add deferred without loading options`, `getFace named`,
  `metrics` (cell-metrics parity for Inconsolata), `adjusted sizes` (fallback
  size adjustment across `ex_height`/`cap_height`), `face metrics`, plus the
  `Index` sizeof/idx_bits test.
  - **Ported (reduced):** `init`/basic add+get (a regular face round-trips
    through `add` → `get`), the sprite special-index distinction (analog of the
    `Index.special()` test), style grouping (a bold slot is stored separately
    from regular).
  - **Skipped as deferred:** `add full` (no 8192 packed cap — the slotmap has
    no fixed idx-bits limit, so the CollectionFull semantics don't apply),
    `add deferred…` (no deferred faces in the reduced cut), `adjusted sizes`
    (no fallback faces / size adjustment), `metrics`/`face metrics` (already
    covered by F1's `Metrics` tests and F5's reconciliation test), the
    `Index` bitfield sizeof test (N/A: decision 8 replaced the bitfield).
- **`run.zig`** — no standalone inline unit tests in the file (segmentation is
  exercised by higher-level shaper tests upstream). The reduced port covers the
  kept behavior (style-boundary and sprite-isolation segmentation) via the
  integration test rather than porting an inline test that does not exist.
  - **Skipped as deferred:** all the complex-script/ligature/emoji segmentation
    behaviors (breaks 5–8) — no upstream inline test in `run.zig` to port, and
    the behaviors are out of the reduced scope.
- **`harfbuzz.zig`** — cluster-mapping tests exist upstream (Latin, ligature,
  Chakma/Bengali reordering) but are gated on the full HB backend + terminal
  cell types. The reduced port reproduces the **Latin 1:1** and **wide-char**
  cases in the integration test; the complex-script reordering cases are
  **skipped as deferred** (they exercise the `!is_after…` heuristic guard the
  reduced mapping omits).

Count: **~3 upstream Collection inline tests ported (reduced)**, **~5
Collection inline tests skipped as deferred**, **run.zig has 0 inline tests to
port**, **harfbuzz cluster cases: 2 reproduced (Latin/wide), complex-script
skipped as deferred.** The bulk of the reduced verification is the
first-pixels integration test (below), which is the acceptance the plan
specifies for this chunk.

## The first-pixels integration test

Per the plan's acceptance ("shape+rasterize 'hello', an em dash, a CJK char, a
symbol into atlas; positions verified"), the integration test drives the full
reduced substrate:

1. Build a `Collection` with the embedded JetBrains Mono as the primary regular
   face, a `CodepointResolver` (primary + sprite dispatch), and a `Grid` over a
   grayscale `Atlas`.
2. Shape+rasterize the ASCII run `"hello"`: assert 5 glyphs, 1:1 cell mapping,
   every cell got an atlas region with plausible geometry, no ligature.
3. Shape+rasterize an em dash `—` (U+2014): a single glyph in a single cell
   from the primary face.
4. Shape+rasterize a CJK ideograph (U+6C34 水): a single glyph whose advance is
   ~2 cells, occupying 2 cells with one glyph.
5. Rasterize a box-drawing char (U+2500 ─): resolves to `FontIndex::Sprite`,
   comes from `qwertty-term-sprite` (not a face), lands in the atlas.
6. Assert every rasterized cell has a distinct, in-bounds atlas region.

## Deferrals

- **Codepoint overrides** (`CodepointMap`, resolver step 2).
- **Regular-retry / discovery fallback / any-presentation** (resolver steps
  5–7) — no second face and no font discovery in the reduced cut.
- **Font-fallback / bad-ligature / emoji-presentation run splitting**
  (run.zig breaks 5, 7-for-real-fallback, 8).
- **Cursor / selection / invisible-cell run splitting** (breaks 2, 6) — need
  terminal cell state.
- **Complex-script cluster reordering heuristic** (`harfbuzz.zig`'s
  `is_after…` / `is_first_codepoint_in_cluster` guard) — the reduced mapping
  uses the common-case reset only.
- **Run-level shaping LRU** (`shaper/Cache.zig`) — reduced cut caches per-glyph
  renders, not per-run shapes.
- **Color/emoji atlas** — first pixels target a grayscale atlas; emoji
  presentation + `atlas_color` deferred.
- **Bold/italic faces** — slots reserved, wiring present, but the reduced
  resolver routes every style to regular.
- **The packed u16 `Index` bitfield** — replaced by a slotmap-style
  `FontIndex` per decision 8; the 8192-per-style cap and its sizeof test do not
  apply.
