# vt-diff

Differential-testing harness for the Ghostty Rust rewrite. Feeds identical
byte streams to the Zig-built `libghostty-vt` reference terminal and (from
Phase 1) the pure-Rust `qwertty-term-vt` port, then diffs observable state via the
`Oracle` trait (screen text + cursor position today; styles and modes later).

## Layout

- `src/oracle.rs` — `Oracle` trait, `CursorPos`, `ScreenDump`, and the
  `normalize_screen_text` comparison convention.
- `src/ffi.rs` — hand-written declarations for the slice of the libghostty-vt
  C API the harness needs (`ghostty_terminal_*`, `ghostty_formatter_*`).
- `src/reference.rs` — `ReferenceTerminal`, the safe wrapper implementing
  `Oracle` over the C API.
- `tests/smoke.rs` — hello-world smoke test plus replays of the fixtures in
  `crates/spike/tests/fixtures/replay/`.

API notes for the C surface live in `docs/analysis/libghostty-vt-c-api.md`.

## The corpus

`corpus/` is a tree of differential test cases swept by `tests/corpus.rs`
(feature `reference`): one directory per case containing `input.esc` (escaped
byte stream, same convention as `crates/spike/tests/fixtures/replay` — `\e`,
`\n`, `\r`, `\t`, `\\`, `\xHH`; decoder in `src/esc.rs`) plus an optional
`size.txt` (`"COLS ROWS"`, default 80x24).

- Torture suites (hand-authored, vttest/esctest-inspired): `cursor_movement/`,
  `scroll_regions/`, `wrap_semantics/`, `tab_torture/`, `erase_matrix/`,
  `insert_delete/`, `alt_screen/`, `charset_linedrawing/`, `sgr_matrix/`,
  `reset_behaviors/`, `kitty_keyboard_torture/` (push/pop/query/set on the
  progressive-enhancement flag stack), `rep_torture/` (REP after wide chars,
  combining marks, SGR-only sequences, cursor moves, wraps, erases),
  `xtwinops_title/` (title push/pop, `CSI 22/23 t`), `decrqm_matrix/`
  (DECRQM over every implemented ANSI/DEC mode plus unrecognized-mode and
  ANSI-form-is-dead-code edge cases), `kitty_graphics_torture/` (APC
  transmit/transmit-and-display/query/display/delete, chunked `m=1`
  transfers, and error paths — direct medium, uncompressed RGB/RGBA only;
  file/temp/shm media and PNG/zlib are seams in both engines, not exercised
  here), `reply_diffing/` (DA1/DA2, DSR/CPR, kitty-keyboard query,
  OSC 4/10/11/52 query no-reply agreement).
- Real-app captures: `real_apps/` — vim edit/quit/`:terminal` sessions,
  `less` paging and search, colored `git log`, a `tmux` session (new window,
  split, kill), two `top` refresh cycles, an `nvim` edit session, and an
  `fzf` filter session — captured once by `scripts/capture_real_apps.py`
  (PTY at 80x24) and checked in; tests never respawn the apps. `htop` and a
  kitty-graphics-emitting app (kitty/kitten, chafa, viu, icat, timg) were not
  installed on the capturing machine and were skipped rather than
  approximated.

### Reply-byte diffing

`ReferenceTerminal` registers `GHOSTTY_TERMINAL_OPT_WRITE_PTY` (see
`src/reference.rs`) so bytes the reference engine writes back in response to
queries (DECRQM, DSR/DA, kitty-keyboard query, DECRQSS, kitty graphics APC
responses, ...) are captured in `ReferenceTerminal::output()` instead of
being silently dropped (the library's documented default with no callback
registered). `tests/corpus.rs` diffs this against
`RustTerminal::output()` for every case, not just the `reply_diffing/`
suite — a query sequence anywhere in the corpus is now checked byte-for-byte
on both sides.

OSC 52 (clipboard) and OSC 4/10/11 (color) queries currently produce no
reply from *either* engine — both treat the response path as an
embedder-specific seam rather than a terminal-core feature — so those cases
in `reply_diffing/` currently document agreement-by-omission, not a tested
reply format.

A `SKIP` file in a case directory marks a known divergence: the sweep skips
it and a dedicated `#[ignore]` test in `tests/corpus.rs` documents the exact
disagreement (run those with `-- --ignored`).

## The `reference` feature

Everything that touches libghostty-vt is gated behind the off-by-default
`reference` cargo feature so that `cargo check --workspace` stays green on
machines without the Zig artifact. Without the feature the crate compiles to
just the `Oracle` trait.

### Building libghostty-vt

Requires Zig 0.15.2 (`mise exec zig@0.15.2 -- ...` if your system zig
differs):

```sh
cd ~/local/ghostty
zig build -Demit-lib-vt=true
```

This installs `zig-out/lib/libghostty-vt.a` (plus dylibs) and the headers to
`zig-out/include/ghostty/`.

### Running the harness

`build.rs` looks for `libghostty-vt.a` in `$GHOSTTY_VT_LIB_DIR`, falling back
to `~/local/ghostty/zig-out/lib`:

```sh
# with the default location:
cargo test -p vt-diff --features reference

# or explicitly:
GHOSTTY_VT_LIB_DIR=/path/to/ghostty/zig-out/lib \
    cargo test -p vt-diff --features reference
```
