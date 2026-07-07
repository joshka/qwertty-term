# Stream dispatch layer (`stream.zig` + `stream_terminal.zig`)

Surveyed and ported against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse HEAD`). The Rust port lives in
`crates/ghostty-vt/src/stream.rs` (+ `stream/tests.rs`) and the differential
oracle in `crates/vt-diff/src/rust_engine.rs`.

`stream.zig` (3.7k) composes the [`UTF8Decoder`](vt-parser.md) and
[`Parser`](vt-parser.md) and routes every parser `Action` through
`csiDispatch`/`escDispatch`/`oscDispatch`/`execute` into a `comptime Handler`
via one `handler.vt(action, value)` call. `stream_terminal.zig` (2.2k) is the
concrete handler that maps those actions onto `Terminal` methods and drives
query replies out through effect callbacks (`writePty`, `bell`, …).

## How stream routes Parser actions to a handler (Zig comptime → Rust trait)

Ghostty's `Stream(Handler)` is generic over a `comptime Handler` type and calls
`handler.vt(comptime action: Action.Tag, value: Action.Value(action))`. The
`Action.Tag` is a comptime enum selecting both the operation and its value
type; Zig monomorphizes the whole thing into a switch inside
`stream_terminal.vtFallible` (`stream_terminal.zig:133-296`).

Rust has no comptime-tag-indexed value types, so **the port splits the single
`vt` into one trait method per operation family** (`Handler::print`,
`cursor_up`, `erase_display`, `set_mode`, `device_status`, …). This keeps the
*routing* identical — `Stream::csi_dispatch`/`esc_dispatch`/`osc_dispatch`/
`execute` mirror the Zig functions 1:1 — while letting each handler method carry
a normally-typed value. `stream_terminal.Handler` becomes `TerminalHandler`, the
concrete impl over a `Terminal`. **Justification:** a trait-per-op is the direct
Rust analogue of the comptime-tag interface, gives partial handlers (test spies)
free no-op defaults, and keeps every dispatch decision (which final byte + which
intermediates → which op, with which default params) in the stream where
upstream keeps it, so the two dispatch tables stay diffable line-for-line.

The borrow twist: `Parser::next` returns `[Option<Action>; 3]` whose CSI/ESC/DCS
payloads borrow the parser's arrays (valid until the next `next()`, exactly like
the Zig slices). Calling `&mut self` handler methods while holding that borrow is
illegal in Rust, so `next_non_utf8` first converts each action into an owned
`Emitted` (copying the ≤4 intermediates / ≤24 params into small `Vec`s), which is
behavior-equivalent to Zig's borrow-until-next-call contract, then dispatches.

## Fast-path scalar/SIMD split (note only)

`stream.zig`'s `nextSlice` has (a) a SIMD `utf8DecodeUntilControlSeq` bulk path
that decodes ground-state UTF-8 into a 4096-codepoint buffer, and (b)
hand-inlined `csi_entry`/`csi_param` fast paths (`stream.zig:781-864`) that
dispatch CSIs *without* going through `Parser.next`. Both are behavior-equivalent
throughput optimizations (Phase-7 perf item per the rewrite prompt). **The port
implements only the scalar path**: ground-state bytes go through the
`Utf8Decoder`; non-ground bytes go through `Parser::next`; the CSI fast paths are
omitted because `Parser::next` already produces identical actions (the fast paths
carry a `csiDispatchFinal` that duplicates the parser's finalize logic exactly).
The differential suite confirms this produces identical screen state.

## C0 handling (`execute`, `stream.zig:957-987`)

C1 (`c > 0x7F`) is re-dispatched as `ESC (c-0x40)`. Otherwise the C0 switch:
NUL/SOH/STX ignored; `ENQ`→enquiry, `BEL`→bell, `BS`→backspace, `HT`→tab(1),
`LF/VT/FF`→linefeed, `CR`→carriage_return, `SO`→invoke GL=G1, `SI`→invoke GL=G0;
everything else ignored. Ported 1:1 in `Stream::execute`.

## CSI dispatch table (`csiDispatch`, `stream.zig:989-2150`)

Enumerated below — **implemented** = wired to a `Terminal` method through
`TerminalHandler`; **seam** = routed to a handler method that is a documented
no-op in this chunk; **not modeled** = the stream simply doesn't route it (the
dispatch prong is a no-op, matching upstream's "log and ignore").

| Final | Op | Status |
|---|---|---|
| `A`/`k` | CUU cursor up | implemented |
| `B` | CUD cursor down | implemented |
| `C` | CUF cursor right | implemented |
| `D`/`j` | CUB cursor left | implemented |
| `E` | CNL (down + CR) | implemented |
| `F` | CPL (up + CR) | implemented |
| `G`/`` ` `` | HPA cursor col | implemented |
| `H`/`f` | CUP set cursor pos | implemented |
| `I` | CHT horizontal tab | implemented |
| `J` | ED erase display (+ `?` protected) | implemented |
| `K` | EL erase line (+ `?` protected) | implemented |
| `L` | IL insert lines | implemented |
| `M` | DL delete lines | implemented |
| `P` | DCH delete chars | implemented |
| `S` | SU scroll up | implemented |
| `T` | SD scroll down | implemented |
| `W` | CTC tab set/clear/reset | implemented |
| `X` | ECH erase chars | implemented |
| `Z` | CBT tab back | implemented |
| `@` | ICH insert blanks | implemented |
| `a` | HPR col relative | implemented |
| `b` | REP repeat previous char | **not modeled** (needs `previous_char`; fixtures/diff don't use it) |
| `c` | DA1/DA2/DA3 device attributes | implemented (reply) |
| `d` | VPA cursor row | implemented |
| `e` | VPR row relative | implemented |
| `g` | TBC tab clear | implemented |
| `h`/`l` | SM/RM set/reset mode (ansi + `?` private) | implemented |
| `m` | SGR (via `sgr::Parser`) | implemented; `>` XTMODKEYS form **not modeled** |
| `n` | DSR / CPR (+ `?`) | implemented (reply); `>` modify-key form not modeled |
| `p` | DECRQM request mode (`$` / `?$`) | implemented (reply) |
| `q` (space) | DECSCUSR cursor style | implemented (blink mode only; style rendering is a screen concern) |
| `q` (`"`) | DECSCA protected mode | implemented |
| `q` (`>`) | XTVERSION | implemented (reply) |
| `r` | DECSTBM margins / `?` restore-mode | implemented |
| `s` | DECSLRM margins / `?` save-mode / SC-ambiguous | implemented |
| `t` | XTWINOPS size reports / title push-pop | **seam** (no-op tail; fixtures/diff don't use it) |
| `u` | DECRC (no intermediate) | implemented; kitty-keyboard forms are a **seam** |
| `}` (`$`) | DECSASD active status display | implemented |
| others | — | ignored (matches upstream `log.warn` + return) |

Kitty-keyboard (`u` with `?`/`>`/`<`/`=`), XTSHIFTESCAPE (`s >`), and XTWINOPS
title push/pop are **seams** — the stream doesn't route them (they touch state
this chunk's `Terminal` doesn't own: `kitty_keyboard`, `mouse_shift_capture`,
a title stack). They are the same seams upstream keeps in
`stream_terminal.zig` (`.kitty_keyboard_*`, `.mouse_shift_capture`,
`.title_push`/`.title_pop`).

## ESC dispatch table (`escDispatch`, `stream.zig:2312-2582`)

All implemented 1:1 (charset designations `B`/`A`/`0`; `7` DECSC; `8` DECRC +
`#8` DECALN; `D` IND; `E` NEL; `H` HTS; `M` RI; `N`/`O` SS2/SS3; `V`/`W` SPA/EPA
protected; `Z` DECID; `c` RIS; `n`/`o` LS2/LS3; `~`/`}`/`|` LS1R/LS2R/LS3R;
`=`/`>` application/normal keypad; `\` ST no-op).

## OSC / DCS / APC routing

- **OSC**: `Action::OscStart/OscPut/OscEnd` feed the structured `osc::Parser`
  (allocator-permitting, so OSC 4/52 don't spuriously invalidate); `end()` →
  `osc::Command` → `osc_dispatch`. Implemented: `SemanticPrompt` (OSC 133 → the
  ported `Terminal::semantic_prompt`), `ChangeWindowTitle` (0/2),
  `ReportPwd` (7), `MouseShape` (22, stored), `ColorOperation`
  (4/5/10-19/104/110-119 palette + fg/bg/cursor dynamic set/reset). `KittyColor`
  (21), hyperlinks (8), and all conemu/notification/clipboard/kitty-text/dnd/
  context-signal commands are **seams / no-effect** (exactly the set upstream's
  `oscDispatch` `log.debug`-ignores or routes to Screen-level effects).
- **DCS**: `Action::DcsHook/DcsPut/DcsUnhook` feed the ported `dcs::Handler`;
  only `Decrqss` produces a terminal-visible reply (SGR answered from
  `Terminal::print_attributes`; DECSTBM/DECSLRM answered from the scrolling
  region; XTGETTCAP/tmux are seams). Matches upstream, which ignores
  `dcs_hook`/`put`/`unhook` for terminal state.
- **APC**: `Action::ApcStart/ApcPut/ApcEnd` route to `Handler::apc_*`; the
  concrete `TerminalHandler` leaves them as the kitty-graphics / glyph **seam**
  (the raw-buffer `apc::Handler` exists but its exec glue into kitty storage is
  `TODO(chunk:kitty-gfx)`).

## Replies (DSR/DA/CPR/DECRQSS) — output-queue design

Upstream surfaces replies through effect callbacks (`writePty(data)`), which the
embedder wires to the pty. The spike accumulated replies into a buffer. The port
follows the spike: `TerminalHandler` owns a `pub output: Vec<u8>` reply queue;
every report method (`device_attributes`, `device_status` CPR, `request_mode`,
`decrqss`, `xtversion`) formats its bytes and pushes them, in order.
`take_output()` drains it. This keeps the layer sync + allocation-light and lets
the differential harness (which compares *screen text + cursor*, not replies)
ignore them while still exercising the reply path in unit tests
(`cpr_reply`, `decrqss_sgr_reply`, `da_primary_reply`). CPR honors origin mode
exactly as upstream (`deviceStatus`, `stream_terminal.zig:325-359`).

## Terminal methods added this chunk

`Terminal::resize`, `deccolm` (mode-3-gated 80/132 switch), `semantic_prompt`
(OSC 133 dispatch, ported from `Terminal.semanticPrompt` incl.
`semantic_prompt_fresh_line`), `cursor_is_at_prompt`, and `print_attributes`
(DECRQSS SGR reply body, port of `printAttributes`).

## Zig-vs-Rust test counts

| Source | Zig tests | Rust port | Notes |
|---|---|---|---|
| `stream.zig` | 38 | 18 dispatch-routing (spy) + 2 print | Ported the portable subset (cursor/mode/erase/DECSCUSR/DECSCA/insert/SCORC/tab/SGR/print/invalid-utf8). Skipped: `test Action` (C-ABI meta), 2 SIMD-path tests (perf path not ported), and the kitty-keyboard / XTWINOPS-title-push-pop / XTSHIFTESCAPE prong tests (seams). |
| `stream_terminal.zig` | 65 | 13 integration (`TerminalHandler`) | Ported the color (OSC 4/104/10/11/12), query-ignore, title/pwd, and reply (CPR/DECRQSS/DA) tests. Skipped: kitty-graphics/glyph-APC exec tests, kitty-keyboard, mouse, and title-stack tests (all seams). |
| **stream/tests.rs total** | — | **33** | + 3 fixture replays against the Rust engine (`fixture_*`). |
| vt-diff `rust_engine.rs` | — | 3 | Rust oracle standalone (hello/empty/3 fixtures). |
| vt-diff `differential.rs` | — | 9 | Rust-vs-reference (fixtures + 8 hand streams), `--features reference`. |

The unported Zig tests exercise seam subsystems (kitty graphics/glyph, kitty
keyboard, tmux, mouse, title stack) or the SIMD fast path — all of which produce
no terminal-state effect this chunk owns, so their behavior is already covered by
the differential suite where it matters.

## Differential results

`cargo test -p vt-diff --features reference` — **all green, zero divergences**:

- **3 replay fixtures** (`prompt_and_color`, `alternate_screen_roundtrip`,
  `wide_text_and_resize`) — identical screen text + cursor Rust vs Zig.
- **8 hand-written streams**: wrap (soft-wrap past right edge), scroll region
  (DECSTBM + linefeeds), alt-screen (1049 enter/leave), SGR (bold/256/rgb/
  underline), wide CJK chars, cursor moves (absolute+relative+erase), tabs+erase,
  insert/delete chars. All identical.

No deliberate divergences. The three fixtures also pass identically against the
Rust engine alone (`cargo test -p ghostty-vt --lib stream`) and the reference
(`cargo test -p vt-diff --features reference tests::smoke`).

## Remaining seams

1. **Kitty graphics / glyph APC exec** — `apc::Handler` buffers raw payloads; the
   glue into kitty storage is `TODO(chunk:kitty-gfx)`. `Handler::apc_*` default
   no-op in `TerminalHandler`.
2. **Kitty keyboard protocol** (`CSI … u` push/pop/set/query) — not routed;
   needs `Screen.kitty_keyboard`.
3. **XTWINOPS size reports + title push/pop** (`CSI … t`) — no-op tail; needs a
   size effect + title stack.
4. **XTSHIFTESCAPE / mouse shift capture / mouse event+format** — stored on
   `flags` upstream; interpreted by the input layer (`chunk:input`).
5. **DCS XTGETTCAP + tmux control mode** — parsed by `dcs::Handler` but produce
   no terminal-state effect (matches upstream); tmux client is `chunk:tmux`.
6. **REP** (`CSI b`) — needs `previous_char` repeat plumbing.
7. **Kitty color protocol** (OSC 21) set/query effects — no-op; mirrors
   `color_operation` when landed.

## PROGRESS (pass 1 — stream dispatch + differential oracle DONE)

Landed: the `Stream<H>` engine (scalar decoder + parser + full CSI/ESC/OSC/DCS/
APC routing), the `Handler` trait (comptime-`vt` → trait-per-op mapping), the
concrete `TerminalHandler` over `Terminal` with a drainable reply queue, and the
`Terminal` methods the stream drives that weren't yet ported (`resize`,
`deccolm`, `semantic_prompt`/`cursor_is_at_prompt`, `print_attributes`). The
`vt-diff` crate gained the in-tree `RustTerminal` oracle and a Rust-vs-reference
differential suite.

Gates: `cargo check --workspace` green; `cargo test -p ghostty-vt` 874 lib
(33 new stream) + prior differential/unicode/doctests all pass; `cargo test -p
vt-diff` (3) and `--features reference` (differential 9 + rust_engine 3 +
smoke 3 + oracle 4) all pass; `cargo fmt` clean; `cargo clippy` clean on
`ghostty-vt` + `vt-diff` (pre-existing spike clippy warnings untouched — spike
source not edited per charter).

All three replay fixtures pass **identically** against both the Rust engine and
the Zig reference, plus 8 hand-written streams — the Phase-1 keystone
(bytes in, terminal state out, differentially proven) is complete.
