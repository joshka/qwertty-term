# Changelog

All notable changes to the qwertty-term crate family. The crates share one
workspace version and release together. This project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html); on 0.1.x every
change is additive.

## Unreleased

### qwertty-term-vt

- Added `Stream<TerminalHandler>::terminal()`, `terminal_mut()`, and
  `into_terminal()` accessors, replacing the `stream.handler.terminal`
  reach-through (which still works).

### qwertty-term-renderer

- Added `Engine::render(snapshot, grid, opts) -> Frame` — the one-call render
  path (`update_frame` + `sync_atlas` + `draw_frame` in the required order).
  The three underlying steps remain public.
- Added `engine::Frame`: a typed readback buffer pairing pixel dimensions with
  the pixels; `bgra()`/`into_bgra()` for the raw readback and `to_rgba()` for
  the swizzled copy, so the pixel format is stated rather than assumed.
- Added `Engine::for_grid(&Grid)` and `Engine::with_backend_for_grid(Metal,
  &Grid)`, which read cell geometry from the font grid's own metrics —
  eliminating the cell-size desync footgun of passing `cell_width`/
  `cell_height` by hand.
- Added `FullSnapshot::capture_live(&Terminal)` — `capture(terminal, 0)`
  without the magic-zero scrollback argument.

## 0.1.0 — 2026-07-08

Initial release of all eight crates: `qwertty-term`, `qwertty-term-vt`,
`qwertty-term-font`, `qwertty-term-renderer`, `qwertty-term-termio`,
`qwertty-term-input`, `qwertty-term-sprite`, `qwertty-term-ffi`.
