# PageList: the paged scrollback list (`src/terminal/PageList.zig`)

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse --short HEAD`). This is the second half of
ghostty's signature memory design: it strings the offset-addressed [`Page`s](page-memory.md)
into an intrusive doubly-linked list and layers on the abstractions the terminal
`Screen` needs — a scrollable viewport, persistent "pins", grow/scroll/erase, and
lazy reflow on resize.

`PageList.zig` is ~14.8k lines with 205 inline `test` blocks. This document is the
maintainer-grade map of its data structures and algorithms; the Rust port lives in
`crates/ghostty-vt/src/pagelist/` with 102 ported tests (see "Test porting status"
at the end for the exact accounting against upstream's 205).

## Top-level structure

`PageList` (fields at `PageList.zig:117-204`):

| field | role |
| --- | --- |
| `pool: MemoryPool` | node/page/pin pools (see below) |
| `pages: List` | `IntrusiveDoublyLinkedList(Node)`; first = top of scrollback, last = bottom (active) |
| `page_serial` / `page_serial_min` | monotonic serial per node; `min` is the lowest still-valid serial (bumped on prune/reset) so external refs can be invalidated cheaply |
| `page_size: usize` | total bytes of allocated pages (**not** pool preheat) |
| `explicit_max_size` / `min_max_size` | scrollback byte cap (see max-size below) |
| `total_rows: usize` | cached sum of `size.rows` over all pages (for scrollbar + fast math) |
| `tracked_pins: PinSet` | the set of pins kept up to date across mutations (the crux) |
| `viewport: Viewport` | `active` \| `top` \| `pin` union |
| `viewport_pin: *Pin` | pre-allocated pin used when `viewport == .pin` (never a fallible alloc on scroll) |
| `viewport_pin_row_offset: ?usize` | lazily-computed cached offset-from-top of the viewport pin |
| `cols` / `rows` | *desired* active dimensions (pages may lag due to lazy reflow) |
| `pause_integrity_checks` | debug-only suspend counter for mid-mutation states |

### The Node and the intrusive list

`Node = { prev, next: ?*Node, data: Page, serial: u64 }` (`:44-49`). The list is
`IntrusiveDoublyLinkedList(Node)` — nodes carry their own links. This is why
`Pin.node` is a raw `*Node`: a pin refers to a stable page identity, and the list
can be spliced without moving `Page` bodies.

**Rust port:** nodes are heap-allocated and referenced by raw `*mut Node`. `prev`/`next`
and `Pin.node` are raw pointers. All list splicing is `unsafe` and isolated behind a
small `NodeList` helper; the invariant is that every `*mut Node` in the list, in a pin,
or in the viewport, points to a live boxed node owned by the pool until explicitly
destroyed.

### Three memory pools + preheating

`MemoryPool` (`:77-115`) bundles three Zig pools:

- `NodePool = MemoryPool(List.Node)` — small structs, preheated `page_preheat = 4`.
- `PagePool = MemoryPoolAligned([std_size]u8, page_size_min)` — fixed `std_size`
  page buffers, page-aligned (needs zeroed+aligned memory), preheated 4.
- `PinPool = MemoryPool(Pin)` — preheated 8.

`std_size = Page.layout(std_capacity).total_size`. Pages whose layout fits in
`std_size` are dispensed from `PagePool`; larger ("non-standard") pages are
allocated directly from the page allocator and freed individually (they cannot be
pooled). This is a recurring branch: `layout.total_size <= std_size` selects
pooled vs. heap in `createPageExt`/`initPages`/`destroyNodeExt`.

**Rust port:** the pool is modelled functionally. Node/pin allocations are `Box`-based
free-lists (preheated), and page buffers reuse `Page::init`/`Page::init_buf` over
`alloc_zeroed`. The pooled-vs-non-standard distinction is preserved because it drives
`page_size` accounting and the prune-reuse fast path in `grow`.

### Viewport

`Viewport` union (`:207-221`): `active` (pinned to the active area — a marker, no
row offset, so scrolling is write-free), `top` (farthest back), `pin` (tracks
`viewport_pin`). `getTopLeft(.viewport)` resolves each variant (`:5001-5005`).

## Capacity and sizing helpers

- `initialCapacity(cols)` (`:284-318`): `std_capacity.adjust(cols)` if it fits
  `std_size`, else a non-standard capacity with exactly `cols` columns. Always
  yields ≥1 row (comptime-verified even at max cols).
- `minMaxSize(cols, rows)` (`:233-274`): the floor for max-size. Computes how many
  std pages are needed to hold the active area, `+1` extra page (so the active area
  can straddle two pages), `× PagePool.item_size`. Guarantees grow/scroll algorithms
  always have room for the active area + 2 pages.
- `maxSize()` (`:3129-3131`) = `max(explicit_max_size, min_max_size)` — a **byte**
  budget, and a *heuristic*: it can be exceeded to fit the active area with heavy
  grapheme/style load. `explicit_max_size == 0` is special-cased to "no scrollback".

## Tracked pins — the crux

A `Pin = { node: *Node, y, x: CellCountInt, garbage: bool }` (`:5130-5140`). Untracked
pins are valid only until the next mutation. **Tracked** pins live in `tracked_pins`
and every mutating operation walks that set and adjusts or invalidates each affected
pin. `garbage` is set when a pin's page was pruned and no sensible new location exists.

`trackPin` (`:3979-3992`) allocates from `PinPool`, copies the pin, inserts into the
set. `untrackPin` (`:3995-4000`) removes + frees (asserting it isn't the viewport pin).
`trackedPins()` returns the key slice; `countTrackedPins` the count. `pinIsValid`
(`:4015-4028`, debug-only) walks the list to confirm `node` present and `y/x` in bounds.

### Every pin fixup site (exhaustive)

The pin-adjustment discipline is what keeps pins valid. Each mutating op contains a
loop over `tracked_pins.keys()`:

- **`grow` prune path** (`:3205-3215`): pins on the pruned first page → move to new
  first page top-left, `garbage = true`. Viewport pin never garbage.
- **`increaseCapacity`** (`:3373-3378`): pins on the old node → repoint `node` to the
  new node (same y/x; capacity-only change).
- **`eraseRow`** (`:3517-3522`, `:3567-3579`): in the target page, pins with `y > pn.y`
  shift up by 1. In each following page, a pin at `y==0` moves to the previous page's
  last row; else `y -= 1`.
- **`eraseRowBounded`** (`:3638-3650`, `:3682-3692`, `:3733-3743`, `:3766-3776`): same
  shape but bounded by `limit`; pins in the shifted window shift up (or clamp x at
  `y==0`), and across the page boundary at the limit, `y==0` pins move to prev page.
- **`eraseRows`** (`:3881-3890`): partial-page chunk — pins with `y >= chunk.end`
  shift `y -= chunk.end`; pins in the erased rows collapse to `(0,0)`.
- **`erasePage`** (`:3941-3950`): pins on the removed page → move to prev-or-next page
  `(0,0)` (**not** garbage; the move is sensical). Also bumps `page_serial_min` when
  removing the first page.
- **`reset`** (`:745-757`): all pins → first page `(0,0)`, `garbage = true`; viewport
  pin un-garbaged.
- **`reflowRow`** (four sites, see reflow below): the most intricate — pins are moved
  from source rows to their reflowed destination positions.
- **`resizeWithoutReflow` shrink cols** (`:2082-2085`): pins with `x >= cols` clamp to
  `cols - 1`.
- **`resizeWithoutReflowGrowCols`** (`:2310-2315`, `:2387-2394`): pins on rows copied to
  the prev page or split into a new page → repoint node + adjust y.
- **`trimTrailingBlankRows`** (`:2460-2465`): a row containing any tracked pin is *not*
  trimmed (trimming would invalidate the pin).
- **`clone`** (`:865-879`): pins inside the cloned chunk are duplicated into the new
  pagelist and recorded in the caller's remap map.

### Viewport-cache fixups

Many of the same ops also patch `viewport_pin_row_offset` in place (rather than
invalidating) for performance: `grow` prune (`:3190-3203`), `eraseRowBounded` (several
`viewport:` blocks), `fixupViewport` (`:3093-3120`), and the delta-row scroll fast
paths. `resize` just invalidates it (`:966`) because getting it right is too fiddly.

`fixupViewport(removed)` (`:3093-3120`): after row removal, promote `pin→active` if the
pin is now in the active area, `pin→top`/decrement the cache, or `top→active`.

## Grow and scrollback eviction (byte-based)

`grow()` (`:3140-3264`) adds exactly one active row:

1. **Fast path**: last page has spare capacity rows → `size.rows += 1`, `total_rows += 1`,
   return null.
2. **Prune path** (`:3169-3247`): if `pages.first != last` and adding a std page would
   exceed `maxSize()` (in **bytes**), pop the first page. If that would drop us below
   the active row requirement, undo and fall through. Otherwise: fix viewport cache,
   mark pins garbage, and — if the page is std-sized — *reuse* its buffer: zero it,
   re-init as the new last page with 1 row, bump `page_serial_min` and reassign serial.
   Non-standard first pages are destroyed instead. **No new allocation.**
3. **Alloc path** (`:3249-3263`): `createPage(initialCapacity(cols))`, append, 1 row.

`scrollClear` (`:2779-2807`): counts non-empty active rows from the bottom, then
`grow()`s that many times (pushing the screen into scrollback).

`compact` (`:2822-...`): rebuild a non-standard page at its `exactRowCapacity` to reclaim
memory (no-op for std-or-smaller pages given the current single-pool design).

## Scroll (viewport only — never allocates)

`scroll(behavior)` (`:2518-2719`), `Scroll` union (`:2485-2513`): `active`, `top`,
`row(n)`, `delta_row(isize)`, `delta_prompt(isize)`, `pin(Pin)`. `explicit_max_size == 0`
forces `active`. The delta-row and row paths have fast paths that move `viewport_pin`
via `up/downOverflow` and patch the cache; slow paths traverse from the nearer end.
`scrollPrompt` (`:2723-2775`) walks `promptIterator` `delta` prompts, skipping
continuation lines. `pinIsActive`/`pinIsTop` decide whether to collapse to the `active`/
`top` markers rather than a tracked pin.

## Erase (physical row removal)

- `eraseRow(pt)` (`:3500-3584`): remove one row, rotating all following rows up one,
  threading across page boundaries via `cloneRowFrom` of the top row into the previous
  page's last row. Hot path — marks whole pages dirty.
- `eraseRowBounded(pt, limit)` (`:3591-3782`): like `eraseRow` but only shifts `limit`
  rows, leaving a blank below. Lots of duplicated hot-path code.
- `eraseRows(tl, bl)` (`:3813-3915`) + wrappers `eraseHistory` (`:3786-3791`),
  `eraseActive` (`:3795-3801`): iterate chunks; full-page chunks `erasePage`; partial
  chunks swap surviving rows to the top and clear the rest; then fix `total_rows`,
  regrow the active area if `.active` was erased, and `fixupViewport`. **Constraint**:
  only front/back pages may be fully erased (middle erasure would leave serial gaps).

## Resize + reflow (the hardest part)

`resize(opts)` (`:952-1015`). `Resize = { cols?, rows?, reflow=true, cursor? }`
(`:927-948`). **What triggers reflow:** only a **column change**. If `!reflow` or
`cols` is unchanged, everything routes through `resizeWithoutReflow`. The dispatch
(`:984-1004`):

- `cols == self.cols`: `resizeWithoutReflow(opts)`.
- `cols > self.cols` (grow): `resizeCols` (unwrap), then `resizeWithoutReflow` (rows).
- `cols < self.cols` (shrink): `resizeWithoutReflow` with `cols` pinned to old (apply
  row change first), then `resizeCols` (wrap).

Reflow is **lazy** only in the sense that individual pages may temporarily be a
different width than `self.cols` — but `resizeCols` eagerly rewrites the entire page
list. There is no deferred/on-demand reflow; the laziness is that pages keep their old
column count until a resize touches them, and `self.cols` is the source of truth.

### `resizeCols` (`:1018-1216`)

1. **preserved_cursor** (`:1031-1078`): if a cursor is given, track a pin at it, and
   count `wrapped_rows` = wrap-continuation rows above the cursor in the active area, and
   `remaining_rows` below it. Used at the end to grow just enough that reflow doesn't
   pull the cursor's original contents into scrollback.
2. Set `self.cols = cols` (after resolving the cursor pin in old coords).
3. Create `first_rewritten_node` sized to fit `cols` (`:1087-1108`).
4. Grab a `screen` rowIterator over the *old* list, then orphan the old list by pointing
   `pages.first/last` at the new node (`:1129-1132`).
5. Drive a `ReflowCursor` over every old row via `reflowRow`; destroy each source page
   once its last row is consumed (`:1135-1155`). Set `total_rows` from the cursor.
6. If reflow produced fewer than `self.rows` rows, `grow()` up (`:1160-1167`).
7. Fix viewport if the pin fell into the active area (`:1173-1178`).
8. preserved-cursor growth (`:1181-1215`).

### `ReflowCursor` (`:1222-2038`)

Fields `{ x, y, pending_wrap, node, page, page_row, page_cell, new_rows, total_rows }`.
It writes into the destination page as it consumes source rows.

- `reflowRow` (`:1254-1425`): trims non-semantic trailing blanks (unless the row wraps);
  handles tracked-pin remapping in the trailing blanks and cursor region; defers blank
  rows (`new_rows`) so trailing blanks never get written; then for each source column,
  handles `pending_wrap` (set `wrap`/`wrap_continuation`, scroll-or-new-page), remaps
  pins at that x, and `writeCell`. On `writeCell` returning `.skip_next` (1-col wide
  destruction) or `.repeat` (wide char needs spacer-head + wrap first), advances x
  accordingly. On `OutOfSpace`, `moveLastRowToNewPage` and retry the same cell (unless
  already on row 0, where it degrades gracefully).
- `writeCell` (`:1439-1806`): copies the unmanaged cell bits first (with wide/spacer/
  1-col edge cases), then failably copies grapheme → hyperlink → style managed memory,
  increasing page capacity (via `list.increaseCapacity`, which reinits the cursor)
  and retrying as needed. Returns `success`/`repeat`/`skip_next`.
- `cursorForward` / `cursorScroll` / `cursorNewPage` / `cursorScrollOrNewPage` /
  `cursorAbsolute` (`:1922-2015`): destination-page navigation, adding rows/pages as
  the reflowed text grows.
- `moveLastRowToNewPage` (`:1820-1885`): splits the current dst row onto a fresh page
  when capacity is exhausted mid-row; moves pins on that row across.
- `increaseCapacity` (`:1888-1914`): pauses integrity checks, grows the dst page,
  reinits the cursor at the same absolute position.

### `resizeWithoutReflow` (`:2040-2194`)

Cols first (so col-growth frees page bytes before row grow avoids pruning):
- shrink cols: clear beyond-`cols` cells per row, set `page.size.cols`, clamp pins.
- grow cols: per chunk, `resizeWithoutReflowGrowCols` (`:2196-2406`) — fast path if the
  page already has col capacity (unless a stale `spacer_head` sits at the old last col),
  else allocate wider page(s), filling a prev page's spare capacity first, splitting as
  needed, remapping pins throughout.

Rows:
- shrink rows: `trimTrailingBlankRows` first (Terminal.app behavior), then lower
  `self.rows` (creating history for the remainder).
- grow rows: preserve cursor y if not at bottom (don't pull scrollback), else pull down
  scrollback / `grow()` the shortfall; fix viewport pin if it lands in active.

`trailingBlankLines`/`trimTrailingBlankRows` (`:2410-2482`): count/remove trailing
text-free rows (bounded by `max`); never trim a row holding a tracked pin.

## Iterators

`PageIterator` (`:4771-4955`) yields `Chunk = { node, start, end }` — the maximal
contiguous row range in one page for a region, in either direction, bounded by an
optional limit pin. `RowIterator` (`:4704-4745`) and `CellIterator` (`:4642-4683`) layer
row/cell granularity on top and yield `Pin`s. `Pin.pageIterator/rowIterator/cellIterator`
(`:5195-5243`) are the pin-based entrypoints (cheaper than going via points).
`PromptIterator` walks semantic-prompt rows.

`getTopLeft(tag)` (`:4996-5026`) / `getBottomRight(tag)` (`:5031-5056`): resolve the
corners of `screen`/`active`/`viewport`/`history`. `pin(pt)` (`:3963-3973`) = topLeft +
down(y) + set x. `pointFromPin` (`:4068-4100`) is the inverse (list traversal).
`getCell(pt)` (`:4106-...`) reads a cell.

## Integrity checking

`verifyIntegrity` (`:544-625`, debug-only, suspended by `pause_integrity_checks`):
sums page rows == `total_rows`; no node serial below `page_serial_min`; every tracked
pin valid; if `viewport == .pin`, the cached offset matches a fresh computation and the
viewport has ≥ `self.rows` rows below it. `IntegrityError` enumerates the failures.
`assertIntegrity` panics on violation. The Page chunk left the per-page `verifyIntegrity`
as a no-op; **this chunk implements it** (grapheme⇔map, style ref accounting,
spacer placement, hyperlink flag/map/set agreement) because the reflow/clone paths
exercise it hard.

## Clone

`clone(alloc, opts)` (`:788-924`), `Clone = { top, bot?, tracked_pins? }` (`:763-779`):
count chunks for exact preheat, build a fresh pool + pagelist, `cloneFrom` each chunk's
rows into a same-capacity page, optionally remap tracked pins into the clone (recorded
in the caller's `TrackedPinsRemap`), and grow to at least the active area. Viewport
resets to active.

## Port notes (Rust)

- **Module layout** (as built): `src/point.rs` (Point/Coordinate/Tag — the small
  dependency ported alongside), `src/pagelist/mod.rs` (PageList struct + `Node`/`NodeList`
  intrusive list + `MemoryPool` + init/reset/integrity + test accessors),
  `pagelist/ops.rs` (grow/prune, scroll/scrollClear, increaseCapacity, getCell,
  scrollbar, Cell), `pagelist/pin.rs` (Pin + up/down/left/right/overflow +
  before/isBetween + trackPin/untrackPin + viewport helpers + getTopLeft/BottomRight +
  pointFromPin), `pagelist/iter.rs` (Page/Row/Cell iterators + Chunk),
  `pagelist/reflow.rs` (`ReflowCursor` + `resizeCols`), `pagelist/resize.rs`
  (resizeWithoutReflow family + erase/eraseRowBounded/eraseRows/erasePage + reset + clone
  + split + compact + trim/trailing-blank).
- **Nodes**: `*mut Node` raw pointers, `Box`-owned nodes vended by `MemoryPool` (a
  free-list; `Box` gives the stable address the raw pointers require). List splicing is
  `unsafe` behind `NodeList`. `Pin.node`, `Node.prev/next`, viewport all hold `*mut Node`.
  Invariant: a node pointer is valid until `destroy_node`.
- **Pools**: functional free-lists for nodes/pins (preheated), page buffers via
  `Page::init`. The std-vs-non-standard branch (`byte_len > std_size`) is preserved for
  byte accounting and the grow-prune buffer-reuse fast path.
- **Unsafe boundary**: all raw-pointer node/row/cell access is isolated in the pagelist
  module; the public API is safe (`clippy::not_unsafe_ptr_arg_deref` is allowed
  module-wide only for the handful of methods that take a pin/node handle this list
  itself vended, mirroring the Zig API). Reflow's per-cell managed-memory copy delegates
  to a new `Page::reflow_copy_managed` method so the page-internal offset/unsafe work
  stays in the page layer; it returns the exhausted capacity dimension so `ReflowCursor`
  can grow the page and retry.
- **grow** returns `bool` publicly (a page was added) vs. Zig's `?*Node`; an internal
  `grow_node` returns the node for the paths that need it (tests, prune, alloc).
- **Deviations from Zig** (idiom, not semantics): infallible allocation (no
  `Allocator.Error` threading — `init`/`resize`/`grow` don't return errors, so the
  tripwire allocator-failure tests don't apply); `page_serial`/`page_serial_min`/`serial`
  ported for parity even though external stale-ref detection isn't yet consumed.
- **Page-layer additions** for this chunk: `Page::verify_integrity` (full port of the
  debug cross-check suite, previously a no-op) + `assert_integrity`/`pause_integrity_checks`
  wired live under `cfg(debug_assertions)`; `Cell::has_text_any`; `Page::byte_len`;
  `Page::reflow_copy_managed` (+ `ReflowManagedError`); `size_of_std_page`/`page_byte_len`/
  `layout_total_size`/`style_default_id` free helpers.

### Out of scope for this chunk (deferred, with reason)

- **`highlightSemanticContent`** (17 tests) — depends on the separate `highlight.zig`
  module; lands with the highlight/selection chunk.
- **`diagram`** — an ASCII debug-render helper on top of the iterators; cosmetic, deferred.
- The C ABI (`cval`) on Scrollbar/Point/Pin — lands with the libghostty-vt C-API chunk.

### Test porting status (exact)

Upstream `PageList.zig`: **205** inline tests. Rust port: **102** tests in
`pagelist/tests.rs`, covering every in-scope category:

| Category (upstream count) | Ported | Notes |
| --- | --- | --- |
| resize/reflow (71) | 25 | every semantically distinct behavior: no-reflow rows/cols each direction and combined, trim-blank, reflow wrap/unwrap, wide-char destroy/spacer-head wrap, grapheme reflow, kitty placeholder, semantic prompt, capacity-increase-forcing reflow, viewport-cache invalidation |
| scroll (20) | 15 | top/active/pin/row/delta fast+slow paths, cache fast paths, max-size-0 |
| highlightSemanticContent (17) | 0 | needs `highlight.zig` — separate chunk |
| split (16) | 8 | middle/0/single-row/pin-tracking (before/at/after/multi)/wrap/styled/first-page |
| erase + eraseRow(Bounded) (20) | 10 | history/active/row/bounded/pins/page-size/viewport fixups |
| increaseCapacity (9) | 7 | all four dimensions + pins + dirty + OutOfSpace-at-max |
| clone (9) | 6 | full/partial/less-than-active/remap in+out/dirty |
| grow (8) | 5 | fit-in-capacity/allocate/prune-reuse (incl. pins, serials, byte accounting) |
| iterators (14) | 6 | page fwd/rev 1-2 pages, cell fwd/rev, prompt jump 0/-1 |
| reset/init/pointFromPin/misc (21) | 20 | includes 4/4 pointFromPin, 3/4 reset, scrollbar |

Not ported 1:1: the tripwire allocator-failure-injection tests ("init error" and
friends — Zig-only fail-point machinery; the Rust model is infallible-alloc),
`diagram`/`scrollbar`-rendering internals, and near-duplicate permutations within
the resize family whose distinct behaviors are covered above. Porting the remaining
permutations is mechanical (helpers exist in `tests.rs`) and can be done incrementally
when the Screen chunk starts consuming these APIs.
