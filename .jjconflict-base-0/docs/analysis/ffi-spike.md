# M5 FFI spike: qwertty-term C ABI + Swift round-trip

Status: **spike complete, go/no-go ACCEPTED (Josh, 2026-07-08) (pending maintainer review)**.
Scope: de-risk the Rust -> C-ABI -> Swift/AppKit seam with a thin end-to-end
round-trip before committing to the full M5. Plan: `docs/plans/m5-ffi-spike.md`.

## What was built

- `crates/qwertty-term-ffi` -- a `staticlib` + `cdylib` + `rlib` crate exposing a C
  ABI that mirrors the shapes of upstream `include/ghostty.h` (opaque handles,
  sized `*_s` structs, `*_e` enums, a runtime-callback struct). Spike surface:
  - `qwertty_term_init`
  - `qwertty_term_app_new` / `_tick` / `_free`
  - `qwertty_term_surface_new` / `_free` (wraps `Stream<TerminalHandler>` + a
    cols/rows/scrollback config struct)
  - `qwertty_term_surface_write_pty_bytes` (raw shell output in)
  - `qwertty_term_surface_key` (mirrored key-event struct -> `qwertty-term-input`
    encode -> engine, side effects drained internally)
  - `qwertty_term_surface_read_text` (screen text dump out; pre-M3 stand-in for
    `surface_draw`; caller-buffer + length convention)
  - `qwertty_term_surface_take_pty_reply` (engine reply bytes out; DSR/DA/CPR)
  - clipboard callback registration (on the runtime config) + firing on OSC 52
  - wakeup callback stub (declared in the runtime config; not fired in-spike)
  - `catch_unwind` at every extern boundary -> error code / null handle
- `crates/qwertty-term-ffi/include/qwertty_term.h` -- cbindgen-generated, checked in,
  with a drift-check test (`tests/header_drift.rs`).
- `macos-spike/` -- a `swiftc`-compiled single-file Swift driver (`main.swift`)
  with a clang `module.modulemap` and `build.sh`. **Not** an Xcode project.
- Rust-side ABI tests (`src/tests.rs`, 9 tests) covering lifecycle, null-safety,
  buffer-size query, key round-trip, text dump, clipboard callback, reply drain.

## Driver output (verbatim)

Swift **was** available (`swiftc` 6.2.4, arm64-apple-macosx15.0), so this is a
real Swift verification, not a C fallback:

```text
==> Building qwertty-term-ffi staticlib (debug)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s
==> Compiling Swift driver
==> Running Swift driver
-----------------------------------------------------------------------
PASS: qwertty_term_init
PASS: qwertty_term_app_new returns non-null
PASS: qwertty_term_surface_new returns non-null
PASS: surface_write_pty_bytes
PASS: surface_key
PASS: surface_read_text succeeds
      screen first line = "hi"
      full screen = "hi\nx\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n"
PASS: screen contains PTY text 'hi'
PASS: screen contains typed key 'x'
PASS: OSC 52 write accepted
PASS: clipboard callback fired
PASS: clipboard kind is STANDARD
PASS: clipboard data is raw base64 'aGk='
      clipboard data = "aGk="
PASS: teardown (surface_free + app_free)

ALL PASS
```

## Threading contract (documented at the boundary)

Mirrors upstream's apprt model. In short: **nothing in this ABI is
thread-safe.**

- An **app and all its surfaces form a single-thread apartment.** Every
  `qwertty_term_app_*` and `qwertty_term_surface_*` call must happen on the one
  thread that owns them (upstream: the AppKit main thread). There is no internal
  locking; handles are `*mut` to plain Rust structs with no `Sync`/`Send`
  guarantee. Calling two functions on the same handle concurrently is a data
  race and UB.
- **`wakeup_cb` is the single exception: it may be invoked from any thread.**
  It is the "please schedule a tick on your event loop" signal, which a real PTY
  reader thread (off the main thread) needs to raise. The embedder's `wakeup_cb`
  implementation must therefore be thread-safe and must *not* call back into any
  `qwertty_term_*` function directly -- it should only hop to the owning thread
  (e.g. `DispatchQueue.main.async`) and drive the ABI from there. The spike
  never fires `wakeup_cb` off-thread (the engine is synchronous), but the
  contract is fixed now so the future PTY thread has a legal path.
- **`write_clipboard_cb` (and any future action callback) fires synchronously,
  on the calling thread, inside a `write_pty_bytes` / `key` call.** It is
  therefore main-thread-only, like the calls that trigger it. The callback must
  not re-enter the ABI on the same handle (no reentrancy guard exists).
- Callback string arguments (clipboard `data`) are **borrowed for the duration
  of the call only**; the embedder must copy anything it needs to retain.

This matches upstream `apprt/embedded.zig`, where surface calls are
main-thread-only and only the wakeup path is cross-thread.

## Friction points (every one hit)

1. **cbindgen < 0.28 silently drops every function on Rust 2024 edition.**
   Edition 2024 requires `#[unsafe(no_mangle)]` (the bare `#[no_mangle]` is a
   hard error). cbindgen 0.27 only recognizes bare `#[no_mangle]`/`export_name`
   and emitted an **empty** header (10 `WARN: Skipping ... - (not no_mangle...)`
   lines, exit 0 -- easy to miss). **Fix: require cbindgen >= 0.28** (we pin
   0.29). This is the single most important finding for the real M5: the header
   toolchain version must be pinned, and the drift check (below) is what makes a
   silently-empty header loud.
2. **Double-prefixed enum variants.** Setting both `prefix_with_name = true`
   *and* `rename_variants = "QualifiedScreamingSnakeCase"` produced
   `QWERTTY_TERM_RESULT_QWERTTY_TERM_RESULT_SUCCESS`. `QualifiedScreamingSnakeCase`
   already prefixes with the enum name; drop `prefix_with_name`. Net config in
   `cbindgen.toml` gives clean `QWERTTY_TERM_RESULT_SUCCESS`, matching upstream's
   `GHOSTTY_*` style.
3. **cbindgen as a dev-dependency, not a build-dependency.** The drift-check
   test links the cbindgen *library* API to regenerate in-memory and diff. Test
   code cannot see `[build-dependencies]`, so cbindgen must be a
   `[dev-dependencies]`. (We deliberately do *not* regenerate at build time --
   that would make `cargo build` depend on a heavy proc-macro tree and a
   writable source tree.)
4. **The legacy key encoder does not emit printable text.**
   `qwertty-term-input`'s `key_encode::legacy_stub` only maps special keys
   (Enter/Tab/arrows) and ctrl-combos; it ignores `event.utf8`. Routing a typed
   "x" through it yields empty output. This is correct per upstream's design:
   printable text arrives via a *separate* path (`ghostty_surface_text` vs
   `ghostty_surface_key`; the spike window uses egui `Event::Text` vs
   `Event::Key`). The FFI therefore splits: a plain-text key writes its `text`
   bytes straight to the PTY; special/control keys go through the encoder. The
   real M5 should expose a distinct `qwertty_term_surface_text` entry point to
   mirror upstream exactly (and to keep IME/preedit coherent), rather than
   overloading `surface_key`.
5. **Swift ergonomics were smooth, with two notes.** (a) A `@convention(c)`
   callback cannot capture Swift state, so results trampoline through a global
   (or, in the real app, through `userdata` back to a Swift object -- exactly
   what upstream does). (b) `withUnsafeBufferPointer` / `withCString` /
   `[CChar]` cover the buffer + string conventions cleanly; the two-call
   size-then-fill `read_text` convention maps to Swift naturally. No bridging
   header, no Objective-C shim needed -- a bare clang `module.modulemap` was
   enough for `import CQwerttyTerm`.
6. **Linking a Rust staticlib from swiftc needed only `-framework
   CoreFoundation`.** No explicit `-lc++`/`-lresolv`/`-lSystem` -- the Swift
   driver pulls the base system libs in. A `cdylib` (`.dylib`) is the eventual
   app-bundle target; the staticlib is what the spike links. For the real app
   we will want the standard Rust-macOS link set documented, but the spike shows
   the minimal set is small.
7. **`String::new()` default for `CString::new(...).unwrap_or_default()`** in
   the clipboard path: OSC 52 bodies are base64 (or empty), so they never
   contain interior NULs; the `unwrap_or_default` is defensive only. Worth a
   note because a *decoded* clipboard payload (if a future entry point decodes)
   could contain NULs and would need a length-carrying convention instead of a
   C string.

## Header drift-check design

- **Source of truth:** the Rust `#[repr(C)]` types + `extern "C"` fns in
  `crates/qwertty-term-ffi/src/lib.rs`. cbindgen (config: `cbindgen.toml`) generates
  the header from them.
- **Checked in:** `crates/qwertty-term-ffi/include/qwertty_term.h`. This is what the
  Swift `module.modulemap` points at, so it is the interface contract the Swift
  side compiles against.
- **The check:** `tests/header_drift.rs` regenerates the header *in memory* via
  the cbindgen **library** API (same `cbindgen.toml`, so the only variable is
  the Rust source), normalizes line-endings/trailing-whitespace, and asserts it
  equals the checked-in file. On mismatch it writes
  `include/qwertty_term.generated.h` next to the committed one (for a quick
  `diff`) and fails with the regenerate command. Runs under `cargo test
  --workspace`, so CI catches drift with no external binary on PATH.
- **Why library API, not shelling out to the `cbindgen` CLI:** no PATH
  dependency in CI, and the version is pinned in `Cargo.lock` via the
  dev-dependency, so the check can't drift because someone's local CLI is a
  different cbindgen version.
- **Regenerate command** (also in the test's failure message and the header
  banner):
  `cbindgen --config crates/qwertty-term-ffi/cbindgen.toml --output crates/qwertty-term-ffi/include/qwertty_term.h`

## Go/no-go recommendation (ACCEPTED — ratified by Josh, 2026-07-08)

**GO** on adapting upstream's Swift/AppKit sources rather than writing a shell
from scratch, at **high confidence** for the app/surface/input/clipboard core.

Evidence: a real `swiftc` build linked the Rust staticlib and drove the full
round-trip (create app + surface, PTY write, key, screen read, OSC 52 clipboard
callback) with **zero** surprises in the ABI/link/import mechanics. The upstream
header shapes port to cbindgen output cleanly; Swift consumes the cbindgen
header via a bare module map (no bridging header, no ObjC shim); callbacks,
opaque handles, sized structs, and the two-call buffer convention all behave.
The one real toolchain trap (cbindgen version vs edition 2024) is caught by the
drift test.

Confidence qualifiers / where the risk still lives (none block GO, all are
"expected M5 work, not spike surprises"):

- **Renderer (`surface_draw`)** is stubbed as a text dump; the Metal/pixel path
  is post-M3 and not de-risked here. This is the largest remaining unknown but
  is orthogonal to the FFI seam itself.
- **Action callback** (upstream's `ghostty_runtime_action_cb`, a big tagged
  union) is not modeled; the spike only did wakeup + clipboard. The tagged-union
  ABI is more cbindgen surface area to validate but nothing suggests it won't
  work (vt-diff already binds tagged data cleanly).
- **`ghostty_surface_text` / IME / preedit** should be a distinct entry point
  (see friction #4), not folded into `surface_key`.

## First 5 upstream Swift files to adapt (and what each needs from the ABI)

Ordered by the round-trip they unblock. Paths are under
`~/local/ghostty/macos/Sources/`.

1. **`Ghostty/Ghostty.App.swift`** -- the app wrapper. Needs:
   `qwertty_term_init`, `qwertty_term_app_new/_tick/_free`, and the
   `QwerttyTermRuntimeConfig` struct with `userdata` + `wakeup_cb`. Adaptation:
   replace `qwertty_term_app_*` calls with `qwertty_term_app_*`; drop the config-object
   plumbing (`ghostty_config_t`) the spike doesn't have yet and pass the
   grid/scrollback via the surface config instead. This is where the wakeup ->
   `DispatchQueue.main.async` -> `app_tick` loop lives.
2. **`Ghostty/Surface View/SurfaceView_AppKit.swift`** (the AppKit NSView;
   shared base in `Surface View/SurfaceView.swift`) --
   the surface lifecycle + input entry. Needs: `qwertty_term_surface_new/_free`,
   `qwertty_term_surface_key`, `QwerttyTermInputKey`/`QwerttyTermInputMods`/
   `QwerttyTermInputAction`. Adaptation: map NSEvent key/mods to the mirrored key
   struct (upstream already has this mapping -- keep it, retarget the struct);
   split printable text to a future `surface_text` (friction #4).
3. **`Ghostty/Ghostty.Input.swift`** (+ `Ghostty/NSEvent+Extension.swift`) --
   the NSEvent -> Ghostty key/mods translation tables. Needs: the
   `QwerttyTermInput*` enum/struct *values* to line up with the Rust
   `qwertty-term-input` `Key`/`Mods`/`Action`. Adaptation: this file
   is mostly pure mapping and adapts almost verbatim once the enum names match;
   it is the reason we mirrored `qwertty_term_input_key_s`'s shape.
4. **`Ghostty/Ghostty.Action.swift`** (+ the clipboard bits of
   `SurfaceView`) -- the runtime action/clipboard callback handlers. Needs: the
   `QwerttyTermWriteClipboardCb` signature + `QwerttyTermClipboard` enum (and,
   later, the action tagged union). Adaptation: wire the `@convention(c)`
   trampoline through `userdata` to the Swift surface object; the spike proves
   the clipboard leg end-to-end.
5. **`Ghostty/Surface View/SurfaceView_AppKit.swift`'s Metal-layer draw path**
   (the same NSView from item 2, its `draw`/`CAMetalLayer` half) -- the draw
   host. Needs: a real
   `qwertty_term_surface_draw` (post-M3) + a size/content-scale API. Adaptation:
   the *largest* effort and the least de-risked here; for M5 it can start
   against the `read_text` stand-in to bring the view up, then swap to the pixel
   path when the renderer lands.
