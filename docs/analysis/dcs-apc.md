# DCS and APC handlers (`src/terminal/dcs.zig`, `src/terminal/apc.zig` + `apc/`)

Surveyed against ghostty commit `2da015cd6`.

Both handlers are pure consumers of the parser's hook/put/unhook (DCS) and
start/put/end (APC) events documented in `docs/analysis/vt-parser.md` ("DCS hook surface",
deviation #3 for APC/SOS/PM). Neither handler touches the state machine; each is a small,
independent `Handler` struct with its own internal state enum, fed byte-by-byte and
producing a `Command` union on completion.

## `dcs.zig` (430 lines) — `Handler`

### Lifecycle

`Handler.hook(alloc, dcs: DCS) ?Command` (`dcs.zig:25-43`) is called on `dcs_hook`
(`DCS { intermediates: []const u8, params: []const u16, final: u8 }`, defined at
`Parser.zig:124-136`). It asserts `state == .inactive` on entry (the parser guarantees
unhook happens before the next hook), sets `state = .ignore` as a fail-safe default, then
calls `tryHook` to classify the command. On success `state` becomes the command-specific
active state; on failure (`error` or unrecognized) it stays `.ignore` and `hook` returns
`null`. `hook` itself only ever returns a `Command` for the tmux-enter case (see below) —
every other command produces its result at `unhook`, not `hook`.

`Handler.put(byte) ?Command` (`dcs.zig:114-122`) forwards to `tryPut`; on error it discards
state (`state.deinit()`) and drops to `.ignore`, silently absorbing the rest of the DCS
string. Only the tmux state can produce a `Command` mid-stream (each control-mode line is
a command); the other two states just buffer.

`Handler.unhook() ?Command` (`dcs.zig:157-199`) finalizes: builds the `Command` for the
buffered state, then unconditionally resets `state = .inactive` (via `defer`). It
deliberately does **not** call `state.deinit()` in the general case — ownership of the
buffered memory (e.g. the XTGETTCAP `Writer.Allocating`) transfers into the returned
`Command`, which the caller must `deinit`.

### Commands enumerated (exactly 3, by `dcs.intermediates`/`dcs.final`, `tryHook` at
`dcs.zig:50-110`)

| Intermediates | Final | Params | Command | Notes |
|---|---|---|---|---|
| (none) | `p` | must be exactly `[1000]` | **Tmux control mode enter** | Gated by `build_options.tmux_control_mode` (compiled in whenever oniguruma is available — i.e. normal builds; see below). `ESC P 1000 p`. |
| `+` | `q` | any | **XTGETTCAP** | `ESC P + q <hex-encoded-names> ESC \` |
| `$` | `q` | any | **DECRQSS** | `ESC P $ q <setting> ESC \` |

Anything else (0 intermediates + any other final; 1 intermediate that isn't `+`/`$`; 2+
intermediates) → `tryHook` returns `null` → `state = .ignore`, `hook` returns `null`. This
matches the "unknown DCS command" test (`dcs.zig:298-308`, hooks final `A` with zero
intermediates).

#### XTGETTCAP (`dcs.zig:83-90, 136-142, 173-179, 228-248`)

State: `std.Io.Writer.Allocating` (growable byte buffer, initial capacity 128). `put`
appends the raw byte, checked against `self.max_bytes` (see limits below) — no
transformation at put time. `unhook` **upper-cases every buffered byte in place**
(`std.ascii.toUpper`, `dcs.zig:177`) — ghostty always emits/interprets XTGETTCAP names as
the hex-encoded uppercase form regardless of what case the client sent — then moves the
buffer into `Command.XTGETTCAP { data, i: usize = 0 }`.

`Command.XTGETTCAP.next()` (`dcs.zig:235-247`) is a semicolon-delimited iterator over the
buffered bytes: splits on `;`, returns each segment (NOT hex-decoded — caller does a
comptime lookup table keyed by the raw hex string), advances past the separator, returns
`null` once exhausted. Segments are returned exactly as buffered (post-uppercasing), so
`"who"` → `"WHO"` even though `"WHO"` isn't valid hex — decoding is entirely the caller's
problem; `next()` is a pure splitter.

Tests pin: single key (`"536D756C78"`, hex for `"Smulx"`), mixed-case input normalized to
upper on read-out, multiple `;`-separated keys, and garbage input (`"who"` → `"WHO"`,
still split correctly) — `dcs.zig:310-370`.

#### DECRQSS (`dcs.zig:96-103, 144-151, 181-197`)

State: fixed 2-byte buffer (`data: [2]u8 = undefined, len: u2 = 0`) — no allocation.
`put` appends while `len < 2`; a 3rd+ byte is an `error.OutOfMemory` (caught by the
`put` wrapper, which discards to `.ignore`). `unhook` maps the buffered bytes to a
`DECRQSS` enum by **exact length and content** — this is a fixed dispatch table, not
generic parsing:

| `len` | bytes | Result |
|---|---|---|
| 0 | — | `.none` |
| 1 | `m` | `.sgr` |
| 1 | `r` | `.decstbm` |
| 1 | `s` | `.decslrm` |
| 1 | other | `.none` |
| 2 | `" q"` (space then `q`) | `.decscusr` |
| 2 | other | `.none` |
| ≥3 | — | `unreachable` (buffer physically caps at 2; the `put` overflow guard makes this unreachable in practice) |

So DECRQSS in ghostty currently recognizes exactly 4 settings: SGR (`m`), DECSTBM (`r`),
DECSLRM (`s`), DECSCUSR (`" q"`). Anything else hooks successfully (the DCS is
well-formed) but reports `.none` at unhook — the terminal still consumes the sequence,
it just has nothing to say back. Test `"DECRQSS invalid command"` (`dcs.zig:386-406`)
pins both an unrecognized 1-byte setting (`z` → `.none`) and the overflow path (`" q"` is
exactly 2 bytes and fits — wait, re-check: the test sends `'"'`, then `' '`, then `'q'` —
3 bytes into a 2-byte buffer, so the 3rd `put('q')` overflows to `error.OutOfMemory` →
discard → `.ignore` → `unhook()` returns `null`). This is the "3rd put silently kills the
whole DCS" behavior, exercised directly.

#### Tmux control mode (`dcs.zig:53-75, 130-134, 168-171, 408-430`)

Gated by `build_options.tmux_control_mode`, which `build_options.zig:75` sets equal to
`oniguruma` availability — i.e. **compiled in on ordinary builds** (oniguruma absent only
in exotic/minimal builds), not a niche feature. `hook` requires params `== &.{1000}`
exactly (`ESC P 1000 p`, no other params, no intermediates) and, uniquely among the three
commands, **returns a `Command` immediately from `hook`** (`.tmux = .enter`) rather than
waiting for `unhook`. State becomes `terminal.tmux.ControlParser` (a full line-oriented
parser for tmux's control-mode protocol, `src/terminal/tmux/control.zig`, 839 lines —
part of a 4.35k-line `tmux/` subsystem: `control.zig` + `viewer.zig` (2.28k) +
`output.zig` (638) + `layout.zig` (638) implementing a tmux *client*, not just a
tokenizer). Each `put` forwards to `tmux.put(byte)`, which returns a
`?terminal.tmux.ControlNotification` whenever a complete control-mode line/event is
parsed (arbitrarily many `Command`s can be produced across one DCS session — the DCS
stays hooked for the entire tmux session). `unhook` deinits the tmux parser and returns
`.tmux = .exit`.

**Seam decision**: the tmux control-mode *client* (control.zig/viewer.zig/output.zig/
layout.zig) is out of scope for this chunk — it is a large, independent subsystem (state
sync with a real tmux server: panes, windows, layout parsing, output multiplexing) that
happens to ride in on a DCS hook, analogous to kitty graphics riding in on APC. See "Seam
design" below.

### Buffer limits (`dcs.zig:16-19, 137-142, 145-151`)

`Handler.max_bytes: usize = 1024 * 1024` (1 MiB), applied only to XTGETTCAP
(`list.written().len >= self.max_bytes` before each byte is written — checked BEFORE
append, so the buffer never exceeds the limit; overflow is `error.OutOfMemory`, caught by
`put`'s wrapper → discard → `.ignore`). DECRQSS has its own fixed 2-byte cap, not governed
by `max_bytes`. Tmux control mode has no `max_bytes` enforcement in `dcs.zig` itself (the
`ControlParser` manages its own buffering — out of scope here).

### Error handling shape

Both `hook` and `put` are non-throwing at the call boundary: internal errors
(unrecognized command, allocation failure, buffer overflow) are caught inside the public
function and converted to `state = .ignore` + `null`/no-op, never propagated. This means
a malformed or oversized DCS sequence degrades to "silently ignored, rest of the bytes
absorbed until unhook" rather than erroring the whole stream — the same policy the parser
itself uses for CSI param overflow.

### Inline tests: 8

`"unknown DCS command"`, `"XTGETTCAP command"`, `"XTGETTCAP mixed case"`,
`"XTGETTCAP command multiple keys"`, `"XTGETTCAP command invalid data"`,
`"DECRQSS command"`, `"DECRQSS invalid command"` (two DCS sessions in one test — the
`.none` case and the overflow case), `"tmux enter and implicit exit"` (skipped when
`tmux_control_mode` is off — always-on given oniguruma is the norm).

---

## `apc.zig` (402 lines) + `apc/` — `Handler`

### Lifecycle

`Handler.start()` (`apc.zig:34-37`) — called on `apc_start` — deinits any stale state
(defensive; should already be `.inactive` after a prior `end`) and resets to
`.identify{ len: 0, buf: undefined }`.

`Handler.feed(alloc, byte)` (`apc.zig:45-114`) — called on `apc_put`, once per byte —
dispatches on current state. `.inactive` is `unreachable` (must `start()` first).
`.ignore` drops the byte with no work. `.identify` runs the protocol-sniffing state
machine (below). Once a protocol is identified, subsequent bytes forward to that
protocol's own `feed`, which can itself fail and transition to `.ignore` (state is
deinited first to avoid leaks — see the "kitty feed error deinits parser" test).

`Handler.end() ?Command` (`apc.zig:116-147`) — called on `apc_end` — always deinits and
resets to `.inactive` (via `defer`), and returns a `Command` only from the two identified
protocol states (`.kitty`, `.glyph`); `.inactive`/`.ignore`/`.identify` all return `null`
(reaching `end` while still in `.identify` means the sequence was too short to identify
or never hit its terminator condition — not an error, just nothing to do).

### Protocol identification (`apc.zig:150-170`, "identify" state)

The **first byte** discriminates:

1. **`G` as the very first byte** (`id.len == 0 and byte == 'G'`, gated by
   `build_options.kitty_graphics` which is unconditionally true except on
   `wasm32-freestanding`) → immediate transition to kitty graphics, *no* buffering, *no*
   terminator needed — kitty graphics commands begin parsing right after the `G`
   (`ESC _ G <kitty-encoded-command> ESC \`).
2. Otherwise, bytes accumulate into a 4-byte `id.buf` until a `;` is seen. On `;`, the
   accumulated prefix is compared against known protocol identifiers:
   - `"25a1"` (exactly, case-sensitive, ASCII digits/letters as given — hex for U+25A1
     WHITE SQUARE, the "tofu" glyph) → **Glyph protocol**
     (`ESC _ 25a1;<verb>...`).
   - anything else → `.ignore`.
3. If the 4-byte identify buffer fills before a `;` (or before a `G`-fast-path applies),
   state becomes `.ignore` (`id.len >= id.buf.len`, `apc.zig:91-94`) — so no identifier
   longer than 4 bytes is or ever will be supported without changing `identify.buf`'s
   size.

Each protocol can also be individually disabled at runtime via `Handler.enable(protocol,
bool)` (backed by `std.EnumSet(Protocol)`, default all-enabled): a disabled protocol's
matching prefix is treated exactly like an unrecognized one (`.ignore`), even though the
identifier matched — checked via `self.enabled.contains(...)` at both the `G` fast path
and the `;`-terminated path.

### Protocols enumerated (exactly 2 native + 1 seamed)

| Identifier | Protocol enum | Parser type | Max bytes (default) | Notes |
|---|---|---|---|---|
| `G` (first byte) | `.kitty` | `kitty_gfx.CommandParser` (`kitty/graphics_command.zig`, part of a 6.3k-line `kitty/` subsystem) | 65 MiB | Chunked image transfer; **out of scope, sibling chunk owns `crates/ghostty-vt/src/kitty/`** |
| `25a1;` | `.glyph` | `glyph.CommandParser` (`apc/glyph/request.zig`) | 1 MiB | Custom-glyph registration protocol (Private-Use-Area glyf/COLR outlines); depends on `font/Glyph.zig` + `font/opentype/glyf.zig` — **out of scope, font subsystem not yet ported** (see Seam design) |

`Protocol.defaultMaxBytes` (`apc.zig:199-209`) hard-codes these two values; `max_bytes` on
`Handler` is a `std.EnumMap(Protocol, usize)` overridable per-protocol
(`Handler{ .max_bytes = .init(.{ .kitty = 4 }) }` in the test at `apc.zig:301`).

### Glyph protocol detail (`apc/glyph.zig` + `apc/glyph/{request,response,execute,Glossary}.zig`, ~2.18k lines total)

Substantial and font-coupled — enumerated here for completeness, but **seamed, not
ported** (see below). Wire format (documented in `apc/glyph/request.zig:1-149`):
`ESC _ 25a1 ; <verb> [ ; key=value ]* [ ; <payload> ] ESC \` with four verbs:

- `s` — support query (advertises `fmt=glyf,colrv0,colrv1`).
- `q` — codepoint coverage query (`cp=<hex>` → `status=system,glossary` list).
- `r` — register a glyph outline at a PUA codepoint (`cp`, `fmt`, `reply`, `upm`, `aw`,
  `lh`, `width`, `size`, `align`, `pad`, then a base64 payload; a full TrueType simple-glyf
  decoder lives in `Glossary.zig`/`font/opentype/glyf.zig`).
- `c` — clear one or all registrations.

`CommandParser` (`apc/glyph/request.zig:11-52`) itself is a thin byte-accumulator
(`feed`/`complete`) very similar in shape to XTGETTCAP's buffer — the complexity is all
in `complete`'s parsing (option grammars, base64/glyf decode) and in `execute.zig`/
`Glossary.zig` (a 1024-entry FIFO-eviction glossary keyed by codepoint, PUA-range
validation, font glyph outline storage). This is real terminal-state (`Glossary` is meant
to be held on the `Screen`/`Terminal`, not just the APC handler) plus font-format
decoding — a full vertical slice through a layer (fonts) this chunk must not touch.

### Buffer limits

`identify` state: fixed 4-byte inline buffer, no allocation, overflow → `.ignore`
(`apc.zig:91-94`). Kitty/glyph protocols each carry their own `max_bytes` (default 65 MiB
/ 1 MiB respectively) enforced inside their own `feed` (kitty: `graphics_command.zig`,
out of scope; glyph: `apc/glyph/request.zig`, out of scope) — the APC `Handler` itself
does not re-check these; it only supplies the configured limit at construction time and
reacts to the sub-parser's error by discarding to `.ignore` (`apc.zig:100-112`,
`"kitty max bytes exceeded"` test at `apc.zig:295-312` — note the test's off-by-one-ish
boundary: `max_bytes = 4`, 4 bytes of data are accepted, the 5th byte trips the limit).

### Error handling shape

Same policy as DCS: sub-parser errors during `feed` are caught, the sub-parser is
`deinit`ed to avoid leaks, and state drops to `.ignore` — never propagated to the caller.
`end()` on a still-`.ignore`/`.identify` state is not an error, just `null`.

### Inline tests: 15

`"unknown APC command"`, `"garbage Kitty command"` (skip if no kitty_graphics),
`"Kitty command with overflow u32"` (skip), `"Kitty command with overflow i32"` (skip),
`"kitty feed error deinits parser"` (skip), `"kitty max bytes exceeded"` (skip), `"valid
Kitty command"` (skip), `"identify with unrecognized command"`, `"identify buffer
overflow"`, `"identify with no input"`, `"identify with unknown partial input"`,
`"garbage glyph command"`, `"valid glyph command"`, `"disabled glyph command is
ignored"`. (14 unconditional + the tmux-analogous "6 kitty tests" are gated on
`kitty_graphics`, which is compiled in on every real target, so in practice all 15 run.)

---

## Seam design (CRITICAL — read before touching `apc.rs`)

Three sub-consumers are deliberately **not ported** in this chunk because they belong to
other subsystems/chunks or unported phases:

1. **Kitty graphics** (`kitty_gfx.CommandParser`/`Command`) — owned by the sibling
   `crates/ghostty-vt/src/kitty/` chunk running concurrently.
2. **Glyph protocol** (`apc/glyph/*`) — depends on the font subsystem
   (`font/Glyph.zig`, `font/opentype/glyf.zig`), which is Phase 3, not yet ported at all.
3. **Tmux control mode** (`terminal.tmux.*`) — a 4.35k-line tmux *client*, independent of
   both DCS parsing and the VT core; no other chunk currently owns it, so it's flagged as
   fully deferred (not even stubbed elsewhere).

The Rust port models this as a **narrow seam trait per sub-protocol**, so the identify
logic (which bytes select which protocol, buffer limits, error→ignore policy) is ported
faithfully now, while the actual command semantics slot in later:

- `apc::GraphicsProtocol` trait (in `crates/ghostty-vt/src/apc/mod.rs`, `TODO(chunk:
  kitty-gfx)`): `fn feed(&mut self, byte: u8) -> Result<(), ()>` and
  `fn complete(self: Box<Self>) -> Result<KittyRaw, ()>` where `KittyRaw` is presently
  just the raw accumulated byte payload (`Vec<u8>`) behind a placeholder type — the kitty
  chunk replaces `KittyRaw` with its real `Command` type and provides the trait impl.
  `Handler` is generic/boxed over "does something exist for `G`" — concretely, since
  `ghostty-vt` cannot depend on a not-yet-written sibling module without a build-order
  dependency, the seam is done as **the handler yielding the raw payload bytes on `G`
  identification**, i.e. `Handler`'s `.kitty` state is *just* a byte buffer
  (`Vec<u8>` + `max_bytes` check, mirroring the XTGETTCAP buffer exactly), and `Command`
  carries `Command::KittyRaw(Vec<u8>)` instead of a real parsed command. Integration
  swaps this for a call into the kitty chunk's real parser at merge time (mechanical:
  replace the buffer-push in `feed` with a call to the sibling's incremental parser, and
  the raw-bytes-return in `complete`/`end` with a call to its `complete`). This keeps
  identify-time behavior (the `G` fast path, `max_bytes` enforcement, error→ignore) fully
  tested today against real kitty graphics APC byte streams (all 6 kitty tests port
  against the raw-buffer stand-in, asserting buffer contents / max_bytes-exceeded
  behavior instead of parsed command fields).
- **Glyph protocol**: same shape, `Command::GlyphRaw(Vec<u8>)`, `TODO(chunk: font/glyph-
  protocol)`. Not a live chunk right now, so this is a pure placeholder with no assigned
  consumer yet — flagged in the port-status ledger for whoever picks up the font phase.
- **Tmux control mode**: DCS `Command::TmuxRaw(TmuxEvent)` where `TmuxEvent` is `Enter`,
  `Line(Vec<u8>)` (one accumulated control-mode protocol line, newline-delimited per
  tmux's own framing — `ControlParser.put` in the Zig source parses line-by-line and
  yields a `ControlNotification` per complete line; the seam accumulates raw lines
  instead of parsing tmux's notification grammar), `Exit`. `TODO(chunk: tmux-control-
  mode)`, unassigned.

This mirrors the OSC seam precedent set in `docs/analysis/vt-parser.md` ("OSC boundary"):
raw byte/line accumulation now, structured parsing later, with the exact hook points
commented for the future chunk.
