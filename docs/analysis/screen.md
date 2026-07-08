# Screen: the viewport/cursor layer (`src/terminal/Screen.zig`)

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse --short HEAD`). `Screen.zig` is ~10.5k lines
with 188 inline `test` blocks. It sits directly on top of
[`PageList`](pagelist.md) (which sits on [`Page`](page-memory.md)) and is the
layer a `Terminal` drives: it owns the **cursor**, the **charset state**, the
**kitty keyboard flag stack**, the **selection**, **semantic-prompt** state,
**dirty** flags, and the **resize** entry point. The Rust port lives in
`crates/qwertty-term-vt/src/screen/`.

## Screen's responsibilities vs. Terminal's

`Screen` is *state + primitive editing*; `Terminal` (a later chunk) is
*protocol + policy*. Concretely:

- **Screen owns**: the `PageList`, a single tracked cursor pin and its cached
  `page_row`/`page_cell` pointers, the active style/hyperlink cached against the
  cursor's page, charset slots, the kitty keyboard `KeyFlagStack`, the
  selection (tracked), semantic-prompt state, protected mode, and the renderer
  `Dirty` bits. It exposes cursor motion (`cursor*`), scroll/clear/erase
  primitives, and `resize`.
- **Terminal owns** (not here): scrolling *regions* (top/bottom/left/right
  margins), origin mode, `print` (the wide-char/grapheme/wrap state machine),
  tab stops, mode state (`modes.zig`), SGR parsing (`sgr.zig`), the full OSC/CSI
  dispatch, and the primary/alternate screen swap. `Screen` deliberately has
  `NOTE(qwerasd)` comments (`cursorResetWrap` `:1227`, `splitCellBoundary`
  `:1520`) flagging that it is *not scrolling-region aware* — that is Terminal's
  job.

There are **two** `Screen` instances per `Terminal` (primary + alternate);
`Screen` itself knows nothing about that duality.

## The `Cursor` struct (`Screen.zig:122-185`)

The cursor is the heart of Screen. Fields:

| field                                    | role                                                                                             |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `x`, `y: CellCountInt`                   | position **within the active area** (not screen/history)                                         |
| `cursor_style: CursorStyle`              | visual style (`cursor.zig`: bar/block/underline/block_hollow); defaults `.block`                 |
| `pending_wrap: bool`                     | the "last column flag (LCF)"; next print soft-wraps                                              |
| `protected: bool`                        | new cells get the protected attribute                                                            |
| `style: style.Style`                     | the concrete active style *value* (source of truth)                                              |
| `style_id: style.Id`                     | the **page-specific** interned id for `style`; `default_id` (0) when default                     |
| `hyperlink_id: hyperlink.Id`             | active OSC8 link id in the cursor's page (0 = none)                                              |
| `hyperlink_implicit_id: OffsetInt`       | monotonic counter for links without an explicit id                                               |
| `hyperlink: ?*Hyperlink`                 | heap copy of the active link (page-independent) so it can be re-inserted when the page pin moves |
| `semantic_content: Cell.SemanticContent` | output/input/prompt applied to newly written cells                                               |
| `semantic_content_clear_eol: bool`       | input-area "clear to EOL" hint                                                                   |
| `page_pin: *PageList.Pin`                | the **tracked** pin locating the cursor (always accurate across mutations)                       |
| `page_row: *Row`, `page_cell: *Cell`     | cached pointers derived from the pin — the fast path                                             |

The invariant (`assertIntegrity` `:344-365`): `cursor.x/y` must equal
`pointFromPin(.active, page_pin)`. The pin is the truth; x/y and the row/cell
pointers are a cache that every mutating op must keep coherent (or call
`cursorReload`).

### Style id caching against the page

Styles are interned per-*page* in a `RefCountedSet`. Because the cursor can
migrate between pages (scroll, resize, capacity increase), the cursor holds the
style **value** and re-interns it on the destination page:

- `manualStyleUpdate` (`:2018-2097`): release old `style_id` on the current
  page; if the new `style` is default, set id 0 and return; else
  `page.styles.add(value)`. On `OutOfMemory`/`NeedsRehash` it drives
  `increaseCapacity(.styles | rehash)` and, on `OutOfSpace`, `splitForCapacity`,
  then retries. This is *the* style-caching workhorse.
- `cursorChangePin` (`:1140-1215`): the ONLY sanctioned writer of `page_pin.*`.
  If the new pin is on a different page it releases style+hyperlink on the old
  page and re-interns on the new page (via `manualStyleUpdate`/`startHyperlink`).
  Also marks both old and new page rows dirty (ligature run-splitting).
- `increaseCapacity` (`:568-636`): Screen's wrapper over `PageList.increaseCapacity`
  that, when the cursor's own page is reallocated, re-adds the cursor style and
  hyperlink to the new page and `cursorReload`s.
- `splitForCapacity` (`:2113-2153`): splits the cursor page at the point using
  less capacity, then fixes the cursor pin via `cursorChangePin`.

### Hyperlink state

- `startHyperlink(uri, id?)` (`:2212-2261`): builds a page-independent
  `Hyperlink`, loops calling `startHyperlinkOnce` and growing the right page
  sub-allocator (`string_bytes`/`hyperlink_bytes`/rehash) until it fits.
- `startHyperlinkOnce` (`:2266-2287`): ends any prior link, dupes the
  `Hyperlink` into `self.alloc`, `page.insertHyperlink`, caches
  `hyperlink`/`hyperlink_id`.
- `endHyperlink` (`:2291-2314`): release the id on the page, free the heap copy.
- `cursorSetHyperlink` (`:2317-2376`): put cursor cell -> id in the page map,
  `use` the refcount; on `HyperlinkMapOutOfMemory` grow string/hyperlink bytes
  and retry.

## `cursorAbsolute` / `cursorReload` paths

- `cursorAbsolute(x, y)` (`:734-752`): move the pin up/down by the y delta from
  the current cursor y (a cheap relative move, since the pin is already near),
  set x, then `cursorChangePin`, refresh row/cell.
- `cursorReload` (`:757-792`): the expensive recovery path. Derives the active
  point from the (always-accurate) pin; if the pin is now *outside* the active
  area it repoints to the active top-left. Refreshes x/y and row/cell, and
  re-interns the style (`manualStyleUpdate`) because the page may have changed.
  Called after any op that invalidates the cached pointers wholesale
  (`scrollClear`, `eraseHistory`, `eraseActive`, `resize`, `increaseCapacity`).
- The relative cursor moves (`cursorUp/Down/Left/Right/RowUp/HorizontalAbsolute`,
  `:661-731`) are the fast paths: they bump the pin and pointer arithmetic
  directly, asserting bounds, never reloading.

## Scroll and the cursor-scroll family

- `scroll(behavior)` (`:1272-1290`): thin pass-through to `PageList.scroll`,
  plus marking kitty images dirty. `Screen.Scroll` (`:1261-1269`) mirrors
  `PageList.Scroll`.
- `scrollClear` (`:1294-1306`): `PageList.scrollClear` + `cursorReload`.
- `cursorDownScroll` (`:796-906`): the print-time scroll. Precondition: cursor on
  the last active row. Three cases:
  - **no scrollback, 1 row**: just `clearCells` the row.
  - **no scrollback, N rows**: `eraseRow(.active)` (rotates rows up), then
    *restore* the cursor pin (eraseRow moved it up by one, which we don't want)
    and refresh row/cell.
  - **scrollback**: `grow()` one row; `cursorChangePin` to the new bottom row
    (handling the prune case where `grow` moved the pin to a new page's
    top-left); mark dirty; bg-fill the new row if a bg style is set.
- `cursorScrollAbove` (`:910-1004`) + `cursorScrollAboveRotate` (`:1006-1056`):
  scroll only the region at/above the cursor by inserting a blank row and
  rotating everything *below* the cursor down one (cheaper than shifting all
  scrollback up). Fast path when cursor is on the last page; cross-page path
  clones the boundary rows between pages.

## Clearing / erase paths

- `clearRows(tl, bl, protected)` (`:1340-1369`): iterate a page region, per row
  `clearCells` (or `clearUnprotectedCells`), reset row struct, mark dirty.
- `clearCells(page, row, cells)` (`:1374-1463`): releases graphemes / hyperlinks
  / styles for the range (guarded by row flags), recomputes or clears the row
  flags, handles the kitty placeholder recount, then `@memset` to `blankCell()`
  (the cursor-bg-colored blank). *(In the Rust port, `Page::clear_cells` already
  performs the release + flag recompute; Screen adds only the blank-cell fill.)*
- `clearUnprotectedCells` (`:1466-1490`): `clearCells` over maximal
  non-protected runs.
- `splitCellBoundary(x)` (`:1524-...`): wide-char/spacer boundary cleanup on the
  cursor row (spacer-tail after wide, spacer-head at wrap). *Depends on the
  print pipeline; ported as scaffolding.*
- `eraseHistory`/`eraseActive` (`:1316-1332`): pass-through to PageList + reload.

## Resize entry points into PageList

`Screen.resize(Resize{cols, rows, reflow, prompt_redraw})` (`:1655-1834`):

1. Mark kitty images dirty.
2. **Release the cursor style** (set default, `manualStyleUpdate`) and restore
   it after — the cursor may land on a different page.
3. **Release the cursor hyperlink** from the old page (keep the heap copy),
   re-add after resize.
4. Track a pin for the **saved cursor** (DECSC) so its x/y reflows too.
5. **prompt_redraw**: if the cursor is on a prompt/input line (checked via
   `cursor.semantic_content != .output`, not the row flag, to catch unmarked
   continuation lines), clear from the prompt start (`.true`) or just the cursor
   line (`.last`) so the shell can redraw.
6. `PageList.resize({rows, cols, reflow, cursor:{x,y,pin}})` — the actual
   reflow/resize (preserve-cursor semantics live in PageList's `resizeCols`).
7. If `no_scrollback`, `eraseHistory(null)` (PageList always keeps ≥1 page).
8. `cursorReload`; fix up the saved-cursor pin's x/y (+ pending-wrap unset);
   re-add the hyperlink.

The **preserve-cursor** semantics (counting wrapped/remaining rows so reflow
doesn't pull the cursor's line into scrollback) are entirely inside
`PageList.resizeCols` — Screen just passes the cursor `{x, y, pin}`.

## Charset state (`CharsetState`, `:199-247`)

Four graphical charset slots `g0..g3` (each `utf8`/`ascii`/`british`/
`dec_special`), a `gl`/`gr` active-slot mapping (default `G0`/`G2`), and an
optional `single_shift`. A bespoke `CharsetArray.get/set` (over a plain
`EnumArray`) for print-hot-path speed. Reset to defaults on RIS.
*(`charsets.zig` is the sibling `terminal-state` chunk; the port carries a
minimal local stub — see placeholder inventory.)*

## Kitty keyboard flag stack (`kitty/key.zig`)

`FlagStack` (a fixed 8-deep stack of `Flags: packed struct(u5)`) implements the
CSI `> u` / `< u` / `= u` push/pop/set of the kitty keyboard protocol.
`Flags = {disambiguate, report_events, report_alternates, report_all,
report_associated}`. `set(mode: set|or|not, v)`, `push` (wraps, evicting
oldest), `pop(n)` (resets if `n >= len`, DoS defense). Screen owns one as
`kitty_keyboard`. This is small and self-contained; **ported 1:1**.

## Semantic-prompt state (`SemanticPrompt`, `:95-119`)

`{seen: bool, click: SemanticClick}` where `SemanticClick = none |
click_events(ClickEvents) | cl(Click)`. `seen` is flipped true the first time a
`prompt` semantic content is set (`cursorSetSemanticContent` `:2381-2412`),
letting Terminal skip semantic work when never seen. The `ClickEvents`/`Click`
enums come from `osc/parsers/semantic_prompt.zig` (sibling OSC chunk — narrow
local placeholder here). `cursorSetSemanticContent` also stamps
`page_row.semantic_prompt` for prompt kinds.

## Dirty tracking

`Screen.Dirty` (`:85-93`) = `{selection, hyperlink_hover}` — screen-wide render
hints (selection changed; a hovered OSC8 link spans lines). Distinct from the
per-**row** `dirty` bit (in `Page.Row`) and the per-**page** `dirty` bit, which
the cursor/scroll/clear ops set directly (`cursorMarkDirty` = `page_row.dirty =
true`; `cursorChangePin` marks both old/new; the rotate paths mark whole pages).
`PageList` exposes `is_dirty`/`mark_dirty`/`clear_dirty` for the row bits.

## What Screen delegates to PageList

Everything about *memory and layout*: pin tracking/fixups, grow/prune/scrollback
eviction, scroll (viewport), erase (physical row removal), resize+reflow,
clone, point<->pin resolution, iterators. Screen only adds the *cursor cache*,
*style/hyperlink interning against the cursor page*, *charset/kitty/semantic
state*, the *blank-cell (bg) fill* on clears, and the *dirty render hints*.

## Port notes (Rust, `crates/qwertty-term-vt/src/screen/`)

- **Module layout**: `screen/mod.rs` (the `Screen` struct + init/deinit/reset/
  clone + cursor cache + all `cursor*` motion + scroll/clear/erase/resize +
  style/hyperlink caching + select scaffold + dirty), `screen/cursor.rs`
  (`Cursor`, `SavedCursor`, `CursorStyle`), `screen/kitty_key.rs` (`FlagStack`,
  `Flags`, `SetMode` — 1:1 from `kitty/key.zig`), `screen/charset.rs` (charset
  **stub**), `screen/semantic.rs` (`SemanticPrompt`, `SemanticClick`, and the
  OSC placeholder enums), `screen/hyperlink.rs` (the page-independent owned
  `Hyperlink` the cursor caches).
- **The cursor pin** is a `*mut Pin` (a tracked pin vended by
  `PageList::track_pin`), matching Zig's `*Pin`. `page_row`/`page_cell` are
  cached raw pointers refreshed from `pin.row_and_cell()`.
- **grow()** returns `bool` in the Rust PageList (Zig `?*Node`); Screen detects
  the prune/page-change case by comparing `cursor.page_pin.node` before/after,
  exactly as Zig's `old_pin.node == …` checks do.
- **Style/hyperlink interning** uses `Page::styles()` + `StyleSet::add/release/
  use_id/get` and `Page::insert_hyperlink`/`set_hyperlink`, grabbing
  `page.memory_mut()` as the `base` pointer before the `&mut` set borrow.
- **`Page::clear_cells(row, left, end)`** already does the grapheme/hyperlink/
  style release + flag recompute + kitty recount; Screen's `clear_cells` calls
  it and then memsets the range to `blank_cell()`. A `Page::fill_cells(row,
  left, end, cell)` helper (added this chunk) does the release-then-fill in one
  pass for parity with Zig's single-`@memset`.

### Test porting status (exact)

Upstream `Screen.zig`: **188** inline tests. `kitty/key.zig`: **5** (2 anonymous +
3 named). Rust port: **88** in `screen/tests.rs` + **5** in `screen/kitty_key.rs`
= **93**.

The port hinges on two harness helpers ported into `screen/mod.rs` under
`#[cfg(test)]`: `test_write_string` (a 1:1 port of `Screen.testWriteString`, the
"jank print" — self-contained, uses the crate's `unicode::codepoint_width`) and
`dump_string` (a restricted `.plain`-emit port of `dumpString` over the pagelist
row iterator). These unlock the cursor/scroll/clear/erase/resize/clone/dirty/
semantic-prompt tests.

| Category (upstream)                        | Ported | Notes                                                                                     |
| ------------------------------------------ | ------ | ----------------------------------------------------------------------------------------- |
| read/write (5)                             | 5      | via the harness                                                                           |
| clearRows/eraseRows (6)                    | 5      | styled-line variant deferred (needs SGR)                                                  |
| scrolling/scrollback/scroll-and-clear (16) | 13     | 2 "across pages preserves style" deferred (SGR); "moves selection" deferred               |
| clone (15)                                 | 8      | 7 selection-carrying clones deferred                                                      |
| clear history / clear above (4)            | 4      |                                                                                           |
| resize no-reflow (11)                      | 11     | incl. trims-blank-lines (bg-cell write) + soft-wrap + semantic-prompt-preserve            |
| resize reflow (39)                         | 39     | full block incl. wide-char/spacer-head/grapheme, cursor-preserve, prompt_redraw last+true |
| kitty FlagStack (5)                        | 5      | ported from `kitty/key.zig`                                                               |

**Deferred tests (by reason):**

- **Selection (~45)**: `select*`, `selectionString`, `selectLine`, `selectWord`,
  `selectOutput`, `selectAll`, `lineIterator`, `clone contains … selection`,
  `scrolling moves selection` — `Selection.zig` is a later chunk.
- **SGR/`setAttribute` (~13)**: `style basics/reset`, `clearRows active styled
  line`, `cursorCopy style *`, `increaseCapacity cursor style ref count`,
  `setAttribute increases capacity`/`splits page`, and the "across pages
  preserves style" cursor/scroll tests (they only use `setAttribute` to plant a
  bold style they then assert survived) — `sgr.zig` is a sibling chunk.
- **Hyperlink (~9)**: `hyperlink start/end`, `hyperlink reuse`, `hyperlink cursor
  state on resize`, `cursorSetHyperlink OOM`, `cursorCopy hyperlink *`,
  `increaseCapacity cursor hyperlink` — the cursor hyperlink *caching* is ported
  and Miri-clean, but these tests assert page-level hyperlink set/URI state that
  needs deeper hyperlink query plumbing.
- **`cursorCopy` (`x/y` variant portable but deferred)**: `cursorCopy` itself is
  not ported (it migrates style+hyperlink between two screens); deferred with the
  style/hyperlink query tests.
- **`promptClickMove` (17)**: needs `Selection`/prompt-click plumbing — later chunk.
- **`resize more cols bounded scrollback keeps viewport valid`**: needs
  `scrollbar()`/`pageIterator()`/pin-scroll plumbing beyond this chunk.

### Deviations / deferrals (with reason)

- **Selection is SCAFFOLD only** — `Selection.zig` is a later chunk. The
  `selection: Option<SelectionPlaceholder>` field, `select`/`clearSelection`
  hooks, and the `dirty.selection` bit are shaped and wired, but all the
  `select*`/`selectionString`/`lineIterator`/`promptClickMove` query methods are
  NOT ported.
- **`setAttribute`** (SGR) is `sgr.zig` sibling-chunk territory; not ported
  (Terminal drives it). Cursor style caching is exercised directly instead.
- **`Style::bg_cell`** is not yet ported (deferred in the Page chunk), so
  `blank_cell()` returns the default cell when the cursor style is
  non-default-bg (TODO noted inline). This only affects bg-color preservation on
  clears — visible correctness lands with the SGR chunk.
- **`prompt_redraw == .true`** resize path needs a PageList prompt iterator that
  is not yet ported; the `.last` path (cursor row only) is ported, `.true` is a
  TODO.
- **Kitty graphics image storage** (`kitty_images`) is a separate chunk; the
  `kitty_images.dirty = true` calls become no-ops / a single bool.
- **`charsets.zig`** — stub enums only (`Charset`, `Slots`); marked
  `TODO(chunk:terminal-state)`.
- **`osc/semantic_prompt`** `Click`/`ClickEvents`/`PromptKind`/`Redraw` — narrow
  local placeholders marked `TODO(chunk:osc)`.
