# qwertty-term-vt

The terminal emulation core of [qwertty-term](https://github.com/joshka/qwertty-term):
parser, stream handler, terminal state machine, screen, and the page-based
scrollback memory model. A subsystem-by-subsystem Rust port of Ghostty's
`src/terminal/` (commit `2da015cd6`), with differential testing against the
original `libghostty-vt` as the correctness oracle.

It is deliberately **dependency-light, synchronous, and runtime-free** so it can
be embedded, fuzzed, and published independently — no global state, no async, no
window. Feed it bytes, read a styled grid back.

## What it does

- **Parser**: CSI / OSC / DCS / APC / ESC state machine, UTF-8 decode, param
  overflow policy.
- **Screen / grid**: pages, scrollback, wide chars, graphemes, ref-counted
  styles; Unicode grapheme break + width (UAX #29 / #11, exact).
- **State**: cursor movement, scroll regions, erase/insert/delete, SGR, modes,
  mouse tracking, alt-screen, charsets, kitty graphics + keyboard protocols.
- **Embedding seam**: `Terminal::snapshot` / `snapshot_window` (owned styled
  cells) and the `formatter` module (plain / VT / HTML dumps).

## Usage

```rust
use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};

let terminal = Terminal::new(Options { cols: 80, rows: 24, ..Default::default() });
let mut stream = Stream::new(TerminalHandler::new(terminal));
stream.feed(b"\x1b[1mhello\x1b[0m");
assert_eq!(stream.terminal().plain_string().trim_end(), "hello");
```

For the full "bytes in, pixels out" embedding story (this crate + the font and
renderer crates), see [`docs/embedding.md`](../../docs/embedding.md) and the
`examples/frame-capture` crate.

## Verification

~1,500 in-crate tests, a differential corpus against `libghostty-vt` (screen
text, cursor, and formatter output), resize-interleaved fuzzing, and Miri.

## License

MIT OR Apache-2.0
