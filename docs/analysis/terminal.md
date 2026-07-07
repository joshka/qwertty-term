# Terminal: the protocol/policy layer (`src/terminal/Terminal.zig`)

Surveyed against ghostty commit `2da015cd6ac06cedc89e09756e895d2c1715205d`
(verify with `git -C ~/local/ghostty rev-parse HEAD`). `Terminal.zig` is
**13956 lines** with **381 inline `test` blocks** (`grep -c '^test '`). It is
the Phase-1 keystone: the state machine that sits on top of
[`Screen`](screen.md) (two of them, primary + alternate, via `ScreenSet`) and
ties together every landed VT module — [`modes`](terminal-state.md),
[`charsets`](terminal-state.md), [`Tabstops`](terminal-state.md),
[`sgr`](terminal-state.md), [`csi`](terminal-state.md),
[`color`](terminal-state.md), and the [`osc`](osc.md) semantic-prompt parser.
The Rust port lives in `crates/ghostty-vt/src/terminal/`.

`Terminal` owns *protocol + policy*; `Screen` owns *state + primitive editing*
(see `screen.md`, "Screen's responsibilities vs. Terminal's"). Terminal never
touches the PageList directly except through the handful of fast-path methods
Screen re-exposes (`pages.eraseRowBounded`, `pages.scroll`).

## Fields (`Terminal.zig:45-134`)

| field | Zig type | role |
| --- | --- | --- |
| `screens` | `ScreenSet` | primary + (lazy) alternate screen; `.active`/`.active_key` |
| `status_display` | `ansi.StatusDisplay = .main` | DECSASD/DECSSDT; non-`.main` prints are black-holed |
| `tabstops` | `Tabstops` | HT/HTS/TBC stop set, `TABSTOP_INTERVAL = 8` |
| `rows`, `cols` | `size.CellCountInt` | terminal grid size (mirror of active screen) |
| `width_px`, `height_px` | `u32 = 0` | pixel size for pty/images |
| `scrolling_region` | `ScrollingRegion` | `{top, bottom, left, right}` (0-indexed; l/r margins) |
| `pwd` | `std.ArrayList(u8)` | last reported OSC 7 pwd |
| `title` | `std.ArrayList(u8)` | OSC 0/2 window title |
| `colors` | `Colors` | `{background, foreground, cursor: DynamicRGB, palette: DynamicPalette}` |
| `previous_char` | `?u21 = null` | for REP (`ESC [ n b`) repeat |
| `modes` | `modespkg.ModeState = .{}` | current/saved/default mode bitsets |
| `mouse_shape` | `mouse.Shape = .text` | OSC 22 mouse shape |
| `glyph_glossary` | `glyph.Glossary = .empty` | Glyph Protocol registrations (build-gated) |
| `flags` | packed struct | see below |

`flags` (`:89-134`): `shell_redraws_prompt: osc.semantic_prompt.Redraw = .true`
(Kitty prompt-redraw-on-resize extension), `modify_other_keys_2: bool` (ESC[4;2m
XTMODKEYS), `mouse_event: mouse.Event`, `mouse_format: mouse.Format` (tracked
separately from modes because mode order matters), `mouse_shift_capture`
(XTSHIFTESCAPE tri-state), `focused: bool = true`, `password_input: bool`,
`selection_scroll: bool`, `search_viewport_dirty: bool`, `dirty: Dirty`.

`Dirty` (`:159-179`) = `{palette, reverse_colors, clear, preedit,
glyph_glossary}` — terminal-level render hints, distinct from `Screen.Dirty`.

`ScrollingRegion` (`:181-192`): `top < bottom`, `left < right`, `right <= cols-1`.

`Colors` (`:139-157`) with a `default` (all `.unset` + `.default` palette).

`Options` (`:194-224`): `{cols, rows, max_scrollback = 10_000, colors,
default_modes: ModePacked, kitty_image_storage_limit, kitty_image_loading_limits}`.
`init` (`:226-261`) builds the ScreenSet, tabstops (interval 8), a full-screen
scrolling region, empty pwd/title, and seeds `modes.values`/`modes.default` from
`default_modes`. `deinit` (`:263`) frees screens/tabstops/pwd/title/glossary.

## The print path (`print` `:740`, `printCell` `:1094`, `printWrap` `:1262`)

This is the hot heart of the terminal. `print(c: u21)`:

1. **Status-display gate**: if `status_display != .main`, drop the char.
2. **Right limit**: `right_limit = if cursor.x > region.right then cols else
   region.right + 1` — the effective right edge accounting for l/r margins.
3. **Grapheme clustering** (`:764-949`, mode 2027 `grapheme_cluster`): if `c > 255`
   and clustering on and `cursor.x > 0`, find the previous cell (accounting for
   pending-wrap, wraparound, spacer-tail), run `uucode.grapheme.BreakState`
   over the prev cell's grapheme cluster + `c`. If **no break**, `c` attaches to
   prev via `graphemeWidthEffect`:
   - `.ignore` → drop; `.no_change` → just append;
   - `.wide` → prev cell becomes wide: back up cursor `prev.left`, possibly
     insert spacer_head + `printWrap` if at right edge (with the row-wrap-first
     integrity dance and the grapheme-transfer-across-pages block `:854-897`),
     then write a `.spacer_tail` and advance;
   - `.narrow` → prev cell reverts to narrow, clear its spacer_tail, back the
     cursor up one (or clear pending_wrap if at edge).
   Then `appendGrapheme(prev.cell, c)` and return.
4. **Width**: `if c <= 0xFF then 1 else unicode.table.get(c).width` (asserted
   `<= 2`). The Rust port MUST use `crate::unicode::codepoint_width` (the same
   uucode-derived tables the parser/screen chunks already use), matching
   upstream exactly.
5. **Zero-width** (`:962-1012`): attach as grapheme to the prev non-spacer-tail
   cell (unless mode 2027 is on, which drops it). VS15/VS16 (`FE0E`/`FE0F`) only
   attach if prev is `extended_pictographic`.
6. `previous_char = c`.
7. **Soft-wrap**: if `pending_wrap` and `wraparound` mode → `printWrap()`.
8. **Insert mode** (mode 4): if on and not at EOL, `insertBlanks(width)`.
9. **Write** by width: width-1 → `printCell(c, .narrow)`; width-2 →
   emit spacer_head+`printWrap` if at `right_limit-1` (only a real spacer_head at
   `right_limit == cols`, else a narrow), then `printCell(c, .wide)`,
   `cursorRight(1)`, `printCell(0, .spacer_tail)`.
10. **Pending wrap**: if now at `right_limit-1`, set `pending_wrap = true` and
    don't move; else `cursorRight(1)`.

`printCell(unmapped_c, wide)` (`:1094-1261`): applies the **single-shift /
GL charset mapping** (`.utf8`/`.ascii` pass through; else map via
`charsets.table` after clamping to u8, non-ASCII → space), then clears any
prior wide-partner cells / graphemes for the target when `cell.wide != wide`,
releases the old style ref if the style id changes, writes the cell struct
(`content_tag=.codepoint`, style_id, wide, protected, semantic_content), uses
the new style ref, marks Kitty placeholder rows, and re-applies the active
hyperlink (`cursorSetHyperlink`) or clears a stale one.

`printWrap` (`:1262-1301`): mark `page_row.wrap` only if at the true screen
edge (`cursor.x == cols-1`), preserve semantic-content across the `index()`,
move to `region.left` via `cursorHorizontalAbsolute`, propagate prompt →
`prompt_continuation`, mark `wrap_continuation`.

`printString`/`printSlice`/`printRepeat` (`:301-368`) and the SIMD fast paths
(`printSliceFast` `:369`, `printSliceFill` `:502`, `printSliceEligible`,
`printSliceCheckExpected`) are throughput optimizations over the same semantics;
the Rust port implements the scalar `print` faithfully and treats the slice
fast-paths as an optional later optimization (they are behavior-equivalent).

## Every CSI/ESC operation Terminal implements

Grouped, with `Terminal.zig` line refs:

- **Charset**: `configureCharset` (`:1302`), `invokeCharset` (`:1308`,
  GL/GR/single-shift).
- **Simple motion**: `carriageReturn` (`:1327`, origin/left-margin aware),
  `linefeed` (`:1341`, `index` + optional CR in LNM), `backspace` (`:1347`).
- **Cursor moves with clamping**: `cursorUp/Down/Right` (`:1354`/`:1372`/`:1389`,
  scroll-region-clamped, reset pending_wrap), `cursorLeft` (`:1403`, the reverse-
  wrap / XTREVWRAP / XTREVWRAP2 state machine — the trickiest motion op),
  `setCursorPos` (`:1974`, DECSLRM/origin-aware, 1-indexed→0), `horizontalTab`
  (`:1762`)/`horizontalTabBack` (`:1775`), `tabSet`/`tabReset`/`tabClear`
  (`:1790-1805`).
- **Save/restore**: `saveCursor` (`:1507`, DECSC → per-screen `saved_cursor`
  incl. origin mode + charset), `restoreCursor` (`:1523`, DECRC, style-first
  because it can fail; falls back to default style on error).
- **Index family**: `index` (`:1821`, LF/IND — the scroll-vs-move decision incl.
  the scrollback fast path via `cursorScrollAbove`, the l/r-margin slow path via
  `scrollUp`, and the `eraseRowBounded` hot path when no bg fill needed),
  `reverseIndex` (`:1956`, RI).
- **Margins**: `setTopAndBottomMargin` (`:2034`, DECSTBM), `setLeftAndRightMargin`
  (`:2045`, DECSLRM, gated on `enable_left_and_right_margin`).
- **Scroll**: `scrollDown` (`:2059`, SD = cursor-to-top-of-region + insertLines),
  `scrollUp` (`:2081`, SU), `scrollViewport` (`:2160`).
- **Insert/delete lines**: `insertLines` (`:2251`, IL — full l/r-margin-aware
  row shift + bg fill), `deleteLines` (`:2452`, DL).
- **Insert/delete/erase chars**: `insertBlanks` (`:2631`, ICH), `deleteChars`
  (`:2732`, DCH), `eraseChars` (`:2782`, ECH).
- **Erase**: `eraseLine` (`:2834`, EL — right/left/complete/right-unless-pending
  + protected-mode variants), `eraseDisplay` (`:2914`, ED — below/above/complete/
  scrollback/scroll_complete + protected).
- **Screen alignment / reset**: `decaln` (`:3046`, DECALN fill with `E`),
  `fullReset` (`:3580`, RIS), and the soft-reset path lives in the mode/switch
  handlers.
- **SGR**: `setAttribute` (`:3176`, applies an `sgr.Attribute` to the cursor
  style, then `manualStyleUpdate`), `printAttributes` (`:3185`, DECRQSS query).
- **Screen switch**: `switchScreen` (`:3400`, primary↔alternate with the cursor-
  copy rules per DEC mode 1047/1049/47), `switchScreenMode` (`:3470`),
  `deccolm` (`:3278`, 80/132 column switch).
- **Reporting/pixel/misc**: `setPwd`/`getPwd` (`:3354`/`:3364`),
  `setTitle`/`getTitle` (`:3370`/`:3380`), `resize` (`:3306`),
  `setProtectedMode` (`:1561`), `semanticPrompt` (`:1588`, OSC 133 dispatch;
  `semanticPromptFreshLine` `:1709`), `cursorIsAtPrompt` (`:1745`),
  `plainString`/`plainStringUnwrapped` (`:3566`/`:3571`), the kitty-graphics
  seam (`kittyGraphics` `:3123`, `setKittyGraphicsSizeLimit`/`…LoadingLimits`),
  and the Glyph Protocol seam (`glyphProtocol` `:3134`).

## Seams (deps NOT yet landed on trunk)

The Rust trunk does **not** have these Zig modules; the port introduces small
local Terminal-owned equivalents or marked seams:

- **`ScreenSet.zig`** (150 lines) — ported inline as `terminal::ScreenSet`
  (primary + lazy alternate, `active`/`active_key`, `get`/`get_init`/`remove`/
  `switch_to`, generation counters). Owns the `Box<Screen>`s.
- **`ansi.zig`** — only `StatusDisplay {main, status_line}` and
  `ProtectedMode {off, iso, dec}` are needed; ported as tiny local enums.
- **`mouse.zig`** — `Shape`, `Event`, `Format` are only stored on `flags`, never
  interpreted by Terminal itself (the stream/input layer does). Ported as
  minimal enums / `TODO(chunk:input)` where interpretation would be needed.
- **`kitty.zig` graphics** — `kittyGraphics`, size/loading-limit setters, and the
  `screen.kitty_images.dirty = true` scroll hooks are the **kitty-gfx seam**.
  Marked `TODO(chunk:kitty-gfx)`; a sibling workspace owns
  `crates/ghostty-vt/src/kitty/`. Terminal exposes a seam method/trait shaped so
  the kitty chunk maps 1:1, and the dirty hooks become no-ops for now.
- **`apc/glyph.zig`** — `glyph_glossary` + `glyphProtocol` — `TODO(chunk:apc)`.
- **`stream.zig` / `stream_terminal.zig`** — `vtStream`/`vtHandler` and the whole
  CSI/ESC/OSC dispatch table are the **NEXT chunk**. NOT ported here. Method
  names/signatures on Terminal are kept shaped so the stream layer maps 1:1.
- **`Screen` gaps this port needs but Screen deferred**: `cursorScrollAbove`
  (Screen has `cursor_down_scroll` only), a public grapheme-append, a `Screen`
  `protected_mode` field, `cursorRowUp`, and a cell-slice `clear_cells`
  signature. See PROGRESS note.

## Reconciliations done this chunk (Screen placeholders)

1. **`screen/charset.rs` stub deleted.** Its `Charset`/`Slots`/`ActiveSlot`
   duplicated the real `crate::charsets` enums (identical variants). The
   Screen-owned `CharsetArray`/`CharsetState` (which are *not* in the Zig
   `charsets.zig` — they live in `Screen.zig`) were hoisted into
   `crate::charsets` so both Screen and Terminal share one definition.
   `screen/mod.rs` + `screen/cursor.rs` now `use crate::charsets::CharsetState`.
2. **`screen/semantic.rs` unified with OSC.** The local placeholder `Click`/
   `ClickEvents`/`PromptKind`/`Redraw` enums were replaced with
   `pub use crate::osc::{Click, ClickEvents, PromptKind, Redraw}` (the OSC chunk
   landed the real parsed types). The Screen-owned `SemanticClick` + container
   `SemanticPrompt` are kept.
3. **`Style::bg_cell` + `Screen::blank_cell` bg fallback completed.** Added
   `Cell::color_rgb`/`set_color_rgb` (LSB-first `r|g<<8|b<<16` packing matching
   the Zig `Cell.RGB` packed struct), `Style::bg_cell() -> Option<Cell>` (port of
   `Style.bgCell`), and rewired `Screen::blank_cell` to return the bg-colored
   blank when the cursor style is non-default (port of `blankCell`). Erase/clear
   paths now preserve the active SGR background.

## Test porting status

Upstream `Terminal.zig`: **381** inline tests (`grep -c '^test '`). This chunk
ports **27** Rust tests (`terminal::tests` + `terminal::screen_set::tests`) —
the tier-1 operation net plus the common-path print tests. The remaining ~354
Zig tests are DEFERRED behind the operations not yet ported (erase/scroll/
insert-delete-line family, alt-screen switch, full/soft reset, DECALN, SGR
`setAttribute`, mode-2027 grapheme clustering). See the PROGRESS note.

## PROGRESS (this chunk — INCOMPLETE, hand-off state)

This is a large keystone chunk; per the chunk's stated priority ordering
(compile-green > analysis doc > print+cursor+tests > erase/scroll+tests >
alt-screen/reset+tests), the following landed and the rest is deferred with a
clear path.

**Landed (all `cargo test -p ghostty-vt` green, fmt+clippy clean, Miri-clean
over `terminal::` with no skips):**

- **Reconciliations** (Screen placeholders): `screen/charset.rs` stub deleted →
  `CharsetArray`/`CharsetState` hoisted to `crate::charsets`; `screen/semantic.rs`
  unified to re-export `crate::osc::{Click, ClickEvents, PromptKind, Redraw}`;
  `Style::bg_cell` + `Cell::color_rgb`/`set_color_rgb` + `Screen::blank_cell`
  bg-fallback completed.
- **Terminal struct** + `ScreenSet` (primary/lazy-alternate) + `ansi`/`mouse`
  minimal types (`StatusDisplay`, `ProtectedMode`, `MouseShiftCapture`),
  `Colors`, `Dirty`, `Flags`, `ScrollingRegion`, `Options`, `new`.
- **Tier-1 ops**: `configure_charset`/`invoke_charset`, `carriage_return`,
  `linefeed`, `backspace`, `cursor_up/down/right/left` (incl. the full
  reverse-wrap/XTREVWRAP2 state machine), `save_cursor`/`restore_cursor`,
  `set_protected_mode`, `horizontal_tab`/`horizontal_tab_back`/`tab_set`/
  `tab_reset`/`tab_clear`, `index` (no-scroll moves + full-screen scrollback +
  full-width `erase_row_bounded` hot path), `reverse_index`, `set_cursor_pos`,
  `set_top_and_bottom_margin`, `set_left_and_right_margin`, `set_pwd`/`get_pwd`,
  `set_title`/`get_title`, `print_string`/`plain_string`/`plain_string_unwrapped`.
- **Print path** (`terminal/print.rs`): the **non-grapheme-clustering** `print` /
  `print_cell` / `print_wrap` / `print_zero_width` — width via
  `crate::unicode::codepoint_width` (upstream tables), charset/single-shift
  mapping, soft-wrap, narrow/wide/spacer-head/spacer-tail handling, VS15/VS16
  emoji-base gating via `properties().emoji_vs_base`, style-ref release/use,
  hyperlink re-apply.

**Screen surface exposed this chunk (all `pub(crate)`):** `cursor_page`,
`cursor_cell_left`/`cursor_cell_right`, `cursor_down_or_scroll`,
`cursor_set_hyperlink` (moved out of `#[cfg(test)]`), `clear_cells_page`,
`cursor_row_up` (new), `Page::{clear_grapheme, update_row_grapheme_flag,
update_row_hyperlink_flag}`, `Screen::dump_string` (moved out of `#[cfg(test)]`),
`Screen::protected_mode` field (new).

**DEFERRED (not ported) + why — the next passes:**

1. **mode-2027 grapheme clustering** in `print` (`Terminal.zig:764-949`):
   needs `moveGrapheme`, cross-page grapheme transfer, `graphemeWidthEffect`.
   Marked `TODO(chunk:terminal-print-grapheme)`. ~15 Zig print tests deferred
   (Devanagari, multicodepoint grapheme, ZWJ, VS16-with-second-char).
2. **erase family** (`eraseChars`, `eraseLine`, `eraseDisplay`) and
   **insert/delete** (`insertBlanks` — stubbed, `deleteChars`, `insertLines`,
   `deleteLines`): need a cell-slice `clear_cells` + margin-aware row/cell shift
   + SGR bg fill exposed from Screen. `TODO(chunk:terminal-edit)`.
3. **scroll family** (`scrollUp`, `scrollDown` — stubbed, `scrollViewport`):
   depend on `insertLines` + `cursorScrollAbove` (Screen has only
   `cursor_down_scroll`; `cursor_scroll_above` was deferred in the Screen chunk).
   `index`'s l/r-margin slow path and its `blankCell().isZero()` bg-fill check
   likewise wait on this. `TODO(chunk:terminal-scroll)`.
4. **alt-screen switch** (`switchScreen`/`switchScreenMode`, DEC 47/1047/1049
   cursor-copy rules), **deccolm**, **full/soft reset** (`fullReset`),
   **DECALN** (`decaln`): need `cursorCopy` between screens (deferred in Screen
   chunk) + `reset` wiring. `TODO(chunk:terminal-screens)`.
5. **SGR** (`setAttribute`, `printAttributes`): map `sgr::Attribute` →
   `Style` mutation + `manual_style_update`. `TODO(chunk:terminal-sgr)`.
6. **semantic prompt** (`semanticPrompt` OSC 133 dispatch, `cursorIsAtPrompt`):
   straightforward once `cursor_set_semantic_content` variants are threaded.
   `TODO(chunk:terminal-osc133)`.
7. **kitty graphics / glyph / mouse interpretation / stream handler**: seams for
   sibling chunks (`kitty-gfx`, `apc`, `input`, `stream`) — see Seams section.

The ~354 deferred Zig tests map onto tiers 1-6 above; port them alongside each
tier. The method names/signatures already shaped here mean the stream chunk maps
1:1 without renames.
