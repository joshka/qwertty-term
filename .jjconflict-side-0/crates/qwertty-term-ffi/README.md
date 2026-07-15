# qwertty-term-ffi

The C ABI over the [qwertty-term](https://github.com/joshka/qwertty-term) engine
— the Rust side of the Rust → C-ABI → Swift/AppKit seam that lets the native
macOS app drive the engine.

It mirrors the *shapes* of upstream Ghostty's `include/ghostty.h` (opaque
handles, sized `*_s` structs, `*_e` enums, a runtime-callback struct with
wakeup and clipboard callbacks) so upstream's Swift sources can be **adapted
rather than rewritten**. See `docs/analysis/ffi-spike.md`.

## Scope

This crate does **not** reproduce the whole upstream C surface. The spike scope
is app + surface lifecycle: creating and tearing down the app/surface handles,
feeding input, and the runtime-callback plumbing — enough to bring the AppKit
shell up on the Rust engine. Broader surface coverage is added as the app needs
it.

## Consuming it

The public surface is `extern "C"` functions plus `#[repr(C)]` types, meant to
be called from C / Swift (not idiomatic Rust). A C header is generated from the
crate; the AppKit app links against the built static/dynamic library. Depends
only on `qwertty-term-vt` and `qwertty-term-input` (both cross-platform).

## License

MIT OR Apache-2.0
