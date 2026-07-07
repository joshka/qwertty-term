# Terminal search subsystem (`src/terminal/search/*.zig`)

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse --short HEAD`). This chunk ports the **core** of the
search subsystem: the sliding-window matcher plus the three synchronous, viewport-aware
search entry points that drive it. The async `Thread.zig` wrapper (Phase 2) and the
`ScreenSearch` result-cache (`screen.zig`, depends on the not-yet-ported `Screen`) are
**deferred** and documented below for the future chunks.

The Rust port lands under `crates/ghostty-vt/src/search/`:

| Zig file (lines)             | Rust file                        | Ported here? |
| ---------------------------- | -------------------------------- | ------------ |
| `sliding_window.zig` (1676)  | `search/sliding_window.rs`       | yes          |
| `active.zig` (175)           | `search/active.rs`               | yes          |
| `viewport.zig` (384)         | `search/viewport.rs`             | yes          |
| `pagelist.zig` (441)         | `search/pagelist.rs`             | yes          |
| `screen.zig` (1618)          | —                                | deferred (needs `Screen`) |
| `Thread.zig` (905)           | —                                | deferred (Phase 2, async) |

## The key finding: search is **literal case-insensitive ASCII substring**, not regex

The single most load-bearing fact for the port: **upstream search uses no regex engine at
all.** The entire matcher is `std.ascii.indexOfIgnoreCase` over an encoded byte buffer
(`sliding_window.zig:179, 205, 219`). There is no oniguruma, no PCRE, no `std.regex` — the
"needle" is a raw `[]const u8` compared case-insensitively byte-for-byte.

Consequences:

- **Case handling is ASCII-only.** `indexOfIgnoreCase` folds only `A-Z`/`a-z`; non-ASCII
  bytes match exactly. The Zig test `SlidingWindow single append case insensitive ASCII`
  (`:690`) pins this: `"Boo!"` matches `"boo!"`.
- **Reverse search** is done by reversing *both* the needle and the encoded page bytes at
  `append` time, then searching forward, and un-reversing the resulting chunk geometry
  (`sliding_window.zig:112-115, 588-594, 463-512`). No separate reverse matcher.
- **No dependency needed in Rust.** Per the rewrite-prompt decision table, literal-substring
  search means **zero regex crate**. `indexOfIgnoreCase` ports to a small hand-rolled
  ASCII-case-insensitive `memmem` (a windowed `eq_ignore_ascii_case` scan). `ghostty-vt`
  gains **no new dependency** and **no feature flag** — the preferred outcome the prompt
  called out. (If upstream ever adds regex, that is a future additive feature gate; today it
  would be dead code.)

## `SlidingWindow` — the matcher (`sliding_window.zig:38-633`)

The core engine. It maintains a **circular byte buffer** of encoded page text plus a
parallel **metadata buffer** describing which page node each byte range came from, and
searches for the needle incrementally as pages stream in and out. The invariant
(`:16-18`): a page's data is never pruned until (1) it has been searched and (2) enough
tail bytes are retained to catch a needle that straddles the next page boundary.

### State (`:45-85`)

- `data: CircBuf(u8)` — encoded page text. Rust: `VecDeque<u8>` (its `as_slices()` gives the
  two-slice wrapped view Zig gets from `getPtrSlice`; `drain(..n)` / `pop_front` mirror
  `deleteOldest`).
- `meta: CircBuf(Meta)` — one `Meta` per appended node: `{ node, serial, cell_map }` where
  `cell_map: []Coordinate` maps **each encoded byte** to its `(x, y)` cell within the page.
  Rust: `VecDeque<Meta>` with `cell_map: Vec<Coordinate>`.
- `chunk_buf` — scratch `MultiArrayList(Flattened.Chunk)` reused across `next()` calls so the
  returned `Flattened` needs no fresh allocation. Rust: `Vec<highlight::Chunk>`.
- `data_offset` — cursor into `data` marking where the *next* search begins (so a partially
  consumed `meta[0]` isn't re-searched). `:61-64`.
- `needle: []u8` — **owned** (duped at init, reversed for reverse direction). `:66-67`.
- `direction: {forward, reverse}` — append order + reversal semantics. `:69-85`.
- `overlap_buf: [needle.len*2]u8` — scratch for the cross-page overlap search. `:81-84`.

### The matcher: `next()` (`:166-283`)

Searches the current window for the next needle occurrence, pruning as it goes. Returns a
`Flattened` highlight (referencing internal `chunk_buf` memory, valid only until the next
`next()`/`append()`; clone to retain — `:161-165`). Algorithm:

1. If `data.len() < needle.len`, return `null` (can't match). `:169-170`.
2. Take the two-slice view from `data_offset`. Search `slices[0]` with
   `indexOfIgnoreCase`; on hit → `highlight(idx, needle.len)`. `:172-184`.
3. **Overlap search** (`:187-216`): if both slices are non-empty (the circular buffer
   wrapped), build `overlap_buf` from up to `needle.len-1` bytes of `slices[0]`'s suffix +
   `slices[1]`'s prefix and search that. This is the *within-buffer* wrap, distinct from the
   cross-page overlap. Map a hit back into data-buffer coordinates.
4. Search `slices[1]`. `:218-224`.
5. **No match**:
   - Single-char needle special case: clear the whole buffer, return `null`. `:226-231`.
   - Otherwise **prune** (`:235-275`): walk `meta` in reverse, retaining just enough trailing
     metas to hold `needle.len-1` bytes (the max a straddling match could need), and
     `deleteOldest` everything before that from both `meta` and `data`. Then set
     `data_offset` to `data.len() - needle.len + 1` so the retained tail is re-examined on the
     next `append`. `:277-282`.

### `highlight(start_offset, len)` → `Flattened` (`:296-517`)

Maps a byte-range match back to page-node chunks:

- Walk `meta` forward accumulating `cell_map` lengths to find which meta holds the match
  **start**; the fast path (likely) is start and end in the *same* meta (`:347-368`). If the
  match spans metas, emit a start chunk (`start.y .. page.rows`), zero-or-more full middle
  chunks, and an end chunk (`0 .. end.y+1`) (`:369-434`). `top_x`/`bot_x` come from the
  `cell_map` `x` of the matched bytes.
- Advance `data_offset` past the match (`+1` so the same match isn't re-returned), and prune
  fully-consumed leading metas/data (`:436-461`).
- **Reverse fixup** (`:463-512`): the chunks were built in forward-of-reversed order, so
  reverse the chunk arrays and invert the first/last chunk's `start`/`end` geometry and swap
  `top_x`/`bot_x`. Single-chunk case just swaps the y-pair.

### `append(node)` → bytes added (`:526-607`)

Encodes one page node into `data`/`meta`:

1. Run the page through `PageFormatter` with `{ .emit = .plain, .unwrap = true }` and a
   `point_map` so every emitted byte gets a `Coordinate`. `:546-562`.
2. If the node's last row is **not** soft-wrapped, append a trailing `'\n'` and map it to the
   last coordinate (so a needle can't match across an explicit line break). `:564-576`.
3. Empty encode (whitespace-only page) → add nothing (regression test `:1650`). `:578-584`.
4. Reverse direction → reverse the bytes and the `cell_map`. `:586-594`.
5. Ensure capacity, append to `data`/`meta`. `:596-606`.

**Critical port dependency — `point_map`.** The Rust `formatter.rs` port explicitly
*deferred* `point_map`/`pin_map` (see `formatter.rs:18`) and only exposes a whole-`Screen`
`render_range`, not per-`Page` encoding. Search needs per-node encoding **with** the
byte→coordinate map. So the port lands a self-contained `encode_page_plain(node)` helper in
`search/sliding_window.rs` that mirrors the **plain + unwrap** subset of
`PageFormatter.formatWithState` + its `point_map` accounting
(`formatter.zig:797-1360`). The plain path is much smaller than the full formatter (no
headers, styles, or hyperlinks) — only
  - blank-cell runs → spaces, each mapped to the coordinate reached by walking back from the
    current `(x,y)` (`formatter.zig:1150-1183`),
  - codepoint/grapheme cells → their UTF-8 bytes, each mapped to `(x,y)`
    (`formatter.zig:1305-1324`),
  - deferred blank rows flushed as `'\n'`, mapped to the prior row's coordinate
    (`formatter.zig:1078-1103`).
This keeps `formatter.rs`'s deferral intact and confines the search-only encoding to the
search module.

### Test-only helper

`testChangeNeedle` (`:610-614`) swaps the needle in place (same length) so the
circular-buffer-boundary tests can force a prune with one needle then match with another.
Ported as a `#[cfg(test)]` method.

## `ActiveSearch` (`active.zig:20-100`)

Searches only the **active area** (the mutable bottom of the PageList). `init` builds a
**forward** `SlidingWindow` (`:26-33`; active area is small, so forward is fine and skips the
reversal work). `update(list)` (`:53-93`) clears the window, appends the last-N pages
covering `list.rows` active rows (walking `pages.last` backward), then adds one-or-more
**prior** soft-wrapped pages to cover `needle.len-1` bytes of overlap. It returns the
first (reverse-order) node it covered so a `PageListSearch` can hand off history search from
there (`:44-52`). `next()` just delegates to `window.next()`. This is the entry point
re-run every frame as the active area mutates.

## `ViewportSearch` (`viewport.zig:26-263`)

Searches the pages the **viewport** covers, with change detection so it only re-searches when
needed. A `Fingerprint` (`:230-263`) is the ordered list of node pointers the viewport spans;
`update()` (`:74-192`) rebuilds the fingerprint and:

- If it equals the old fingerprint **and** (optionally, via `active_dirty` tracking) the
  active area is clean, return `false` (no re-search). `:82-121`.
- Otherwise clear the window, append the fingerprint nodes plus leading/trailing soft-wrap
  overlap pages, and return `true`. `:122-191`.

`active_dirty: ?bool` (`:36-39`) is a 3-state knob: `null` = always re-search when the
viewport overlaps the active area; `Some(false)` = clean, skip; `Some(true)` = dirty,
re-search (and reset to `false`). Dirty marking is the caller's responsibility. Ported 1:1.

## `PageListSearch` (`pagelist.zig:32-135`)

Searches the whole PageList **in reverse** (most-recent-first) from a starting node. `init`
(`:47-74`) tracks a pin at the start node (so pruning can't invalidate our position), builds
a **reverse** window, and feeds the start page. `next()` delegates. `feed()` (`:112-134`)
appends `prev` nodes until `needle.len` more bytes are loaded (enough for at least one
match), advancing the tracked pin; returns `false` when the pin goes `garbage` (its page was
reused — equivalent to reaching the list start) or there are no more nodes. Assumes nodes are
immutable — the caller pairs it with `ActiveSearch` for the mutable tail (`:15-30`).

## Match representation → highlight conversion

Every searcher returns `highlight.Flattened` (already landed at
`crates/ghostty-vt/src/highlight.rs` by the highlight chunk). `Flattened` is the
mutation-robust currency of the whole search + renderer pipeline (see `highlight.md:126-136`):
each `Chunk = { node, serial, start_row, end_row }` plus window-level `top_x`/`bot_x`. Tests
collapse it via `Flattened.untracked()` → `Untracked { start, end }` pins and assert
`pages.point_from_pin(tag, pin)`. The port wires the sliding window's `chunk_buf` entries
straight into `highlight::Chunk` and returns `highlight::Flattened`; no new conversion type is
introduced. `ScreenSearch` (deferred) is where a *selected* match gets promoted `Untracked ->
Tracked` for cross-frame persistence.

## Incremental invalidation on scroll / prune

- **Scroll (active area grows):** `ActiveSearch.update` / `ViewportSearch.update` are
  re-invoked; they `clearAndRetainCapacity` the window and re-append the current pages. The
  window owns no page memory — it copies text at append time — so a mutated active area is
  simply re-encoded.
- **Prune (scrollback trimmed):** `PageListSearch` holds a **tracked pin**; when the
  PageList reuses/prunes the pinned page, the pin becomes `garbage` and `feed()` reports "no
  more data". The sliding window is told nothing directly — the invariant is that if any
  fed page becomes invalid, the caller clears the window and restarts (`:33-37`).
- **Fingerprint (viewport moved):** node-pointer identity comparison detects viewport
  movement; a moved viewport forces a re-search, an unchanged one (clean active area) is a
  no-op.

## `Thread.zig` — deferred (Phase 2). Interface notes

`Thread.zig` (905 lines) is the **async wrapper**, not part of the core matcher, and is
deferred with its tests. It is a libxev-driven thread that owns a `ScreenSearch`
(`screen.zig`) and a `ViewportSearch`, runs a refresh timer (`REFRESH_INTERVAL = 24ms`,
~40 FPS, `:39-42`) to poll for terminal changes, and posts flattened-highlight results back
to the renderer. Its public surface (for whoever ports Phase 2):

- **`Thread.init(alloc, opts) / deinit / threadMain`** — standard ghostty thread trio;
  `threadMain` names the thread, drops to `.utility` QoS on Darwin, and interleaves search
  work with the xev loop (`Thread.zig:80-200+`).
- **`Mailbox`** — a `BlockingQueue` of messages (start/stop/set-needle/viewport-changed etc.)
  driven by an `xev.Async` wakeup handle; `stop` is a second async; `refresh` is an
  `xev.Timer`.
- **`Options` + `event_cb`** — a callback (`.quit`, results-ready, …) with userdata, invoked
  to wake the renderer.
- **`Search`** — the per-active-search state bundle the thread owns (needle + `ScreenSearch`
  + `ViewportSearch`), created when a needle is set, torn down on clear.

Porting it requires: an async runtime decision (Rust has no libxev; likely a dedicated
`std::thread` + channel, or tokio if the app adopts it), the `ScreenSearch` result cache,
and the renderer message plumbing — all out of scope here. The core landed in this chunk is
**thread-ready**: every searcher is a plain synchronous struct with `update`/`feed`/`next`
that a future thread can call under a lock, exactly as Zig's thread does.

## Rust port notes

- **Module** `crates/ghostty-vt/src/search/` (`mod.rs` + `sliding_window.rs`, `active.rs`,
  `viewport.rs`, `pagelist.rs`), registered `pub mod search;` in `lib.rs`. In-crate because
  it needs `pub(crate)` PageList internals (`Node`, `node_page`, `Node.{next,prev,serial}`,
  `Pin.node`) and the crate-private `highlight::Chunk.node`.
- **No new dependency, no feature flag** — literal ASCII substring search (see key finding).
- **`CircBuf` → `VecDeque`.** `getPtrSlice` → `as_slices()`; `deleteOldest` → `drain`/
  `pop_front`. The circular-boundary tests assert the two-slice shape, which `VecDeque`
  reproduces once it wraps.
- **`point_map` encoder** ported as a search-local `encode_page_plain` (plain+unwrap subset
  of `PageFormatter`), because `formatter.rs` deferred `point_map` and exposes no per-page
  path.
- **Infallible alloc** — `SlidingWindow.init`/`append` are `Allocator.Error!…` in Zig; the
  Rust model allocates infallibly (matching the PageList/highlight ports) so these return the
  value directly (`append` still returns the byte count).
- **Tests** ported 1:1 per file (no consolidation): sliding_window 25, active 2, viewport 4,
  pagelist 7. `\r\n` in the Zig vt-stream setups maps to `carriage_return()`+`linefeed()`;
  `\x1b[2J`/`\x1b[H` map to `erase_display(Complete)`/`set_cursor_pos(1,1)`; needle-swap tests
  use a `#[cfg(test)]` `test_change_needle`. One test-only environment note: the Zig
  `Terminal.init` default `max_scrollback = 10_000` is what lets pages spill; the Rust test
  helpers pass it explicitly.
- **Miri** (bounded, `-Zmiri-disable-isolation`): 20/38 tests pass with no UB — all 14
  single-page sliding-window tests (incl. circular-buffer wrap, boundary matches, reversed,
  soft-wrapped, whitespace-only), both active tests, 3/4 viewport, pagelist `simple_search`.
  The remaining 18 exceed a 3-minute per-test budget under Miri purely from setup cost
  (filling a whole page — thousands of interpreted row writes — or ~15k `grow` calls):
  sliding_window `two_pages*` (11), viewport `history_search_no_active_area`, pagelist
  `feed_*` (6). All pass under normal `cargo test`; the paths they add (cross-meta chunk
  spanning, tracked-pin prune) share the pointer discipline Miri validated on the
  single-page paths.
