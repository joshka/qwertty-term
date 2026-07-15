# qwertty-term-sprite

Procedural glyph rasterizer for terminal "sprite" fonts.

This crate draws the glyphs a terminal renders *itself* rather than pulling from
a font file, and does so **seam-free at any cell size** — adjacent cells line up
pixel-for-pixel. It is a standalone Rust port of Ghostty's `src/font/sprite/`
subsystem (commit `2da015cd6`).

Covered glyph sets:

- Box Drawing (`U+2500..U+257F`)
- Block Elements (`U+2580..U+259F`)
- Geometric Shapes — corner-triangle subset (`U+25E2..U+25FF`)
- Braille Patterns (`U+2800..U+28FF`)
- Powerline + Powerline Extra — geometric subset (`U+E0B0..U+E0D4`)
- Branch / git-graph symbols (`U+F5D0..U+F60D`)
- Symbols for Legacy Computing (`U+1FB00..U+1FBEF`)
- Symbols for Legacy Computing Supplement — implemented subset
  (`U+1CC1B..U+1CEAF`, incl. octants `U+1CD00..U+1CDE5`)
- Cursors and text decorations (underline variants, strikethrough, overline)
  via the `Sprite` pseudo-codepoints

## Design

- **No emulator types in the API.** Input is a plain [`Metrics`] struct (cell
  size, line thicknesses, decoration positions — all in pixels); output is a
  [`Glyph`] holding an 8-bit alpha bitmap plus placement offsets. Nothing from
  any terminal core leaks across the boundary, so any renderer, screenshot tool,
  or font utility can depend on it.
- **2D backend: [`tiny-skia`](https://crates.io/crates/tiny-skia).** Pure-Rust,
  no C dependencies. Paths and strokes are rasterized by tiny-skia; axis-aligned
  rectangles, single pixels, and the compositing tricks are done directly on the
  alpha buffer (as upstream also bypasses its vector lib for those). See
  `docs/analysis/sprite.md` for the full rationale.
- **Deterministic.** The same codepoint and metrics always produce
  byte-identical output.

## Usage

```rust
use qwertty_term_sprite::{Metrics, Sprite, render, has_codepoint};

// Cell metrics in pixels. `simple` derives thicknesses/positions from the
// cell size; construct `Metrics` directly when you have real font metrics.
let metrics = Metrics::simple(9, 18);

// A Unicode sprite glyph: U+2500 BOX DRAWINGS LIGHT HORIZONTAL.
if has_codepoint(0x2500) {
    let glyph = render(0x2500, &metrics).unwrap();
    // `glyph.alpha` is row-major, width*height bytes (0 = transparent).
    assert_eq!(glyph.alpha.len(), (glyph.width * glyph.height) as usize);
    // `glyph.offset_x` / `glyph.offset_y` place the trimmed bitmap in the cell.
}

// A cursor pseudo-glyph, addressed by its `Sprite` codepoint.
let cursor = render(Sprite::CursorBar.codepoint(), &metrics).unwrap();
```

`render` returns `None` for any codepoint this crate does not draw (check
`has_codepoint` first if you want to fall through to a real font).

## Status

- All upstream codepoint ranges are ported. A dispatch-coverage test asserts the
  exact set of ranges against the upstream table so drift is caught.
- **Not yet wired into a renderer/atlas** — that is a later chunk's job. This
  crate compiles, tests, and documents standalone usage only.
- **Golden-PNG parity vs upstream fixtures is deferred.** Upstream verifies
  exact pixels against checked-in PNGs (`src/font/sprite/testdata/`); this port's
  tests verify structural properties (non-empty, in-bounds, seam-continuous,
  deterministic, full coverage) instead. Pixel-exact parity would be a valuable
  follow-up once a renderer exists to generate comparable output.

## License

MIT OR Apache-2.0
