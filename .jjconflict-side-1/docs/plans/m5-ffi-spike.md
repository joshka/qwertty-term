# Plan: M5 ghostty-ffi spike (start any time after M2)

Purpose: de-risk the least-explored seam — the C ABI + Swift shell — with a thin round-trip
BEFORE committing to the full M5. One Opus chunk, timeboxed.

## Decisions (locked)

1. **Mirror `include/ghostty.h` naming and shapes** (opaque `ghostty_app_t`/`surface_t`/
   `config_t`, sized structs with leading size fields, the runtime-callback struct with
   wakeup/action/clipboard callbacks) so upstream's Swift sources can be ADAPTED, not
   rewritten. Reference implementations: upstream `src/terminal/c/*.zig` (the vt C API our
   vt-diff already binds) and the app-level API consumed by `macos/Sources/Ghostty/*.swift`.
2. **cbindgen generates the header** from `crates/ghostty-ffi`; a CI check diffs it against
   a checked-in copy so drift is loud.
3. Spike scope (NOT the full API): `ghostty_init`, app new/free/tick, surface new/free,
   `surface_key` (one key press via ghostty-input), `surface_draw` into an offscreen target
   (or, pre-M3, a `surface_text` state dump), clipboard callback round-trip. A 100-line
   Swift or C driver program exercises it end-to-end.
4. **Threading contract documented at the boundary**: which calls are main-thread-only,
   which lock internally — mirror upstream's (mostly single-threaded, wakeup_cb from any
   thread).

Acceptance: driver program creates a surface, sends keys, reads screen text (and pixels
post-M3), receives a clipboard callback; `cargo test` covers the ABI layer via the C API
from Rust (like vt-diff does for the reference). Output: docs/analysis/ffi.md + a go/no-go
note on adapting upstream Swift (list the first 5 Swift files to adapt and what they need).
