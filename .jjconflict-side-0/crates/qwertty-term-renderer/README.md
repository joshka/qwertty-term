# qwertty-term-renderer

The GPU renderer core of [qwertty-term](https://github.com/joshka/qwertty-term):
screen/cell geometry, cursor-style resolution, the `RenderSnapshot` contract
between the terminal engine and any backend, and a Metal backend with offscreen
pixel readback. Ported from Ghostty's `src/renderer/` (commit `2da015cd6`) with
the shaders carried over verbatim.

## What it does

- **Geometry / snapshot contract**: `size` (surface/cell/grid coordinate math),
  `cursor` (focus/blink/preedit resolution), and `snapshot::RenderSnapshot` —
  the decoupling seam a `qwertty-term-vt` snapshot flows through.
- **Metal engine** (`Engine`, macOS): turns a snapshot + a `qwertty-term-font`
  `Grid` into GPU buffers and draws them — upstream's shaders verbatim,
  grayscale + color atlases, per-row dirty tracking (equality-proven vs full
  redraw), run-based shaping cache, IOSurface-backed presentation.

## Offscreen rendering — bytes to pixels

```rust,no_run
# use qwertty_term_font::{Grid};
# use qwertty_term_renderer::engine::{Engine, FrameOptions};
# use qwertty_term_renderer::snapshot::FullSnapshot;
# use qwertty_term_vt::terminal::Terminal;
# fn demo(mut grid: Grid, terminal: &Terminal) -> Result<(), Box<dyn std::error::Error>> {
let mut engine = Engine::for_grid(&grid)?;              // cell geometry from the grid
let snapshot = FullSnapshot::capture_live(terminal);   // the live screen
let frame = engine.render(&snapshot, &mut grid, FrameOptions::default())?;
let _rgba = frame.to_rgba();                            // width*height*4, RGBA8
# Ok(())
# }
```

The `engine` module docs carry the full runnable quickstart, and
`examples/frame-capture` wires this to a PNG encoder. See
[`docs/embedding.md`](../../docs/embedding.md).

## Platform

The `Engine` / Metal path is `cfg(target_os = "macos")`. On docs.rs the crate is
built for a darwin target so `Engine`, `render`, and `Frame` are documented. The
geometry / snapshot-contract layer is cross-platform.

## License

MIT OR Apache-2.0
