# libghostty-vt C API notes (differential-harness surface)

Surveyed against ghostty commit `2da015cd6` (2026-07-06). Reference build:
`zig build -Demit-lib-vt=true` with Zig 0.15.2 (`minimum_zig_version` in
`build.zig.zon`; use `mise exec zig@0.15.2 -- zig build ...` since Homebrew
zig is 0.16.0). Artifacts install to `zig-out/lib/` (`libghostty-vt.a` ~12 MB,
`libghostty-vt.{0.1.0,0,}.dylib`) and headers to `zig-out/include/ghostty/`.
There is also a `test-lib-vt` build step for the library's own tests and a
`qwertty-term-vt.xcframework` output.

Headers live in-source at `include/ghostty/vt/*.h` (umbrella:
`include/ghostty/vt.h`); the Zig implementation of the C bindings is
`src/terminal/c/*.zig` (one file per header, e.g. `terminal.zig`,
`formatter.zig`). The top-level `include/ghostty.h` is the **full app
(embedded apprt) API**, unrelated to libghostty-vt — do not mix them.
The upstream docs mark the whole vt API "incomplete, work-in-progress,
definitely going to change"; expect churn when bumping the ghostty checkout.

**Gotcha:** `zig-out/include/ghostty/vt/result.h` is a stale artifact of an
older build (no longer in the source tree) and declares an outdated
`GhosttyResult` that clashes with `types.h`. Ignore it; `types.h` is
authoritative.

## Conventions

- **Result codes** (`GhosttyResult`, C `int`): `GHOSTTY_SUCCESS = 0`,
  `OUT_OF_MEMORY = -1`, `INVALID_VALUE = -2`, `OUT_OF_SPACE = -3`,
  `NO_VALUE = -4`.
- **Enums are C `int`** on the Zig side (pre-C23 headers force this with an
  `INT_MAX` sentinel). Bind them as `c_int` constants, not Rust enums.
- **Opaque handles** are typedef'd pointers (`GhosttyTerminal`,
  `GhosttyFormatter`, ...). NULL is accepted by `_free` (no-op) and most
  entry points (no-op or `GHOSTTY_INVALID_VALUE`).
- **Sized structs**: option structs carry a leading `size_t size` field that
  must equal `sizeof(struct)` (ABI versioning; C macro
  `GHOSTTY_INIT_SIZED`). Zero-init everything else you don't set.
  `GhosttyTerminalOptions` is the exception — no size field (upstream TODO
  notes the ABI risk).
- **By-value struct passing**: `ghostty_terminal_new` and
  `ghostty_formatter_terminal_new` take their options structs **by value**.
  Layouts on arm64 macOS (verified by a layout test in `vt-diff/src/ffi.rs`):
  `GhosttyTerminalOptions` 16 B, `GhosttyFormatterScreenExtra` 16 B,
  `GhosttyFormatterTerminalExtra` 32 B, `GhosttyFormatterTerminalOptions`
  56 B. `ghostty_type_json()` returns a JSON description of every struct
  layout for the current target — useful to cross-check bindings.
- **Allocators**: every constructor takes `const GhosttyAllocator *`;
  NULL selects the default (libc-backed) allocator. Custom allocators are a
  vtable struct (`allocator.h`). Buffers returned by `_alloc` variants
  (e.g. `ghostty_formatter_format_alloc`) must be freed with
  `ghostty_free()` using the **same** allocator.
- **Strings**: `GhosttyString` is a borrowed `{ptr, len}`; lifetimes are
  documented per-API (typically "until the next mutating terminal call").

## Terminal (`terminal.h`, `src/terminal/c/terminal.zig`)

Lifecycle:

- `ghostty_terminal_new(allocator, &term, GhosttyTerminalOptions{cols, rows,
  max_scrollback})` — cols/rows must be > 0. **Gotcha:** the header documents
  `max_scrollback` as "number of lines", but the value is passed straight
  through to `Screen.Options.max_scrollback`, which is **bytes** of page
  memory, rounded up to the page size (`src/terminal/Screen.zig` init doc);
  0 keeps no scrollback at all (verified empirically — note the `Options`
  field comment saying "zero means unlimited" is the stale one of the two
  contradictory comments in Screen.zig).
- `ghostty_terminal_free(term)`.
- `ghostty_terminal_reset(term)` — RIS; keeps dimensions.
- `ghostty_terminal_resize(term, cols, rows, cell_width_px, cell_height_px)`
  — primary screen reflows (when wraparound enabled), alt screen does not.
  Pixel cell size feeds size reports and image protocols; pass 1x1 if
  unused.

Input:

- `ghostty_terminal_vt_write(term, data, len)` — feeds the VT stream parser.
  **Never fails**; malformed input is logged internally and must not corrupt
  state (this is the fuzzing contract the Rust port must match).
- Side-effectful sequences (BEL, OSC title/pwd, DA/DSR queries, XTVERSION,
  XTWINOPS, ENQ, color-scheme DSR) are **dropped by default**. To observe
  them, register "effects" callbacks via
  `ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_*, fn)` plus
  `GHOSTTY_TERMINAL_OPT_USERDATA` (one shared userdata for all callbacks).
  Callbacks run synchronously inside `vt_write`; they must not reenter
  `vt_write` on the same terminal and must not block. Query responses arrive
  via the `WRITE_PTY` callback; response data is only valid during the call.
  The differential harness registers none of these yet.

State readback — `ghostty_terminal_get(term, GHOSTTY_TERMINAL_DATA_*, out)`
with a type-punned out-pointer per key (`get_multi` batches for FFI-overhead
amortization). Keys used by the harness: `COLS`/`ROWS` (u16), `CURSOR_X`/
`CURSOR_Y` (u16, 0-indexed, active area), `CURSOR_PENDING_WRAP` (bool),
`ACTIVE_SCREEN` (enum: 0 primary / 1 alternate). Also available: cursor
visibility/style, kitty keyboard flags, scrollbar (total/offset/len rows),
title/pwd (borrowed `GhosttyString`, valid until next `vt_write`/`reset`),
total/scrollback row counts, effective + default colors (fg/bg/cursor/
256-palette; `GHOSTTY_NO_VALUE` when unset), kitty graphics storage handle,
selection snapshot, viewport-pinned flag.

Modes: `ghostty_terminal_mode_get/set(term, mode, bool)` with `GhosttyMode`
from `modes.h` (packed encoding: number + ANSI/DEC flag).

Grid access: `ghostty_terminal_grid_ref` resolves a `GhosttyPoint`
(active/viewport/screen/history coordinate spaces) to a borrowed
`GhosttyGridRef` for per-cell inspection (codepoints, styles, wrap state —
`grid_ref.h`); refs are invalidated by any mutating call. `_grid_ref_track`
returns an owned handle that survives scroll/reflow and must be freed with
`ghostty_tracked_grid_ref_free`. `screen.h`/`point.h`/`selection.h` carry the
supporting types. This is the API a styles/attributes differ will need;
upstream says it is not fast enough for render loops (use `render.h`'s
render-state API there).

## Formatter (`formatter.h`, `src/terminal/c/formatter.zig`)

The screen-text dump used by the harness:

- `ghostty_formatter_terminal_new(allocator, &fmt, term,
  GhosttyFormatterTerminalOptions)` — options: `emit` (PLAIN=0 / VT=1 /
  HTML=2), `unwrap` (join soft-wrapped lines), `trim` (strip trailing
  whitespace on non-blank lines), `extra` (opt-in state emission: cursor CUP,
  SGR, hyperlinks, modes, palette, tabstops, ... — for state snapshots in the
  VT format), `selection` (NULL = whole screen).
- The formatter **borrows** the terminal: terminal must outlive the
  formatter. It reads current state at each format call, so one formatter
  can be reused across writes (the harness instead creates one per dump —
  cheap, and keeps borrow windows trivially correct).
- It formats the **active screen only** (whole screen space: scrollback +
  visible grid, from top-left of `.screen`). With `max_scrollback = 0`
  screen == visible grid, which is why `ReferenceTerminal::new` defaults to
  no scrollback. Trailing blank grid rows come out as blank lines; the
  harness normalizes (trim per line + drop trailing blanks) before
  comparing — same convention as the replay fixtures' `expected.txt`.
- `ghostty_formatter_format_buf(fmt, buf, len, &written)` — with
  `buf = NULL` returns `GHOSTTY_OUT_OF_SPACE` and the required size in
  `written`; same on a too-small buffer. `_format_alloc` returns an
  allocated buffer (free with `ghostty_free`, same allocator).
- `ghostty_formatter_free(fmt)`.

## Threading

No locking anywhere in the API; a terminal and everything borrowed from it
is single-threaded. Specific notes: the render-state API (`render.h`) is the
designed cross-thread seam (external lock required); `ghostty_sys_set`
(`sys.h`) installs **process-global** function pointers (logging, PNG decode
for kitty graphics) that should be configured once at startup and whose
callbacks "must be safe to call from any thread"; unicode helpers
(`unicode.h`) are pure and thread-safe. The Rust wrapper keeps the terminal
`!Send`/`!Sync` via its raw-pointer field.

## Surface not bound (available when the harness grows)

Standalone parsers: OSC (`osc.h`) and SGR (`sgr.h`). Encoders: key
(`key.h`, kitty protocol with options sourced from a terminal via
`ghostty_key_encoder_setopt_from_terminal`), mouse (`mouse.h`), focus
(`focus.h`). Paste-safety checks (`paste.h`). Kitty graphics inspection
(`kitty_graphics.h`; borrowed handles invalidated by mutating calls;
placement iterator is caller-owned). Render state (`render.h`). Build info
(`build_info.h`: SIMD, kitty-graphics, tmux-control-mode toggles). Wasm
helpers (`wasm.h`).

## Rust-side integration (crates/vt-diff)

- `build.rs` links `libghostty-vt.a` from `$GHOSTTY_VT_LIB_DIR` (default
  `~/local/ghostty/zig-out/lib`), gated behind the `reference` cargo
  feature so trunk builds without the Zig artifact.
- **Gotcha:** macOS `ld` prefers `libghostty-vt.dylib` over the `.a` in the
  same directory, producing binaries with an unresolvable
  `@rpath/libghostty-vt.dylib`. `build.rs` therefore stages the `.a` alone
  into `OUT_DIR` and links against that.
- The static archive needs no extra system libraries beyond what Rust links
  by default on macOS (verified: tests link and run with just
  `-lqwertty-term-vt`).
- Replay-fixture check: all three spike fixtures
  (`alternate_screen_roundtrip`, `prompt_and_color`,
  `wide_text_and_resize`) match `expected.txt` under the normalization
  convention — no semantic divergence to document.
