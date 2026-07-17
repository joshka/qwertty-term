# Embedding qwertty-term

qwertty-term is not just an app — the engine, font stack, and renderer are plain
Rust crates on [crates.io](https://crates.io/crates/qwertty-term-vt) with no
global state, no async runtime, and no window requirement. You can feed a
terminal raw bytes and read pixels back, entirely headless. This is what a
terminal recorder like [betamax](https://github.com/joshka/betamax) (the named
reference consumer) embeds.

## The three crates an embedder needs

- `qwertty-term-vt` — terminal state machine: feed bytes, snapshot the styled grid.
- `qwertty-term-font` — font substrate: embedded/system faces, metrics, glyph atlas.
- `qwertty-term-renderer` — offscreen GPU renderer (Metal, macOS) with pixel readback.

`qwertty-term-vt` alone is enough if you only need the parsed grid / scrollback
(text, styles, cursor) — e.g. to render with your own rasterizer, as betamax
did before adopting the renderer. Add the font + renderer crates when you want
qwertty-identical pixels.

## The flow: bytes in, pixels out

```text
VT bytes ──▶ Stream<TerminalHandler> ──▶ FullSnapshot::capture_live
                                                     │
       Grid (qwertty-term-font) ─────────────────────┤
                                                     ▼
                                     Engine::render(snapshot, grid, opts) ──▶ Frame
                                                                               │
                                                          Frame::to_rgba() ──▶ PNG / image buffer
```

No pty, no wall-clock, no window. The same input produces byte-identical output
on every run (with the embedded font), which is the property a deterministic
recorder needs — the cursor blink phase and other time-varying inputs are
injected via `FrameOptions`, not read from a clock.

## Where to start

- **Runnable example:** `examples/frame-capture` — VT bytes in, PNG frames out
  in ~100 lines of actual logic, including the recorder loop (a frame per marker
  byte). `cargo run -p frame-capture -- --help`.
- **API quickstart:** the `qwertty_term_renderer::engine` module docs carry a
  copy-pasteable `render` walkthrough (feed → `capture_live` → `for_grid` →
  `render` → `Frame`).
- **VT-only embedding:** `qwertty_term_vt::stream::Stream::terminal()` plus
  `Terminal::snapshot_window` / the `formatter` module for plain/VT/HTML dumps.

## Platform note

The renderer (`Engine`, `Frame`) is macOS/Metal only today; `cfg(target_os =
"macos")`-gated. `qwertty-term-vt` and `qwertty-term-font`'s table/metrics layer
are cross-platform; the FreeType face path (ADR 003) is behind the `freetype`
feature. On docs.rs the renderer and font crates are built for a darwin target
so their full API is visible.

## Versioning

All nine crates share one workspace version and release together. 0.1.x is
additive-only; breaking changes are batched into a single 0.2.0 with migration
notes (see `CHANGELOG.md`).

Eight are the embeddable library/bin crates covered by this guide. The ninth,
`qwertty-term-gtk`, is the Linux GTK4 desktop host — published for completeness,
not intended as an embedding dependency.
