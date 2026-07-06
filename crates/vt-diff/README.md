# vt-diff

Differential-testing harness for the Ghostty Rust rewrite. Feeds identical
byte streams to the Zig-built `libghostty-vt` reference terminal and (from
Phase 1) the pure-Rust `ghostty-vt` port, then diffs observable state via the
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
