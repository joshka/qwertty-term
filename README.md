# ghostty-rs

A full Rust rewrite of [Ghostty](https://ghostty.org) — terminal emulator engine, font
stack, Metal renderer, and native macOS app — ported subsystem-by-subsystem from the Zig
source (pinned at `2da015cd6`) with differential testing against the original as the
correctness oracle.

Status: **daily-drivable on macOS.** Native AppKit app with tabs, splits (zoom, dimming,
equalize), Cmd+F search, scrollback, IME, kitty + legacy keyboard protocols, mouse
reporting, shell integration, themes, ligatures, Apple Color Emoji, and nerd-font glyph
sizing at parity with upstream. First [vtebench](https://github.com/alacritty/vtebench)
baselines: faster than Ghostty 1.3.1 in 9 of 10 suites on the same machine
(`docs/benchmarks/`).

```sh
cargo run -p ghostty-app --release          # the terminal
cargo run -p frame-capture -- --help        # headless VT-bytes → PNG (embeddability demo)
cargo test --workspace                      # ~1500 engine tests + differential + smokes
```

## Design highlights

- `ghostty-vt` — the VT engine: page-based scrollback, ref-counted styles, kitty
  graphics/keyboard, verified by a 176-case differential corpus against `libghostty-vt`,
  fuzzing (incl. resize-interleaved), and Miri.
- `ghostty-font` — CoreText faces + discovery, rustybuzz shaping, procedural sprite glyphs
  (`ghostty-sprite`, pixel-identical to upstream goldens), emoji + nerd-font constraints.
- `ghostty-renderer` — Metal, IOSurface-backed presentation, upstream's shaders verbatim,
  run-based shaping with caching, per-row dirty tracking (equality-proven vs full redraw).
- `ghostty-termio` — rustix PTY + upstream's two-stage read pipeline (no async runtime;
  see `docs/adr/002`).
- Embeddable by construction: `examples/frame-capture` renders deterministic PNGs from
  bytes through public APIs only.

Docs: `docs/rewrite-prompt.md` (mission/constitution), `docs/roadmap.md` (work breakdown),
`docs/analysis/` (~30 commit-stamped subsystem analyses), `docs/adr/` (decisions).

## Relationship to upstream

Not affiliated with the Ghostty project. Ported code preserves upstream semantics and
attribution (MIT, see LICENSE); deliberate deviations (TOML config, no tokio, native-Rust
app shell) are recorded as ADRs. Upstream bugs found during porting are reported back
(`work/upstream/`).
