# qwertty-term

A fast, native **macOS terminal emulator** — and the application crate of the
[qwertty-term](https://github.com/joshka/qwertty-term) project, a full
subsystem-by-subsystem Rust rewrite of [Ghostty](https://ghostty.org) (Zig source pinned at
commit `2da015cd6`) with differential testing against the original as the correctness
oracle.

This crate is the AppKit app itself: it wires the reusable engine crates
(`qwertty-term-vt`, `-font`, `-renderer`, `-termio`, `-input`) into a real terminal window
with a Metal renderer, native tabs and splits, and the full VT/keyboard/mouse stack.

## What it is / does / achieves

**Daily-drivable on macOS**, at parity with upstream Ghostty for the everyday surface:

- **Windowing**: native NSWindow tabs, splits (create / navigate / resize / zoom / equalize,
  unfocused-pane dimming), quick-terminal dropdown, IME, retina Metal rendering.
- **Text**: CoreText fonts with ligatures, Apple Color Emoji, nerd-font glyph sizing,
  procedural box/braille/powerline sprites — pixel-matched to upstream's goldens.
- **Terminal**: the certified VT engine — kitty graphics *and* image rendering, kitty +
  legacy keyboard protocols, mouse reporting, OSC 8 hyperlinks, OSC color queries,
  bracketed paste, scrollback with wheel + `Cmd+F` search.
- **Integration**: shell integration (bash/zsh/fish, OSC 133 prompt marks, cwd inheritance),
  bell + desktop notifications, `notify-on-command-finish`, clipboard hardening.
- **Config**: a small TOML config (a deliberate deviation, ADR'd) with live reload
  (`Cmd+Shift+,`), a full ported keybind system (leader keys, chains, `text:` actions), and
  a growing option surface plus a `+import-ghostty-config` converter for onboarding.

**How it achieves it**: the engine is a byte-faithful port verified by a 176-case
differential corpus against `libghostty-vt`, resize-interleaved fuzzing, and Miri; the
renderer runs upstream's shaders verbatim with equality-proven per-row dirty tracking. It
is *correct first* — performance is competitive but not yet dedicated-tuned (see the repo's
`docs/benchmarks/`).

## Run

Requires **macOS 13+** (Metal). Linux support is in progress (headless/software backend
groundwork only; no Linux app shell yet).

```sh
cargo install qwertty-term      # or: cargo run -p qwertty-term --release  (from a checkout)
qwertty-term
```

Config lives at `~/.config/qwertty-term/config.toml`. Import an existing Ghostty config with
`qwertty-term +import-ghostty-config`.

## The crate family

`qwertty-term` is the app; the pieces are reusable, published crates with no global state —
so the engine, fonts, and renderer can be embedded headlessly (VT bytes in, RGBA/PNG frames
out; see `examples/frame-capture` and `docs/embedding.md`):

| Crate | Role |
| ----- | ---- |
| [`qwertty-term-vt`](https://crates.io/crates/qwertty-term-vt) | terminal emulation core (parser, screen, scrollback) |
| [`qwertty-term-font`](https://crates.io/crates/qwertty-term-font) | font discovery, shaping, glyph rasterization |
| [`qwertty-term-renderer`](https://crates.io/crates/qwertty-term-renderer) | Metal (and software) cell renderer |
| [`qwertty-term-termio`](https://crates.io/crates/qwertty-term-termio) | PTY + read/write pipeline |
| [`qwertty-term-input`](https://crates.io/crates/qwertty-term-input) | key/mouse encoding + keybind model |
| [`qwertty-term-sprite`](https://crates.io/crates/qwertty-term-sprite) | procedural box/braille/powerline glyphs |

## License & attribution

MIT. A port of Ghostty (© Mitchell Hashimoto and contributors, MIT); ported code preserves
upstream semantics and attribution. Not affiliated with the Ghostty project. See
[`LICENSE`](https://github.com/joshka/qwertty-term/blob/main/LICENSE).
