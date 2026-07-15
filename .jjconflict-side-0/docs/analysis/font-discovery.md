# Font discovery, the `Score` ranking, and the full fallback resolver (analysis)

Analysis of Ghostty's macOS CoreText **discovery** layer and the
`CodepointResolver`'s **full fallback chain**, for the qwertty-term port (chunk
**M3 F5-full**). This completes the deferred half of F5: F5-reduced landed
load-by-name + a single-style `Collection` + a two-step resolver (sprite +
primary), documented in `docs/analysis/font-coretext.md` and
`docs/analysis/font-shaping.md`. This doc covers what those explicitly deferred:
the discovery `Score` algorithm, `DeferredFace` lazy loading, resolver steps
4-7 (fallback search incl. on-demand discovery), `Collection` style completion +
fallback list, and Apple-Color-Emoji / presentation handling.

All line references are against commit **`2da015cd6`** (read via
`git show 2da015cd6:src/font/…` from the reference checkout at
`/Users/joshka/local/ghostty`). Files:
`discovery.zig` (1,349), `CodepointResolver.zig` (562), `Collection.zig`
(1,523), `DeferredFace.zig` (530), `face/coretext.zig` (1,162).

## 0. Where F5-reduced stopped, and what "full" adds

F5-reduced (per `font-shaping.md`) implemented the resolver as
**sprite? → primary-has-it? → else notdef**, with a `Collection` holding one
`regular` face plus empty style slots. Anything not in the embedded JetBrains
Mono (emoji, CJK, symbols the primary lacks) resolved to notdef.

F5-full closes that gap with the pieces upstream uses to actually *find a font*
for an arbitrary codepoint:

1. **CoreText discovery** (`discovery.zig`'s `CoreText`): descriptor → matching
   font descriptors → `Score`-sorted list, plus the `CTFontCreateForString`
   codepoint path.
2. **`DeferredFace`**: a lazily-loaded face that answers `hasCodepoint` cheaply
   (a CMap probe on the already-materialized `CTFont`) without paying the full
   `Face` init cost until the glyph is actually needed.
3. **Resolver steps 4-7**: exact match over the priority list, regular retry,
   discovery fallback (adds a discovered deferred face to the collection), and
   the any-presentation last resort.
4. **`Collection` completion**: a fallback list per style + `completeStyles`
   (synthesize italic/bold from regular).
5. **Presentation**: `text` (VS15/U+FE0E) vs `emoji` (VS16/U+FE0F), the UCD
   default for a bare codepoint, and how it steers face selection and the
   text-vs-color atlas.

## 1. The `Descriptor` (search query)

`discovery.Descriptor` (discovery.zig:34-89) is the platform-neutral search
query. Fields the CoreText path uses:

| field        | meaning                                             | zig    |
| ------------ | --------------------------------------------------- | ------ |
| `family`     | family name ("Fira Code", "monospace", …)           | :43    |
| `style`      | style-name string filter ("Bold Italic", …)         | :49    |
| `codepoint`  | a codepoint the font MUST render (0 = don't care)   | :52    |
| `size`       | point size (for emoji px conversion; 72 dpi on mac) | :57    |
| `bold`       | want the bold **trait**                             | :61    |
| `italic`     | want the italic **trait**                           | :62    |
| `monospace`  | want the monospace **trait**                        | :63    |
| `variations` | variation-axis targets (wght/ital/slnt)             | :68    |

`toCoreTextDescriptor` (discovery.zig:161-244) builds a `CTFontDescriptor` from
these by assembling a `CFMutableDictionary` of attributes:

- `kCTFontFamilyNameAttribute` ← `family` (if set)
- `kCTFontStyleNameAttribute` ← `style` (if set)
- `kCTFontCharacterSetAttribute` ← a `CFCharacterSet` over the single
  `codepoint` (if `> 0`) — this is what makes CoreText's collection query
  *pre-filter* to fonts that contain the codepoint.
- `kCTFontSizeAttribute` ← rounded `size` (if `> 0`)
- `kCTFontTraitsAttribute` ← a nested dict `{ kCTFontSymbolicTrait: <u32> }`
  built from `{ bold, italic, monospace }` symbolic-trait bits, but *only* if at
  least one bit is set (traits_cval > 0).

**Port scope.** The port models `Descriptor` with the same fields. It builds the
descriptor dictionary with `family`, `character_set` (from `codepoint`),
`style`, `size`, and the symbolic-traits nested dict. Variation-axis targeting
in the descriptor is **deferred** (the `Score` still reads a font's variation
axes to *derive* bold/italic — see §2 — but we do not push variation targets
into the search descriptor; documented as a deferral).

## 2. The `Score` algorithm (discovery.zig:586-830) — ported faithfully

CoreText's `CTFontCollectionCreateMatchingFontDescriptors` returns an *unsorted*
candidate array. Upstream sorts it with `sortMatchingDescriptors`
(discovery.zig:568-584), which orders by a `Score` computed per candidate; a
**higher** score sorts **earlier** (`lhs.int() > rhs.int()`).

`Score` is a Zig `packed struct` whose fields are laid out **least- to
most-significant**, so the struct-as-integer comparison is a lexicographic
comparison with the *last-declared* field as the highest-priority tiebreak. The
fields, in increasing precedence (discovery.zig:592-620):

| field (low→high bit) | type   | meaning                                            | weight/precedence |
| -------------------- | ------ | -------------------------------------------------- | ----------------- |
| `glyph_count`        | `u16`  | number of glyphs in the font, clamped to `u16` max | lowest tiebreak   |
| `fuzzy_style`        | `u8`   | fuzzy style-string match quality (higher = better) | ↑                 |
| `bold`               | `bool` | font bold-ness matches descriptor's `bold`         | ↑                 |
| `italic`             | `bool` | font italic-ness matches descriptor's `italic`     | ↑                 |
| `exact_style`        | `bool` | case-insensitive exact match on style string       | ↑                 |
| `monospace`          | `bool` | font has the monospace trait                       | ↑                 |
| `codepoint`          | `bool` | font contains the requested codepoint              | **highest**       |

Precedence rationale (upstream comments):

- **`codepoint` is the single most important thing** when a codepoint is
  requested — a font that renders the char always beats one that doesn't.
- **`monospace`** is next: for a terminal we strongly prefer monospace, but
  never over having the actual glyph.
- **`exact_style`** beats trait matches so a user can override trait-based
  bold/italic selection by naming a style explicitly.
- **`italic` before `bold`**: being wrongly (non-)italic is subjectively worse
  than being the wrong weight.
- **`fuzzy_style`** is a soft signal below the hard trait matches.
- **`glyph_count`** is the final tiebreak: all else equal, prefer the font with
  more glyphs (usually the more complete family member).

### 2.1 How each dimension is computed (`Score.score`, discovery.zig:626-830)

The candidate `CTFontDescriptor` is first **loaded** into a `CTFont` at size 12
(discovery.zig:632-636). If loading fails the score stays all-zero (we never
want a font we can't load). Then:

- **`glyph_count`** (discovery.zig:639-645): `CTFontGetGlyphCount`, cast to
  `u16`, saturating at `u16` max.
- **`codepoint`** (discovery.zig:647-664): if `desc.codepoint > 0`, encode it to
  UTF-16 (surrogate pair for astral) via
  `CFStringGetSurrogatePairForLongCharacter`, then
  `CTFontGetGlyphsForCharacters` — `true` iff the font maps the char to a glyph.
- **`monospace`** (discovery.zig:668-679): read the descriptor's
  `kCTFontTraitsAttribute` → `kCTFontSymbolicTrait` number → `monospace` bit.
- **`bold` / `italic`** (discovery.zig:683-782): a *derived* boolean, refined
  beyond the symbolic traits by reading raw sfnt tables and variation axes:
  1. start from symbolic-traits `bold`/`italic`;
  2. OR-in `head.macStyle` bit 0 (bold) / bit 1 (italic) if the `head` table
     parses;
  3. OR-in `OS/2.fsSelection.bold` / `.italic` if the `OS/2` table parses;
  4. if the font has variation axes AND an instance's `variation` values:
     - `wght` value `> 600` ⇒ bold (**replaces**, not ORs — a variable font's
       weight instance is authoritative);
     - `ital` value `> 0.5` ⇒ italic (and once seen, `slnt` is ignored);
     - else `slnt <= -5.0` ⇒ italic (≥5° clockwise slant).

  The score field is then `self.bold = (desc.bold == is_bold)` and
  `self.italic = (desc.italic == is_italic)` — i.e. it scores a *match between
  what was asked and what the font is*, not raw bold-ness.
- **`exact_style` / `fuzzy_style`** (discovery.zig:784-827): read the
  descriptor's `kCTFontStyleNameAttribute` string. The set of *desired* styles
  is `desc.style` if given, else derived from `bold`/`italic`:
  - bold+italic → `{"bold italic","bold","italic","oblique"}`
  - bold → `{"bold","upright"}`
  - italic → `{"italic","regular","oblique"}`
  - neither → `{"regular","upright"}`

  `exact_style` = case-insensitive equality of the style string with
  `desired[0]`. `fuzzy_style` starts at `style_str.len`, subtracts (saturating)
  the length of each desired substring that appears in the style string, then is
  flipped to `maxInt(u8) -| remainder` so that **fewer non-matching characters
  ⇒ higher score**. This is the mechanism the disabled `test "coretext sorting"`
  documents: for an italic request, "SF Pro Regular Italic" outscores "SF Pro
  Thin Italic" — both contain "italic", but the longer non-desired remainder
  ("Thin ") lowers the score less than a naive shortest-name rule would, and the
  test's comment explicitly notes it must NOT prefer the shorter "Thin Italic".

### 2.2 Port representation of `Score`

Rust has no packed-struct-as-integer, so the port encodes the same lexicographic
order explicitly. Two faithful options; the port uses **(a)**:

(a) a `#[derive(PartialEq, Eq, PartialOrd, Ord)]` **tuple/struct in
most-significant-first field order** — `(codepoint, monospace, exact_style,
italic, bold, fuzzy_style, glyph_count)` — so derived `Ord` is exactly
upstream's `int()` comparison, and we sort descending (higher = earlier). This
is the same ordering as the packed struct read most-significant-first.

(b) pack the fields into a single `u64` by hand. Rejected: error-prone and no
clearer than (a); the tuple's derived `Ord` is provably the packed comparison.

The port keeps every scored dimension and its weight. The `head`/`OS/2` reads
reuse F1's `ttf-parser`-based table access (over the CoreText-copied table
bytes) rather than a bespoke opentype parser, matching the reduction already
made in `font-coretext.md` §2. Variable-axis derivation (`wght`/`ital`/`slnt`)
is ported against CoreText's `variation_axes` / `variation` dictionaries.

**Determinism.** The sort is `std.mem.sortUnstable` upstream; ties are broken by
`glyph_count` and then by the unstable sort's arbitrary order. The port sorts
with a total order whose final tiebreak, after `glyph_count`, is the CoreText
candidate array's original index — making the port's ranking **fully
deterministic** for a fixed system font set (the determinism test relies on
this; see §6).

## 3. Codepoint search: `CTFontCreateForString` vs collection matching

Upstream has **two** ways to find a font for a codepoint, and uses them in a
specific order (discovery.zig:385-447, `discoverFallback`):

1. **Han-block special-case** (discovery.zig:399-420): if the codepoint is in
   CJK Unified Ideographs (`U+4E00..=U+9FFF`), go **straight** to
   `discoverCodepoint` (the `CTFontCreateForString` path). The comment explains
   why: CoreText's collection matching does not properly account for **system
   locale** when picking a Han font (a zh vs ja vs ko user wants different
   glyphs for the same codepoint), whereas `CTFontCreateForString` on the
   primary font asks CoreText itself — which respects locale — for the substitute
   font. (refs: unicode U4E00 chart; Chromium's LocaleInFonts notes.)
2. **General path** (discovery.zig:422-446): run the normal descriptor
   `discover()` (character-set-filtered collection + `Score` sort). If that
   returns **zero** results and a codepoint was requested, fall back to
   `discoverCodepoint` (`CTFontCreateForString`) — this is the
   [ghostty#2499](https://github.com/ghostty-org/ghostty/issues/2499) fix: some
   codepoints (notably certain emoji) are found by CoreText's own substitution
   but not by a character-set collection query.

### 3.1 `discoverCodepoint` (discovery.zig:451-550)

This is the `CTFontCreateForString` mechanism:

1. Pick the **original** font whose cascade CoreText should consult, honoring the
   requested style: bold+italic → bold-italic face if the collection has one,
   else bold, else italic, else regular (discovery.zig:469-496). This matters
   because CoreText's substitution starts from a *base* font and the cascade can
   differ by style.
2. UTF-8-encode the codepoint into a `CFString`; compute the UTF-16 range length
   (2 for a surrogate pair, else 1) via
   `CFStringGetSurrogatePairForLongCharacter` (discovery.zig:499-522).
3. `original.font.createForString(str, CFRange(0, range_len))`
   (`CTFontCreateForString`, discovery.zig:525-528) → the font CoreText would use
   to render that substring, or null.
4. **LastResort rejection** (discovery.zig:534-546): copy the returned font's
   PostScript name; if it equals `"LastResort"`, return null. The LastResort
   font is CoreText's final fallback and contains only replacement glyphs
   (the ☒ boxes) — rendering it is worse than admitting we found nothing.
5. Return `font.copyDescriptor()`.

### 3.2 Apple Color Emoji special-casing

There is **no explicit "Apple Color Emoji" name check** in `discovery.zig` — and
that is the key insight. Emoji resolution works *entirely* through the generic
codepoint path:

- For a bare emoji codepoint (e.g. `U+1F600` 😀), the resolver's default
  presentation is `emoji` (UCD `is_emoji_presentation`, §5). The primary
  (JetBrains Mono) misses it. Discovery's general `discover()` may return
  nothing (emoji fonts are often not surfaced by a plain character-set
  collection query — the #2499 case), so `discoverFallback` calls
  `discoverCodepoint`, and `CTFontCreateForString` on the primary returns
  **Apple Color Emoji** — because that is the font macOS itself substitutes for
  emoji. The name is never hard-coded; CoreText's own cascade picks it.
- The resulting `DeferredFace` reports `hasColor()` true (symbolic-trait
  `TraitColorGlyphs`), so `hasCodepoint(cp, .emoji)` matches, the face is added
  to the collection as a fallback, and rasterizing its glyph goes through the
  **color (BGRA)** path (`face/coretext.zig` §3-4, already implemented in
  F5-reduced's `rasterize`).

So the port's Apple-Color-Emoji handling is: (1) default an emoji codepoint's
presentation to `emoji`; (2) let `CTFontCreateForString` from the primary pick
the system emoji font; (3) rasterize its glyph as BGRA. No name special-casing.

The one place upstream *does* name-check emoji is a different subsystem —
`SharedGridSet.zig`'s `collection` construction prepends a discovered
`"Apple Color Emoji"` face to the collection *at startup* on macOS (so emoji are
found without a per-codepoint discovery round-trip). That is a **startup
optimization**, not a correctness requirement: with the per-codepoint fallback
above, emoji still resolve without it. The port implements the per-codepoint
path (correctness) and notes the startup pre-load as an optional optimization
the app-wiring chunk can add (it belongs with config → collection construction,
outside this crate's territory).

## 4. `DeferredFace` (DeferredFace.zig, CoreText arm)

A `DeferredFace` is "everything needed to load a face, but not loaded yet." The
CoreText variant (DeferredFace.zig:94-108) holds just a `*macos.text.Font` (the
`CTFont` materialized during discovery at size 12) plus variation targets.

The two operations that matter for the resolver:

- **`hasCodepoint(cp, p)`** (DeferredFace.zig:357-385, CoreText arm): the cheap
  probe. If a presentation `p` is requested, gate on the font's symbolic-traits
  color bit (`color_glyphs ⇒ emoji` else `text`) and reject on mismatch. Then
  UTF-16-encode `cp` and `CTFontGetGlyphsForCharacters` — true iff mapped. This
  needs **no** full `Face` init: the `CTFont` is already in hand, and glyph
  lookup is a CMap query. This is the whole point of "deferred" — a fallback
  list can probe dozens of faces for a codepoint without rasterizer setup.
- **`load(opts)`** (DeferredFace.zig:253-264, `loadCoreText`): materialize a
  real `Face` via `Face.initFontCopy(ct.font, opts)` (re-copy the `CTFont` at
  the caller's pixel size) + apply variations.

**Port representation.** A `DeferredFace` wrapping a retained `CTFont`, with:

- `has_codepoint(cp, presentation) -> bool` — the symbolic-trait presentation
  gate + `glyphs_for_characters` probe (reusing the exact UTF-16 encoding
  `coretext.rs::glyph_index` already does).
- `load(size_px) -> Result<Face>` — `CTFontCreateWithFontDescriptor` /
  `copyWithAttributes` at `size_px`, producing the same `coretext::Face` the
  rest of the crate uses. Name-loaded faces have no `source_bytes`, so their
  metrics come from CoreText accessors (already handled in
  `Face::face_metrics`'s no-source-bytes arm).
- `family_name()` / `name()` — for the fallback logging + fuzzy-name tests.

Variation re-application on load is **deferred** (the reduced descriptor does
not push variation targets; noted).

## 5. Presentation: text vs emoji (VS15/VS16) through the resolver

`Presentation` (main.zig:62-66) is `{ text = 0 (U+FE0E), emoji = 1 (U+FE0F) }`.
`Collection.PresentationMode` (Collection.zig:862-873) is the three-state the
resolver actually threads: `explicit(p)` | `default(p)` | `any`.

The flow (CodepointResolver.zig:147-227 + Collection.zig:803-834):

1. **Explicit presentation.** When shaping sees a variation selector in a
   grapheme (`U+FE0E` → text, `U+FE0F` → emoji), the caller passes
   `p = explicit`. This is a hard requirement: a face satisfies the codepoint
   only if its glyph's color-ness matches (`Entry.hasCodepoint`,
   Collection.zig:816-826: for a loaded face, `text ⇒ !isColorGlyph`,
   `emoji ⇒ isColorGlyph`).
2. **Default presentation.** For a bare codepoint (no VS), the resolver derives
   the default from the UCD: `is_emoji_presentation(cp) ? .emoji : .text`
   (CodepointResolver.zig:152-157). This is why `😀` (emoji-presentation by
   default) routes to a color font and `✌` (`U+270C`, text-presentation by
   default) routes to a text glyph unless VS16 forces emoji.
3. **The fallback/non-fallback asymmetry** (Collection.zig:808-814): for a
   `default(p)` mode, a **non-fallback** (primary/user) face matches with `any`
   presentation (the user asked for this font, honor whatever it has), but a
   **fallback** (discovery-added) face is held to the explicit `p` — so a
   discovered emoji font isn't chosen for a text-default codepoint and vice
   versa.
4. **`any` last resort** (CodepointResolver.zig:224-227): after discovery fails,
   a non-regular request retries regular with `any` presentation; a regular
   request with an already-`any` mode returns null (notdef).

**Port representation.** The port adds a `Presentation { Text, Emoji }` enum and
a `PresentationMode { Explicit(Presentation), Default(Presentation), Any }`.
`get_index` gains an optional presentation argument; when `None`, the default is
computed with `unicode-properties`' `char.emoji_status()` (the
`EmojiPresentation*` variants ⇔ upstream's `is_emoji_presentation`). The
fallback/non-fallback asymmetry is preserved by tagging discovered faces
`fallback = true`.

### 5.1 UCD emoji-presentation data

Upstream reads `uucode.get(.is_emoji_presentation, cp)`. The port uses the
`unicode-properties` crate (already in the local cargo cache, offline-safe):
`c.emoji_status()` returns an `EmojiStatus`, and `is_emoji_presentation` is
`matches!(status, EmojiPresentation | EmojiPresentationAndModifierBase |
EmojiPresentationAndEmojiComponent |
EmojiPresentationAndModifierAndEmojiComponent)`. This is the Unicode
`Emoji_Presentation` property, exactly what upstream queries.

## 6. Resolver steps 4-7 (the ported fallback chain)

F5-full implements the resolver as the full 7-step chain, reduced only where a
piece has no reduced-config surface:

| step | upstream                                    | port                                           |
| ---- | ------------------------------------------- | ---------------------------------------------- |
| 1    | disabled-style then regular                 | kept (styles map; regular can't be disabled)   |
| 2    | codepoint override (`CodepointMap`)         | **deferred** (no config-map surface yet)       |
| 3    | sprite dispatch                             | kept (already in F5-reduced)                   |
| 4    | exact style+presentation over priority list | **new**: search the style's face list in order |
| 5    | regular retry (non-regular missed)          | **new**                                        |
| 6    | discovery fallback (regular only)           | **new**: `discover_fallback` adds deferred     |
| 7    | any-presentation last resort                | **new**                                        |

Step 6 is the load-bearing addition: on a primary miss, the resolver calls
discovery with `{ codepoint: cp, size, bold, italic }`, iterates the returned
deferred faces, and for the first whose `hasCodepoint(cp, p_mode)` holds, adds
it to the collection as a fallback face (`fallback = true`, size-adjustment
`ic_width` per `default_fallback_adjustment`) and returns its index. The
resolver **mutates** the collection (adds faces on demand) — so `get_index` takes
`&mut self`, matching upstream's `getIndex(alloc, …) *CodepointResolver`.
Size-adjustment (rescaling a fallback face to match the primary's `ic_width`) is
**deferred** to keep the port focused on *resolution*; the fallback face is
loaded at the primary's pixel size directly (documented deferral — visually
fallback glyphs may be slightly off-metric until the adjustment lands).

## 7. `Collection` completion + fallback list (Collection.zig)

Two `Collection` additions:

- **Fallback list per style.** F5-reduced stored one face per style. F5-full
  stores, per style, a **priority-ordered list**: `[primary/user faces…,
  discovered fallback faces…]`, each tagged `fallback: bool`. `get_index`
  searches the list in order (step 4). `add`/`add_deferred` append and return
  the slot. The slotmap `FontIndex::Face { style, slot }` (decision 8) already
  models a slot index; F5-reduced pinned `slot = 0`, F5-full uses the real
  append position.
- **`completeStyles`** (Collection.zig:320-…): ensure every style has ≥1 entry
  by synthesizing italic/bold from the first regular face that has *text*
  glyphs (`!hasColor() || glyphIndex('A') != null` — skip an emoji-first
  regular). Synthetic italic applies the `ITALIC_SKEW` transform (already
  present in `coretext.rs`); synthetic bold sets a stroke width. The port ports
  the *structure* (fill empty style slots from regular) and wires the synthetic
  transforms already staged in `coretext.rs`; the exact synthetic-bold stroke
  metrics are a **completeness detail** flagged for the styles chunk.

## 8. The renderer seam (for the COLOR-atlas follow-up chunk)

F5-full's job ends at "the resolver returns a rasterized glyph (alpha8 **or**
BGRA) from a discovered fallback face." Wiring the BGRA output into a GPU color
atlas is the **follow-up** chunk. The seam, precisely:

- **What the resolver/grid now produces.** `render_codepoint('😀')` resolves to
  a discovered Apple-Color-Emoji fallback face and rasterizes a `Bitmap` with
  `format = PixelFormat::Bgra` (4 bpp, premultiplied little-endian BGRA, Display
  P3). Today the reduced `Grid` owns a **single grayscale** `Atlas` and uploads
  every glyph there — it will mis-store a BGRA glyph (4 bytes written into a
  1-byte-per-texel atlas). F5-full's grid path must therefore branch on
  `bmp.format` and route color glyphs to a **separate BGRA atlas**.
- **What the color-atlas chunk needs to add:**
  1. A second `Atlas` with a 4-byte (`Format::Bgra`/RGBA) texel, grown/uploaded
     exactly like the grayscale one.
  2. A per-glyph **atlas-selector bit** carried from `render_glyph` out to the
     renderer, so the cell shader samples the right texture. This is the
     `atlas` field on the frozen `CellText` wire struct (the renderer chunk's
     R4 cell-instance layout): `atlas ∈ { grayscale, color }`. `getPresentation`
     (CodepointResolver.zig:303-314) is the upstream selector — `sprite ⇒ text`
     (grayscale), else `isColorGlyph(glyph) ? emoji(color) : text(grayscale)`.
     The port exposes the same signal: `CachedGlyph` gains an `atlas:
     AtlasKind` (or the grid returns `(CachedGlyph, Presentation)`), which the
     renderer maps to `CellText.atlas`.
  3. Emoji **constraint / cell-fit** sizing (`SharedGrid`'s emoji constraint):
     color glyphs are scaled to fit the cell box; deferred with the sizing work,
     noted so the color-atlas chunk owns it.

F5-full **does not** modify the renderer or the `CellText` struct (sibling
territory). It (a) makes `rasterize` return BGRA for color glyphs (already
does), (b) ensures the resolver reaches a color face for emoji, and (c)
documents this seam so the color-atlas chunk has an exact contract:
*"grid returns a glyph tagged text|emoji; renderer routes emoji → the BGRA atlas
and sets `CellText.atlas = color`."*

## 9. Test inventory (Zig → reduced Rust)

Upstream inline tests across the four files (13 total):

- **`discovery.zig`** (8): `descriptor hash`, `descriptor hash family names`
  (both platform-neutral — ported directly against the port's `Descriptor`
  hash/eq); `fontconfig`, `fontconfig codepoint`, `windows` (non-macOS — skipped
  with a note); `coretext`, `coretext codepoint`, `coretext sorting`. The macOS
  three are ported: `coretext` (discover a family, count > 0), `coretext
  codepoint` (discover a codepoint-bearing font, `hasCodepoint('A')` +
  `'B'`), and `coretext sorting` — the last is **disabled upstream** (SF Pro not
  in CI); the port reproduces its *intent* as a determinism test + a fuzzy-name
  test (see below) rather than depending on SF Pro.
- **`CodepointResolver.zig`** (3): `getIndex` (ASCII → primary, emoji → emoji
  face, text-emoji via presentation, box → null without sprite), `getIndex
  disabled font style`, `getIndex box glyph`. Ported/adapted: the ASCII-→-primary
  and box-→-sprite cases already exist (F5-reduced); F5-full adds the
  emoji-resolves-to-a-color-face case (via real discovery on macOS) and the
  disabled-style → regular case. The upstream test uses **embedded** Noto emoji
  fonts (`font.embedded.emoji`) which are not bundled in the port; the port
  instead resolves a real emoji through system discovery (documented
  difference).
- **`DeferredFace.zig`** (2): `fontconfig` (skipped), `coretext` — ported:
  discover Monaco, `hasCodepoint(' ')`, `name().len > 0`, `load` then
  `glyph_index(' ')`.

Plus the **integration + acceptance tests** the brief specifies:
resolve+rasterize `😀` (BGRA, non-empty), `水` (system CJK font), `Ω`, a
Nerd-Font codepoint if present (else noted); a **Score determinism** test (same
query → identical resolved face across repeated runs); a **fuzzy-name** test
("jetbrains mono" finds JetBrains Mono if installed, else skip-with-note).

## 10. Deferrals (carried forward)

- **Codepoint overrides** (resolver step 2, `CodepointMap`) — no config surface.
- **Fallback size-adjustment** (`ic_width` rescale of discovered faces) — the
  face loads at primary px size; harmonization deferred.
- **Variation-axis targeting in the search descriptor** — the `Score` reads
  axes to derive bold/italic, but we don't push variation targets into the
  query.
- **Synthetic-bold stroke metrics** in `completeStyles` — structure ported,
  exact stroke width is a styles-chunk detail.
- **Startup Apple-Color-Emoji pre-load** (`SharedGridSet`) — per-codepoint
  discovery makes it optional; belongs with app-side collection construction.
- **The COLOR (BGRA) atlas + `CellText.atlas` selector wiring** — the renderer
  follow-up chunk (§8).
- **Fontconfig / Windows discovery backends** — non-macOS; out of territory.
