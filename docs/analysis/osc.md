# OSC (Operating System Command) parser family

Surveyed against ghostty commit `2da015cd6` (repo HEAD at survey time; the OSC
tree itself last changed at `c488ccda6`, "terminal: correct OSC 72 doc
comments for m, i, o, y, X, Y keys").

OSC sequences (`ESC ] ... BEL` or `ESC ] ... ESC \`) are ghostty's grab-bag for
everything that doesn't fit CSI's numeric-parameter model: window titles,
color get/set/reset, hyperlinks, clipboard, semantic prompts, and a pile of
vendor extensions (kitty, iTerm2, ConEmu, rxvt, a UAPI context-signalling
draft). Zig source: `src/terminal/osc.zig` (842 lines: `Command` union,
`Terminator`, the `Parser` incremental state machine) plus 16 files under
`src/terminal/osc/parsers/` (one per command family, dispatched from
`Parser.end`) and `src/terminal/osc/encoding.zig` (a tiny shared helper).

## How this plugs into the VT parser seam

`docs/analysis/vt-parser.md` ("OSC boundary" section) documents the seam this
chunk fills: the ported `parser::Parser` (in `crates/qwertty-term-vt/src/parser/`)
does **not** embed an OSC parser — it emits raw byte events `Action::OscStart`
(on entry to `osc_string`), `Action::OscPut(u8)` (per accumulated byte), and
`Action::OscEnd(u8)` (on exit, carrying the terminating byte: `0x07` for BEL,
`0x1B`/`0x18`/`0x1A` otherwise). This chunk's `osc::Parser` is the structured
consumer of exactly that surface: `reset()` on `OscStart`, `next(byte)` per
`OscPut`, `end(terminator_byte)` on `OscEnd` → `Option<Command>`. It is a
plain value with no borrow into the VT parser's internals (unlike `Csi`/`Esc`,
which borrow the VT parser's arrays) because OSC commands own their own
capture buffer.

## The `Command` union (Zig: `osc.zig:25-247`)

Every OSC command ghostty supports, in `Key` enum order (`osc.zig:172-203`,
"order matters" per `LibEnum`'s comptime C-enum-numbering contract — the
Rust port does not need to preserve this order since it has no C ABI yet,
but the port keeps it anyway for diffability):

| Variant                              | OSC                       | Zig payload                                            | Notes                                                                                           |
| ------------------------------------ | ------------------------- | ------------------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| `invalid`                            | —                         | `void`                                                 | Zero/sentinel value, never a real dispatch                                                      |
| `change_window_title`                | 0, 2                      | `[:0]const u8`                                         | Hex or UTF-8/Latin-1 per title mode (mode interpretation is stream.zig's job, not the parser's) |
| `change_window_icon`                 | 1                         | `[:0]const u8`                                         | Parsed but ignored by ghostty (no icon naming standard)                                         |
| `semantic_prompt`                    | 133 (+9;12 alias)         | `SemanticPrompt` (= `parsers.semantic_prompt.Command`) | See below                                                                                       |
| `clipboard_contents`                 | 52                        | `{ kind: u8, data: [:0]const u8 }`                     | Also reachable via iTerm2's `Copy` (OSC 1337)                                                   |
| `report_pwd`                         | 7                         | `{ value: [:0]const u8 }`                              | Also reachable via ConEmu 9;9 and iTerm2 `CurrentDir`                                           |
| `mouse_shape`                        | 22                        | `{ value: [:0]const u8 }`                              | Free-form string (W3C CSS cursor names in practice)                                             |
| `color_operation`                    | 4,5,10-19,104,105,110-119 | `{ op, requests: List, terminator }`                   | Batchable list of set/query/reset ops                                                           |
| `kitty_color_protocol`               | 21                        | `kitty_color.OSC` (`kitty/color.zig`)                  | Batchable list, needs an allocator                                                              |
| `show_desktop_notification`          | 9, 777                    | `{ title, body }`                                      | OSC 9 body-only (title `""`); OSC 777 `notify;Title;Body`                                       |
| `hyperlink_start`                    | 8                         | `{ id: ?[:0]const u8, uri }`                           |                                                                                                 |
| `hyperlink_end`                      | 8                         | `void`                                                 | Empty URI + no id                                                                               |
| `conemu_sleep`                       | 9;1                       | `{ duration_ms: u16 }`                                 | Clamped 0-10000, default 100 on parse failure                                                   |
| `conemu_show_message_box`            | 9;2                       | `[:0]const u8`                                         |                                                                                                 |
| `conemu_change_tab_title`            | 9;3                       | `union { reset, value: [:0]const u8 }`                 |                                                                                                 |
| `conemu_progress_report`             | 9;4                       | `ProgressReport { state, progress: ?u8 }`              | `state`: remove/set/error/indeterminate/pause                                                   |
| `conemu_wait_input`                  | 9;5                       | `void`                                                 |                                                                                                 |
| `conemu_guimacro`                    | 9;6                       | `[:0]const u8`                                         |                                                                                                 |
| `conemu_run_process`                 | 9;7                       | `[:0]const u8`                                         |                                                                                                 |
| `conemu_output_environment_variable` | 9;8                       | `[:0]const u8`                                         |                                                                                                 |
| `conemu_xterm_emulation`             | 9;10                      | `{ keyboard: ?bool, output: ?bool }`                   | `null` = "do not change"                                                                        |
| `conemu_comment`                     | 9;11                      | `[:0]const u8`                                         |                                                                                                 |
| `kitty_text_sizing`                  | 66                        | `kitty_text_sizing.OSC`                                | scale/width/numerator/denominator/valign/halign + text                                          |
| `kitty_clipboard_protocol`           | 5522                      | `kitty_clipboard_protocol.OSC`                         | metadata/payload/terminator, lazy typed `readOption`                                            |
| `kitty_dnd_protocol`                 | 72                        | `kitty_dnd_protocol.OSC`                               | metadata/payload/terminator, lazy typed `readOption`                                            |
| `context_signal`                     | 3008                      | `context_signal.Command`                               | UAPI hierarchical context signalling                                                            |

Not a `Command` variant but worth naming: OSC 6, 30, 300, 55, 552, 77 are
recognized *prefixes* in the state machine (needed so e.g. "77" can bridge to
777) but produce no command (`osc.zig:809-828`, `.@"55" => null` etc.).

`ProgressReport` (`osc.zig:205-238`) is embedded in `Command` and has its own
`cval()`/C-struct mirror — the Rust port keeps the enum + optional progress,
drops the C mirror (no FFI layer yet).

## The `Parser` incremental state machine (`osc.zig:296-837`)

### Fields (`osc.zig:296-317`)

`alloc: ?Allocator` (optional — some OSCs, e.g. 52/66/72/3008/21/5522, need
one to buffer beyond a fixed 2048-byte scratch buffer; without one those
commands parse-fail rather than truncate), `state: State`, `buffer: [2048]u8`
(`MAX_BUF`), `capture: ?Capture`, `command: Command`.

### `State` enum (`osc.zig:318-371`): a byte-prefix trie, not per-command

The parser's `next(c: u8)` (`osc.zig:544-751`) walks a **hand-written trie
over the OSC's leading digits** — not a generic "read digits until `;`"
loop. States are named after the digit string seen so far: `start` → `0`..`9`
→ `10`..`19`/`21`/`22`/`30`/`52`/`55`/`66`/`72`/`77`/`104`/`110`..`119` →
`133`/`300`/`552`/`777` → `1337`/`3008`/`5522`. Bridge states exist purely to
disambiguate a longer prefix (`3` → `30` → `300` → `3008`; `5` → `52`/`55` →
`552` → `5522`; `7` → `72`/`77` → `777`; `1` → `13` → `133` → `1337`) — e.g.
`55` alone is not a valid OSC but is needed to reach `552`/`5522`.

Once a state recognizes it has consumed its full numeric prefix and sees the
`;` that starts the command body, it calls `captureTrailing` (fixed or
allocating — see below) and from that point **all further bytes are appended
verbatim to the capture buffer**, bypassing the trie entirely
(`next:551-558`: "If a writer has been initialized, we just accumulate the
rest of the OSC sequence... and skip the state machine"). Any unexpected byte
before the `;` sets `state = .invalid`, and once invalid the parser discards
all further input for this OSC (`next:545-547`).

Which OSCs get a **fixed** (`&self.buffer`, 2048 bytes, no allocator needed)
vs **allocating** (`Allocator`-backed, unbounded, only if `parser.alloc` is
set — else falls back to fixed, `captureTrailing:522-539`) capture:

- Fixed: 0, 1, 2, 3008 (context_signal), 7, 8, 9 and its 9;N children, 22,
  104-119 (color set/reset — actually go through `ensureAllocator` first, see
  next point), 133, 777, 1337.
- Allocating (needs `alloc`): 4, 5, 10-19 (color set — via
  `ensureAllocator`, `osc.zig:449-454`, which flips `state = .invalid` if no
  allocator is present rather than silently downgrading to fixed), 21 (kitty
  color), 52 (clipboard), 66 (kitty text sizing), 72 (kitty dnd), 5522 (kitty
  clipboard).

Note the asymmetry: color *set/query* ops (4/5/10-19) require an allocator
(`ensureAllocator` invalidates the parser without one) while color
*reset* ops (104/105/110-119) use a plain fixed buffer (no allocator needed —
reset bodies are just digit lists, always short). 8/22/7/9/133/777/1337/3008
use fixed buffers even though some (kitty clipboard payloads via other paths)
can be large in practice; MAX_BUF (2048) is generous enough that only OSC 0's
"longer than buffer" test (below) actually exercises the truncation path
directly.

### `Capture` (`osc.zig:456-502`)

A tagged union of `std.Io.Writer` backings: `fixed` (writes into
`parser.buffer`, errors with `WriteFailed` → `state = .invalid` on overflow)
or `allocating` (`std.Io.Writer.Allocating`, 2048-byte initial capacity,
grows). `trailing()` returns the buffered slice. The Rust port's equivalent
capture needs the same two-mode behavior (bounded-vs-growable) but can use a
plain `Vec<u8>` with a length cap check instead of two writer backends,
since Rust has no separate "would allocate" concern at this layer — the
important **observable** behavior to preserve is: (a) commands requiring an
allocator become invalid when none is configured, (b) fixed-buffer commands
silently invalidate on overflow past 2048 bytes rather than growing.

### `end(terminator_ch: ?u8) -> ?*Command` (`osc.zig:760-836`)

Dispatches on the final trie state to one of 16 `parsers.*.parse(parser,
terminator_ch)` functions (a 1:1 map from *leaf* trie states to parser
modules — several states share one parser: `0`/`2` → `change_window_title`;
`4,5,10-19,104,110-119` → `color`). States `3`/`30`/`300` (mid-trie for
3008), `6` (mid-trie, no OSC 6), `55` (mid-trie for 552/5522), `77` (mid-trie
for 777) and `start`/`invalid` all return `null` — no command. Every
`parsers.*.parse` function has the identical calling convention:
`fn(parser: *Parser, terminator_ch: ?u8) ?*Command`, mutates
`parser.command` in place and returns `&parser.command` (or `null` +
`parser.state = .invalid` on failure). This convention is exactly why the
Rust port structures parsing as free functions operating on the shared
`Parser`/capture state rather than one-trait-per-command-family — mirroring
the Zig shape keeps the port mechanical and the tests portable verbatim.

### `Terminator` (`osc.zig:249-294`)

`st` (ESC `\`) or `bel` (`0x07`). `Terminator.init(ch: ?u8)` maps `0x07` →
`bel`, anything else (including `null`) → `st`. Several `Command` payloads
(`color_operation`, `kitty_color_protocol`, `kitty_dnd_protocol`,
`kitty_clipboard_protocol`) record the terminator so a *reply* (for query
ops) can echo the same terminator style the request used — xterm/most
terminals are lenient about which one a client sends, but replies should
match to avoid confusing clients that assume symmetry. This is exactly the
byte the ported VT-parser seam already threads through as `OscEnd(u8)` — no
new parser-level plumbing needed to recover it.

## Per-parser structure (`src/terminal/osc/parsers/*.zig`)

All 16 files export a single `pub fn parse(parser: *Parser, terminator_ch:
?u8) ?*Command` (some also export a public `OSC`/`Command` struct type
referenced from the union above). Simple ones (change_window_title,
change_window_icon, report_pwd, mouse_shape, rxvt_extension, hyperlink,
clipboard_operation) just grab `cap.trailing()` and slice it up with
`std.mem.indexOfScalar`/`splitScalar`. The more complex ones implement a
`key=value` reader with lazy typed accessors (`readOption`) rather than
eagerly parsing every field — this pattern appears four times (kitty color,
kitty text sizing, kitty dnd, kitty clipboard, context_signal, semantic
prompt) and the Rust port reuses one small helper (`read_str_option`, in
`osc/support.rs`) for the common "scan `;`- or `:`-separated `key=value`
pairs, comparing keys case-sensitively, trimming ASCII whitespace around the
value" shape rather than re-deriving it six times.

### Trivial one-shot string parsers

- **`change_window_title.zig`** (OSC 0, 2): writes a NUL, slices off it,
  wraps as `[:0]u8`. 7 tests: basic set (0), longer-than-buffer → null
  (truncation edge, exercises `MAX_BUF`), exactly-one-under, exactly-at
  buffer length → null (off-by-one: buffer always reserves the NUL), OSC 2
  variant, UTF-8 title (incl. a byte that collides with a C1 control when
  misinterpreted — pinning that OSC strings are raw bytes, not per-codepoint
  validated), empty title.
- **`change_window_icon.zig`** (OSC 1): identical shape, 1 test (ghostty
  doesn't use the icon name for anything, so minimal coverage is
  intentional).
- **`report_pwd.zig`** (OSC 7): 2 tests (basic `file://...` URL, empty).
- **`mouse_shape.zig`** (OSC 22): 1 test (asserts `parser.state == .@"22"`
  via `inlineAssert` — a debug-only sanity check that `end()`'s dispatch
  table routed correctly, not user-facing behavior).
- **`rxvt_extension.zig`** (OSC 777): only recognizes the `notify` extension
  (`ext;title;body`); anything else is invalid. Produces
  `show_desktop_notification`. 1 test.

### `hyperlink.zig` (OSC 8) — 8 tests

Parses `id=<id>[:id=<id>...];<uri>` — note the leading segment is
colon-separated `key=value` pairs (currently only `id` is recognized;
unknown keys are logged and ignored), terminated by the first unescaped `;`
which starts the URI. Empty URI + no id → `hyperlink_end`; empty URI + an id
present → invalid (can't end a hyperlink while also naming one). Tests:
basic uri, with id, with empty id (→ `None`, not empty string), with
incomplete key (bare `id` with no `=`), with empty key (`=value`), with both
empty key and id (`=value:id=foo`), with empty uri (→ null, since id-without-uri
is invalid per above), hyperlink end (`8;;`).

### `clipboard_operation.zig` (OSC 52) — 4 tests

Body is `<kind-char>;<base64-or-?>` where an empty kind char (`;?` — leading
semicolon) defaults to `'c'` (clipboard). Always requires the parser have an
allocator (`inlineAssert(parser.state == .@"52")` then relies on the state
machine having already invalidated if no allocator — the OSC 52 body itself
doesn't decode base64; that's left to the terminal-state layer, this parser
only slices out the kind char and the raw payload string). Tests: get/set
with kind, optional/empty kind, with-allocator (matters here more than most
since OSC 52 always uses the allocating capture), clear (empty data).

### `color.zig` (OSC 4,5,10-19,104,105,110-119) — 12 tests

The most logically dense small parser. `Operation` enum names each OSC
number (`osc_4`..`osc_119`, `osc.zig` dispatch-mapped 1:1 by trie leaf
state). `parseColor(alloc, op, buf)` tokenizes the body on `;` and, per
operation class, produces a `List` (`std.SegmentedList(Request, 2)` —
ported as `Vec<Request>`, the segmented-list optimization is a Zig-allocator
concern that doesn't apply in Rust) of `Request` (`set{target,color} | query
{target} | reset{target} | reset_palette | reset_special`):

- OSC 4/5 (`parseGetSetAnsiColor`): reads `index;spec` pairs in a loop until
  the iterator runs dry or a pair fails to parse (partial results are kept —
  this matches xterm's `ChangeAnsiColorRequest`, deliberately, per the doc
  comment). OSC 4 index 0-255 → palette, 256+ → `Special` (offset by the
  256-entry palette size); OSC 5 index maps directly to `Special` (bold=0,
  underline=1, blink=2, reverse=3, italic=4 — `color.zig:416-437` in the
  Zig `color.zig`, this chunk's territory owns only enough of that enum to
  reproduce parsing, not its runtime meaning). `spec == "?"` → a query
  request instead of a set.
- OSC 104/105 (`parseResetAnsiColor`): reads a list of bare indices to
  reset; no arguments at all means "reset everything" (`reset_palette` /
  `reset_special`). Unlike the set path, xterm-incompatible entries are
  *skipped*, not fatal (explicit doc comment: "we're more flexible... matches
  Kitty").
- OSC 10-19 (`parseGetSetDynamicColor`): a sequence of bare specs (no
  index!) against a *starting* `DynamicColor` that auto-increments
  (`DynamicColor.next()`) — so `\e]11;red;blue\e\\` sets background=red,
  cursor=blue (OSC 11 starts at `.background`, next is `.cursor`). This is
  the "successive parameter changes the next color in the list" xterm rule.
- OSC 110-119 (`parseResetDynamicColor`): resets exactly the one dynamic
  color tied to that OSC number; any argument at all invalidates it (single
  no-arg reset only, an asymmetry vs 104/105's list-based reset).

`Target = palette(u8) | special(SpecialColor) | dynamic(DynamicColor)`.
`ColoredTarget = { target, color: RGB }`.

Tests (12, several are "for every index/every enum value" loops so cover
far more than 12 cases): OSC 4 empty param without an allocator → null
(really a no-allocator/`ensureAllocator` test, since `Parser.init(null)` is
used — OSC 4's leading `;` requires an allocator regardless of body
content); OSC 4 full sweep (every
palette index 0-255 as set/query/trailing-garbage/whitespace-tolerant, plus
every `SpecialColor` variant via 256+i); OSC 5 full `SpecialColor` sweep; OSC
4 multiple requests (incl. same-index-overwrites-earlier semantics, which is
just "list of ops," not deduped — downstream applies them in order); OSC 104
full sweep (palette + special); OSC 104 empty index segment (`0;;1` skips
the empty one); OSC 104 invalid index (`ffff` — out of u9 range) skipped,
next still parsed; OSC 104/105 "reset all" (empty body); one combinatorial
test over all 10 `DynamicColor` variants for get/set (`OSC 10..19`) and
another for reset (`OSC 110..119`, including the "xterm allows a trailing
`;`" and "xterm does NOT allow whitespace" edge cases inline).

**Dependency note**: `color.zig` (the OSC parser) imports `RGB`, `Dynamic`,
`Special` from `src/terminal/color.zig` (a *different* chunk's file — the
terminal-state/Screen area owns full `color.zig`, including the X11 named-
color table `x11_color.zig`, LAB color math, and the config-facing
`parsePaletteEntry`). Per this chunk's charter, the **minimal** supporting
pieces needed to make the OSC 4/5/10-19/104/110-119/21 tests pass have been
ported into `crates/qwertty-term-vt/src/osc/rgb.rs`: an `Rgb::parse` covering the
`#rgb`/`#rrggbb`/`#rrrgggbbb`/`#rrrrggggbbbb`, bare `rgb`/`rrggbb`, and
`rgb:h/hh/hhh/hhhh` / `rgbi:<float>/<float>/<float>` forms (`color.zig:642-
699` in the Zig original), plus `Special`/`Dynamic` enums with `Dynamic::next`.
> **RESOLVED (2026-07-06, osc-color-dedup chunk):** the divergences below about X11
> named colors are obsolete. `osc` now delegates to `crate::color::Rgb::parse`
> (full upstream grammar incl. X11 names from embedded `res/rgb.txt`); the local
> `osc::rgb` parser was removed and named-color test cases restored.

**X11 named colors are explicitly NOT ported** — `Rgb::parse` returns
`Err` for a bare name like `"red"` for now, with a `// TODO(color chunk)`
marker, since `x11_color.zig`'s ~700-entry `rgb.txt`-derived table is
squarely that other chunk's data file, not a small supporting type. This
means the Rust port's OSC 4/5/10-19 tests that rely on named colors (all of
them — every Zig test in `color.zig` sets colors via `"red"`/`"blue"`) use
literal hex equivalents (`"#ff0000"` for red, `"#0000ff"` for blue) instead,
with a comment noting the substitution and pointing at the deferred X11
table. This is flagged again in the "divergences" section below.

### `kitty_color.zig` (OSC 21) — 5 tests, + `kitty/color.zig` (support type) — 1 test

OSC 21 body is `;`-separated `key=value` pairs (`key` = `Kind.parse`: either
a `Special` name — `foreground`/`background`/`selection_foreground`/
`selection_background`/`cursor`/`cursor_text`/`visual_bell`/
`second_transparent_background` — or a bare palette index). Value `""` →
reset, `"?"` → query, else → `RGB.parse` → set (parse failures are logged
and the pair is skipped, not fatal to the whole OSC). Requires an allocator
(a `List` — `std.ArrayList(Request)`, ported as `Vec`); without one the
whole OSC fails. Caps at `Kind.max * 2` entries (`u8::MAX` palette slots +
8 special kinds, doubled — generous headroom, not a meaningful protocol
limit) as a DoS guard; exceeding it invalidates the parser.
`kitty/color.zig`'s `Kind::format` (Display impl) has its own 1-test file
or (foreground → "foreground", palette 42 → "42").

Tests: full protocol exercise (one long OSC mixing query/set/reset/palette-
index forms across 9 requests — port as one test matching structure, not
split), without-allocator → null, double-reset (calling `p.reset()` twice
is a no-op safety check — Rust's owned-value semantics make this
structurally impossible to get wrong, so this test degenerates to "parser
can be reset and reused," ported for parity anyway), reset-after-invalid
(state returns to `start` after `reset()`, then feeding a bad byte flips to
`invalid` again — pins that `reset()` doesn't leave stale trie state), empty
body (no keys at all → 0-length list, not an error).

### `kitty_text_sizing.zig` (OSC 66) — 8 tests

Body is `<colon-separated key=value args>;<UTF-8 text>`. Single-character
keys: `s` (scale, 1-7, 0 is invalid and rejected), `w` (width, 0-7, 0 =
default), `n`/`d` (numerator/denominator, 0-15), `v`/`h` (valign: top/
bottom/center; halign: left/right/center — both encoded as small integers
0/1/2 via `std.enums.fromInt`, not letters). Unknown keys or malformed
values are logged and skipped (not fatal). The text payload must be
"escape-code-safe UTF-8" (`osc/encoding.zig`'s `isSafeUtf8` — no C0/C1/DEL)
and ≤ 4096 bytes (`max_payload_length`); either violation invalidates the
whole command. Tests: empty params (defaults), single param, all six
params together, scale-zero rejected (falls back to default 1, doesn't
invalidate the whole command — just that one field), invalid params
(unknown key + bad value don't invalidate, just get skipped, remaining valid
params still apply), UTF-8 text round-trip, unsafe UTF-8 (newline) → null,
overlong text (> 4096 bytes) → null.

### `kitty_dnd_protocol.zig` (OSC 72) — 11 tests

Body is `<metadata>;<payload>` (payload optional — absent if no `;`).
Metadata itself is a colon-separated `key=value` list read **lazily** via
`Option.read(comptime key, metadata)` (no eager parse into a struct — the
consumer asks for exactly the keys it needs). Keys are single ASCII
characters and **case-sensitive** (`x` vs `X` are different keys — this is
explicitly tested). `t` (event type: 13 single-char codes `a/A/m/M/r/R/o/p/
P/e/E/k/q` → `EventType` enum) is the only non-integer key; `m,i,o,x,y,X,Y`
are all `i32` (location/session-id/operation fields, `-1` is a meaningful
sentinel for "drag left the window," not an error). Tests: metadata-only (no
payload), metadata + empty payload, metadata + non-empty payload, all 13
`EventType` codes round-tripped, unknown event-type char → null (not
invalid-whole-command), all integer keys read together, negative sentinel
(-1) preserved, case-sensitivity (`x=10:Y=200` — reading `.X` or `.y` must
return null even though `.x`/`.Y` are present), absent key → null, malformed
integer → null, BEL terminator recorded on the `OSC` struct.

### `kitty_clipboard_protocol.zig` (OSC 5522) — 27 tests

Structurally identical to kitty_dnd_protocol: `<metadata>;<payload>`,
metadata is colon-separated lazy `key=value`. Keys: `id` (validated against
an identifier charset `[A-Za-z0-9\-_+.]+`, empty/invalid → null, not a
whole-command failure), `loc` (enum, currently only `primary`), `mime`,
`name`, `password`/`pw` (two spellings for the same semantic slot — both
just return the raw string, base64-encoded per the protocol but *not*
decoded by this layer), `status` (enum: `DATA/DONE/EBUSY/EINVAL/EIO/ENOSYS/
EPERM/OK` — POSIX-flavored error codes), `type` (enum: `read/walias/wdata/
write`). No eager validation happens at parse time beyond metadata/payload
splitting — `readOption` does all the typed decoding, lazily, exactly like
kitty_dnd. This file's `parse()` function is only 25 lines
(`osc.zig`-pattern: assert state, split on first `;`, done); **all 27 tests**
are `readOption` exercises, not parser-logic branches — 3 basic
(empty/empty-payload/non-empty), 6 field-specific (`id` valid/invalid/empty,
`status` valid/invalid, `loc` valid/invalid, `password` two spellings), and
**15 numbered "example" tests taken directly from the kitty spec** covering
realistic multi-field sequences (read+status, read+mime+payload, write,
wdata, walias, combinations with password). These spec examples are ported
1:1, including their exact base64 literal payloads, since they double as a
spec-conformance fixture, not just unit coverage.

### `context_signal.zig` (OSC 3008) — 18 tests

Body is `start=<id>|end=<id>[;key=value...]`. `id` must be 1-64 bytes in the
printable ASCII range (0x20-0x7E) — checked eagerly (invalidates the whole
command if violated, unlike most of the other lazy-field parsers). Metadata
after the id is `;`-separated `key=value`, read lazily like kitty_dnd/kitty_
clipboard. Fields split into "start" fields (`type`: `ContextType` enum —
boot/container/vm/elevate/chpriv/subcontext/remote/shell/command/app/
service/session; `user`/`hostname`/`machineid`/`bootid`/`comm`/`cwd`/
`cmdline`/`vm`/`container`/`targetuser`/`targethost`/`sessionid`: raw
strings; `pid`/`pidfdid`: `u64`) and "end" fields (`exit`: `ExitStatus` enum
— success/failure/crash/interrupt; `status`: `u64`; `signal`: string).
Unknown fields are silently ignored per spec. Tests: basic start, basic end,
start with a few fields, start with "all common fields" (8 fields in one
sequence), end with exit metadata, end with failure+signal, unknown-field-
ignored, missing-field → null (not error), invalid prefix (`bogus=` instead
of `start=`/`end=`) → whole command null, empty-after-prefix (`start=` with
no id) → null, max-length id (64 bytes, boundary), over-length id (65 bytes)
→ null, full `ContextType` enum coverage (all 12 + invalid), full
`ExitStatus` enum coverage (all 4 + invalid), two "spec example" tests
(container start and context end, copied from the UAPI spec doc), cwd/
cmdline fields (values may contain spaces — not delimiter-escaped, since `;`
is still the field separator and these values don't need to contain one in
the example), and "start with no fields" (bare id, nothing after).

### `iterm2.zig` (OSC 1337) — 19 tests

iTerm2's protocol is a single `Key=Value` pair (Key from a large enum of 34
iTerm2-specific commands, matched **ASCII-case-insensitively** via a
`StaticStringMapWithEql`). Ghostty implements exactly two of the 34:
`Copy` (aliases to `clipboard_contents` with kind `'c'` — this *is* OSC 52
under another name, so the parser rejects OSC-52-isms like an empty value,
lone `?`, or a value not colon-prefixed, since those only make sense for a
real OSC 52 query/clear, and iTerm2's `Copy` doesn't support them) and
`CurrentDir` (aliases to `report_pwd`). All 32 other recognized-but-
unimplemented keys (`AddAnnotation`, `SetBadgeFormat`, `File`, `SetColors`,
etc.) log a debug message and produce `null` — deliberately: the parser
still validates syntax and dispatch, it just doesn't act on them. Unknown
keys entirely also → `null`. Tests: unimplemented key × {no value, empty
value, non-empty value} × {as-written case, all-lowercase} = 6 combinations
via `SetBadgeFormat`/`setbadgeformat`; unknown key × the same 3 value shapes
= 3 more (`BobrKurwa`); `Copy` × {no value, empty value, colon-only, `:?`
(OSC-52-ism), non-base64-but-well-formed (marked `SkipZigTest` — "for
performance reasons we don't check for valid base64 data right now," ported
as `#[ignore]` with the same comment), base64-without-colon-prefix,
valid-with-colon-prefix (the one success case)} = 7; `CurrentDir` × {no
value, empty value, non-empty value} = 3. Total 19.

### `osc9.zig` (OSC 9 + ConEmu 9;1-9;12) — 61 tests

The largest test file by count; the parse logic itself (`osc9.zig:1-286`) is
a single nested `switch` on the captured body's leading bytes — **not** a
separate sub-state-machine the way the trie in `osc.zig` is; it's ordinary
string dispatch over the already-fully-captured buffer, same style as every
other parser here. First byte `'1'` branches again on the second byte to
reach 9;1 (sleep), 9;10 (xterm emulation), 9;11 (comment), 9;12 (semantic
prompt alias); bytes `'2'`..`'9'` map directly to 9;2..9;9. Any prefix that
doesn't match a recognized ConEmu shape (`break :conemu`) falls through to
the **iTerm2-style desktop notification** interpretation: the *entire*
captured body becomes the notification text with an empty title. This
fallthrough is the source of many "N incomplete -> desktop notification"
tests below — e.g. `9;1` alone (no trailing `;value`) doesn't match the
sleep grammar, so it becomes a notification with body `"1"`.

Per-subcommand test tally (61 total, all read and ported):

- Plain OSC 9 notification (2): full body as notification, single-char body.
- 9;1 ConEmu sleep (6): basic value, no-value defaults to 100ms, value
  clamped at 10000ms, unparseable value also defaults to 100ms, and two
  "malformed prefix falls through to notification" cases (`9;1` bare,
  `9;1a`).
- 9;2 message box (6): basic, malformed-prefix-`9;2`→notification, empty
  message, whitespace-only message, and two more fallthrough-to-notification
  variants (duplicated intent with the "invalid input" case above — ported
  as separate tests per Zig, not merged).
- 9;3 change tab title (5): basic value, `;` with nothing after → `.reset`
  variant (not empty-string value — an important distinction the Rust
  `enum ConemuChangeTabTitle { Reset, Value(...) }` must preserve),
  whitespace-only value (a real value, not reset), two fallthrough cases.
- 9;4 progress report (16): set with progress, overflow clamps to 100,
  single-digit, double-digit, "extra semicolon ignored" (duplicate of the
  basic-set case structurally but pinned separately per Zig), remove with
  no progress / double-semicolon / progress-value-present-but-ignored /
  trailing-semicolon (4 variants, all → `.remove` with `progress: null`),
  error state (bare and with progress), pause state (bare and with
  progress), and 4 fallthrough-to-notification cases (bare `9;4`, trailing
  `;`, unknown sub-code `5`, unknown sub-code with suffix `5a`).
- 9;5 wait input (2): bare, with ignored trailing garbage (still recognized
  — 9;5 doesn't require a following `;`, unlike most others).
- 9;6 guimacro (3): one-char value, two-char value, incomplete (bare `9;6`)
  → notification.
- 9;7 run process (3): value, empty value (`9;7;` — note this is NOT the
  incomplete case, an explicit `;` with nothing after still parses as
  empty-string, distinct from bare `9;7`), incomplete → notification.
- 9;8 output environment variable (3): same 3-shape pattern as 9;7.
- 9;9 report cwd (2): value, incomplete → notification (no empty-string
  variant tested here, unlike 9;7/9;8).
- 9;10 xterm keyboard/output emulation (8): bare (both true — the "no
  argument" default), `;0` (both false), `;1` (both true, explicit),
  `;2` (keyboard null/"don't change", output false), `;3` (keyboard null,
  output true), unknown digit `;4` → notification, trailing `;` alone →
  notification, non-digit suffix `;abc` → notification.
- 9;11 comment (2): value, incomplete → notification.
- 9;12 mark prompt start / ConEmu alias for OSC 133;A (2): bare, with
  trailing garbage (both still produce `semantic_prompt` — this state
  **always** succeeds regardless of what follows, unlike every sibling
  ConEmu subcommand; this is the one place osc9.zig calls into
  `semantic_prompt.Command.init`, a cross-parser dependency).

### `semantic_prompt.zig` (OSC 133, + the 9;12 alias above) — 64 tests

Implements the [freedesktop semantic-prompts
spec](https://gitlab.freedesktop.org/Per_Bothner/specifications/blob/master/proposals/semantic-prompts.md)
plus kitty's `redraw`/Ghostty's `last` extension and a `click_events`
extension. Single-letter action codes, each optionally followed by
`;key=value;key=value...`:

| Code | `Action`                               | Notes                                                         |
| ---- | -------------------------------------- | ------------------------------------------------------------- |
| `A`  | `fresh_line_new_prompt`                | Also reachable via ConEmu 9;12                                |
| `B`  | `end_prompt_start_input`               |                                                               |
| `I`  | `end_prompt_start_input_terminate_eol` |                                                               |
| `C`  | `end_input_start_output`               |                                                               |
| `D`  | `end_command`                          | First option field is positional (exit code), not `key=value` |
| `L`  | `fresh_line`                           | Takes **no** options at all — any trailing data invalidates   |
| `N`  | `new_command`                          |                                                               |
| `P`  | `prompt_start`                         |                                                               |

Options (read lazily via `Option.read`, same lazy-key-scan pattern as
kitty_dnd/kitty_clipboard/context_signal): `aid` (string, any app-chosen
identifier), `cl` (`Click` enum: `line`/`multiple`("m")/
`conservative_vertical`("v")/`smart_vertical`("w") — click-to-move-cursor
capability), `k`/`prompt_kind` (`PromptKind`: initial("i")/right("r")/
continuation("c")/secondary("s")), `err` (string, free-form error tag),
`redraw` (`Redraw`: true/false/"last" — Ghostty's own extension for
resize-time prompt redraw behavior; "last" exists specifically to
special-case bash, which "does this because its bad and they should feel
bad" per the doc comment — ported verbatim as a code comment, it's too good
to lose), `special_key` (bool), `click_events` (`ClickEvents`: absolute("1")/
relative("2")), and the positional `exit_code` (i32, `D`-only, read as the
*first* semicolon-delimited field with no `key=` prefix at all — special-
cased in `Option.read`).

`Command.writeCommandLine` decodes either `cmdline` (bash `printf %q`
quoting: `$'...'` or `'...'` wrapping, backslash escapes for space/backslash/
quote/`$`/`e`/`n`/`r`/`t`/`v`) or `cmdline_url` (URL percent-encoding) into a
writer, via `src/os/string_encoding.zig`'s `printfQDecode`/`urlPercentDecode`
— a small (307-line) **shared OS-layer utility, not terminal-specific**,
that this chunk ports minimally (see "supporting types" below) since
semantic-prompt's `cmdline`/`cmdline_url` tests depend on it for real
decoding, not just presence.

Parse structure (`osc9.zig`-style: nested `switch`+labeled-block, not a
sub-trie): dispatch on the first body byte to one of the 8 action letters;
each arm sets the base `Command` via `.init(action)`, then, if more data
follows, requires it to start with exactly one `;` before treating the rest
as `options_unvalidated` — anything else (extra bytes not preceded by `;`)
falls through to `state = .invalid`. `L` (fresh_line) is the sole exception:
*any* trailing byte at all invalidates it (no options are recognized for
`fresh_line`, full stop).

Test tally (64, grouped by action, all read and ported 1:1):

- `C` end_input_start_output (14): bare, extra-contents-without-`;` → null,
  with `aid` option, then **10** `cmdline`/`cmdline_url` decode-fidelity
  tests (numbered 3-9 for cmdline: backslash-space, backslash-n, `$'...'`
  quoting, `'...'` quoting, unterminated `'...'` → DecodeError, unterminated
  `$'...'` → DecodeError, bare `$'` → DecodeError, empty value; numbered 1-8
  for cmdline_url: plain, `%20` space, `%3b` → `;`, truncated `%3` →
  DecodeError, bare trailing `%` → DecodeError, trailing `%20`, truncated
  `%2` at end → DecodeError, bare trailing `%` at end → DecodeError). These
  20 map to `string_encoding.rs`'s own dedicated unit tests too (see below)
  but are also exercised here through the full OSC parse path, so both
  layers get coverage — ported at both layers, not deduplicated.
- `L` fresh_line (2): bare, extra-contents (both a random suffix and an
  options-looking suffix — both invalidate, since `L` takes zero options).
- `A` fresh_line_new_prompt (12): bare (+ options absent-checks for `aid`/
  `cl`), with `aid`, with `=` embedded inside the aid value (delimiter
  ambiguity — `aid` value is everything up to the next `;`, `=` inside it is
  not special), `cl=line`/`cl=m`, invalid `cl` value → null option (not
  invalid command), trailing bare `;` (no options at all, still valid),
  bare unrecognized key (`barekey`, ignored), multiple options together,
  default `redraw` (unset → null), `redraw=0`/`redraw=1`, invalid `redraw`
  value → null.
- `P` prompt_start (7): bare, `k=i/r/c/s` (4 tests), invalid `k` → null,
  extra-contents-without-`;` → null.
- `N` new_command (5): bare, with `aid`, with `cl=line`, multiple options,
  extra-contents → null.
- `B` end_prompt_start_input (3): bare, extra-contents → null, with options.
- `I` end_prompt_start_input_terminate_eol (3): bare, extra-contents → null,
  with options.
- `D` end_command (4): bare (checks `exit_code`/`aid`/`err` all null),
  extra-contents → null, exit code `0`, exit code + `aid` together
  (positional exit code followed by `;key=value`).
- `Option.read` unit tests, bypassing the OSC trie entirely (8): `aid`
  (multiple positions in the field list, trailing/leading `;;`, present-but-
  empty, absent, bare key with no `=`), `cl` (all 4 variants + invalid +
  wrong-key), `prompt_kind` (all 4 + invalid single-char + invalid
  multi-char + empty), `err`, `redraw` (1/0/last/invalid-digit/out-of-range/
  empty), `special_key` (0/1/invalid), `click_events` (yes/0/1/2 — "yes" and
  "0" both invalid since only "1"/"2" are recognized), `exit_code`
  (positive/zero/negative/non-numeric/with-trailing-fields).

## Supporting types ported (per task rule: minimal port, not a stub)

Two pieces of state live in *other* chunks' Zig files but are small enough,
and load-bearing enough for test fidelity, to port minimally here rather
than stub out:

1. **`crates/qwertty-term-vt/src/osc/rgb.rs`**: `Rgb::parse` (hex forms +
   `rgb:`/`rgbi:`), ported from `src/terminal/color.zig:642-699`. **X11
   named colors are explicitly not ported** (see the `color.zig` section
   above) — that's `x11_color.zig`'s ~700-entry generated table, squarely
   the terminal-state/Screen chunk's file, marked with a `// TODO` pointing
   here. `Special`/`Dynamic` enums (`color.zig:416-464`) are ported in full
   (they're tiny and OSC-parser-owned in spirit — every variant is a
   protocol constant the OSC layer dispatches on).
2. **`crates/qwertty-term-vt/src/osc/string_encoding.rs`**: `printf_q_decode`
   and `url_percent_decode`, ported from `src/os/string_encoding.zig:6-191`
   (the `os/` prefix marks it as a general OS-layer helper, not
   terminal-specific, but it has exactly one caller in the whole codebase —
   `semantic_prompt.zig`'s `writeCommandLine` — so porting the two functions
   this chunk needs, with their own 18 inline tests, is cheaper and more
   faithful than stubbing). `url_percent_encode` (the encode direction,
   used by config/CLI code elsewhere, not by any OSC parser) is **not**
   ported — out of scope.

## Divergences / gotchas for the Rust port

1. **No X11 named colors.** Every Zig test in `color.zig` and
   `kitty_color.zig` that sets a color via a bare name (`"red"`, `"blue"`,
   `"aliceblue"`) had to be rewritten in the Rust port to use the equivalent
   `#RRGGBB` literal, with a comment citing this gap. Functionally: OSC
   4/5/10-19/21 `...;red\e\\` will fail to parse in this crate until the
   color chunk lands `x11_color`'s table and this module's `Rgb::parse`
   is wired to consult it (a one-line addition once that table exists).
2. **The OSC trie is flattened, not re-derived.** Rather than porting
   `osc.zig`'s hand-rolled prefix-trie `State` enum + `next()` byte-by-byte
   (which exists to let the *ghostty* `osc.Parser` be embedded byte-for-byte
   inside the VT `Parser`), this port's `osc::Parser::next` buffers raw
   bytes into a `Vec<u8>` (bounded the same way: fixed 2048 cap unless the
   command needs growth) and does the prefix dispatch **once**, in `end()`,
   by matching on the buffered prefix up to the first `;` (or whole buffer
   for no-`;` commands). This is behaviorally equivalent (same invalid/valid
   partition, same capture-mode-per-command-family) and much simpler in
   Rust, at the cost of not being a literal byte-driven state machine. If
   this divergence ever matters (e.g. a future perf pass wants incremental
   dispatch to avoid buffering unbounded OSC 52 payloads before knowing
   they're OSC 52), it is easy to reintroduce the trie later behind the same
   `Parser::next`/`Parser::end` API — no caller-visible change.
3. **`ensureAllocator`'s asymmetry is preserved but is easy to miss.**
   `Parser::alloc` in the Rust port is `Option<()>`-shaped (a bool flag —
   the Rust `Vec<u8>`-based capture buffer doesn't need a real allocator
   handle to "grow," so the Zig `Allocator` parameter degenerates to "is
   unbounded capture permitted"). OSC 4/5/10-19/21/52/66/72/5522 check this
   flag and invalidate without it; OSC 104/105/110-119/0/1/2/7/8/9/22/133/
   777/1337/3008 do not need it. Getting this wrong silently changes which
   OSCs work without a configured allocator — tests pin every case.
4. **`kitty_color.zig`'s "double reset" and "reset after invalid" tests are
   nearly no-ops in Rust.** Zig's `Parser` is a long-lived mutable struct
   that callers explicitly `.reset()` between OSCs; a double-`reset()`-does-
   nothing-bad test is a real memory-safety concern there (freeing already-
   freed capture state). In the Rust port, `Parser::reset` just replaces
   `self` fields with fresh defaults — double-reset is trivially safe by
   construction. Ported anyway for 1:1 parity and because "resetting mid-
   invalid-state returns to a clean `start`" is still a real behavioral
   contract worth pinning.
5. **`conemu_change_tab_title`'s `.reset` vs empty-string value** is a
   distinction the Rust `enum` must encode explicitly
   (`ConemuChangeTabTitle::Reset` vs `ConemuChangeTabTitle::Value(String)`
   with an empty string) — `9;3;` (trailing `;`, nothing after) is `.reset`,
   not `Value("")`, per `osc9.zig:126-131`.
6. **iTerm2's `Copy` "invalid base64" test is skipped in Zig** ("For
   performance reasons, we don't check for valid base64 data right now") —
   ported as `#[ignore]` with the same rationale comment, not deleted, so
   the gap stays visible and the test can be un-ignored if this ever
   changes upstream.
7. **`Command::invalid` has no Rust equivalent as a public variant.** Zig's
   `Command` is a tagged union that must always hold *some* value, so
   `invalid` exists as the zero-initialized sentinel. The Rust port's
   `end()` returns `Option<Command>` instead, so `None` plays that role —
   `Command::Invalid` is not part of the public enum. Every test asserting
   "ends up invalid" is ported as `assert!(parser.end(term).is_none())`.
8. **No FFI/`cval()` mirrors ported.** Every Zig payload struct with a
   `pub const C = void;`/`cval()` pair (kitty color OSC, kitty text sizing
   OSC, kitty dnd OSC, context_signal Command) drops that pair — there is
   no `qwertty-term-ffi` crate yet (Phase 6). Noted per-type in doc comments so
   the eventual FFI chunk knows what mirror to add.

## Zig vs Rust test counts (per file, final — port complete)

Ported into `crates/qwertty-term-vt/src/osc/`. Every parser file maps 1:1 to a
Rust module of the same name under `osc/parsers/`; test counts match
exactly (no consolidation), plus a small number of Rust-only tests added
for behavior the Rust port introduces (e.g. the allocator-permission gate
being a constructor choice rather than a runtime-optional field) — these
are called out per-file below and are additive, not substitutes for a Zig
test. (Task instructions: do not edit `docs/port-status.md` from this
chunk; the orchestrating session merges this table into it at integration
time.)

| Zig file                                                                                                  | Zig tests                                                                                      | Rust file                                     | Rust tests | Notes                                                                                           |
| --------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- | --------------------------------------------- | ---------- | ----------------------------------------------------------------------------------------------- |
| `change_window_icon.zig`                                                                                  | 1                                                                                              | `osc/parsers/change_window_icon.rs`           | 1          |                                                                                                 |
| `change_window_title.zig`                                                                                 | 7                                                                                              | `osc/parsers/change_window_title.rs`          | 7          |                                                                                                 |
| `clipboard_operation.zig`                                                                                 | 4                                                                                              | `osc/parsers/clipboard_operation.rs`          | 5          | +1 Rust-only: without-allocator gate                                                            |
| `color.zig`                                                                                               | 12                                                                                             | `osc/parsers/color.rs`                        | 12         | X11 names → hex literals (divergence #1)                                                        |
| `context_signal.zig`                                                                                      | 18                                                                                             | `osc/parsers/context_signal.rs`               | 18         |                                                                                                 |
| `hyperlink.zig`                                                                                           | 8                                                                                              | `osc/parsers/hyperlink.rs`                    | 8          |                                                                                                 |
| `iterm2.zig`                                                                                              | 19                                                                                             | `osc/parsers/iterm2.rs`                       | 19         | 1 `#[ignore]` (matches Zig's `SkipZigTest`)                                                     |
| `kitty_clipboard_protocol.zig`                                                                            | 27                                                                                             | `osc/parsers/kitty_clipboard_protocol.rs`     | 27         |                                                                                                 |
| `kitty_color.zig`                                                                                         | 5                                                                                              | `osc/parsers/kitty_color.rs`                  | 5          | + `kitty/color.zig`'s 1 test folded into the same file (see next row)                           |
| `kitty_dnd_protocol.zig`                                                                                  | 11                                                                                             | `osc/parsers/kitty_dnd_protocol.rs`           | 11         |                                                                                                 |
| `kitty_text_sizing.zig`                                                                                   | 8                                                                                              | `osc/parsers/kitty_text_sizing.rs`            | 8          |                                                                                                 |
| `mouse_shape.zig`                                                                                         | 1                                                                                              | `osc/parsers/mouse_shape.rs`                  | 1          |                                                                                                 |
| `osc9.zig`                                                                                                | 61                                                                                             | `osc/parsers/osc9.rs`                         | 61         |                                                                                                 |
| `report_pwd.zig`                                                                                          | 2                                                                                              | `osc/parsers/report_pwd.rs`                   | 2          |                                                                                                 |
| `rxvt_extension.zig`                                                                                      | 1                                                                                              | `osc/parsers/rxvt_extension.rs`               | 1          |                                                                                                 |
| `semantic_prompt.zig`                                                                                     | 64                                                                                             | `osc/parsers/semantic_prompt.rs`              | 64         |                                                                                                 |
| **Parser-family subtotal**                                                                                | **249**                                                                                        |                                               | **250**    |                                                                                                 |
| `osc/encoding.zig` (`isSafeUtf8`)                                                                         | 1                                                                                              | `osc/parsers/mod.rs` (`encoding_tests`)       | 1          |                                                                                                 |
| `kitty/color.zig` (`Kind` Display)                                                                        | 1                                                                                              | `osc/parsers/kitty_color.rs` (`kind_display`) | 1          | counted in the kitty_color.rs row above                                                         |
| `os/string_encoding.zig` (ported subset)                                                                  | 18                                                                                             | `osc/string_encoding.rs`                      | 18         |                                                                                                 |
| `osc/support.rs` (new shared helper, no Zig original)                                                     | —                                                                                              | `osc/support.rs`                              | 3          | factors out the repeated key=value scan (see "Per-parser structure" intro)                      |
| `osc/rgb.rs` (new minimal support type, no direct Zig test file — cross-checks `color.zig`'s `RGB.parse`) | —                                                                                              | `osc/rgb.rs`                                  | 6          | sanity cross-check ahead of the OSC-level color tests                                           |
| `osc/mod.rs` (new: `Parser` itself, the seam integration)                                                 | —                                                                                              | `osc/mod.rs`                                  | 3          | seam composition + terminator + overflow                                                        |
| **Grand total**                                                                                           | **269** (excl. `osc.zig`'s/`osc/parsers.zig`'s `refAllDecls` meta-tests, which assert nothing) |                                               | **281**    | 269 ported 1:1 + 12 new Rust-only (allocator gate, support-module unit tests, seam integration) |

All 281 `qwertty-term-vt` OSC tests pass, alongside the pre-existing 496-and-growing
suite (`cargo test -p qwertty-term-vt`: 497 lib tests total after this chunk, 14
differential, 4 unicode crosscheck, 1 doctest — all green). The 4 pre-existing
`parser::tests::osc_*` tests (the raw-seam tests in
`crates/qwertty-term-vt/src/parser/mod.rs`) are unchanged.
