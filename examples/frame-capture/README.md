# frame-capture

VT bytes in, PNG frames out — a small, headless example proving the ghostty-rs
embeddability story.

A terminal recorder like [betamax](https://github.com/joshka/betamax) (a Rust
VHS-style recorder that wants ghostty-identical pixels) needs exactly three
things from a terminal stack, none of them a window:

1. a terminal state machine it can feed raw bytes
   (`ghostty_vt::{Terminal, Stream}`),
2. a deterministic font substrate
   (`ghostty_font`: embedded JetBrains Mono, metrics, glyph atlas),
3. an offscreen renderer with pixel readback
   (`ghostty_renderer`: `Engine` + Metal offscreen target).

This example wires those three public APIs together in ~100 lines of actual
logic (`src/main.rs`, `render_frame` is the whole story): feed bytes, snapshot,
render, read back, encode PNG. No pty, no wall-clock, no window — the same
input produces byte-identical PNGs on every run (with the default embedded
font; `--font-family` renders with a system font, which is deterministic per
machine but not across machines).

## Usage

```text
frame-capture [OPTIONS] [INPUT]

ARGS:
    [INPUT]    File of raw VT bytes to feed the terminal. `-` or absent: stdin.

OPTIONS:
    --cols <N>                 Terminal width in cells            [default: 80]
    --rows <N>                 Terminal height in cells           [default: 24]
    --font-size <PX>           Font size in pixels                [default: 16]
    --font-family <NAME>       System font family to render with. Default is
                               the embedded JetBrains Mono (deterministic).
    --out-dir <DIR>            Directory for frame PNGs           [default: .]
    --out <FILE>               Exact output path; single-frame (eof) mode only.
    --frame-on <eof|marker>    When to emit a frame               [default: eof]
    --split-on-marker <SEQ>    Marker byte sequence for `marker` mode; implies
                               --frame-on marker. Supports escapes: \xNN, \e,
                               \n, \r, \t, \0, \\.
    -h, --help                 Print this help.
```

By default one frame is emitted after all input has been fed (`--frame-on
eof`). With `--split-on-marker`, a frame is emitted at each occurrence of the
marker byte sequence — the marker bytes are consumed, never fed to the
terminal — plus a final frame at EOF if bytes follow the last marker. That is
the recorder loop: a producer interleaves output with markers, and each marker
becomes one animation frame.

## Sample commands

Render the bundled starship-style prompt + `ls` session:

```sh
cargo run -p frame-capture -- --cols 60 --rows 8 \
    --out demo.png examples/frame-capture/samples/starship-ls.vt
```

Capture a live command's output (colors survive because it is just bytes):

```sh
ls --color=always | cargo run -p frame-capture -- --cols 80 --rows 24
```

Multi-frame capture, one frame per marker:

```sh
printf 'one\x1c\r\ntwo\x1c\r\nthree' \
    | cargo run -p frame-capture -- --cols 20 --rows 4 \
        --split-on-marker '\x1c' --out-dir frames/
```

## Determinism

Same input, same flags, embedded font → byte-identical PNGs. The integration
test (`tests/capture.rs`) enforces this: it renders a scripted session (SGR
colors, box drawing, an emoji) twice and byte-compares the outputs, alongside
dimension and ink assertions.

## Requirements

macOS with a Metal device. Without one (CI runners, containers) the program
prints `SKIP:` and exits 0, matching the workspace's GPU-test convention. On
other platforms it does the same at startup.
