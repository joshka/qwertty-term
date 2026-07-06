# Terminal state modules: sgr, csi, modes, charsets, Tabstops, color

Surveyed against ghostty commit `2da015cd6ac06cedc89e09756e895d2c1715205d`
(verify with `git -C ~/local/ghostty rev-parse HEAD`). This document covers
six small-to-medium `src/terminal/` modules ported together as one chunk:
five standalone state/parsing modules plus the completion of the `color.zig`
port (whose `Rgb`/`Palette`/`Name`/`DEFAULT` subset had already landed
alongside the page memory model chunk). Rust ports live at
`crates/ghostty-vt/src/{sgr,csi,modes,charsets,tabstops}.rs` and
`crates/ghostty-vt/src/color/{mod,x11_color}.rs`.

## sgr.zig -> `sgr.rs`

**Purpose**: parses SGR (`ESC [ ... m`) parameter lists into a stream of
[`Attribute`] values — bold/italic/underline/blink/etc. toggles, 8/16/256/RGB
color sets, and an `Unknown` catch-all carrying the unparsed params for
diagnostics. This is the hottest, most fuzzed path in the whole terminal
module (`sgr.zig` is 1103 lines with 31 inline tests, several added directly
in response to fuzzer crashes).

**Key types**: `Attribute<'a>` (a tagged union in Zig, an enum in Rust —
renamed the Zig-identifier-syntax variants `@"8_bg"`/`@"256_fg"`/
`@"256_underline_color"` to `Bg8`/`Fg256`/`UnderlineColor256`, no semantic
change); `Underline` (`enum(u3)`, single/double/curly/dotted/dashed);
`Unknown<'a>` (borrows `full`/`partial` `&[u16]` slices from the parser,
which is why `Attribute` and `Parser` both carry a lifetime); `Parser<'a>`
(an iterator-shaped struct: `params`, `params_sep: SepList`, `idx`, with a
`next() -> Option<Attribute<'a>>`).

**Gotchas encoded in the inline tests** (all ported 1:1):

- **Colon vs semicolon separators matter for exactly four params**: `4`
  (underline style), `38`/`48`/`58` (fg/bg/underline direct/indexed color).
  A colon after any other param produces `Unknown`, consuming the whole
  colon-chained run (`consumeUnknownColon`) rather than misinterpreting it.
- **`38:2:...`/`48:2:...`/`58:2:...` accept 3 *or* 4 colon-separated values**
  after the `2` — 4 means an optional ITU-T colorspace identifier is present
  (`Pi`) and must be skipped, 3 means it's absent. Semicolon-separated
  direct-color (`38;2;r;g;b`) never has the colorspace slot. Anything else
  (too many/few colons) falls back to `Unknown`.
- **A trailing colon with no following subparam** (`ESC[58:4:m`, i.e. params
  `[58, 4]` with colon bits at both index 0 and 1) must not panic reading
  `slice[1]` on the *next* `next()` call when that leftover `4` is
  reinterpreted as an underline-style request — this exact input was a
  fuzzer-found crash (`afl-out/stream/default/crashes/id:000021`) and is
  pinned as its own test.
- **Missing color components** (`38;5` with nothing after, `48;5` likewise)
  must exhaust the parser without panicking — also a fuzzer-motivated test.
- **Real-world pathological input**: two distinct Kakoune-generated SGR
  sequences (GitHub discussion #5930 and an earlier crash) combine
  underline-style, direct fg, direct bg, and underline-color in one CSI with
  a specific colon/semicolon mix; both are ported verbatim as regression
  tests.
- `21` (historically "double underline" in some terminals) maps to
  `Underline::Double`, not bold-off — `22` is the bold-off code, distinct
  from `24` (underline-off, i.e. `Underline::None`).
- `5`/`6` both map to `Blink` (blink and rapid-blink are not distinguished).

**Divergences**: no C ABI union (`Attribute.Value`/`.C`/`.cval`,
`lib.TaggedUnion`) — this chunk is Rust-only, FFI is `ghostty-ffi`'s job
later; `Name`/`Rgb` are reused from `crate::color` rather than duplicated.
`sgr::Underline` is kept as its own type distinct from
`crate::page::style::Underline` even though they currently share variants —
one is the wire-parse result, the other is cell-style storage; a later
stream/terminal chunk maps one to the other.

**Test count**: Zig 31, Rust 30. The one gap is `"sgr: Attribute C compat"`,
which only asserts `Attribute.C` (the C ABI shim type) exists — not
applicable since this port has no C ABI layer.

## csi.zig -> `csi.rs`

**Purpose**: small CSI-command enums used by the stream/terminal layer to
interpret ED/EL/TBC parameters and XTWINOPS report/title-stack requests.
55 lines, no inline tests, no parsing logic of its own — pure vocabulary.

**Key types**: `EraseDisplay` (ED: below/above/complete/scrollback, plus
Kitty's `scroll_complete` extension = 22); `EraseLine` (EL:
right/left/complete/`right_unless_pending_wrap`); `TabClear` (TBC:
current/all); `SizeReportStyle` (XTWINOPS 14/16/18/21 t); `TitlePushPop`
(XTWINOPS 22/23, push/pop + window/icon-title index).

**Gotcha**: `EraseLine` and `TabClear` are Zig **non-exhaustive** enums
(`enum(u8) { ...; _ }`) specifically so that converting an arbitrary
user-supplied CSI parameter byte via `@intToEnum`/`@enumFromInt` can never
fail — untrusted terminal input must never panic the parser. Rust has no
non-exhaustive-enum-with-catchall equivalent, so both are modeled as a
regular enum plus an `Other(u8)` variant, with an infallible
`from_param(u8) -> Self` constructor standing in for the Zig cast. No Zig
tests existed to port for this behavior (0 inline tests in the file), so the
Rust tests covering `from_param`'s known/catch-all split are net-new
sanity checks, not ports.

**Test count**: Zig 0, Rust 5 (all net-new, documented as such — no ported
behavior to diverge from).

## modes.zig -> `modes.rs`

**Purpose**: the full catalog of ANSI (`ESC[4h`-style, no `?` prefix) and
DEC (`ESC[?25h`-style) private modes Ghostty supports, packed boolean
storage for their current/saved/default values, and DECRPM
(`ESC[?1$p` -> `ESC[?1;1$y`) report encoding. 386 lines, 12 inline tests
(two are indented sub-tests on `ModeTag`/`ModeState` easy to miss with a
naive `grep '^test '` — the accurate count needs `grep '^\s*test '`).

**Key types**: `Mode` (one variant per supported mode — 41 entries, DECCKM
through `in_band_size_reports`/XTQMODKEYS-adjacent 2048); `ModeTag`
(`{ value: u16, ansi: bool }`, the wire identity of a mode independent of
whether it's recognized); `ModeValues` (one bool per mode); `ModeState`
(current/saved/default `ModeValues`, with `set`/`get`/`save`/`restore`/
`reset`/`get_report`); `Report`/`ReportState` (DECRPM response encoding).

**Gotcha — the comptime table is the spec, not a coincidence**: Zig
generates `Mode` (an enum whose backing `u16` is bit-cast to/from
`ModeTag{ value: u15, ansi: bool }`), `ModePacked` (one bit per mode,
asserted `@sizeOf == 8`), and `modeFromInt` all from one array of
`{ name, value, ansi, default }` entries, so they cannot drift apart. Rust
has no comptime struct-field/enum-variant generation, so this port uses one
`macro_rules!` (`define_modes!`) invoked once over a hand-transcribed version
of the same 41-entry list — the entry list is still the single source of
truth for `Mode`, `ModeValues`, and `mode_from_int`, just expanded by macro
instead of `@Type()`. One entry's Zig field name, `132_column`, is not a
legal Rust identifier (leading digit); it's named `column_132` internally
while the wire-facing name string stays `"132_column"`.

**Gotcha — `ModeTag` cannot bit-cast in Rust**: Zig's `ModeTag.fromMode`
does `@bitCast(@intFromEnum(mode))`, relying on the enum's backing integer
literally being the packed struct's bits. Rust enums don't support custom
bit-layout tags, so `ModeTag::from_mode` is a lookup (`match` over
`mode_value_ansi`) instead of a transmute — same result, different
mechanism; the Zig `ModeTag.test "order"` (which only checks the bit-cast
itself round-trips) has no direct analogue and is ported as a behavioral
round-trip check instead (`mode_tag_from_mode_round_trips`).

**Gotcha — the `@sizeOf(ModePacked) == 8` regression guard**: this Zig test
exists purely so a future accidental extra/removed mode field trips a
visible assertion. `ModeValues` here uses one `bool` (1 byte) per field
rather than one bit, so the byte count necessarily differs; the port keeps
the *spirit* (assert the field count is the expected 41) rather than the
literal byte size, documenting the divergence in the test itself.

**Test count**: Zig 12, Rust 12 — full 1:1 coverage (mechanism differs on 2
of them per the gotchas above, semantics preserved).

## charsets.zig -> `charsets.rs`

**Purpose**: the four charset "slots" (G0-G3, selected via `ESC ( `/`ESC )`/
`ESC *`/`ESC +`) and which slot is active for GL (7-bit) vs GR (8-bit),
switched by SI/SO/locking shifts (a later `Terminal` chunk owns the
shift-state machine; this module only owns the *tables*). 115 lines, 1
inline test.

**Key types**: `Slots` (G0-G3), `ActiveSlot` (GL/GR), `Charset`
(utf8/ascii/british/dec_special), `table(Charset) -> &[u16; 256]`.

**Gotcha**: `utf8` is not a remap table at all — it means "pass bytes
through the UTF-8 decoder unmodified" — and `table(Charset::Utf8)` is a
Zig `unreachable` (ported as a Rust `unreachable!()` panic); callers must
check for `Utf8` before calling `table()`. The `british` table only remaps
`#` (0x23) to `£` (0x00A3); `dec_special` remaps the printable range
0x60-0x7E to box-drawing/technical symbols (the classic "DEC Special
Graphics" set every box-drawing TUI depends on) per
<https://en.wikipedia.org/wiki/DEC_Special_Graphics>.

**Test count**: Zig 1, Rust 3 (the 1 ported test — every non-utf8 charset's
table is exactly 256 entries — plus 2 net-new sanity checks: the `Utf8`
panic path, and a spot-check of two well-known DEC special mappings).

## Tabstops.zig -> `tabstops.rs`

**Purpose**: tracks which columns have a tabstop set, for HT (`\t`)
movement and DECST8C-style resets. 271 lines, 5 inline tests.

**Key types**: `Tabstops` — a two-segment store: a fixed 512-column
"preallocated" region covering the overwhelming majority of real terminal
widths without any allocation, plus a `Vec` grown only past that.

**Divergence — no packed bitset**: Zig hand-rolls a `[N]u8` bitset with
precomputed shift masks (`Unit = u8`, `masks`, `entry()`/`index()` helpers)
purely for memory density. This port uses a plain `Vec<bool>`/boxed
`[bool; 512]` array instead — correctness of `set`/`get`/`unset`/`resize`/
`reset` doesn't depend on the bit-packing, only the byte footprint does, and
that tradeoff isn't load-bearing for this chunk.

**Gotcha — `reset(interval)` only sets stops going forward from `interval`**,
stopping strictly before the last column (`i < self.cols - 1`); an interval
of `0` clears all stops and sets none. `resize()` never applies an interval
to newly-grown columns (a `TODO` in the Zig source itself) — callers must
call `reset()` again after growing if they want stops re-applied.

**Divergence — no allocator-failure test**: Zig's `resize()` can return
`error.OutOfMemory` via an injected test allocator (`tripwire`) and must
leave `cols` unchanged on failure; safe Rust's `Vec::resize` aborts the
process on allocation failure rather than returning a recoverable error, so
there's no failure path to preserve state around. That Zig test is ported as
a same-intent invariant instead: resizing within/at the existing capacity
never perturbs `cols` or existing stops.

**Test count**: Zig 5, Rust 5 (one is a documented behavioral substitute,
see above; the rest are direct ports).

## color.zig + x11_color.zig -> `color/mod.rs` + `color/x11_color.rs`

**Purpose**: completes the color port started with the page-memory chunk
(`Rgb`, `Palette`, `Name`, `DEFAULT` already existed). This pass adds
`Rgb::parse` (hex/`rgb:`/`rgbi:`/X11-name parsing used by config/OSC color
values), luminance/contrast helpers (minimum-contrast rendering depends on
these later), `Special`/`Dynamic` (xterm's OSC 4/5/10-19 special/dynamic
color slot enums), `DynamicPalette`/`DynamicRgb` (mutable palette/color with
reset-to-default tracking, used for OSC 4/104 and OSC 10-19/110-119),
`generate_256_color` (CIELAB-interpolated theme-derived 256-color cube), and
`parse_palette_entry` (config `"N=COLOR"` syntax). `color.zig` is 1250 lines
with 25 inline tests (23 substantive + 2 compile-existence checks on
`Special`/`Dynamic`); `x11_color.zig` is 107 lines with 2 inline tests.

**Key types/fns**: `Rgb::parse` (accepts `#rgb`/`#rrggbb`/`#rrrgggbbb`/
`#rrrrggggbbbb`, bare `rgb`/`rrggbb` for Ghostty config compat, `rgb:h/h/h`
(1-4 hex digits per channel, optional ITU-style colorspace-free form),
`rgbi:f/f/f` (float intensities in `[0,1]`), and X11 names); `Rgb::luminance`/
`contrast`/`perceived_luminance` (W3C WCAG formulas); `PaletteMask` (which
indices were user-modified — a plain `[bool; 256]` standing in for Zig's
`StaticBitSet(256)`, see divergence note); `DynamicPalette` (current +
original + mask, with `set`/`reset`/`reset_all`/`change_default`);
`DynamicRgb` (single overridable/resettable color, e.g. cursor/fg/bg);
`generate_256_color` (trilinear CIELAB interpolation of an 8-corner cube
from bg/base16/fg, plus a 24-step grayscale ramp); the private `Lab` struct
(`from_rgb`/`to_rgb`/`lerp`, full sRGB<->linear<->XYZ<->CIELAB pipeline).

**Gotchas**:

- **`Rgb::parse`'s dispatch order is significant**: `#`-prefixed forms are
  checked first (length dictates 4/8/12/16-bit-per-channel hex), then X11
  named colors (case-insensitive, so a bare `"white"` doesn't fall through
  to hex parsing), then bare 3/6-digit hex (Ghostty config compat — *not* a
  general "hex without #" rule; other lengths are rejected), then finally
  `rgb:`/`rgbi:` — a bare `"12345"` (5 hex-looking digits) is deliberately
  `InvalidFormat` since it matches none of the length-gated hex forms.
- **The `rgb:`/`rgbi:` colon-vs-slash grammar**: `rgb:<r>/<g>/<b>` where each
  component is 1-4 hex digits scaled to 4/8/12/16 bits by *replication*
  (`value * 255 / max_for_width`, not left-shift) — so `rgb:f/ff/fff` is
  white (all channels saturate), not distinct per-width values.
- **`generate_256_color`'s light-theme inversion**: for light themes (fg
  darker than bg) with `harmonious=false`, bg and fg are swapped before
  building the cube so index 16 (cube origin) is always the *darker* color
  and index 231 the *lighter* — keeping the 256-color cube's black-to-white
  orientation consistent regardless of whether the user's theme is light or
  dark. `harmonious=true` skips this swap, preserving the theme's literal
  bg/fg-to-cube-corner mapping instead (useful when the caller wants the
  cube's corners to literally be the configured colors, accepting a
  non-monotonic grayscale ramp as the tradeoff — pinned by the
  "light theme harmonious grayscale ramp" test showing luminance
  *decreasing* through the ramp in that mode).
- **`skip` mask indices are preserved verbatim**, not interpolated — user
  theme overrides for cube/ramp indices must survive palette regeneration.
- **X11 lookup is ASCII case-insensitive and space-insensitive within
  reason** (`"medium spring green"` and `"MediumSpringGreen"` both resolve),
  because it's backed by `x11_color::get`, a lowercased-key `HashMap` built
  once from the embedded `res/rgb.txt` (copied verbatim from the Zig tree;
  fixed-column parsing: `r=line[0..3]`, `g=line[4..7]`, `b=line[8..11]`,
  `name=line[12..]`, matching the upstream file's fixed-width layout
  exactly). `x11_color::entries()` preserves file order (first entry:
  `"snow"`).
- **`parse_palette_entry`'s index parsing follows Zig's
  `std.fmt.parseInt(u8, s, 0)`** base-0 auto-detection: `0x`/`0o`/`0b`
  prefixes (case-sensitive prefix, standard digit parsing), otherwise
  decimal; out-of-range values are a distinct `Overflow` error from
  malformed syntax's `InvalidFormat`.

**Divergences**: no C ABI (`RGB.C`/`.cval`/`PaletteC`/etc.) — Rust-only, same
rationale as sgr.rs. `PaletteMask` is a plain bool array, not a packed
bitset — see gotcha-adjacent note; behavior (not memory layout) is what
`generate_256_color`'s tests pin. Zig's `error{InvalidFormat}`/
`error{Overflow}` become small Rust error enums (`ParseColorError`,
`ParsePaletteEntryError`) with `Display`/`Error` impls per normal Rust
convention rather than Zig error unions.

**Test count**: Zig 25 (23 substantive `RGB`/`DynamicPalette`/`generate256Color`/
`LAB` tests + 2 compile-existence checks `test Special`/`test Dynamic`, N/A —
no port needed, the types are exercised by every other test in the file) +
x11_color.zig's 2, Rust 26 (color/mod.rs) + 2 (x11_color.rs). color/mod.rs's
26 = 23 ported Zig tests + 3 pre-existing sanity tests from the earlier
partial port (`default_palette_named`, `default_palette_cube_and_ramp`,
`rgb_is_three_bytes` — not Zig-sourced, kept from before this chunk started).

## Test porting status (exact counts)

| Zig file | Zig tests | Rust file | Rust tests | Gap explanation |
|---|---|---|---|---|
| `sgr.zig` | 31 | `sgr.rs` | 30 | 1 C-ABI-only test, N/A (no FFI layer in this chunk) |
| `csi.zig` | 0 | `csi.rs` | 5 | No Zig tests to port; 5 net-new covering the `EraseLine`/`TabClear` non-exhaustive-enum modeling |
| `modes.zig` | 12 | `modes.rs` | 12 | Full 1:1 (2 tests changed mechanism, not behavior — see gotchas) |
| `charsets.zig` | 1 | `charsets.rs` | 3 | 1 ported + 2 net-new (utf8 panic path, DEC special spot-check) |
| `Tabstops.zig` | 5 | `tabstops.rs` | 5 | 1 test's mechanism changed (alloc-failure -> no-op-resize invariant, no Rust equivalent to a recoverable `Vec` alloc failure) |
| `color.zig` | 25 | `color/mod.rs` | 26 | 23 ported (2 compile-existence checks N/A) + 3 pre-existing (from the earlier page-memory-chunk partial port, not newly added) |
| `x11_color.zig` | 2 | `color/x11_color.rs` | 2 | Full 1:1 |
| **Total** | **76** | | **83** | |

All 293 tests in `ghostty-vt`'s lib target pass (`cargo test -p ghostty-vt
--lib`), along with the 14 differential-parser tests and 4 unicode
cross-check tests. `cargo fmt --check` and `cargo clippy -p ghostty-vt
--all-targets` are clean.

## Deferred / out of scope for this chunk

- **`stream.zig`/`stream_terminal.zig`/`Terminal.zig` integration**: none of
  these six modules are wired into a stream/terminal dispatch yet — that's
  explicitly a later chunk (per `docs/port-status.md`, `Terminal.zig` alone
  has 50+ inline tests and is its own unit of work). `sgr::Attribute` and
  `page::style::Style`/`Underline` are intentionally still separate types
  pending that wiring.
- **`csi.zig`'s zero-test status** means there's no Zig behavior to verify
  against beyond type shape; if `csi.zig` gains dispatch logic upstream
  later, re-diff before assuming this port is still complete.
- **C ABI for every module** (`Attribute.C`, `RGB.C`, `PaletteC`, etc.) is
  deferred to `ghostty-ffi`, per the crate's embeddability rules (Rust API
  primary, FFI is a wrapper, never the only door to a capability) — noted
  per-module above, not repeated here.
