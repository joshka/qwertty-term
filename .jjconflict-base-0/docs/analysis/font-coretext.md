# CoreText face loading & rasterization (analysis)

Analysis of Ghostty's macOS CoreText font backend, for the ghostty-rs port
(chunk **M3 F5-reduced**). All line references are against
`src/font/face/coretext.zig` and `src/font/face.zig` at commit **`2da015cd6`**
(read via `git show 2da015cd6:…` from the reference checkout).

This complements `docs/analysis/font-foundations.md`, which covers the
table-derived `Metrics`/`FaceMetrics` layer (F1). Here we cover: how a
`CTFont` is loaded, how upstream reconciles CoreText metric accessors against
sfnt tables, how glyphs are rasterized through a CoreGraphics bitmap context,
color-glyph detection, and the points→pixels DPI handling.

## 1. Loading a CTFont

### 1.1 Load-by-name (discovery, reduced)

The reduced discovery path is exactly what upstream's own `coretext.zig` tests
use (`test "name"`, coretext.zig:1000-1016, and `test` at :970-998):

```zig
const name = try macos.foundation.String.createWithBytes("Menlo", .utf8, false);
const desc = try macos.text.FontDescriptor.createWithNameAndSize(name, 12);
const ct_font = try macos.text.Font.createWithFontDescriptor(desc, 12);
var face = try Face.initFontCopy(ct_font, .{ .size = .{ .points = 12 } });
```

i.e. `CTFontDescriptorCreateWithNameAndSize(name, size)` →
`CTFontCreateWithFontDescriptor(desc, size, matrix=null)`. The full
name-matching path in `discovery.zig` (`toCoreTextDescriptor`,
discovery.zig:161-243, building a `CTFontCollection` and score-sorting the
matches) is **out of scope** for F5-reduced; the descriptor-from-name shortcut
above is sufficient and is what the port implements.

Note a CoreText quirk that matters for the "nonsense name falls back
gracefully" requirement: `CTFontDescriptorCreateWithNameAndSize` **never
fails** on a bad name — it returns a descriptor that CoreText later resolves to
a system default face (typically Helvetica/`.AppleSystemUIFont`). Upstream never
hits this because discovery filters candidates up front; the port therefore
loads the CTFont, reads back its resolved family name (`copyFamilyName`,
coretext.zig:205-206), and treats a family that does not case-insensitively
contain the requested name as a **miss** → embedded fallback.

### 1.2 Load-from-data (embedded fallback)

`Face.init` (coretext.zig:51-70):

```zig
const data = try macos.foundation.Data.createWithBytesNoCopy(source);
const desc = macos.text.createFontDescriptorFromData(data) orelse
    return error.FontInitFailure;
const ct_font = try macos.text.Font.createWithFontDescriptor(desc, 12);
return try initFontCopy(ct_font, opts);
```

`createFontDescriptorFromData` is `CTFontManagerCreateFontDescriptorFromData`.
The port uses this for the embedded JetBrains Mono fallback (a miss on
load-by-name, or a caller that explicitly wants the bundled font). The data is
copied into a `CFData` (the port uses `CFDataCreate` rather than
`…NoCopy`, so the `CFData` owns its bytes and the caller's slice need not
outlive it — simpler and safe for the `&'static` embedded bytes anyway).

### 1.3 Sizing: `initFontCopy` and points→pixels

Discovery loads at a nominal size (12), then the final size is applied by
`initFontCopy` (coretext.zig:76-88):

```zig
const ct_font = try base.copyWithAttributes(opts.size.pixels(), null, null);
```

The size passed to `copyWithAttributes` / `CTFontCreateWithFontDescriptor` is
**`opts.size.pixels()`**, i.e. CoreText's "point size" argument is fed the
*pixel* size. `DesiredSize.pixels()` (face.zig:55-58) is:

```zig
pub fn pixels(self: DesiredSize) f32 {
    return (self.points * @as(f32, @floatFromInt(self.ydpi))) / 72;
}
```

and `default_dpi` **on macOS is 72** (face.zig:28:
`if (builtin.os.tag == .macos) 72 else 96`). So with the default DPI,
`pixels() == points`, and CoreText's internal point size numerically equals the
pixel size. The renderer supplies the real screen DPI at runtime; on a 2× (144
dpi) display, `pixels() = points * 2`.

**Port decision (F5-reduced):** the reduced API is `load_by_name(name,
size_px)` — the caller passes the already-resolved pixel size, so the port
feeds `size_px` straight to CoreText as the point-size argument, exactly
matching what upstream's `pixels()` produces under the default 72-dpi path.
The full `DesiredSize { points, xdpi, ydpi }` shape (face.zig:44-70) is
deferred to whichever chunk wires config → face; the *conversion* it performs
is documented here so it can be reintroduced without surprise.

`px_per_em` for metrics is read straight back from the loaded font via
`CTFontGetSize` (coretext.zig:639: `const px_per_em: f64 = ct_font.getSize();`).

## 2. Metrics: CoreText accessors vs sfnt tables (reconciliation)

`getMetrics` (coretext.zig:570-868) is the crux of the reconciliation question.
Upstream's rule is **tables first, CoreText accessors only as a fallback when a
table is missing or a field is absent**. Concretely, it `CTFontCopyTable`s the
`head`/`post`/`OS/2`/`hhea` tables (coretext.zig:574-632) and parses them with
its *own* opentype parser (the same tables F1's `tables::face_metrics` reads via
ttf-parser). The CoreText accessors appear only in the `orelse` arms:

| Metric                         | Primary source (table)       | CoreText fallback                                              | coretext.zig |
| ------------------------------ | ---------------------------- | -------------------------------------------------------------- | ------------ |
| units_per_em                   | `head.unitsPerEm`            | `CTFontGetUnitsPerEm`                                          | 634-638      |
| px_per_em                      | — (always CT)                | `CTFontGetSize`                                                | 639          |
| ascent/descent/line_gap        | `hhea` + `OS/2` ladder       | `getAscent`/`-getDescent`/`getLeading` (only if **no `hhea`**) | 642-702      |
| underline pos/thick            | `post`                       | none (→ `null`, estimated)                                     | 704-725      |
| strikethrough pos/thick        | `OS/2`                       | none (→ `null`, estimated)                                     | 727-744      |
| cap_height / ex_height         | `OS/2.sCapHeight`/`sxHeight` | `getCapHeight`/`getXHeight` (per-field)                        | 748-765      |
| cell_width (max ASCII advance) | — (always CT glyph query)    | `getAdvancesForGlyphs`                                         | 773-805      |
| ascii_height (ASCII bbox)      | — (always CT glyph query)    | `getBoundingRectsForGlyphs`                                    | 773-805      |
| ic_width ("水")                | — (always CT glyph query)    | `getAdvances`/`getBoundingRects`                               | 807-846      |

The vertical-metrics ladder (coretext.zig:642-702) is intricate and F1 already
ports it verbatim in `tables::vertical_metrics`: use `OS/2` `sTypo*` if
`fsSelection.use_typo_metrics`; else prefer non-zero `hhea`; else non-zero
`OS/2` `sTypo*`; else `OS/2` `usWin*` (flipping usWinDescent's sign because it
is positive-down). The one path F1 **cannot** reach (and does not need to for a
well-formed font) is the "no `hhea` table at all" arm that falls to
`getAscent`/`getDescent`/`getLeading` — a font with no `hhea` is malformed and
ttf-parser would reject it earlier.

**How the port reconciles the two backends.** For a font loaded from the *same
bytes* (the embedded JetBrains Mono), the sfnt tables CoreText copies out are
byte-identical to the tables ttf-parser parses. Therefore every *table-derived*
field (ascent, descent, line_gap, underline, strikethrough, cap/ex when OS/2
carries them, units_per_em) is identical between the two backends by
construction. The only fields that can legitimately differ are the three that
have **no table equivalent** and are computed by measuring glyphs:

- **cell_width** — max advance over printable ASCII. CoreText's
  `getAdvancesForGlyphs` returns *scaled, hinted, subpixel* advances; F1
  measures `glyph_hor_advance` (unscaled design units) and scales linearly. For
  a monospace font every ASCII advance is the same design value, so both agree
  to sub-pixel precision.
- **ascii_height** — height of the union bounding box of printable ASCII.
  CoreText's `getBoundingRectsForGlyphs` returns the *rasterizable* bounds
  (may include hinting/overshoot adjustments); F1 unions the `glyph_bounding_box`
  extents from the `glyf`/`CFF` outlines. These can differ by a fraction of a
  pixel. `ascii_height` only feeds `ic_width` estimation and the icon-height
  heuristic, so a sub-pixel delta does not move `cell_width`/`cell_height`/
  `cell_baseline`.
- **ic_width** — advance of "水" if present (absent in JBM → `None` both ways).

The port therefore exposes `Face::face_metrics()` that builds a `FaceMetrics`
using CoreText for exactly the three glyph-measured fields and the CT-reported
`px_per_em`/`units_per_em`, and reuses F1's *table* reads for the rest by
parsing the same font bytes with ttf-parser. The reconciliation test compares
the resulting `Metrics::calc(...)` cell_width/cell_height/cell_baseline against
F1's pinned values (see §5).

## 3. Rasterization (`renderGlyph`, coretext.zig:289-567)

Given a glyph id, upstream:

1. **Bounding rect** (coretext.zig:301): `getBoundingRectsForGlyphs(.horizontal,
   [glyph], null)` — CoreGraphics coord space, origin bottom-left, +Y up. This
   rect's origin gives the glyph's bearings; its size gives the ink extent.
2. **Color / sbix detection** (coretext.zig:304-306): `isColorGlyph` (see §4);
   `sbix` = color and the face has an `sbix` table (bitmap emoji).
3. **Synthetic bold** (coretext.zig:315-320): if `synthetic_bold` is set and not
   sbix, grow the rect by `line_width` on width/height and shift origin by
   `-line_width/2` on each axis (the stroke bleeds half a line-width past each
   edge). `line_width` = `max(points/14, 1)` (coretext.zig:193-196).
4. **Empty-glyph short circuit** (coretext.zig:326-334): if either rect
   dimension `< 0.25` px, return a 0-sized glyph (space, control chars).
5. **Constraints / cell-centering** (coretext.zig:336-390): the reduced port
   does **not** implement nerd-font `constrain(...)` or sbix pixel-quantization
   (that is F6/F7 territory) — it keeps the natural glyph size. It *does* keep
   the sub-pixel bookkeeping below, which is what makes rasterization correct.
6. **Sub-pixel canvas sizing** (coretext.zig:396-413):
   - `canvas_padding = 1` if `thicken && !sbix` else `0` (font-smoothing may add
     ≤1 px per edge). The reduced port's `rasterize` uses `thicken=false`, so
     padding is 0; the field is preserved for parity.
   - whole-pixel bearings `px_x = floor(x) - pad`, `px_y = floor(y) - pad`;
   - fractional remainder `frac_x = x - floor(x)`, `frac_y = y - floor(y)`;
   - canvas `px_width = ceil(width + frac_x) + 2*pad`, likewise height.
7. **Bitmap context config** (coretext.zig:415-461) — the settings the port
   must mirror exactly:

   |                | text (non-color)                  | color (emoji)                                    |
   | -------------- | --------------------------------- | ------------------------------------------------ |
   | depth          | 1                                 | 4                                                |
   | colorspace     | `linearGray`                      | `displayP3`                                      |
   | bitmap info    | `AlphaOnly` (`kCGImageAlphaOnly`) | `ByteOrder32Little \| PremultipliedFirst` (BGRA) |
   | bits/component | 8                                 | 8                                                |
   | bytes/row      | `px_width * depth`                | `px_width * 4`                                   |

   The buffer is zero-filled (`@memset(buf, 0)`), then an explicit fill —
   `setGrayFillColor(0,0)` / `setRGBFillColor(0,0,0,0)` — plus `fillRect` over
   the whole canvas (coretext.zig:464-476) to guarantee no uninitialized pixels.
8. **Context flags** (coretext.zig:482-498):
   - `setAllowsFontSmoothing(true)`, `setShouldSmoothFonts(thicken)`;
   - `setAllowsFontSubpixelPositioning(true)`, `setShouldSubpixelPositionFonts(true)`
     — needed for the fractional alignment;
   - `setAllowsFontSubpixelQuantization(false)`, `setShouldSubpixelQuantizeFonts(false)`
     — off, because the port manages positions itself;
   - `setAllowsAntialiasing(true)`, `setShouldAntialias(true)`.
9. **Draw color** (coretext.zig:500-508): color glyphs draw white
   (`setRGBFillColor(1,1,1,1)` + stroke); text glyphs draw
   `setGrayFillColor(strength/255, 1)` where `strength` is `thicken_strength`
   (`0` in the reduced path → gray 0 == white in an alpha-only context, i.e.
   full coverage). The alpha channel is what we keep.
10. **Synthetic bold stroke** (coretext.zig:512-515):
    `setTextDrawingMode(.fill_stroke)` with `setLineWidth(line_width)`.
11. **CTM: translate then scale** (coretext.zig:522-534):
    `translateCTM(frac_x + pad, frac_y + pad)` positions the glyph's bottom-left
    at the right sub-pixel offset; `scaleCTM(width/rect.w, height/rect.h)`
    applies any constraint stretch (identity in the reduced port).
12. **Draw** (coretext.zig:542-545): `drawGlyphs([glyph],
    [{ -rect.origin.x, -rect.origin.y }], ctx)` — the negated bearings place the
    glyph's ink bottom-left at CTM origin `[0,0]`.
13. **Bearings out** (coretext.zig:553-566):
    - `offset_x = px_x` — left of cell to left of ink box;
    - `offset_y = px_y + px_height` — **bottom of cell to top of ink box**
      (baseline-relative +Y-up bearing; note the +height flip).

### Mapping to the port's `Bitmap`

The reduced `rasterize(glyph_id) -> Bitmap` returns:

```text
Bitmap { width, height, bearing_x, bearing_y, data: Alpha8 | Bgra }
```

- `width`/`height` = `px_width`/`px_height`;
- `bearing_x` = `offset_x` (= `px_x`);
- `bearing_y` = `offset_y` (= `px_y + px_height`), matching upstream's
  `Glyph.offset_y` — distance from cell bottom to ink-box top;
- `data` = the raw context buffer: `Alpha8` (1 bpp) for text,
  `Bgra` (4 bpp, premultiplied, little-endian BGRA) for color glyphs.

Atlas upload (`atlas.reserve` + `atlas.set`, coretext.zig:548-549) is left to
the caller (F6/renderer); `rasterize` returns the CPU bitmap only.

## 4. Color-glyph detection (`ColorState`, coretext.zig:890-968)

At face init (coretext.zig:103-107): if `getSymbolicTraits().color_glyphs`
(the `kCTFontTraitColorGlyphs` bit, 1<<13), build a `ColorState`; else the face
can never be colored and `color` stays `null`.

`ColorState.init` (coretext.zig:905-945) reads two tables:

- `sbix` (coretext.zig:909-914): presence of a non-empty `sbix` table ⇒ assume
  every glyph is a color bitmap (upstream's own TODO to refine).
- `SVG` (the space-padded `"SVG "` tag; coretext.zig:917-938): parsed; per-glyph
  presence is queried later.

`isColorGlyph(glyph_id)` (coretext.zig:952-967): cast to u16 (special >16-bit
ids are never colored); `true` if `sbix`; else `true` if the SVG table has the
glyph.

**Port scope:** the reduced port ports the *symbolic-trait* gate
(`has_color`) and the sbix-presence check (enough to pick Alpha8 vs Bgra for
the embedded fonts, none of which are color — so the text path is exercised).
Full SVG-table glyph membership is deferred; documented here.

## 5. Metrics reconciliation test (vs F1)

F1's smoke test pins, for embedded JetBrains Mono @ 16px:
`cell_width=10, cell_height=21, cell_baseline=5, underline_position=18,
underline_thickness=1, strikethrough_position=11, strikethrough_thickness=1`
(flagged **unverified-vs-upstream**). This chunk verifies them by loading the
same embedded bytes through CoreText and comparing `Metrics::calc` output. The
verdict is recorded in the crate's `coretext` test module and in the chunk
hand-off notes.
