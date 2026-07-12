# qwertty-term-font

Font loading, cell-metrics derivation, text shaping, and texture-atlas
allocation for [qwertty-term](https://github.com/joshka/qwertty-term). A
standalone Rust port of Ghostty's `src/font/` OpenType layer, `Metrics.zig`, and
`Atlas.zig` (commit `2da015cd6`).

## What it does

- **Faces**: CoreText face loading + family/codepoint discovery, byte-backed
  named faces, embedded default fonts (JetBrains Mono + symbols, vendored).
- **Metrics**: cell-size / baseline / underline / strikethrough derivation
  (upstream's rounding, centering, and modifier-redistribution rules).
- **Shaping**: [`rustybuzz`](https://crates.io/crates/rustybuzz) (pure-Rust
  HarfBuzz) run-based shaping with ligatures; variable-font `wght` axis + a
  synthetic-bold fallback ladder.
- **Rasterization + atlas**: CoreGraphics glyph rasterization, an alpha /
  color glyph atlas, nerd-font per-icon constraint sizing, Apple Color Emoji.

The `Grid` type ties resolver + metrics + atlas together and is what the
renderer consumes each frame.

## Platform

CoreText is the only fully-wired backend today, so the face/discovery layer is
`cfg(target_os = "macos")`. The table-parsing and metrics layer is
cross-platform; an optional FreeType face path (ADR 003) is behind the
`freetype` feature. On docs.rs the crate is built for a darwin target (with
`freetype`) so the full API is visible.

## Usage

```rust
use qwertty_term_font::coretext::Face;
use qwertty_term_font::{CodepointResolver, Collection, Grid, Metrics};

let face = Face::load_embedded(16.0)?;
let metrics = Metrics::calc(face.face_metrics());
let resolver = CodepointResolver::new(Collection::new(face));
let grid = Grid::new(resolver, metrics)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

See [`docs/embedding.md`](../../docs/embedding.md) for the full embedding flow.

## License

MIT OR Apache-2.0
