# qwertty-term-input

Input encoding for [qwertty-term](https://github.com/joshka/qwertty-term): turns
key / mouse / paste **events** into the exact PTY bytes a terminal program
expects. A Rust port of Ghostty's `src/input/` (commit `2da015cd6`).

## What it does

- **Modern kitty keyboard protocol** (`CSI … u`) with progressive-enhancement
  flags.
- **Legacy encoder**: PC-style function keys, xterm `modifyOtherKeys`, and the
  "fixterms" CSI-u extension (ctrl+letter, etc.).
- Mouse reporting and bracketed-paste framing.

## Freestanding by design

This crate does **not** depend on `qwertty-term-vt`. The caller reads whatever
terminal-mode state it needs (cursor-key mode, kitty flags, mouse-tracking mode,
bracketed paste, `macos-option-as-alt`, …) and passes it in as plain parameters
via each module's `Options` struct. That keeps the encoder testable in isolation
and reusable by any front-end.

```rust
use qwertty_term_input::key::{Key, KeyEvent};
// Build a KeyEvent from your windowing layer, then encode it to PTY bytes
// with `encode(&event, &options)`; the Options carry the terminal mode state.
```

## License

MIT OR Apache-2.0
