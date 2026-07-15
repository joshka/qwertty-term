# Font foundations: opentype tables, Metrics, Atlas

Analysis of Ghostty's opentype table layer (`src/font/opentype/`), the derived
cell-metrics algorithm (`src/font/Metrics.zig`), and the texture atlas
bin-packer (`src/font/Atlas.zig`), plus the crate/library decisions for the
`qwertty-term-font` port.

- **Upstream reference:** commit `2da015cd6` (the designated port baseline).
- **Scope:**
  `src/font/opentype/{sfnt,head,hhea,os2,post,svg,glyf}.zig`,
  `src/font/opentype.zig`, `src/font/Metrics.zig`, `src/font/Atlas.zig`,
  `src/font/embedded.zig`, `src/font/backend.zig`, plus the consumer
  (`src/font/face/coretext.zig`'s `getMetrics`) that determines what the
  `Metrics` derivation actually needs from the tables.

## What each opentype table provides, and who consumes it

Ghostty's `opentype/` package is a thin, allocation-light SFNT reader plus
per-table parsers. `sfnt.zig` (318 lines) is the entry point: it parses the
`OffsetSubtable` + `TableRecord[]` directory (`SFNT.init`) and exposes
`getTable(tag) -> ?[]const u8`, a linear scan over records. It also defines
the OpenType primitive types used by every other table: `uint8/16/24/32`,
`int8/16/32`, `Fixed` (16.16), `F2DOT14` (2.14), `F26Dot6` (26.6),
`FWORD`/`UFWORD` (design-unit int16/uint16), `LONGDATETIME`, `Tag` (`[4]u8`),
`Offset8/16/24/32`, `Version16Dot16`. These are all backed by one generic
`FixedPoint(T, int_bits, frac_bits)` packed-struct implementation with
`to(FloatType)`, `from(float)`, and banker's-adjacent `round()` (`.5` rounds
away from zero, tested in `test FixedPoint`).

Per-table parsers, each `extern struct` read big-endian directly off the
table bytes (`readStructEndian(.., .big)`):

- **`head.zig`** (179 lines) — `unitsPerEm`, glyph bbox extrema
  (`xMin/yMin/xMax/yMax`), `indexToLocFormat` (needed only for `loca`/`glyf`
  glyph lookup, not for metrics). Consumer: `units_per_em` in
  `coretext.zig::getMetrics` (falls back to CoreText's own
  `getUnitsPerEm()` if the table read fails, e.g. `bhed` bitmap-only fonts).
- **`hhea.zig`** (116 lines) — `ascender`, `descender`, `lineGap` (all
  `FWORD`, i.e. design units), plus advance/sidebearing extrema not used for
  metrics. Consumer: primary source of vertical metrics unless OS/2 says to
  prefer typo metrics (see derivation below).
- **`os2.zig`** (583 lines) — five wire-format variants (`OS2v0` through
  `OS2v5`, table length keyed off `version` 0/1/2-3-4/5) collapsed into one
  generic `OS2` struct with `?`-optional fields for anything not present in
  the older versions. Fields metrics cares about: `sTypoAscender/Descender/
  LineGap`, `usWinAscent/Descent`, `xAvgCharWidth`, `yStrikeoutSize/Position`,
  `sCapHeight`, `sxHeight`, `fsSelection.use_typo_metrics` (bit 7 of
  `fsSelection`, decoded via the `FSSelection` packed struct). Consumer:
  vertical-metrics fallback chain, strikethrough position/thickness,
  cap/ex-height.
- **`post.zig`** (82 lines) — `italicAngle` (`Fixed`), `underlinePosition`/
  `underlineThickness` (`FWORD`), `isFixedPitch`. Only versions where the
  fixed-size header is all that's needed are parsed (2.0/2.5 extra glyph-name
  data is not read). Consumer: underline position/thickness (with a
  "broken underline" zero-guard, see below); `isFixedPitch` is used elsewhere
  for monospace detection, not by Metrics.
- **`svg.zig`** (114 lines) — not consumed by Metrics at all. Used by the
  glyph-rendering path to answer "does glyph N have an SVG outline"
  (`hasGlyph`) via a binary search over glyph-ID-range records, with fast
  paths for the table's overall min/max range. Relevant to the *future*
  glyph-protocol chunk, not this one.
- **`glyf.zig`** (1046 lines) — a hand-rolled TrueType glyph outline decoder
  (simple + composite glyphs, `F2DOT14`-scaled component transforms, on/off
  curve point flags with run-length repeat, quantized coordinate deltas).
  This is the biggest file by far and is **not** consumed by `Metrics` at
  all — it exists purely to rasterize glyph outlines for the CPU rasterizer
  path (used when a system font-rendering backend doesn't otherwise offer a
  bitmap, e.g. embedded fonts / non-CoreText backends). Not in scope for this
  chunk beyond the bare-glyf feasibility check below.
- **`opentype.zig`** (25 lines) — just re-exports (`SVG`, `OS2`, `Post`,
  `Hhea`, `Head`, `Glyf`) plus a `refAllDecls` smoke test.

### The `Metrics.FaceMetrics` derivation inputs (from `coretext.zig::getMetrics`)

`Metrics.calc` (see below) consumes a `FaceMetrics` struct that platform face
code must populate. Reading how CoreText's backend does it
(`face/coretext.zig:570-868`) is the ground truth for "what does a
table-parsing backend need to expose," since it is a real, working consumer:

1. Read `head`, `post`, `OS/2`, `hhea` tables (each independently optional —
   any read/parse failure degrades gracefully rather than erroring).
2. `units_per_em` = `head.unitsPerEm`, else CoreText's own value.
3. `px_per_unit` = `px_per_em / units_per_em`.
4. Vertical metrics (`ascent`, `descent`, `line_gap`) — a fallback chain:
   - No `hhea` table at all → use CoreText's own ascent/descent/leading.
   - `hhea` present but no `OS/2` → use `hhea` scaled by `px_per_unit` as-is.
   - `OS/2.fsSelection.use_typo_metrics` set → use OS/2 `sTypo*` scaled.
   - Else, prefer `hhea` if `ascender != 0 or descender != 0`.
   - Else, prefer OS/2 `sTypo*` if `ascender != 0 or descender != 0`.
   - Else, fall back to OS/2 `usWinAscent`/`usWinDescent` (note:
     `usWinDescent` is **positive**-down, unlike `sTypoDescender` and
     `hhea.descender`, so its sign must be flipped when used as `descent`).
   - This exact ladder is *not* what ttf-parser's own `Face::ascender()` /
     `descender()` / `line_gap()` do (see decision below) — ghostty's is a
     closer match to what FreeType's `sfobjs.c` does for its generic metrics,
     but the two disagree on tie-break order, so bare `Face::ascender()`
     cannot be used as a drop-in.
5. Underline position/thickness from `post`, with a "broken underline"
   guard: if `post.underlineThickness == 0`, both position and thickness are
   treated as absent (`null`) *unless* `underlinePosition != 0`, in which case
   the position is still trusted but not the thickness.
6. Strikethrough position/thickness from OS/2 `yStrikeoutPosition`/
   `yStrikeoutSize`, with the same broken-value guard pattern.
7. Cap/ex height from OS/2 `sCapHeight`/`sxHeight` if present (and nonzero
   after scaling), else CoreText's own `getCapHeight()`/`getXHeight()`
   (glyph-measurement fallback — not available to a pure table-parsing
   backend; see "bare CoreText fallback" note in the decision below).
8. `cell_width` and `ascii_height` are measured, not read from a table: get
   glyph IDs for the printable ASCII range (0x20..0x7E), get their horizontal
   advances, take the max advance as `cell_width`; get the glyphs' overall
   bounding-rect height as `ascii_height`.
9. `ic_width` (CJK water ideograph U+6C34 advance) is measured the same way,
   discarded if the glyph's bbox width exceeds its advance (patched-font
   corruption guard).

None of steps 8-9's *measurement* is table data — they require rasterizer/
shaping-level glyph lookup and advance-width queries (`cmap` + `hmtx`), which
`ttf-parser` also provides (`Face::glyph_index`, `Face::glyph_hor_advance`,
`Face::glyph_bounding_box`/`outline_glyph`), so a from-tables backend can
replicate this without CoreText.

## The `Metrics` derivation algorithm

`Metrics.calc(face: FaceMetrics) -> Metrics` (Metrics.zig:227-334) turns the
above face-level floats into pixel-integer cell metrics:

1. `face_width = face.cell_width`; `face_height = face.lineHeight()`
   (`ascent - descent + line_gap`) — kept **unrounded** for later diffing.
2. `cell_width = round(face_width)`, `cell_height = round(face_height)`
   (round-half-away-from-zero, not ceil — chosen because it best preserves
   "authorial intent" and keeps low/high-DPI spacing visually consistent, at
   the cost of allowing rare 1px glyph overflow for badly-authored fonts).
3. Line gap is split in half and pushed to both edges of the cell:
   `half_line_gap = line_gap / 2`.
4. `face_baseline = half_line_gap - descent` (note: `cell_baseline` is
   measured from the **bottom** of the cell, unlike every other position
   field, which is from the top).
5. `cell_baseline = round(face_baseline - (cell_height - face_height) / 2)` —
   centers the face vertically in the *rounded* cell height by nudging the
   baseline by half the rounding error.
6. `face_y = cell_baseline - face_baseline` — the offset between the drawn
   baseline and the font's "natural" baseline; kept around specifically so
   the modifier system (below) can tell whether the face sits above or below
   dead center when redistributing height changes.
7. `top_to_baseline = cell_height - cell_baseline` simplifies the position
   conversions (top-relative vs bottom-relative).
8. Underline/strikethrough thickness: `max(1, ceil(thickness))` — ceiling
   (not round) so a sub-pixel-thick line never disappears; clamped to a
   1px floor.
9. Underline/strikethrough position: `round(top_to_baseline - position)` —
   converts the font's baseline-relative position (+Y up) into a
   top-of-cell-relative pixel offset.
10. `icon_height = face_height` (unrounded); `icon_height_single =
    (2*cap_height + face_height) / 3` — a heuristic borrowed from nerd-fonts'
    `font-patcher` script for single-cell-width icon constraints.
11. All fields assembled, then `clamp()`'d against the `Minimums` struct
    (mostly a 1-unit floor per field, to prevent divide-by-zero and
    zero-thickness downstream).

`FaceMetrics` itself carries convenience getters with their own fallback
heuristics when a font doesn't supply a value directly:
`capHeight` (0.75× ascent if absent), `exHeight` (0.75× cap height),
`asciiHeight` (1.5× cap height), `icWidth` (min of ascii height and 2 cell
widths), `underlineThickness` (0.15× ex height), `strikethroughThickness`
(= underline thickness), `underlinePosition` (one underline-thickness below
baseline), `strikethroughPosition` (centered on half the ex height).

### The modifier system

`ModifierSet` is an `AutoHashMapUnmanaged(Key, Modifier)` (one entry per
metric a user has overridden via config, e.g. `adjust-cell-height`). `Key` is
a comptime-generated enum covering every `u32`/`i32`/`f64` field of
`Metrics`. `Modifier` is `union(enum) { percent: f64, absolute: i32 }`:

- `percent` deltas are stored as *multipliers already offset by 1* — parsing
  `"20%"` yields `percent = 1.2`, `"-20%"` yields `0.8`, `"0%"` yields `1.0`
  (parse clamps at `percent <= -1 → 0`, matching "can't shrink below zero
  size"). `Modifier.apply` for ints rounds `v * max(0, p)`; for floats it's
  `v * max(0, p)` unrounded.
- `absolute` deltas are added directly (saturating arithmetic — `+|`/`-|` —
  so overflow clamps rather than wraps; unsigned fields clamp their result at
  0 rather than going negative).

`Metrics.apply(mods)` iterates the set and, for most fields, just does
`field = modifier.apply(field)` then re-`clamp()`s. Two fields get special
handling:

- **`cell_width`/`cell_height`**: clamped to a minimum of 1 unconditionally
  (divide-by-zero guard downstream). `cell_height` additionally triggers a
  **redistribution** of the size delta across every position anchored to the
  cell edges, because those anchors are absolute pixel offsets from the top
  or bottom of the cell and must move when the cell resizes:
  - `diff = new_height - original_height`; `half_diff = diff / 2`.
  - If `diff` is odd, the extra pixel goes to whichever edge the face is
    *already* offset toward — computed via
    `position_with_respect_to_center = face_y - (original_height -
    face_height) / 2` (this is 0 exactly when the face was perfectly
    centered in the original cell height; positive means the baseline sits
    above center). If positive, top gets `ceil(half_diff)` and bottom gets
    `floor(half_diff)`; if non-positive, the assignment flips. This is the
    trickiest piece of the whole algorithm and is exactly what the two
    "adjust cell height" tests (below) exist to pin.
  - `cell_baseline` and `face_y` (bottom-relative) get `diff_bottom` added
    via saturating float→int add (`addFloatToInt`, asserts the float is a
    whole number first). `underline_position`/`strikethrough_position`
    (top-relative) get `diff_top`; `overline_position` (signed, can go
    negative) gets `diff_top` via saturating i32 add.
- **`icon_height`**: also updates `icon_height_single` by the same modifier,
  independently of `face_height`/`face_y` (icon sizing is meant to be
  adjustable without perturbing the cell's own vertical anchors).

`clamp()` runs again after every `apply()` to re-enforce the `Minimums`
floor, since modifiers (especially large negative `absolute` ones) could
otherwise push a field out of range.

### Test inventory: `Metrics.zig` — 9 tests

1. `Metrics: apply modifiers` — basic percent modifier on `cell_width`.
2. `Metrics: adjust cell height smaller` — 0.75× cell height, deliberately
   chosen so the pixel delta (25px removed, split 13/12) is odd, pinning the
   `diff_top`/`diff_bottom` split direction for a face sitting *above*
   center (`face_y = 0.33`).
3. `Metrics: adjust cell height larger` — 1.75× cell height (75px added,
   split 38/37), same face_y sign, opposite growth direction — pins the
   split logic for growth as well as shrink.
4. `Metrics: adjust icon height by percentage` — icon height + icon height
   single both scale together; face metrics untouched.
5. `Metrics: adjust icon height by absolute pixels` — same, absolute delta.
6. `Modifier: parse absolute` — `"100"`/`"-100"` parse.
7. `Modifier: parse percent` — `"20%"`/`"-20%"`/`"0%"` parse to `1.2`/`0.8`/`1.0`.
8. `Modifier: percent` — apply a percent modifier to a `u32`.
9. `Modifier: absolute` — apply an absolute modifier, including saturation
   at 0 for a large negative delta.

(Two additional tests, `formatConfig percent`/`formatConfig absolute`, exist
in the Zig source but round-trip through Ghostty's config-file formatter,
which has no Rust analog in this crate — out of scope, not counted above.)

## The `Atlas` bin-packer

`Atlas.zig` (889 lines, "889/12" in the task brief — the doc-comment cites
Jukka Jylänki's "A Thousand Ways to Pack the Bin," implemented via the
"skyline" variant used by Nicolas Rougier's `freetype-gl` and Jukka's own
`RectangleBinPack` C++ reference).

Shape:

- `data: []u8` — raw texture bytes, always `size * size * format.depth()`
  (the atlas is always square: width == height).
- `nodes: ArrayList(Node)` where `Node { x, y, width }` is a "skyline"
  segment: the free-space profile is a monotonic list of horizontal
  segments, each recording the topmost occupied `y` for that x-range.
- `format: Format` (`grayscale`=1bpp, `bgr`=3bpp, `bgra`=4bpp` — enum with a
  `depth()` accessor).
- `modified: atomic usize` — bumped on every write (`set`, `setFromLarger`,
  `grow`, `clear`); renderer polls this to decide whether to re-upload to
  the GPU.
- `resized: atomic usize` — bumped only on `grow`; renderer polls this to
  decide whether the GPU texture itself must be reallocated (vs. an
  in-place partial upload).
- A permanent **1px border** around the whole texture (`clear()` seeds one
  node `{x:1, y:1, width:size-2}`), used to avoid bilinear-sampling
  artifacts at the edges of packed regions.

Algorithm (`reserve(width, height)`):

1. Scan all skyline nodes; for each, `fit()` walks forward accumulating
   width until it covers the requested `width`, tracking the max `y`
   encountered (a rectangle can span multiple nodes; its placement height is
   the tallest of the segments it would sit on). Returns `null` if the
   rectangle would exceed `size - 1` in either axis.
2. Among all fitting placements, choose the one with lowest resulting
   `y + height` ("best fit" skyline heuristic), tie-broken by narrowest
   node width — this is the "bottom-left" variant of skyline packing.
3. Insert a new node at the chosen index representing the just-placed
   rectangle's new top edge, then walk forward trimming/removing any
   subsequent nodes now occluded by the new one, and finally `merge()`
   adjacent nodes sharing the same `y` (keeps the node list from growing
   unboundedly with fragmentation).
4. `AtlasFull` error is returned (not an enlarge-and-retry) — growing is a
   separate, explicit `grow()` call the caller must invoke itself.

`grow(size_new)`: allocates new backing storage, copies old texture data
into the top-left (skipping border rows so old border pixels don't leak into
new interior), appends one new skyline node covering the added right-hand
strip, bumps both `modified` and `resized`. Note this is **infallible past
the allocation point** (`errdefer comptime unreachable`) — once the new
buffer exists, the copy/bookkeeping cannot fail, which matters for how the
Rust port should structure its error path (no partial-mutation states to
worry about beyond the initial alloc).

`set`/`setFromLarger` are raw memcpy-per-row writes into the region
(`setFromLarger` supports copying a sub-rectangle out of a larger source
buffer, e.g. writing one glyph out of a whole rasterized run) — both assert
the region lies within `[0, size-1)` (the border) and bump `modified`.

A `Wasm` sub-namespace (409-579) exposes a hand-rolled C ABI for the WASM
build target (`atlas_new/free/reserve/set/grow/clear/debug_canvas`) — not
relevant to this port (no wasm target in qwertty-term yet).

### Test inventory: `Atlas.zig` — 12 tests (+1 wasm-only, not counted)

1. `exact fit` — reserving exactly the usable interior (`size - 2`) succeeds
   once, a subsequent 1x1 reserve fails with `AtlasFull`; `modified` does
   *not* bump on a successful `reserve` alone (only `set` bumps it).
2. `doesn't fit` — reserving the *nominal* size (not accounting for the
   1px border) fails.
3. `fit multiple` — two 15x30 regions fit in a 32x32 atlas (accounting for
   border), a third small region does not.
4. `writing data` — `set()` copies bytes into the correct offset accounting
   for the 1px border (`atlas.data[33]` etc. for a 32-wide atlas), and
   `modified` increases.
5. `writing data from a larger source` — `setFromLarger` extracts the right
   sub-rectangle and none of the surrounding source bytes leak in.
6. `grow` — reserve, fill, exhaust the atlas, `grow` by 1, verify old data
   preserved at (size-adjusted) offsets, verify both `modified` and
   `resized` incremented, verify the newly available space is usable.
7. `writing BGR data` — 3-byte-per-pixel path, offsets scaled by `depth()`.
8. `grow BGR` — same as `grow` but for the BGR format, verifying per-pixel
   channel layout survives the resize and that new nodes correctly account
   for the border on multiple new reservations.
9. `grow OOM` — using a `FixedBufferAllocator` sized to the exact byte
   count needed (4x4 pixels + preallocated node capacity), assert that a
   `grow()` that can't allocate leaves the atlas completely unchanged
   (`modified`/`resized` unchanged, prior data intact) — an atomicity
   guarantee for the failure path.
10. `init error` — uses Ghostty's `tripwire` fault-injection harness to force
    an allocation failure at every failure point inside `init` (`alloc_data`,
    `alloc_nodes`) and assert no leak (via `testing.allocator`'s built-in
    leak check) and that the error propagates.
11. `reserve error` — same tripwire technique for `reserve`'s one failure
    point (`insert_node`).
12. `grow error` — same tripwire technique for `grow`'s two failure points
    (`ensure_node_capacity`, `alloc_data`), plus verifies data/counters are
    unchanged on failure (same atomicity guarantee as test 9, exercised via
    fault injection instead of a real fixed-size OOM).

The `tripwire`-based fault-injection tests (10-12) test Zig's manual
allocator-failure paths, which don't have a direct Rust analog (Rust's
`Vec`/`Box` allocation failure aborts the process by default, it isn't a
`Result` a caller can `?`/handle inline the way Zig's `Allocator.Error` is).
Ported as: the *observable* guarantees (state unchanged on the error path)
are preserved by construction if the fallible step happens before any
mutation — see the etagere decision below for how that maps onto its API.

## Decision: adopt `ttf-parser` for table parsing

**Verdict: adopt.** `ttf-parser` 0.25.1 (already present in the workspace
`Cargo.lock`, MIT/Apache-2.0, `harfbuzz/ttf-parser` upstream, zero-alloc,
`no_std`-capable) exposes everything `Metrics.FaceMetrics` needs:

| Need                                   | ttf-parser API                                                                                                                     |
| -------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `unitsPerEm`                           | `Face::units_per_em() -> u16` (from `head`)                                                                                        |
| `hhea` ascender/descender/lineGap      | `face.tables().hhea` -> `hhea::Table { ascender, descender, line_gap }` (raw, i16)                                                 |
| OS/2 `sTypoAscender/Descender/LineGap` | `face.tables().os2` -> `os2::Table::typographic_{ascender,descender,line_gap}()`                                                   |
| OS/2 `usWinAscent/Descent`             | `os2::Table::windows_{ascender,descender}()`                                                                                       |
| OS/2 `fsSelection.use_typo_metrics`    | `os2::Table::use_typographic_metrics()`                                                                                            |
| OS/2 `xAvgCharWidth`                   | present in the parsed table's byte data; not separately wrapped, but readable via `face.tables().os2`'s underlying bytes if needed |
| OS/2 `yStrikeoutSize/Position`         | `Face::strikeout_metrics() -> Option<LineMetrics { position, thickness }>`                                                         |
| OS/2 `sCapHeight`, `sxHeight`          | `Face::capital_height()`, `Face::x_height()` (both `Option<i16>`, version-gated)                                                   |
| `post` `italicAngle`                   | `Face::italic_angle() -> f32`                                                                                                      |
| `post` `underlinePosition/Thickness`   | `Face::underline_metrics() -> Option<LineMetrics>`                                                                                 |
| `post` `isFixedPitch`                  | `Face::is_monospaced() -> bool`                                                                                                    |

Two important nuances confirmed by reading `ttf-parser`'s source directly
(`src/lib.rs`, `src/tables/{hhea,os2}.rs`), not just its docs:

1. **Do not use `Face::ascender()`/`descender()`/`line_gap()` directly.**
   ttf-parser bakes in its *own* hhea/OS2/typo-metrics fallback ladder
   (`lib.rs:1499-1595`, comment-linked to FreeType's `sfobjs.c`), which is
   close to but not identical to ghostty's (different tie-break precedence,
   and it doesn't expose the "which source won" information the way
   ghostty's chain needs it — e.g. ghostty's `usWinDescent` sign-flip step
   only triggers under a specific combination ttf-parser's ladder doesn't
   surface separately). The port must read `face.tables().hhea` and
   `face.tables().os2` **raw** and re-implement ghostty's exact fallback
   chain (item 4 in the derivation section above) rather than delegate to
   ttf-parser's merged accessor. This is a one-function, well-contained
   piece of hand logic, not a parser — it belongs in `qwertty-term-font`'s
   `Metrics` module, not the opentype layer.
2. **No CoreText-style glyph-measurement fallback is available cross-
   platform.** Steps 7-9 of the derivation (cap/ex-height glyph-measurement
   fallback, `cell_width`/`ascii_height`/`ic_width` measurement) rely on
   CoreText APIs (`getCapHeight`, `getXHeight`, `getBoundingRectsForGlyphs`)
   when the OS/2 table doesn't supply a value. `ttf-parser` can replicate
   the *measurement* mechanism generically via `Face::glyph_index()` +
   `Face::glyph_hor_advance()` + `Face::glyph_bounding_box()`/
   `outline_glyph()` (confirmed present and working — see bare-glyf finding
   below), so this is portable, just needs writing (not a gap, a to-do).

**Hand-port only what ttf-parser can't provide:** in practice, nothing at
the table-parsing layer needs hand-porting for the `Metrics`/`Atlas`/
`embedded` scope of this chunk. `F2Dot14`/`F26Dot6` fixed-point types are
**not** needed here — they're consumed only by `glyf.zig`'s outline decoder
(component transforms) and hinting instruction interpreters, neither in
scope. If the later glyph-protocol chunk needs bare-glyf byte-level access
(e.g. hinting, or matching Ghostty's exact rasterizer numerically),
`ttf-parser::Rect`/`OutlineBuilder` already returns integer-space
coordinates equivalent to what `glyf.zig`'s `Outline.Point` would give, so
even that hand-port may turn out to be unnecessary. Flagged as a report
item for that chunk, not blocked here.

### Bare-glyf parsing feasibility (for the later glyph-protocol chunk)

Checked, not blocking: `ttf_parser::Face::outline_glyph(GlyphId, &mut dyn
OutlineBuilder) -> Option<Rect>` walks whichever outline table is present
(`glyf`+`gvar`, or `cff`/`cff2`) and calls back `move_to`/`line_to`/
`quad_to`/`curve_to`/`close` with **already-decoded, unscaled font-unit
coordinates** — i.e. it does the exact job of ghostty's hand-rolled
`glyf.zig` (1046 lines: simple/composite glyph decode, on/off-curve flag
run-length decode, `F2DOT14`-scaled component transforms) plus CFF outline
decoding as a bonus, in one call. `Face::glyph_bounding_box()` is a thin
wrapper over the same path with a `DummyOutline` builder. This means the
glyph-protocol chunk likely does **not** need to port `glyf.zig` at all —
it can drive `ttf-parser`'s outline callback directly into whatever
rasterizer (e.g. `tiny-skia`, already a dependency of `qwertty-term-sprite`)
that chunk chooses. Recommend that chunk re-verify this against ghostty's
actual rasterized bitmaps for a numeric fidelity check (anti-aliasing/
hinting behavior can differ even with identical outline coordinates), but
the *parsing* half of that problem is already solved by the dependency
adopted here.

## Decision: `etagere` vs porting `Atlas.zig`'s bin-packer

**Verdict: port `Atlas.zig`'s own skyline packer** rather than adopt
`etagere` 0.3.0, on renderer-semantics grounds — not a parsing-layer
decision, a data-model one.

`etagere`'s `AtlasAllocator` (shelf-packing, evaluated via its published
API) allocates/deallocates individual rectangles and returns opaque
`AllocId` handles; it has no concept of:

- **A single shared `modified` generation counter observable across all
  writes.** Ghostty's renderer polls `Atlas.modified` (an atomic `usize`)
  to decide whether *any* region changed since the last GPU upload, without
  needing to track individual allocation IDs — this is a coarse,
  cheap-to-check dirty flag by design. `etagere` has no equivalent; the
  caller would have to invent and maintain this bookkeeping on top of it,
  which is exactly the semantic the task calls out as needing to be
  preserved.
- **A separate `resized` counter distinguishing "texture was reallocated"
  from "texture contents changed in place."** This distinction drives a
  real GPU-side decision (partial buffer update vs. full texture
  recreation + all-region re-upload) that `etagere`'s `grow()` (if driven
  the same way) doesn't surface as a queryable fact — it would need to be
  inferred by the caller comparing sizes before/after, which is exactly the
  bookkeeping this crate exists to centralize.
- **Ghostty's specific "grow preserves a written 1px border, keeps old
  data in the top-left" layout contract**, which the renderer and the
  `Atlas` test suite (`grow`, `grow BGR`) depend on byte-for-byte. `etagere`
  is free to relayout on grow (or simply doesn't support growing an
  existing allocator instance the same way — it's shelf-based, not
  skyline-based, so its packing decisions after a resize would not match
  ghostty's node list at all, and the 12 ported tests assert exact byte
  offsets that only make sense against ghostty's specific algorithm).
- **The raw-byte `data: Vec<u8>` + `set`/`setFromLarger` ownership model.**
  `etagere` is purely a rectangle allocator — it does not own or manage
  pixel data at all (by design, so it can be used for GPU-only atlases).
  Ghostty's `Atlas` owns the CPU-side texture bytes directly (this is a CPU
  atlas that later gets uploaded wholesale or partially to a GPU texture by
  the renderer), so adopting `etagere` would still require writing and
  owning the entire byte-buffer layer this crate provides — at which point
  `etagere` only replaces the placement algorithm, while introducing an API
  mismatch (opaque `AllocId`, no direct x/y/width node semantics) against
  code (renderer, tests) written expecting ghostty's `Region { x, y, width,
  height }` value type.

None of this is a knock on `etagere` as a rectangle packer — it's a solid,
actively maintained shelf-packer used in `wgpu`/`lyon`-adjacent projects.
It's simply solving a narrower problem (rectangle placement) than what
`Atlas.zig` is (rectangle placement + owned pixel storage + a
renderer-facing dirty/resize protocol), and the task's explicit ask is to
preserve *that* protocol. Porting `Atlas.zig`'s skyline packer directly:

- Ports 1:1 with `Region { x, y, width, height }` (already a plain data
  struct, trivially `#[derive(Debug, Clone, Copy)]`-able in Rust) preserved
  verbatim as the public reservation type.
- Preserves `modified`/`resized` as `AtomicUsize` fields exactly as ghostty
  has them (Rust's `std::sync::atomic::AtomicUsize` is a direct match for
  Zig's `std.atomic.Value(usize)`).
- Lets 9 of the 12 non-wasm tests (`exact fit`, `doesn't fit`, `fit
  multiple`, `writing data`, `writing data from a larger source`, `grow`,
  `writing BGR data`, `grow BGR`) port with **byte-identical assertions** —
  they check specific offsets into `atlas.data`, which only make sense
  against this exact node-list algorithm.
- The 3 fault-injection tests (`init error`, `reserve error`, `grow error`)
  have no `tripwire`-equivalent in Rust (there's no ecosystone-standard
  fault-injection harness pulled in here, and Rust's global allocator
  failure model differs fundamentally — `Vec::push` etc. abort rather than
  return `Result` by default). These get ported as **structural**
  assertions instead: since `grow`'s only fallible step (backing-buffer
  allocation) happens before any state mutation (mirroring
  `errdefer comptime unreachable` in the Zig source — "infallible past the
  allocation point"), the Rust port structures `grow` the same way (compute
  new buffer via a fallible allocation try, e.g. `Vec::try_reserve`, before
  touching `self` at all) so the atomicity guarantee holds by construction;
  a test asserts state is unchanged when `try_reserve` is simulated to fail
  via a size that would overflow, rather than via fault injection.

## Deferrals

- `glyf.zig`'s outline decoder is not ported — `ttf-parser::outline_glyph`
  supersedes it for the (out-of-scope) later glyph-protocol chunk; see
  finding above.
- `svg.zig`'s glyph-presence lookup is not ported — not consumed by
  `Metrics`/`Atlas`/`embedded`, belongs with the glyph-protocol chunk if it
  turns out to still be needed once `ttf-parser`'s own `svg` table access
  (`FaceTables::svg`) is evaluated there.
- `F2Dot14`/`F26Dot6` sfnt fixed-point types are not hand-ported — no
  consumer in this chunk's scope; flagged for the glyph-protocol chunk to
  re-evaluate against its actual needs (very likely also unneeded, since
  `ttf-parser` returns already-converted `f32` coordinates).
- CoreText-based cap/ex-height and cell-width/ascii-height/ic-width glyph
  *measurement* fallbacks (steps 7-9 above) are re-implemented against
  `ttf-parser`'s glyph/advance/bbox API rather than any platform text
  system — this is the only piece of `getMetrics` that needed new (not
  hand-ported, not table-parsing) logic, and it is backend-agnostic by
  construction.
</content>
