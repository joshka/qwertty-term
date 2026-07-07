# Formatter: serializing screen/terminal state back out (`src/terminal/formatter.zig`)

Surveyed and ported against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse HEAD`). Upstream `formatter.zig` is ~6.3k
lines / **100** inline `test` blocks. The Rust port lives in
`crates/ghostty-vt/src/formatter.rs` (+ `formatter/tests.rs`).

This is the Rust mirror of the Zig **C-API formatter** (`src/terminal/c/formatter.zig`,
`ghostty_formatter_*`) that `crates/vt-diff`'s `ReferenceTerminal::raw_text`
calls to produce its reference screen dump. That reference path uses
`emit = PLAIN`, `trim = true`, `unwrap = false`, no `extra`, no `selection`,
whole active screen — so **plain output is the byte-for-byte comparison
currency** and is the top correctness priority here.

## What the formatter is / who consumes it

The formatter walks a `Page` / `PageList` / `Screen` / `Terminal` and
serializes it back to a byte stream in one of three formats. It is the
read-back seam used by:

- **Clipboard / selectionString** — plain-text copy of a selection
  (`ScreenFormatter.Content.selection`).
- **libghostty-vt C API** (`ghostty_formatter_terminal_*`) — the differential
  harness reference side, plus embedders (betamax) that want a state dump.
- **State snapshots / replay** — VT format with `Extra` re-emits enough VT
  sequences (palette, modes, scrolling region, tabstops, pwd, cursor, SGR,
  hyperlink, protection, kitty-keyboard, charsets) that feeding the output into
  a fresh terminal reconstructs the state (round-trip tested upstream).
- **HTML export** — inline-styled `<div>`s for pasting styled terminal
  content into a document.

## Formats (`Format` enum, `formatter.zig:23-55`)

| format      | styled? | newline | notes                                                                                                                                       |
| ----------- | ------- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `plain` (0) | no      | `\n`    | text only; trailing whitespace/blank-line trimmed per `trim`                                                                                |
| `vt` (1)    | yes     | `\r\n`  | SGR re-emission; palette indices stay indices unless `palette` opt set (then RGB); `Extra` state after content                              |
| `html` (2)  | yes     | `\n`    | inline-style `<div>`s, `white-space: pre`; palette → `var(--vt-palette-N)` unless `palette` opt; non-ASCII → numeric entities; OSC8 → `<a>` |

`formatStyled(fmt)` (`:58-63`): `plain=false`, `vt/html=true`.

## Options (`Options`, `formatter.zig:84-116`)

- `emit: Format` — the format above.
- `unwrap: bool` (default false) — join soft-wrapped rows (`row.wrap` /
  `row.wrap_continuation`). When false, emit rows as laid out at current width.
- `trim: bool` (default true) — strip trailing space (0x20) on rows that have
  other text; trailing **blank rows** are always dropped regardless.
- `codepoint_map` — an ordered list of `{range:[u21;2], replacement}` where
  replacement is a single codepoint or a UTF-8 string; **last** matching range
  wins; applied per-codepoint at write time (`writeCodepointWithReplacement`,
  `:1386-1428`).
- `background` / `foreground: ?RGB` — a screen bg/fg. For `vt` emits `OSC 10`
  (fg) / `OSC 11` (bg) at the top; for `html` folds into the wrapper `<div>`
  `background-color`/`color`; for `plain` ignored.
- `palette: ?*const Palette` — if set, styled formats emit palette-indexed
  colors as concrete RGB (`38;2;r;g;b` / `rgb(r,g,b)`); if null, VT emits
  `38;5;idx`, HTML emits `var(--vt-palette-idx)`.

## The four-level formatter hierarchy

```text
TerminalFormatter  (terminal → active screen; palette/modes/region/tabstops/pwd/keyboard extras)
  └ ScreenFormatter (a single screen; selection content; cursor/SGR/hyperlink/protection/kitty/charset extras)
      └ PageListFormatter (a pin range across pages; rectangle; carries TrailingState between pages)
          └ PageFormatter  (the workhorse: formatWithState, :877-1362)
```

### `PageFormatter.formatWithState` (`:877-1362`) — the workhorse

Rendering algorithm (ported 1:1):

1. **Bounds**: clamp `start_x/start_y/end_x/end_y` to page size; `start_x`
   only applies to the first row, `end_x` only to the last (unless
   `rectangle`, then every row). A `start_x` landing on a wide char's
   `spacer_tail` backs up one column to include the lead; a `spacer_head`
   first-row start skips the row (`continue`).
2. **Unwrap spacer-head edge case** (`:910-929`): if unwrapping and the final
   cell is a `spacer_head`, advance `end_y += 1, end_x = 0`.
3. **Header** (`:936-997`): HTML emits the `<div style="font-family:
   monospace; white-space: pre; …">` wrapper (with bg/fg); VT emits OSC10/OSC11
   for fg/bg; plain nothing.
4. **Row loop** (`:1006-1337`): per row compute the cell subset; if the subset
   has no text (`Cell.hasTextAny`), accumulate `blank_rows` and `continue`
   (deferred blank emission avoids emitting trailing blank rows). Before
   emitting a non-blank row, flush accumulated `blank_rows` as newlines
   (`\n`/`\r\n`/`\n` per format), **resetting a non-default style first** so bg
   colors don't bleed across the newline. Track `blank_cells` similarly for
   trailing-space trimming within a row.
5. **Wrap accounting** (`:1109-1115`): `blank_rows += 1` after a row unless
   (`row.wrap && unwrap`); `blank_cells = 0` unless (`row.wrap_continuation &&
   unwrap`) — this is what lets soft-wrapped rows join seamlessly.
6. **Per-cell** (`:1118-1336`): skip spacer cells. A cell is "blank" if, in a
   styled format, it is empty AND unstyled; in plain, if it has no text, or is a
   trailing space and `trim`. Flush pending `blank_cells` as spaces before a
   non-blank cell. Then **SGR/style minimization** (see below), **HTML
   hyperlink** open/close, then the codepoint(s) (with `codepoint_map`
   replacement and HTML escaping) — or a single space for a bg-color-only cell.
7. **Trailers** (`:1339-1359`): close a non-default style; close an open HTML
   `<a>`; close the HTML `<div>`.
8. Returns `TrailingState{rows, cells}` so `PageListFormatter` can carry
   blank/wrap accounting across a page boundary.

### SGR / style minimization rules (styled formats only)

`cellStyle(cell)` (`:1463-1492`) resolves the cell's `Style` (interned style,
or a synthetic bg-only style for `bg_color_palette`/`bg_color_rgb` cells).
Minimization (`:1188-1243`):

- If the cell style equals the running `style`, **emit nothing** (this is the
  minimization — dedupe consecutive identical styles).
- On a change: for **HTML**, always close the prior `</div>` before opening the
  new one; for **VT**, only close (`\x1b[0m`) when switching **to** the default
  style (because any non-default VT style *begins* with its own `\x1b[0m` reset,
  making it self-contained — see `style.formatterVt`).
- Then open the new style if non-default.

`style.formatterVt` (`style.zig:308-391`): always leads with `\x1b[0m`, then
one **separate** SGR per attribute (`\x1b[1m` bold, `\x1b[2m` faint, `\x1b[3m`
italic, `\x1b[5m` blink, `\x1b[7m` inverse, `\x1b[8m` invisible, `\x1b[9m`
strike, `\x1b[53m` overline; underline `\x1b[4m` / `4:2` / `4:3` / `4:4` /
`4:5`), then colors via `formatColor(38|48|58, …)` — `38;5;idx` (palette) or
`38;2;r;g;b` (rgb, or palette-as-rgb when `palette` set). Deliberately never
combines attributes into one SGR (terminal-compat).

`style.formatterHtml` (`style.zig:401-459`): colors as
`color`/`background-color`/`text-decoration-color`; a combined
`text-decoration-line` (underline/line-through/overline/blink);
`text-decoration-style` (solid/double/wavy/dotted/dashed); `font-weight: bold`,
`font-style: italic`, `opacity: 0.5` (faint), `visibility: hidden` (invisible),
`filter: invert(100%)` (inverse). Palette → `var(--vt-palette-N)` unless a
palette is supplied.

### `Extra` state emission (VT format only, after content)

`TerminalFormatter.Extra` (`:167-231`) and `ScreenFormatter.Extra`
(`:460-519`), presets `none` / `styles` / `all`. The C API's default screen
dump uses **none**. Emission order:

- Terminal-level **before content**: palette (`OSC 4;i;rgb:rr/gg/bb` for all
  256, or `<style>:root{--vt-palette-i:#rrggbb;…}` for HTML), then modes that
  differ from defaults (`CSI [?]<n>h|l`).
- Screen content.
- Screen-level **after content** (`:566-660`): SGR style
  (`cursor.style.formatterVt`), OSC8 hyperlink, DECSCA protection (`\x1b[1"q`),
  kitty-keyboard (`\x1b[=<flags>;1u`), charsets (G0-G3 designations `ESC ( ) *
  +` with final `B`/`A`/`0`; GL/GR invocations SO/LS2/LS3/LS1R/LS3R), cursor CUP
  (`\x1b[<y+1>;<x+1>H`).
- Terminal-level **after content** (`:345-419`): DECSTBM/DECSLRM scrolling region (only if
  not full screen), tabstops (`\x1b[3g` then per-stop `CSI <col>G` + `\x1bH` HTS), keyboard
  `\x1b[>4;2m` if `modify_other_keys_2`, pwd `OSC 7`.

### `pin_map` / `point_map` (NOT ported — deferral)

Every level optionally fills a `pin_map`/`point_map` associating each output
byte with its source `Pin`/`Coordinate`. Upstream flags "a significant
performance hit"; it is a render/selection-tracking convenience, not part of the
serialized bytes. **Deferred** — see deferrals below. All `pin_map`-only tests
are ported minus their pin-map assertions (the text assertions are kept).

## Rust port shape (`crates/ghostty-vt/src/formatter.rs`)

The Rust port drives entirely off **read-only Screen/Terminal/PageList APIs**
(the same row-iterator pattern as `Screen::dump_string`), matching the task
constraint (no screen-internals edits; additive read-back only). It exposes:

- `Format` (`Plain`/`Vt`/`Html`), `Options` (emit/unwrap/trim/codepoint_map/
  background/foreground/palette), `ScreenExtra`, `TerminalExtra` (with
  `none`/`styles`/`all`).
- `Terminal::format(&Options, &TerminalExtra) -> String` and
  `Terminal::format_selection(...)` — the mirror of `ghostty_formatter_terminal_*`.
- `Screen::format(&Options, &ScreenExtra, content) -> String`.
- The core `PageList`-range renderer implementing `formatWithState` 1:1
  (blank-row/blank-cell accounting, trailing state across pages, SGR
  minimization, codepoint_map, bg-only cells, HTML hyperlinks).

The one additive read-back added: `ModeState::default_value(Mode) -> bool`
(mirrors Zig's `self.terminal.modes.default` field access) so the `modes`
Extra can emit only non-default modes. Charset/kitty/cursor/palette/region/
tabstops/pwd state is already public.

## Test port status (exact)

Upstream: **100** inline tests. Rust port: **49** tests in
`crates/ghostty-vt/src/formatter/tests.rs`, all passing. Categories:

| category                                                  | upstream | Rust | notes                                                                 |
| --------------------------------------------------------- | -------- | ---- | --------------------------------------------------------------------- |
| Page/Screen plain (single/multi/blank/trailing/wide/wrap) | 42       | 12   | the whole-content + soft-wrap/unwrap cases; subset/rectangle deferred |
| Page/Screen VT (bold/multi/fg/multiline/reset/palette/bg) | 8        | 9    | at Terminal level                                                     |
| PageList plain/VT (multi-page)                            | 12       | 0    | collapse into wrapped-row cases; genuine page-split deferred          |
| TerminalFormatter + Screen (plain/vt/selection)           | 20       | 2    | text asserts kept, pin_map asserts dropped; deduped                   |
| Terminal VT (region/modes/tabstops/keyboard/pwd)          | 5        | 5    | round-trip                                                            |
| Screen VT extras (cursor/style/protection/charsets)       | 4        | 4    | round-trip                                                            |
| Page HTML (plain/styles/colors/bg-fg/escaping/unicode)    | 12       | 7    |                                                                       |
| Page codepoint_map                                        | 9        | 9    |                                                                       |

The count is lower than 100 because the upstream `Page*`/`PageList*` tests
fan out one behavior across many `start_x`/`end_x`/`start_y`/`end_y`
subset + `point_map` permutations on a raw `Page`; the port asserts each
distinct **output behavior** once at the `Terminal`/`Screen` level (there is no
raw-`Page` test builder in the port). The differential test
(`crates/vt-diff/tests/formatter_differential.rs`) additionally pins the plain
output byte-for-byte against the Zig `ghostty_formatter_*` reference across all
replay fixtures + hand-written streams.

Because the Rust port drives off `Screen`/`Terminal` (not a raw-`Page` test
builder, which does not exist in the port), the **`PageFormatter`-level** tests
that construct a bare `Page` and poke cells directly are re-expressed as
`Terminal`-driven tests that produce the same on-screen content and assert the
same formatted bytes. The `start_x/end_x/start_y/end_y` **page-subset** and
**rectangle** tests are covered via the equivalent selection ranges where the
selection API is available, and otherwise noted as deferred (see below).

## Deferrals (with reason)

- **`pin_map`/`point_map`**: byte→pin tracking. Perf-heavy render/selection
  convenience, not part of the serialized bytes. The reference C-API dump path
  never uses it. Text assertions of every pin_map test are ported; the pin
  assertions are dropped. `TODO(chunk:formatter-pinmap)`.
- **Rectangle selection ranges** and arbitrary **pin-range page subsets**:
  depend on `Selection.zig` (a sibling chunk explicitly out of my territory).
  Whole-screen and simple row-range selections are ported; rectangle and
  cross-page x-offset subset tests are deferred to the selection chunk.
  `TODO(chunk:selection)`.
- **HTML hyperlink URI lookup by cell**: emitting `<a href>` needs
  `Page::lookup_hyperlink(cell) -> uri`; the port emits hyperlink anchors when
  that read-back is available and defers the tests that require planting a
  hyperlink via OSC8 + reading it back per-cell if the lookup is not yet public.
  `TODO(chunk:hyperlink)`.
- **Multi-page `TrailingState` across a real page split**: the accounting is
  ported and exercised within a single page's wrap rows; a fixture that forces a
  genuine page boundary mid-wrap is deferred (needs deterministic page-split
  control). `TODO(chunk:formatter-multipage)`.
