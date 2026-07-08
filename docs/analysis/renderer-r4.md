# Renderer R4: the cell engine — first pixels

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse --short 2da015cd6`). Working-copy commit at
time of writing: `f87fde1c72b9`. Scope: `src/renderer/cell.zig` (680 lines, 5
inline tests) and the load-bearing subset of `src/renderer/generic.zig`
(`updateFrame` ~1123-1430, `drawFrame` ~1442-1724, `rebuildCells`/`rebuildRow`/
`addGlyph`/`addCursor`/`addUnderline`/`addOverline`/`addStrikethrough`
~2319-3374, `syncAtlasTexture` ~3370). Rust ports live at
`crates/qwertty-term-renderer/src/cells.rs` (Contents + classifiers) and
`crates/qwertty-term-renderer/src/engine.rs` (the render engine), with the
acceptance test at `crates/qwertty-term-renderer/tests/first_pixels.rs`.

This is chunk R4: the cell engine that turns a live-terminal
[`RenderSnapshot`] (R0) into GPU buffers and draws real glyphs through the R1
Metal resources, R2 frame/pass/swap-chain machinery, and R3 shader pipelines,
using the `qwertty-term-font` `Grid` (shaping + glyph→atlas) and `qwertty-term-sprite`
(cursors, decorations, box drawing). It builds strictly on R0-R3 + the font
crates; it adds new modules (`cells`, `engine`) and does not restructure the
R1-R3 modules.

## `cell.zig` → `cells.rs`

### `Contents`

The CPU-side cell store the engine fills each frame. Its shape is dictated by
upstream's row-wise dirty-clearing goal: a flat background-color array
(`bg_cells`, indexed `row * cols + col`) plus a per-row collection of
foreground [`CellText`] lists.

The **cursor-at-`fg[0]` convention** is load-bearing and ported exactly:
`fg_rows` holds `rows + 2` lists, not `rows`. List `0` and list `rows + 1` are
reserved for the cursor glyph; the real rows live at `fg_rows[y + 1]`. Block
cursors go in list `0` (drawn *first*, so text layers on top); every other
cursor style goes in list `rows + 1` (drawn *last*, over text). A single
flattened concatenation of all lists (`sync_from_slices`) then produces the
correct GPU draw order — cursor-under-text, rows, cursor-over-text — without a
separate cursor pass. `clear(y)` zeroes row `y`'s bg cells and clears
`fg_rows[y + 1]` only, so a dirty-row rebuild never disturbs untouched rows
(the whole point of the structure, even though the reduced cut always does a
full rebuild — see below).

Ported operations (1:1 with `cell.zig`): `resize`, `reset`, `bg_cell` (read) /
`set_bg_cell` (write) — the `bgCell` accessor, split into read/write since Rust
has no `*T` return; `add` (append a fg cell to `fg_rows[y + 1]`, with the
`y < rows` assert), `clear`, `set_cursor` (block → list 0, else → list
`rows+1`; both cleared first), `cursor_glyph` (peek both cursor lists). All 5
upstream inline tests are ported: `test Contents`, "Contents clear retains
other content", "Contents clear last added content", "Contents with zero-sized
screen", and the constraint-width classification test (reframed — see below).

Divergence: upstream's `add` is generic over a comptime `Key` selecting the GPU
vertex type (`bg` → `CellBg`, everything else → `CellText`), and `bg` is
`comptime unreachable` in `add` (backgrounds go through `bgCell`, not `add`).
The Rust `Key` is a runtime enum with only the fg variants
(`Text`/`Underline`/`Strikethrough`/`Overline`); backgrounds are never routed
through `add`, matching the upstream contract without the comptime dispatch.

### Codepoint classifiers

Ported from `cell.zig`'s free functions:

- `is_covering(cp)` — U+2588 FULL BLOCK only. Drives the "use fg color for the
  bg" rule so padding extension works over solid blocks (#2099).
- `is_symbol(cp)` — upstream reads a generated `symbols_table`; the reduced
  port enumerates the exact blocks that table covers (PUA + Arrows + Dingbats +
  Emoticons + Misc Symbols + Enclosed Alphanumerics (+ Supplement) + Misc
  Symbols and Pictographs + Transport/Map). Symbol-like glyphs are allowed to
  spill into a second (whitespace) cell.
- `no_min_contrast(cp)` — true for graphics elements (box drawing / block /
  legacy computing / Powerline). These feed `CellText.bools.no_min_contrast`,
  telling the shader to skip WCAG contrast enforcement so deliberate seam
  colors aren't distorted. This is one of the **contrasted-color CPU-side
  decision points** the plan calls for: the CPU decides *per glyph* whether the
  shader's min-contrast branch runs.
- `constraint_width(grid_width, cp, prev_cp, next_cp, at_last_col)` — the glyph
  constraint math, reduced to a pure function of the neighbour codepoints (the
  reduced Grid's rasterizer doesn't yet apply the constraint, but the width
  decision is ported and unit-tested so it's ready to wire). Grid width 2 → 2;
  non-symbols → their grid width; symbol at last col → 1; symbol after a
  (non-graphics) symbol → 1 (keeps PUA icons aligned); symbol before
  whitespace/nothing → 2; else 1. The upstream "Cell constraint widths" test's
  cases are reproduced as direct classifier calls.

## `generic.zig` load-bearing subset → `engine.rs`

`Engine` owns the Metal backend, the swap chain (sync mode, one live permit —
plan decision 3), the CPU-side `Contents`, the `Uniforms`, and the three
first-pixels pipelines (`bg_color`/`cell_bg`/`cell_text`), built from the R3
`PIPELINE_DESCRIPTIONS` table + embedded MSL source. `cell_width`/`cell_height`
(from the font `Grid`'s `Metrics`) fix cell geometry.

### `update_frame(&RenderSnapshot, &mut Grid, FrameOptions)` — the buffer build

Port of the buffer-building half of `updateFrame` + `rebuildCells` +
`rebuildRow`, with the threading (mutex/critical-section), kitty-graphics,
scroll-to-bottom, OSC8-link, search-highlight, selection, overlay, and
custom-shader branches removed (all deferred to later chunks). The reduced flow:

1. **Resize + reset.** If the snapshot's grid size changed, `Contents::resize`
   and recompute the target pixel size (`cols*cell_w × rows*cell_h`; no window
   padding in the reduced cut). Then `Contents::reset` — the reduced cut always
   does a full rebuild (`DirtyStatus::Full`; the full-copy `FullSnapshot` never
   reports partial — plan decision 4, day-one full redraw). The row-wise
   `clear`/dirty machinery is present in `Contents` for when dirty-row
   snapshots land, but `update_frame` doesn't consult `dirty()` yet.
2. **Resolve defaults.** Dynamic OSC 10/11 fg/bg (from the snapshot) win over
   the `FrameOptions` config defaults; the palette comes from the snapshot.
3. **Uniforms** (`build_uniforms`, port of `updateScreenSizeUniforms` + the
   uniform assignments in `updateFrame`): `projection_matrix = ortho2d(0, w, h,
   0)` (no padding, so the plain 0..w/0..h ortho), `screen_size`, `cell_size`,
   `grid_size`, `grid_padding = 0`, `padding_extend = 0`
   (padding_color=background in the reduced cut), `min_contrast`, `bg_color` (=
   default bg, alpha 255), and the three color-management bools all `false`
   (native BGRA, non-linear — matching R1's `linear_blending` stand-in and R3's
   `use_display_p3`/`use_linear_blending`/`use_linear_correction` uniform
   consumption).
4. **Cursor style resolution** via `cursor::style` (R0), fed the snapshot
   cursor + renderer-local focus/blink (preedit is not wired — deferred).
5. **Per-row rebuild** (`rebuild_row`, port of `rebuildRow`): for each
   non-spacer cell, resolve its final `(fg, bg, bg_alpha)` (see below), write
   the bg cell, then — unless the cell is invisible — add underline (first,
   under text), overline, the glyph, and strikethrough (last, over text) via
   the sprite/shaping paths.
6. **Cursor** (`build_cursor`, port of `addCursor` + the cursor uniform block
   of `rebuildCells`).

**The contrasted-color CPU-side decision points** (the plan's explicit ask —
"port the contrasted-color CPU-side decision points that feed the shader
uniforms"):

- `resolve_colors(style, cp, palette, default_fg, default_bg)` reproduces
  `rebuildRow`'s color resolution reduced to the snapshot color model. It
  honors the inverse flag *and* the covering-glyph rule: `use_fg_for_bg =
  inverse != is_covering(cp)` (they cancel if both). `bg_alpha` is `255` when
  the cell is inverse/covering/has an explicit bg, else `0` (so the surface bg
  shows through and no per-cell bg rect is painted) — the exact CPU-side
  bg-fill decision upstream makes, feeding `cell_bg`'s per-cell color buffer.
- `no_min_contrast(cp)` per glyph sets `CellText.bools.no_min_contrast`, and
  `Uniforms.min_contrast` (> 1.0 enables the shader's WCAG contrast remap).
  Together these are what feed the shader's `contrasted_color` branch: the CPU
  decides the threshold (uniform) and the per-glyph opt-out (instance bool);
  the shader does the WCAG math against the bg color it reads from buffer 2.
- The **block-cursor** path sets `Uniforms.cursor_pos`/`cursor_wide`/
  `cursor_color` so the `cell_text` fragment shader flips the glyph *under* the
  block cursor to the cursor-text color (upstream's cursor-text default = the
  cell background, giving inverted text under the cursor). `cursor_pos` is the
  sentinel `[u16::MAX, u16::MAX]` when there's no block cursor.

### Glyph path (`add_cell_glyph`, port of `addGlyph`)

Resolve the cell codepoint to a `FontIndex` via the `Grid`. Sprite codepoints
(box drawing etc.) route to `Grid::render_codepoint` (codepoint == glyph id, no
shaping) and emit a grayscale `CellText` with `no_min_contrast` set. Text
codepoints shape the cell's grapheme (base + combining marks) as a one-cell run
through the reduced rustybuzz `Shaper` (which maps clusters → cell X), then for
each shaped glyph `Grid::render_glyph` rasterizes into the grayscale atlas and
emits a `CellText` with the glyph's atlas rect + bearings, offset by the
shaper's per-glyph `x_offset`/`y_offset`. Zero-size glyphs (blanks) are
skipped, matching `addGlyph`'s early return.

Divergence from upstream's `rebuildRow`: upstream runs one `RunIterator` per
row (segmenting into style/font/cursor runs) and shapes each run once, caching
by run hash. The reduced cut shapes **per cell** (one grapheme at a time). This
is exact for the monospace ASCII/CJK/box first-pixels scope — style-run
segmentation is preserved because each cell carries its own resolved fg, and
sprite cells are already isolated — and it avoids threading run segmentation
and the shaper LRU (`shaper/Cache.zig`, deferred per the font-shaping analysis)
through the snapshot model. Ligatures and complex-script shaping are the
follow-on that reinstates the per-run iterator.

### Decorations (`add_decoration`, port of `addUnderline`/`add*`)

Underline (single/double/dotted/dashed/curly), overline, and strikethrough are
rendered as sprite glyphs from `qwertty-term-sprite` through the `Grid` (the
`Sprite::Underline`… pseudo-codepoints), added to the fg lists exactly as
upstream does — underlines before the glyph (layered under text), strikethrough
after. Their color is the underline color (falling back to fg) / fg, at the
cell's fg alpha.

### Cursor (`build_cursor`, port of `addCursor`)

Renders the cursor sprite (`CursorRect`/`CursorHollowRect`/`CursorBar`/
`CursorUnderline`) through the `Grid` and hands it to `Contents::set_cursor`
with its style, which places it in the block (`fg[0]`) or non-block
(`fg[rows+1]`) cursor list. Wide-cell handling is ported: if the cursor is over
a wide spacer tail, it moves back to the lead cell and `cursor_wide` is set. The
`lock` style needs a nerd-font symbol not in the reduced substrate and is
treated as no-cursor (documented deferral). Cursor color is the default fg
(OSC 12 not wired in the snapshot yet — deferral).

### `draw_frame` — encode the 3 pipelines (port of `drawFrame`'s cell section)

Sync-mode (plan decision 3): acquire a swap-chain slot (one live permit),
resize its target if needed, sync the uniforms / `cells_bg` / gathered `cells`
buffers, then encode one render pass with the three first-pixels steps in
upstream's order:

1. `bg_color` — full-screen triangle, no vertex buffer, uniforms at index 1,
   `cells_bg` at index 2 (buffer convention; the shader reads it for
   padding-extend). Fills the surface bg from the `bg_color` uniform. Blending
   disabled (first pass onto a cleared target).
2. `cell_bg` — full-screen triangle sampling the per-cell `cells_bg` buffer at
   index 2; premultiplied-over blending.
3. `cell_text` — instanced glyph quads: vertex buffer 0 = the gathered
   `CellText` instances, uniforms at 1, `cells_bg` at 2 (for the shader's
   min-contrast bg lookup), textures 0/1 = grayscale/color atlas;
   `triangle_strip`, 4 vertices, `fg_count` instances.

The image / overlay / kitty / custom-shader / post-process passes are all
omitted (deferred). The frame completes synchronously (`waitUntilCompleted`),
after which the IOSurface pixels are coherent and read back via
`Target::read_pixels` (R1) and returned to the caller.

### `sync_atlas` (port of `syncAtlasTexture` + the modified-counter gate)

Uploads the grayscale atlas into the slot `next_frame` will hand out next
(`SwapChain::peek_next_index` + `slot_mut`, both added to the swap chain), gated
on the atlas `modified` counter vs a per-slot last-synced value (upstream
`frame.grayscale_modified`) so an unchanged atlas isn't re-uploaded. If the
atlas outgrew the slot's texture, the texture is reallocated at the new size
(upstream frees + `initAtlasTexture`), then the full region is `replace_region`d
(the CPU streaming path from R1). Must be called after `update_frame` (which
populates the atlas via glyph rendering) and before `draw_frame`.

**Color-atlas seam** (noted per the task): the reduced cut renders all text into
the *grayscale* atlas (`Atlas::Grayscale` on every instance); the color/emoji
atlas is deferred with emoji presentation (see `font-shaping.md`). The 1×1 color
texture stays bound so the shader's two-atlas sampling is well-formed, but no
color-atlas instance is ever emitted, so `sync_atlas` only syncs grayscale.

## THE ACCEPTANCE TEST (`tests/first_pixels.rs`) — offscreen, no window

Drives a real `qwertty_term_vt::Terminal` through a `Stream`:

- Row 0: `\x1b[32m$ \x1b[0mhello` — a green prompt then plain "hello".
- Row 1: `世界──` — two wide CJK chars then two box-drawing chars (U+2500).
- Cursor moved to row 2 (0-indexed), col 3 (`\x1b[3;4H`) — a block cursor over
  an empty cell.

Then: `FullSnapshot::capture` → `Engine::update_frame` → `sync_atlas` →
`draw_frame` (which `waitUntilCompleted`s) → read back the IOSurface pixels
(BGRA, row-padding stripped). Per-assertion results **on this dev Mac (PASS)**:

- **Assertion 1 — background:** an empty cell (row 0, col 18) matches the
  default bg (`0x18` gray) within a ±6 channel-sum delta. **PASS.**
- **Assertion 2 — glyph coverage:** the 'h' cell (row 0, col 2) has a
  max-vs-bg delta > 40 (real ink present). **PASS.**
- **Assertion 3 — wide char spans 2 cells:** the snapshot marks row 1 col 0 as
  the wide lead and col 1 as the spacer, and the engine skips the spacer while
  placing one glyph at the lead — the load-bearing 2-cell property, asserted at
  the grid level. **PASS.** (See the deferral below re: CJK *pixels*.)
- **Assertion 4 — box-drawing sprite coverage:** the first U+2500 cell (row 1,
  the col after the two wide chars) has a max-vs-bg delta > 40 — the sprite
  rasterized and drew. **PASS.**
- **Assertion 5 — cursor fill:** the cursor cell (row 2, col 3) center is not
  the bg (delta > 40) and is the cursor color (default fg) within ±20 — the
  block cursor filled the cell. **PASS.**
- **Bonus:** the frame is dumped to `target/first-pixels.png` (a hand-rolled
  RGBA PNG encoder, stored/uncompressed zlib blocks, no image-crate dep) and
  the path is printed. Visual confirmation: green "$ ", white "hello", the box
  line on row 1, and the cursor block on row 2 all render correctly.

The test skips gracefully (prints `SKIP:`) when no Metal device is present,
matching the R1/R2/R3 convention.

## Priority ladder — what completed

The full ladder completed: **Contents + tests** > **update_frame (text)** >
**draw_frame + acceptance test (text)** > **cursor** > **underlines/decorations**
> **min-contrast** (CPU-side decision points ported: `no_min_contrast` per
glyph + `min_contrast` uniform + block-cursor color override feed the shader).

## Snapshot-API additions

**None.** The R0 `RenderSnapshot` trait + `FullSnapshot` and `qwertty-term-vt`'s
`SnapshotWindow`/`SnapshotCell`/`SnapshotCursor` already carried everything the
engine needs (resolved per-cell `CellStyle`, wide/spacer widths, cursor
position/style/visibility, palette, dynamic fg/bg). No additive accessors were
required on `qwertty-term-vt`'s `snapshot.rs`.

## Swap-chain additions

Two read-only-ish helpers added to `SwapChain` (`swap_chain.rs`) so the engine
can sync the upcoming slot's atlas texture out-of-band before drawing:
`peek_next_index()` (the index `next_frame` will hand out, without advancing or
taking a permit) and `slot_mut(index)` (mutable slot access for per-slot
resource updates). These don't change the existing R2 API.

## Deferrals (out of scope for R4)

- **CJK/emoji glyph pixels.** The reduced substrate shapes only byte-backed
  faces (rustybuzz over embedded font bytes); the sole embedded face (JetBrains
  Mono) has no CJK glyph, and name-loaded system faces have no `source_bytes`
  for the shaper. So the wide-char *pixel* rendering is out of reach — the
  acceptance test proves 2-cell spanning at the grid level instead, and proves
  the wide/two-cell rendering path with the box-drawing sprite. A byte-backed
  CJK fallback face (or a CoreText shaper) reinstates CJK pixels.
- **Color atlas** (emoji): all text goes to the grayscale atlas (see seam
  above).
- **Per-run shaping + shaper LRU** (`shaper/Cache.zig`): the reduced cut shapes
  per cell; ligatures/complex-script need the per-run `RunIterator`.
- **Dirty-row incremental rebuild:** `Contents` supports it (`clear`/per-row
  lists), but `update_frame` always does a full rebuild until a `DirtySnapshot`
  lands (R0's contract).
- **Async pacing / present to a window:** sync mode + offscreen readback only;
  the window host is R5. `draw_frame` returns readback pixels rather than
  presenting to a CALayer.
- **Preedit / OSC-12 cursor color / OSC-8 links / selection / search
  highlights / kitty images / overlays / custom shaders:** all removed from the
  reduced `update_frame`/`drawFrame`.
- **Nerd-font `lock` cursor symbol** (`0xF023`): treated as no-cursor.
- **Window padding / padding-extend:** `grid_padding = 0`, `padding_extend = 0`
  (padding_color=background); the R0 `never_extend_bg` heuristic and the
  extend-into-padding shader path aren't exercised.
- **Config plumbing:** `FrameOptions` stands in for the config-derived
  fg/bg/min-contrast/focus/blink until config lands.
