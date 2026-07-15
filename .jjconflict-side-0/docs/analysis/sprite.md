# Sprite glyph subsystem

Analysis of Ghostty's procedural sprite-font subsystem (`src/font/sprite/`) and
its port to the standalone `qwertty-term-sprite` crate.

- **Upstream reference:** commit `2da015cd6` (the designated port baseline).
  Local working checkout HEAD at time of port: `38e49a232`; the sprite sources
  are byte-identical to the baseline for the files ported here.
- **Scope:** `sprite.zig`, `sprite/canvas.zig`, `sprite/Face.zig`, and
  `sprite/draw/{common, box, block, braille, branch, geometric_shapes,
  powerline, special, symbols_for_legacy_computing,
  symbols_for_legacy_computing_supplement}.zig` plus `draw/octants.txt`
  (~6.2k LoC).

## What the subsystem does

Given a codepoint and cell metrics (in pixels), draw the glyph the terminal
renders itself — box drawing, blocks, braille, powerline separators, git-branch
symbols, geometric shapes, and the Symbols for Legacy Computing blocks — plus
non-Unicode "sprites" for the renderer's own model (cursors, underlines,
strikethrough, overline). The output is an 8-bit alpha coverage bitmap written
into a font atlas.

The value of doing this procedurally rather than from a font is **seam-free
adjacency**: two adjacent box-drawing cells produce one continuous, unbroken
line at *any* cell size, which no font can guarantee across arbitrary grid
metrics. That property is the reason this is a committed library-extraction
candidate.

## Architecture

```text
Face.zig                 top-level: metrics, comptime dispatch table, atlas write
  └─ canvas.zig          Canvas: alpha8 surface over z2d + direct-buffer ops
       └─ draw/common.zig  Fraction, Thickness, Shade, fill/hline/vline helpers
            └─ draw/*.zig   one file per Unicode block, `draw<HEX>` functions
```

### The canvas abstraction over z2d

`Canvas` (canvas.zig) wraps a z2d `alpha8` surface sized `cell + 2*padding` on
each axis (padding is a quarter cell, letting decorations and overshooting
diagonals extend beyond the cell). It exposes:

- **Direct-buffer primitives** — `pixel`, `rect`, `box`: these write bytes
  straight into the surface buffer, bypassing z2d "for performance". Most box
  and block glyphs are built entirely from these.
- **Path primitives over z2d** — `quad`, `triangle`, `line`, `fillPath`,
  `strokePath`: build a `z2d.StaticPath` (offset by the padding transform) and
  hand it to `z2d.painter.fill`/`stroke`. Fill uses non-zero winding; strokes
  use butt caps (round for the undercurl).
- **`innerStrokePath` — the dual-surface multiply trick.** z2d has no inner
  stroke, so: fill a *closed* copy of the path white on surface A (the mask);
  stroke the (open) path at **double** width on surface B; multiply A·B
  per-pixel so only the half of the stroke lying inside the shape survives;
  composite onto the main surface. Used for triangle/half-circle outlines.
- **Whole-buffer transforms** — `invert` (`v → 255-v`), `flipHorizontal`,
  `flipVertical`. Flips also swap the corresponding clip margins. Inversion is
  used by the "negative" legacy-computing glyphs; flips let one glyph be defined
  as the mirror of another (most powerline separators).
- **`trim` + `writeAtlas`** — grow clip margins inward past fully-transparent
  border rows/columns, then copy the trimmed region into the atlas and compute
  placement offsets.

### The Fraction rounding-symmetric cell-fraction system

`common.zig`'s `Fraction` is the heart of seam-freedom and **must be preserved
exactly**. A fraction (0, 1/8, 1/4, 1/3, 3/8, 1/2, …, 1) can be converted to a
pixel coordinate three ways:

- `min(size)` — for a **left/top** edge: `size - round((1 - f) · size)`
- `max(size)` — for a **right/bottom** edge: `round(f · size)`
- `float(size)` — raw `f · size` for path work where pixel alignment is moot

The asymmetry between `min` and `max` is deliberate. `min` measures the
complementary fraction from the far end, so rounding "evens out" across the cell:
for `size = 7`, the `half` line is pixel 3 as a `min` but pixel 4 as a `max`,
which makes both `[start, half]` and `[half, end]` 4px bands (`0..4` and `3..7`)
that meet the identical pixel in an adjacent cell. The load-bearing identity,
pinned by a test in the port, is:

```text
frac.min(size) == size - complement(frac).max(size)
```

This is why a stroke ending at a fraction on one cell's right edge meets the
mirrored stroke on the next cell's left edge with no gap or overlap, at every
size. `Fraction::eighths/quarters/thirds/halves` are index tables so the eighth-
and sixteenth-block glyphs can address boundaries positionally.

### The comptime `draw<HEX>` dispatch

`Face.zig` builds its codepoint → function table **at comptime**: it reflects
over every decl in the draw modules named `draw<HEX>` or `draw<MIN>_<MAX>`,
parses the range straight out of the function name, sorts by `min`, and
`@compileError`s on any overlap. `getDrawFn` then linear-scans the sorted table
(special cursor/underline sprites, which live above the Unicode range, are
matched first by casting the codepoint to the `Sprite` enum and dispatching to
the identically-named function in `special.zig`).

**Port decision — explicit match table, not codegen.** Rust has no comptime
decl reflection, so the choices were (a) a build script parsing the Zig sources
to emit a table, or (b) an explicit hand-written table. The port uses **(b)**:

- The table is ~50 entries; the Zig-name → `(min, max, fn)` mapping is
  mechanical and reviewable on one screen.
- No build-time magic, trivially greppable, and the dispatch stays a plain
  `const` slice searched by binary search.
- Sync risk is bought back by two tests: `dispatch_ranges_match_zig` pins the
  exact `(min, max)` set against a checked-in copy of the upstream table, and
  `ranges_are_sorted_and_disjoint` reproduces the upstream non-overlap
  invariant. If upstream adds or moves a range, the first test fails loudly.

The one wrinkle: powerline is defined upstream as many individual per-glyph
`drawE0BX` functions with **gaps** (e.g. `U+E0C0` is unhandled). The port keeps
the gaps by listing only the handled codepoints, so `has_codepoint` matches
upstream exactly, while a single `draw_e0b0_e0d4` implements them.

### box.zig's `linesChar`/`arc` as the shared hub

`linesChar` renders every intersection-style box-drawing character from a
4-edge `{up, right, down, left}` style spec (`none`/`light`/`heavy`/`double`).
The clever part is the four center-offset computations (`up_bottom`,
`down_top`, `left_right`, `right_left`): a cascade of conditionals that decide
where each arm stops so light/heavy/double lines meet cleanly regardless of
odd/even cell size. The branch order is significant and is preserved verbatim.
`arc` draws a rounded corner as a line-plus-cubic-Bézier stroke. Both are reused
well beyond box drawing: **branch** glyphs are built almost entirely from
`arc` + centered `hline`/`vline`, **powerline** reuses the diagonal helpers, and
several **legacy-computing** glyphs call `linesChar`/`lightDiagonalCross`
directly.

### octants.txt: data-driven supplement

The Symbols for Legacy Computing Supplement octants (`U+1CD00..U+1CDE5`, 230
glyphs) have no discernible mathematical pattern in their codepoint order, so
upstream embeds `octants.txt` — one `BLOCK OCTANT-<digits>` line per codepoint,
in codepoint order — and parses it at comptime into a lookup table of which of
the 8 vertical eighths are filled. The port copies `octants.txt` verbatim into
the crate, `include_str!`s it, and parses it once into a `LazyLock<Vec<Octant>>`
(same logic, runtime-lazy instead of comptime). Each octant then fills up to 8
half-width quarter-height bands via the `Fraction`-based `fill`.

### Trickiest functions

- **`circlePiece` (supplement).** Twelfth/quarter circle pieces and half-
  ellipses are single cubic-Bézier arcs of an ellipse *larger than the cell*,
  offset by `(xp, yp)` so only the visible slice lands in the cell (with the
  cell clip set). The four corner cases each place the move-to and the three
  Bézier control points differently; the constant `c = (√2 − 1)·4/3` is the
  standard quarter-arc Bézier approximation. Transcribed coordinate-for-
  coordinate.
- **`SmoothMosaic` (legacy computing).** 44 mosaic glyphs, each a polygon over
  10 possible anchor points (`tl, ul, ll, bl, bc, br, lr, ur, tr, tc`). Upstream
  encodes each as a 3×4 ASCII-art pattern and derives the anchor flags with
  specific adjacency rules (e.g. `ul` is set only when its cell is `#` *and* not
  both neighbours are). The 44-entry table and the `from` adjacency logic are
  reproduced exactly; the path is built by visiting set anchors in a fixed order
  and closing.
- **Braille 5-pass dot sizing (braille.zig).** Dots must stay crisp and evenly
  spread at any cell size, so the algorithm greedily distributes leftover pixels
  in five ordered passes: ensure non-zero dot width, then non-zero margins, then
  spacing, then more margins, then dot width again — with running `x/y_px_left`
  budgets and invariant asserts. Transcribed pass-for-pass.

## 2D backend decision (the one big design decision)

The crate's canvas needs a small set of operations onto an **alpha8** mask: fill
and stroke of paths (lines, cubic Béziers, circles) and simple prims
(rect/quad/triangle), plus per-pixel composite for the multiply trick and the
fading-line gradient. Options evaluated:

- **tiny-skia — chosen.** Mature (powers `resvg`), pure Rust, zero C deps.
  Provides `PathBuilder` (`line_to`/`cubic_to`/`push_circle`), `fill_path` with
  non-zero winding, `stroke_path` with butt/round caps — exactly the z2d surface
  area we use. Renders into a premultiplied RGBA `Pixmap`; we paint opaque white
  and read back the alpha channel.
- **raqote — rejected.** Also pure Rust and capable, but a smaller / less-
  maintained ecosystem and a heavier API for what is a narrow need; no advantage
  over tiny-skia for an alpha mask.
- **hand-rolled rasterizer — rejected.** Correct anti-aliased path filling and
  stroking (miters, caps, Bézier flattening) is a lot of subtle code to own; the
  seam-critical parts are already handled by the integer `Fraction`/`rect` path,
  so the vector backend only needs to be *good enough* and *stable* — not
  custom.

**How tiny-skia maps onto the Zig/z2d design.** The Zig `Canvas` keeps a single
alpha8 buffer and bypasses z2d for rect/pixel/invert/flip/trim. The port keeps
the same single `Vec<u8>` alpha buffer as the source of truth and does all of
those directly on it; only path fill/stroke go through tiny-skia, whose output
alpha is composited (`src_over`, scaled by the requested coverage) back into the
buffer. `innerStrokePath` is reproduced with two throwaway `Pixmap`s and the
same per-pixel multiply. This keeps `trim`, `invert`, the flips, and atlas
extraction operating on one contiguous buffer, exactly as upstream.

Consequence for parity: anti-aliased edges come from tiny-skia's scan
conversion rather than z2d's, so sub-pixel coverage on curved/diagonal glyphs
may differ by small amounts from the upstream golden PNGs. The seam-critical
straight box-drawing glyphs are pixel-identical because they use the integer
`Fraction`/`rect` path, which is backend-independent.

## Public API shape (extraction policy)

Per the extraction policy, the API carries **no** `qwertty-term-vt` types:

- Input: `Metrics` — a plain struct of cell width/height, line thicknesses,
  and decoration positions in pixels (`Metrics::simple(w, h)` derives sensible
  defaults). Codepoint is a plain `u32`.
- Output: `Glyph { width, height, offset_x, offset_y, alpha: Vec<u8> }` — a
  trimmed row-major alpha8 bitmap plus placement offsets, matching upstream's
  atlas-glyph offset convention.
- `Sprite` enum for the cursor/underline pseudo-codepoints (values match the Zig
  enum, above the Unicode range).
- `has_codepoint(cp)` / `render(cp, &metrics)` as the entry points.

## Tests

Upstream has no inline unit tests (its verification is golden-PNG fixtures in
`src/font/sprite/testdata/`, checked against with a wuffs PNG decode + pixel
diff in `Face.zig`). Those fixtures are now wired in directly — see the
"Golden-PNG parity vs upstream (MB3)" section below. Alongside them the port
carries a structural net that needs no reference image (see
`crates/qwertty-term-sprite/tests/sprites.rs`):

- **smoke** — representative codepoints of every range, and all special sprites,
  render to in-bounds bitmaps at 7 odd/even cell sizes;
- **seam** — `U+2500`/`U+2502` produce a single contiguous, fully-inked band
  (a continuous line across a tiled seam) at every size, plus the
  `Fraction::min`/`max` complement identity for all fractions over sizes 1..128;
- **coverage** — `dispatch_ranges_match_zig` pins the range set; a sweep asserts
  everything `has_codepoint` claims renders and that gap codepoints do not
  (>1000 codepoints claimed);
- **determinism** — same input → byte-identical output.

## Golden-PNG parity vs upstream (MB3)

The structural net above is now backed by a direct pixel comparison against
upstream's own golden fixtures. See `crates/qwertty-term-sprite/tests/golden_parity.rs`.

### Methodology

Upstream's `test "sprite face render all sprites"` (`src/font/sprite/Face.zig`)
renders every sprite codepoint into a set of **16×16 glyph atlases** — one PNG
per 0x100-aligned Unicode block — and diffs them against reference PNGs in
`src/font/sprite/testdata/`. It runs four cell metrics, passed as
`testDrawRanges(width, ascent, descent, thickness)`:

| config         | cell W×H | box/underline thickness | fixture suffix |
| -------------- | -------- | ----------------------- | -------------- |
| (18, 30, 6, 4) | 18×36    | 4                       | `18x36+4`      |
| (12, 20, 4, 3) | 12×24    | 3                       | `12x24+3`      |
| (11, 19, 2, 2) | 11×21    | 2                       | `11x21+2`      |
| (9, 15, 2, 1)  | 9×17     | 1                       | `9x17+1`       |

Those 36 reference PNGs (9 ranges × 4 sizes) are copied verbatim into
`crates/qwertty-term-sprite/tests/testdata/` and are the golden files.

The harness reconstructs each atlas from this crate's output and compares
pixel-for-pixel:

1. Each atlas cell is a padded box `stride = (W + 2·⌊W/4⌋) × (H + 2·⌊H/4⌋)`; the
   glyph occupies the inner `W×H`. Upstream's `Metrics.calc` sets
   `box_thickness = underline_thickness = ⌈thickness⌉`, so the harness builds
   `Metrics` **directly** with the exact thickness (using `Metrics::simple`
   would substitute its own heuristic thickness and mismatch).
2. `render()` returns a *trimmed* bitmap plus offsets; the harness inverts the
   `into_glyph` offset math (`clip_left = offset_x + ⌊W/4⌋`,
   `clip_top = stride_y − clip_bottom − height`) to place it back at its cell
   position, exactly reproducing upstream's un-trimmed atlas layout.
3. The PNGs are 8-bit alpha coverage; decode is via the `png` crate.

Per-range thresholds (a max single-pixel coverage delta and a max fraction of
differing pixels) encode the parity contract. Integer-path families are held to
**zero** (exact); antialiased families get a small perceptual budget sized just
above the measured worst case so the test still catches regressions.

### Results

All 36 (range × size) cells pass. Worst-case diff fraction per family, over all
four sizes (full table printed by `cargo test … golden_parity_report --
--nocapture`):

| family (range)                   | worst diff% | budget% | verdict                       |
| -------------------------------- | ----------- | ------- | ----------------------------- |
| box / block / geometric (U+2500) | 0.48        | 1.0     | exact except curves/diagonals |
| braille (U+2800)                 | 0.00        | 0.0     | pixel-exact, all sizes        |
| powerline (U+E000)               | 0.62        | 2.0     | AA on triangles/arcs          |
| branch (U+F500)                  | 1.27        | 2.0     | AA on curved strokes          |
| branch (U+F600)                  | 0.57        | 2.0     | AA on curved strokes          |
| legacy computing (U+1FB00)       | 3.03        | 4.0     | AA on diagonals/hatch/curves  |
| legacy supplement (U+1CC00)      | 0.59        | 1.0     | AA on triangles               |
| octants (U+1CD00)                | 0.00        | 0.0     | pixel-exact, all sizes        |
| legacy supplement (U+1CE00)      | 0.24        | 1.0     | AA on separated fills         |

Key finding: **every seam-critical, integer-path glyph is pixel-identical to
upstream** — all straight box-drawing lines, junctions, corners, tees, crosses,
block elements, braille dots, sextants and octants. In the entire U+2500 range
only 15 of 256 glyphs differ at all, and each is a rounded corner
(U+256D..2570), a diagonal (U+2571..2573) or a triangle (U+25E2.., U+25F8..).
No fixable defects were found; the port was already faithful.

The human artifact is `target/sprite-parity.png` (written by the
`write_specimen_artifact` test): one row per family at 18×36, three panels
**ours | upstream | diff**. The diff panel paints matches as faded gray, pixels
only upstream has red, pixels only we have green — the same convention as
upstream's `testDiffAtlas`. It shows the ours/upstream columns visually
indistinguishable and the diff reduced to thin green AA fringes on curves and
diagonals.

### Known acceptable divergences (each justified)

All divergences are sub-pixel antialiasing on non-axis-aligned edges, caused by
tiny-skia's scan conversion differing from upstream's z2d (see "2D backend
decision" above). None affect legibility, seam continuity, or the integer grid.

- **Rounded box corners (U+256D..2570), diagonals (U+2571..2573), triangles
  (geometric U+25E2.., U+25F8..; corner triangles).** Coverage on a sloped edge
  is a fill-rule/AA property of the rasterizer, not of the drawing geometry
  (identical path endpoints). Diffs are ≤0.5% of the range and confined to the
  one-pixel edge band.
- **Braille and octants are *not* divergent** — they are pure integer
  rectangles and match exactly, which is why they are pinned to zero tolerance.
- **Legacy-computing diagonal-hatch fills (U+1FB98/99) and rounded-diagonal
  boxes (U+1FBA0..) / curves (U+1FBD0..).** This is the largest divergence
  (3.03% at the smallest 9×17 cell, where a one-pixel AA band is proportionally
  biggest). The port is a line-for-line match of upstream's algorithm —
  upstream itself carries a `// TODO: This doesn't align properly for most cell
  sizes` on the hatch fills — so the difference is purely AA on near-vertical
  diagonals. Budgeted at 4%; all larger sizes stay under 2.3%.
- **Powerline / branch curves.** Arcs and flame separators antialiased slightly
  differently; ≤1.3% worst case, invisible at normal viewing.

### Fixes made

None. The comparison confirmed parity rather than surfacing defects: all
integer-path glyphs are already pixel-exact, and every remaining difference is
the anticipated, deferred tiny-skia-vs-z2d AA on curves/diagonals. The
contribution of this chunk is the permanent golden comparison itself (the 36
checked-in fixtures, the reconstruction harness, the per-family tolerance
contract, and the side-by-side artifact) so any future regression in the
integer paths — or any drift beyond the AA budget — fails CI.

## Deferrals

- **Renderer/atlas wiring.** Deliberately out of scope for this chunk.
- **Nerd-font constraint table.** Flagged as a companion extraction; not part of
  this port.
