# VT parser (`src/terminal/Parser.zig`) and UTF-8 decoder (`src/terminal/UTF8Decoder.zig`)

Surveyed against ghostty commit `2da015cd6`.

Ghostty's VT parser is a byte-at-a-time DEC-style state machine, explicitly modeled on Paul
Williams' vt100.net parser (<https://vt100.net/emu/dec_ansi_parser>), with a comptime-generated
transition table (`src/terminal/parse_table.zig`) and a small fixed-size accumulator struct
(`Parser.zig`). It is deliberately dumb: it classifies bytes into actions and collects
params/intermediates; all interpretation (what CSI `H` means, SGR decoding, OSC commands) lives
one layer up in `stream.zig` and the `csi/sgr/osc/dcs/apc` modules. **UTF-8 is not the parser's
job**: `stream.zig` runs a separate `UTF8Decoder` while the parser is in ground state and only
feeds the parser bytes when a control sequence is in flight.

## File map

| File                                 | Role                                                                                  |
| ------------------------------------ | ------------------------------------------------------------------------------------- |
| `src/terminal/Parser.zig` (1.1k)     | Parser struct, `Action` union, `next(u8) → [3]?Action`, param/intermediate collection |
| `src/terminal/parse_table.zig` (389) | Comptime `[256][14]Transition` table (state × byte → next state + transition action)  |
| `src/terminal/UTF8Decoder.zig` (143) | Hoehrmann DFA UTF-8 decoder, error-replacing, non-allocating                          |
| `src/terminal/stream.zig`            | Composes decoder + parser + handler; SIMD fast path lives here                        |
| `src/simd/vt.zig` (+ `vt.cpp`)       | `utf8DecodeUntilControlSeq` — SIMD decode of ground-state bytes (noted, not ported)   |
| `src/terminal/osc.zig`               | `osc.Parser`, embedded in `Parser` (separate upcoming chunk)                          |

## States and the transition table

`Parser.State` (`Parser.zig:15-30`) is exactly Williams' 14 non-anywhere states:
`ground, escape, escape_intermediate, csi_entry, csi_intermediate, csi_param, csi_ignore,
dcs_entry, dcs_param, dcs_intermediate, dcs_passthrough, dcs_ignore, osc_string,
sos_pm_apc_string`.

`parse_table.zig:45-353` builds `table: [256][14]Transition` at comptime, where
`Transition = { state: State, action: TransitionAction }`. Construction order matters and is
load-bearing: the "anywhere" transitions are written first (`parse_table.zig:57-86`), then each
state's block — later writes **overwrite** earlier ones (duplicate detection is commented out,
`parse_table.zig:358-365`). Any (byte, state) cell never written defaults to "stay in the same
state, no action" (`parse_table.zig:345-352`).

Anywhere transitions (`parse_table.zig:57-86`): `0x18`/`0x1A` → ground + execute; C1
`0x80-0x8F`, `0x91-0x97`, `0x99`, `0x9A` → ground + execute; `0x9C` (ST) → ground, no action;
`0x1B` → escape; `0x98`/`0x9E`/`0x9F` (SOS/PM/APC) → sos_pm_apc_string; `0x9B` → csi_entry;
`0x90` → dcs_entry; `0x9D` → osc_string.

### Deviations from Williams' state machine (all deliberate)

1. **`csi_param` accepts `:` (0x3A) as a `param` action** (`parse_table.zig:255`, header
   comment `parse_table.zig:7-9`). Williams routes 0x3A to csi_ignore. Ghostty needs it for
   SGR subparams (`38:2:R:G:B`, `4:3`). Note asymmetry: in **csi_entry** 0x3A still →
   csi_ignore (`parse_table.zig:316`), so a *leading* colon (`ESC [ : ...`) kills the sequence,
   matching Williams.
2. **OSC strings accept raw high bytes**: `osc_string` maps `0x20-0xFF` → osc_put
   (`parse_table.zig:336`), overwriting the anywhere C1 transitions for this state only. So C1
   controls (including 0x9C ST) do **not** terminate or abort an OSC string — window titles are
   UTF-8. An OSC string exits only on BEL 0x07 (→ ground, xterm extension,
   `parse_table.zig:341`), ESC 0x1B (anywhere), or CAN/SUB 0x18/0x1A (anywhere). Bytes
   0x00-0x06, 0x08-0x17, 0x19, 0x1C-0x1F are ignored inside OSC.
3. **SOS/PM/APC all produce APC events**: Williams' sos_pm_apc_string state ignores its
   contents; ghostty emits `apc_start` on entry, `apc_put` per byte, `apc_end` on exit
   (`Parser.zig:274,307`, `parse_table.zig:111-120`) for all three introducers (ESC X, ESC ^,
   ESC \_; 0x98/0x9E/0x9F). Discriminating APC from SOS/PM is downstream's problem (the APC
   handler keys on the first data byte, e.g. kitty graphics `G`). Data bytes `0x00-0x7F`
   (minus the anywhere aborts) are apc_put; **bytes `0xA0-0xFF` are silently dropped** (no
   table entry → default no-action self-transition) and `0x80-0x9F` hit the anywhere C1 rules
   (abort/execute). Kitty graphics payloads are base64/ASCII so this never bites in practice.
4. **No ignore flag; different overflow policies.** Williams sets an "ignore" flag when
   params/intermediates overflow and suppresses the final dispatch. Ghostty instead:
   - Intermediates: max 4 (`MAX_INTERMEDIATE`, `Parser.zig:190` — sized 4 because the array
     doubles as UTF-8 scratch in the C API path); excess intermediates are **silently
     truncated** and the sequence still dispatches (`Parser.zig:313-322`).
   - Params: max 24 (`MAX_PARAMS`, `Parser.zig:203`, raised from 16 for a 17-param Kakoune
     SGR). Overflow **drops the entire CSI/DCS dispatch** (see below), not just the extras.
5. **Ground prints 0x7F (DEL)** (`parse_table.zig:94` range `0x20-0x7F` → print). Williams
   ignores DEL in ground. Every other state ignores 0x7F per Williams (e.g.
   `parse_table.zig:105,130,257`), except dcs_passthrough which also ignores it
   (`parse_table.zig:243`) and sos_pm_apc_string which apc_puts it (range 0x20-0x7F,
   `parse_table.zig:119`).
6. **BEL terminates OSC** (xterm behavior; Williams' table predates it) — `parse_table.zig:341`.

Everything else (escape/escape_intermediate dispatch ranges, csi_entry/param/intermediate/
ignore routing, the dcs_entry → dcs_param/dcs_intermediate/dcs_ignore/dcs_passthrough lattice,
dcs_param 0x3A/0x3C-0x3F → dcs_ignore) matches Williams exactly.

## `next()`: the three-slot action result

`Parser.next(c: u8) → [3]?Action` (`Parser.zig:251-311`) looks up
`table[c][state]` and returns up to three actions **in order**: (1) exit action of the old
state, (2) transition action, (3) entry action of the new state. Exit/entry actions fire only
when the state actually changes.

- **Exit** (`Parser.zig:269-277`): `osc_string` → `osc_dispatch` if the embedded `osc.Parser`
  accepts (`osc_parser.end(c)` — note it receives the terminating byte, so downstream knows
  BEL vs ST for the reply terminator); `dcs_passthrough` → `dcs_unhook`; `sos_pm_apc_string`
  → `apc_end`.
- **Entry** (`Parser.zig:282-309`): `escape`/`dcs_entry`/`csi_entry` → `clear()`
  (reset intermediates/params/accumulator; Williams clears on these same entries);
  `osc_string` → `osc_parser.reset()`; `dcs_passthrough` → emit `dcs_hook` (see DCS below);
  `sos_pm_apc_string` → `apc_start`.
- **Transition** (`doAction`, `Parser.zig:324-407`): maps `TransitionAction` → optional
  `Action`: print/execute pass the byte through; `collect` appends to intermediates;
  `param` accumulates digits/separators; `osc_put` feeds the embedded osc parser;
  `put` → `dcs_put`; `apc_put`; `csi_dispatch`/`esc_dispatch` build the dispatch payloads.

`Action` (`Parser.zig:51-185`) is a tagged union: `print: u21` (the parser itself only ever
emits ASCII prints; `stream.zig` synthesizes full-codepoint prints from the UTF-8 decoder),
`execute: u8`, `csi_dispatch: CSI`, `esc_dispatch: ESC`, `osc_dispatch: osc.Command`,
`dcs_hook: DCS`, `dcs_put: u8`, `dcs_unhook`, `apc_start`, `apc_put: u8`, `apc_end`.
CSI/ESC/DCS payloads hold **slices into the parser's own arrays** — valid only until the next
`next()` call. Nothing allocates.

## Param and intermediate collection

State (`Parser.zig:205-217`): `intermediates: [4]u8` + index; `params: [24]u16` + index;
`param_acc: u16` (current value accumulator); `param_acc_idx: u8` (digit count);
`params_sep: StaticBitSet(24)` (colon flags).

`param` action (`Parser.zig:333-362`), for bytes the table guarantees are `0-9`, `;`, or `:`:

- `;` or `:`: if `params_idx >= 24`, the separator is **silently swallowed** (no store).
  Otherwise store `param_acc` into `params[params_idx]`, set `params_sep` bit at that index
  iff the separator was `:`, advance, reset accumulator. An empty param slot therefore stores
  0 — "empty means 0" falls out of the accumulator starting at 0 (e.g. `ESC [ ; 4 m` yields
  params `[0, 4]`; `58:2::240:...` yields a 0 in the third slot).
- Digit: `param_acc = param_acc *| 10 +| (c - '0')` — **saturating** arithmetic, so params
  clamp at 65535 rather than wrapping. `param_acc_idx` increments with **wrapping** add; on
  overflow (256 digits) it wraps to 0 and processing of that byte stops. Consequence: a param
  written with exactly 256 digits leaves `param_acc_idx == 0`, so the trailing param is *not*
  finalized at dispatch (it looks like "no pending digits"). Pathological but pinned behavior.

**Colon vs semicolon**: the parser does *not* nest subparams. It flattens everything into the
one `params` array and records, per index i, whether the separator *after* param i was a colon
(`params_sep` bit i; `Parser.zig:85-93`). `sgr.zig` re-derives subparam grouping from the bit
set downstream.

`csi_dispatch` (`Parser.zig:367-397`):

1. If `params_idx >= 24` → **whole dispatch dropped** (returns null; state still → ground).
2. If digits are pending (`param_acc_idx > 0`), finalize the trailing param.
3. Build the CSI. Then, **if the final byte is not `m` and any colon separator was seen, the
   dispatch is dropped** (`Parser.zig:387-394`) — colon subparams are SGR-only. (Note the
   check runs even when `intermediates` are present, so e.g. DECRQM-style `ESC [ ? 2026:1 $ p`
   is dropped too. `ESC [ 38:2 h` produces nothing.)

`esc_dispatch` always fires with collected intermediates + final byte.

## DCS hook surface

Transitions into `dcs_passthrough` (from dcs_entry/dcs_param/dcs_intermediate on `0x40-0x7E`)
emit `dcs_hook` as the *entry* action (`Parser.zig:291-306`) carrying intermediates, params,
and the final byte. Params are finalized on entry, with the same overflow rule as CSI: if
`params_idx >= 24`, **the hook is dropped entirely** — but, unlike CSI, the state machine
still enters dcs_passthrough, so subsequent bytes produce `dcs_put` and the eventual exit
produces `dcs_unhook` *without a preceding hook*. Downstream (`stream.zig` → `dcs.zig`)
tolerates unhooked puts. (The bounds check is a fuzz-found regression fix — test
"dcs: too many params", `Parser.zig:1076-1099`.) Inside dcs_passthrough, `0x00-0x7E` (minus
0x18/0x1A/0x1B) are `put`, 0x7F ignored, C1 bytes hit anywhere rules.

## OSC boundary (this chunk's seam)

In ghostty, `Parser` embeds an `osc.Parser` (`Parser.zig:220`, `osc_parser` field): entry to
osc_string calls `.reset()`, each osc_put byte is forwarded (`Parser.zig:363-365`), and exit
calls `.end(terminating_byte)` which returns `?*osc.Command` → `osc_dispatch`. The osc.Parser
is a large sub-state machine (osc.zig + osc/parsers/, ~6.3k LOC, optionally allocating) and is
a **separate upcoming chunk**.

**Decision taken for the Rust port (this chunk): raw OSC byte events.** Instead of embedding
an OSC parser, `Parser` emits `OscStart` (entry), `OscPut(u8)` (per byte), and
`OscEnd(u8)` (exit, carrying the terminating byte exactly as ghostty passes it to
`osc.Parser.end` — 0x07 for BEL, 0x1B/0x18/0x1A otherwise; ghostty's `Terminator.init`
treats 0x07 as BEL and anything else as ST, `osc.zig:263-268`). This mirrors ghostty's own
APC surface (start/put/end) and the DCS surface (hook/put/unhook). The structured port slots
in later by (a) adding an `osc::Parser` field, (b) replacing the three emission sites in
`Parser::next` (entry reset / put forward / end → `OscDispatch(Command)`), and (c) leaving
every state transition untouched — the table and all non-OSC behavior are unaffected. The
stream layer is the only consumer that changes. Rationale: keeps this chunk allocation-free
and dependency-ordered without inventing a speculative trait for a parser that has exactly
one embedding.

Consequence for test porting: ghostty's four OSC tests in Parser.zig assert structured
`osc.Command` values; they are ported against the raw event surface (accumulated bytes +
terminator) with comments pointing back at the Zig originals.

## UTF8Decoder (`src/terminal/UTF8Decoder.zig`)

A Hoehrmann DFA (<http://bjoern.hoehrmann.de/utf-8/decoder/dfa>) modified for error
replacement. State: `accumulator: u21` + `state: u8` (0 = ACCEPT, 12 = REJECT, others =
mid-sequence; two `const` tables: 256-entry char-class `u4` table, 108-entry transition
table).

`next(byte) → { ?u21, bool }` (`Parser.zig`-style tuple, `UTF8Decoder.zig:56-85`): the bool is
"byte consumed". Behavior:

- ACCEPT → emit the completed codepoint (consumed).
- mid-sequence → emit nothing (consumed).
- REJECT → reset to ACCEPT, emit **one** U+FFFD, and report the byte as **not consumed iff
  the rejection happened mid-sequence** (`initial_state != ACCEPT`). The caller must re-feed
  the same byte (it may be a valid lead byte, or 0x1B). An invalid *lead* byte is consumed
  with its FFFD. Re-feeding can reject at most once more (a fresh-state reject always
  consumes), which `stream.zig:727-730` asserts.

The DFA rejects surrogates, overlongs, and > U+10FFFF, so every *emitted* non-FFFD codepoint
is a valid Unicode scalar. This is **not** the Unicode "maximal subparts" policy: a truncated
3-byte prefix yields one FFFD, but e.g. `F0 9F` + `F0` yields FFFD then restarts on the second
`F0` — equivalent in FFFD counts to maximal-subparts for most inputs but not all; the SIMD
path (below) implements maximal-subparts exactly, and ghostty accepts the discrepancy between
paths (scalar path is the debug/tail path).

### How the decoder and parser compose (`stream.zig`, for context)

`stream.nextSlice` (`stream.zig:494-521`): while the parser is in **ground**, bytes go to the
UTF8Decoder (`nextUtf8`, `stream.zig:713-735`); emitted codepoints are handled by
`handleCodepoint` (`stream.zig:741-761`): `<= 0x0F` → execute, `== 0x1B` → *manually* set
parser state to escape + `clear()` (bypassing the table), else print. When the parser is
*not* in ground, raw bytes go to `parser.next()` (`nextNonUtf8`). So C1 bytes 0x80-0x9F never
reach the parser from ground state (they arrive as decoded codepoints > 0x0F → print, or as
FFFD); the anywhere C1 transitions only fire mid-sequence. The stream also asserts the decoder
is never mid-sequence when a control sequence is in flight (`stream.zig:531-537` drains the
decoder first).

### SIMD hook (noted, not ported)

`stream.zig:500-560`: when in ground state, `simd.vt.utf8DecodeUntilControlSeq` (extern C++
`ghostty_simd_decode_utf8_until_control_seq`, `src/simd/vt.zig:7-13`, `vt.cpp`) bulk-decodes
UTF-8 into a 4096-codepoint buffer until it hits 0x1B, with a scalar
maximal-subparts fallback (`utf8DecodeUntilControlSeqScalar`, `src/simd/vt.zig:39-120`) when
built without SIMD. Truncated sequences at the chunk end are left unconsumed for the next
call. This is a Phase-7 perf item per the rewrite prompt (`memchr`/`simdutf`/`std::simd`);
the Rust port's stream layer starts with the scalar decoder path only.

## Inline tests (conformance anchors)

- `Parser.zig`: **25** `test` blocks — anywhere/APC state walk + print/execute (1), ESC
  dispatch with intermediate (1), CSI: no params / two params / colon subparams ×7 (incl. two
  captured Kakoune SGRs, empty subparam slots, colon-for-non-m dropped), DECRQM double
  intermediate, cursor-style intermediate, param-overflow ×3 ("too many params", "up to max",
  "beyond max drops"), OSC ×4 (title BEL, title ESC-terminated, OSC 112 incomplete, OSC 104
  empty — structured, see seam note), DCS ×3 (XTGETTCAP hook, params, too-many-params
  regression).
- `UTF8Decoder.zig`: **3** tests — ASCII passthrough, well-formed 1-4 byte sequences,
  partially-invalid stream (`F0 9F` + emoji + surrogate `ED A0 80` → `FFFD, 1F604, FFFD,
  FFFD, FFFD`).
- `parse_table.zig`: **1** test (table comptime-builds).

## Port notes (Rust: `crates/qwertty-term-vt/src/parser/`, `src/utf8_decoder.rs`)

- The table is built by a `const fn` mirroring `genTable` (same write order, same
  overwrite-last-wins semantics), stored as a `static [[Transition; 14]; 256]`.
- `Action<'a>` borrows the parser's arrays exactly like the Zig slices; Rust's borrow checker
  enforces ghostty's "valid until next `next()`" comment at compile time. `next()` performs
  all mutations first (transition action, then entry bookkeeping — same order as Zig, which
  matters because entry `clear()` must not precede a dispatch borrow), then materializes the
  three `Option<Action>` values.
- `print` is `char` (not u21): the parser itself only emits ASCII prints, and the decoder's
  DFA only accepts valid scalars, so `char` is lossless here. The C1 range that Zig's u21
  would admit is unreachable from both sources.
- Saturating (`*|`, `+|`) and wrapping (`@addWithOverflow`) param arithmetic are ported
  bit-for-bit, including the 256-digit `param_acc_idx` wrap.
- `Parser::state`/`set_state`/`clear` are public because `stream.zig` pokes them
  (`handleCodepoint` ESC fast path; `consumeUntilGround`).
- `vte` is a dev-dependency **only** (differential oracle); divergences are documented in
  `crates/qwertty-term-vt/tests/parser_vte_differential.rs` rather than forced to parity.
