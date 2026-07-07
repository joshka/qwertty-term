# Page memory model (`src/terminal/page.zig` + support structures)

Surveyed against ghostty commit `2da015cd6`.

The page is the unit of scrollback memory: one contiguous, page-aligned, zero-initialized
allocation holding a fixed-capacity grid plus all its side tables, addressed entirely by
**byte offsets from the allocation base** so the whole block can be memcpy'd, mmap'd,
pooled, or serialized without fixups. `PageList.zig` (out of scope here, next chunk)
strings pages into an intrusive doubly-linked list; everything below is the intra-page
story.

## File map

| File                                | Role                                                                       |
| ----------------------------------- | -------------------------------------------------------------------------- |
| `src/terminal/size.zig`             | `Offset(T)` typed byte offsets, `OffsetBuf` layout cursor, int-size policy |
| `src/terminal/bitmap_allocator.zig` | `BitmapAllocator(chunk_size)` — offset-based chunk allocator               |
| `src/terminal/ref_counted_set.zig`  | `RefCountedSet(T, Id, RefCountInt, Context)` — dedup + refcount            |
| `src/terminal/hash_map.zig`         | `OffsetHashMap(K, V)` — stdlib HashMap fork with offset-based storage      |
| `src/terminal/style.zig`            | `Style` value type + `style.Set = RefCountedSet(Style, u16, u16, …)`       |
| `src/terminal/hyperlink.zig`        | `PageEntry` (offset-based hyperlink), `Set`, `Map`                         |
| `src/terminal/page.zig`             | `Page`, `Row`, `Cell`, `Capacity`, layout math, integrity checks           |

## Why offsets instead of pointers

Every intra-page reference (`rows`→cells, grapheme map values, hyperlink URI slices,
set/table arrays) is an `Offset(T)` — a `u32` byte offset from the *true base* of the
page allocation (`size.zig:44-71`). Consequences:

- `cloneBuf` is a single `memcpy` of the backing memory plus a copy of the (plain-value)
  `Page` struct with `memory` repointed (`page.zig:637-653`). Nothing inside needs fixups.
- Pages can live in pooled slabs, be written to disk, or move between processes.
- `max_page_size = maxInt(u32)` (`size.zig:8`) so `OffsetInt = u32`; ID types are sized so
  a maxed page is still fully addressable: `CellCountInt = u16` (cols/rows),
  `StyleCountInt = HyperlinkCountInt = u16` (≤ one style/link per cell of a row-splittable
  page), `GraphemeBytesInt = StringBytesInt = u32` (`size.zig:22-38`).

`OffsetBuf` (`size.zig:84-145`) is the layout cursor used during init: it carries the true
base plus a running offset, so nested structures record offsets **against the true base**
(not their own sub-slice), letting all runtime calls pass `page.memory` as the base.
`getOffset(T, base, ptr)` (`size.zig:149-158`) is the inverse (pointer → offset).

## Single-allocation layout

`Page.layout(cap)` (`page.zig:1704-1775`) packs, in order, each section aligned forward to
its natural/base alignment, total aligned up to the OS page size:

```text
[Rows]          cap.rows × Row (8 B each)
[Cells]         cap.rows × cap.cols × Cell (8 B each)
[Styles]        RefCountedSet(Style): [table: u16 × table_cap][items: Item × cap]
[GraphemeAlloc] BitmapAllocator(16): [bitmaps: u64 × n][chunks]
[GraphemeMap]   OffsetHashMap(Offset(Cell), Offset(u21).Slice)
[StringAlloc]   BitmapAllocator(32): [bitmaps][chunks]
[HyperlinkSet]  RefCountedSet(hyperlink.PageEntry)
[HyperlinkMap]  OffsetHashMap(Offset(Cell), hyperlink.Id)
```

Derived capacities inside `layout`:

- grapheme map capacity = `ceilPow2(divCeil(cap.grapheme_bytes, grapheme_chunk))` — one
  map slot per potential chunk (`page.zig:1721-1727`).
- hyperlink set capacity = `cap.hyperlink_bytes / @sizeOf(hyperlink.Set.Item)`; map
  capacity = `ceilPow2(hyperlink_count × hyperlink_cell_multiplier(=16))`
  (`page.zig:1736-1748`) — a link may span many cells, so the cell→id map is 16× the
  unique-link capacity.
- Chunk sizes: `grapheme_chunk = 4 × @sizeOf(u21) = 16 B` (most emoji clusters fit),
  `string_chunk = 32 B` (OSC8 IDs/URIs) (`page.zig:84-113`).

Memory is obtained via mmap/VirtualAlloc directly (`page.zig:34-82`) because the OS
guarantees zeroed pages and the alloc path is hot. **Zero-initialized memory is a valid
empty page** — this is a load-bearing invariant: `Cell`/`Row` zero values are valid,
hash-map metadata zero = "free", set table zero = "no item". `initBuf` then only has to
fix up row→cell offsets and section headers (`page.zig:239-290`). `reinit` = zero the
memory (as u64s, for speed) + `initBuf` again (`page.zig:301-306`).

### Capacity, std_capacity, adjust, maxCols

`Capacity` (`page.zig:1805-1905`): `cols`, `rows`, `styles` (default 16),
`hyperlink_bytes` (default 4 items), `grapheme_bytes` (default 1024 = 64 chunks),
`string_bytes` (default 2048 = 64 chunks). `std_capacity` = 215×215, 128 styles, 8192
grapheme bytes (512 under test builds) (`page.zig:1783-1788`) — chosen so a page is a
nice pooled size (512 KiB upstream).

`adjust(.{.cols = n})` (`page.zig:1856-1876`) keeps **total size constant** and trades
rows for columns: available grid bits = `availableBitsForGrid()` (lay the four meta
sections backward from total_size to find where styles would start; `page.zig:1883-1904`,
requires `@sizeOf(Row) % @alignOf(Cell) == 0` so rows+cells pack with no gap), then
`rows = availableBits / (bitsOf(Row) + cols × bitsOf(Cell))`. Errors OutOfMemory when 0
rows fit. `maxCols` is the dual (`page.zig:1836-1849`). PageList uses `adjust` for resize
(`PageList.zig:1089,2231`) — this is the API shape that must survive.

`exactRowCapacity(y_start, y_end)` (`page.zig:688-783`) computes the *minimal* capacity
that can hold a row range: walks cells, counts unique style IDs (bitset over u16),
grapheme bytes via `BitmapAllocator.bytesRequired` (chunk-rounded), unique hyperlinks +
their string bytes, and hyperlink *cells* (map needs `divCeil(cells, 16)` entries).
Set capacities go through `capacityForCount(n) = ceil((n+1) / 0.8125)` (load factor +
reserved ID 0). Used by PageList compaction (`PageList.zig:2834`).

## BitmapAllocator (`bitmap_allocator.zig`)

Fixed-size chunk allocator: an array of `u64` bitmaps (bit=1 ⇒ chunk free) followed by
the chunk slab, both stored as `Offset`s (`:44-48`). `alloc(T, base, n)` computes
`chunk_count = ceil(n × sizeOf(T) / chunk_size)` and scans for a run of free bits;
`free` recomputes the chunk index from pointer arithmetic and re-sets the bits
(`:107-148`). No per-allocation header: the *caller's slice length* is the record of the
allocation size (grapheme map stores `Offset(u21).Slice{offset, len}`).

`findFreeChunks` (`:238-329`): for `n ≤ 64`, the shifted-AND trick — `bitmap & (bitmap>>1)
& … & (bitmap>>(n-1))`, `@ctz` of the result is the start bit; runs ≤ 64 chunks never
cross a bitmap word. For `n > 64`: clz/ctz prefix/suffix matching across words. Alignment
is not handled generally — asserted `chunk_size % alignOf(T) == 0` (`:89`).

Layout quirk (upstream): `layout(cap)` computes `chunks_end = chunks_start +
aligned_cap * chunk_size` (`:222`) — that multiplies a **byte** count by the chunk size,
over-reserving the slab by ~chunk_size× (e.g. the default 2048-byte string area reserves
64 KiB of slab). Conversely the bitmap count is rounded up to whole u64 words
(`alignForward(chunk_count, 64)`), so for capacities under 64 chunks the bitmap
*advertises more chunks than a correctly-sized slab would hold*; upstream is protected
only by the over-reservation. **The Rust port sizes the slab as
`aligned_chunk_count × chunk_size` (exact bitmap coverage)** — identical allocator
behavior (the bitmap governs), strictly safe bounds, less waste. This changes
`Layout.total_size` values vs upstream (internal only; all capacity semantics are
bitmap-driven and unchanged).

## RefCountedSet (`ref_counted_set.zig`)

Deduplicating, ref-counted value store used for styles and hyperlinks. Two arrays, both
offset-based: `table: Offset(Id)` (open-addressed hash table mapping hash→item ID, Robin
Hood with linear probing) and `items: Offset(Item)` where
`Item = {value: T, meta: {bucket: Id, psl: Id, ref: RefCountInt}}` (`:82-102`).

- **ID 0 is reserved** (= "default"/empty); `next_id` starts at 1 (`:140`).
- `load_factor = 0.8125`; `Layout.init(cap)` rounds table_cap to a power of two and sizes
  `items` at `load_factor × table_cap` (`:170-204`). `capacityForCount(n) =
  ceil((n+1)/load_factor)` (`:75-79`).
- `add` (`:238-301`): first trims dead items (`ref == 0`) off the end of the ID space
  (reclaiming their IDs and deleting them from the table), then lookup→ref++ on hit;
  on miss inserts with `next_id`. Errors: `OutOfMemory` when IDs are exhausted and <10%
  are reclaimable, or when any item has probed to PSL 31 (crafted-hash defense,
  `psl_stats[31] > 0`, `:267-269`); `NeedsRehash` when IDs are exhausted but ≥10% of
  them are dead (dead low IDs are unreachable for reuse without a rehash).
- **Items with ref 0 are kept** ("dead", resurrectable) until their bucket or ID is
  needed: `insert` reaps dead items it probes past (calling `Context.deleted` at that
  point, and preferring to reuse their smaller ID; `:602-630`). Robin Hood swap ties are
  broken by ref count so hot items sit earlier in their probe sequence (`:638-652`).
- `deleteItem` (`:452-492`) does backward-shift deletion (pull following chain entries
  back one bucket, decrementing their PSL) and maintains `psl_stats`/`max_psl` so lookups
  can bail at `max_psl` probes (`:505`).
- `addWithId` (`:309-347`): clone fast-path — try to reuse the same ID as the source page
  (dead → replace in place; live+equal → ref++/return null) else fall back to `add`.
- `lookup` never matches dead items (`ref > 0` check, `:530-533`).
- The `Context` supplies `hash`/`eql` (and optional `deleted`); for hyperlinks it carries
  page pointers so hashing can chase offsets into page memory, and `eql`'s first argument
  is always the probe value / second always the resident value (`:40-46`) — this is what
  makes cross-page lookup (`src_page` vs `page` bases) work in `hyperlink.Set`.

The set's own `assertIntegrity` (`:671-712`) is compiled out upstream (`if (false …)`);
its checks (bucket↔item agreement, psl_stats recount, hash(value)+psl == bucket) are
documented here because the Rust port implements them behind a debug flag.

## OffsetHashMap (`hash_map.zig`)

Fork of Zig-stdlib `HashMapUnmanaged` (as of 0.12) with: offsets instead of pointers,
fixed backing memory (no growth: `growIfNeeded` returns OOM past capacity, `:830-833`),
published `layoutForCapacity` (`:855-890`), and exported init-from-buffer.

Layout: `[Header][metadata: u8 × cap][keys: K × cap][values: V × cap]` where
`Header = {values: Offset(V), keys: Offset(K), capacity: u32, size: u32}`. The stored
handle is a single `Offset(Metadata)` pointing at the metadata array; the header lives at
`metadata - sizeOf(Header)` and the key/value offsets inside the header are **relative to
the metadata pointer**, not the page base (`:139-169, 296-308, 881-887`). Capacity is
always a power of two; slot = `hash & (cap-1)`; metadata byte = 7-bit fingerprint (top
hash bits) + used bit; tombstones (`fingerprint=1, used=0`) mark deletions and are
recycled by `getOrPut` (`:185-224, 727-740`). `removeByPtr` recovers the index by pointer
subtraction from the keys array (`:813-824`).

Used as: `GraphemeMap = AutoOffsetHashMap(Offset(Cell), Offset(u21).Slice)`
(`page.zig:95`), `hyperlink.Map = AutoOffsetHashMap(Offset(Cell), hyperlink.Id)`
(`hyperlink.zig:23`) — keyed by **cell offset**, which is why any operation that moves a
cell must move its map entries (`moveCells`/`swapCells`/`moveGrapheme`/`moveHyperlink`).
Hash function: Zig `autoHash` (Wyhash). The Rust port uses its own stable 64-bit mix
(SplitMix64-based); hash choice is internal — tables are rebuilt through the same code
path on both sides and clones are byte copies, so no cross-implementation compatibility
constraint exists.

## Style (`style.zig`)

`Style = {fg_color, bg_color, underline_color: Color, flags}` where
`Color = none | palette(u8) | rgb(RGB)` and flags is a `packed struct(u16)` (bold, italic,
faint, blink, inverse, invisible, strikethrough, overline, underline: enum(u3) none/
single/double/curly/dotted/dashed) (`:19-55`). `default = Style{}` and **default style is
ID 0 by convention** (`default_id`, `:16`) — default-styled cells never touch the set.

Hashing (`:472-546`): `Style` is re-packed into `PackedStyle` (`packed struct(u128)`,
tags-then-data ordering for serialization speed), the two u64 halves are XOR-folded and
finished with `std.hash.int` (an avalanche finalizer). The Rust port packs the same
u128 layout and finishes with a SplitMix64 finalizer (exact hash values are internal).

`style.Set = RefCountedSet(Style, u16, u16, ctx{hash, eql})` (`:549-564`). Helper
accessors `bg`/`fg`/`underlineColor`/`bgCell` resolve a style against a palette and cell
content (bg-color-only cells short-circuit, bold-is-bright palette offset, bold color
override only when fg equals the default) (`:108-226`). VT and HTML formatters render a
style as self-contained SGR resets / inline CSS (`:308-460`); each attribute is emitted
as its own SGR sequence for terminal compatibility.

## Hyperlink data model (`hyperlink.zig`)

`Id = u16`; `Map` maps cell offset → Id. `PageEntry` is the in-page representation:
`{id: explicit(Offset(u8).Slice) | implicit(u32), uri: Offset(u8).Slice}` (`:80-91`) —
all strings live in the page's `string_alloc`. `Hyperlink` is the heap-allocated,
page-independent form (`:29-74`) used when passing links around outside pages.

`hyperlink.Set = RefCountedSet(PageEntry, u16, u16, ctx)` (`:211-240`); the context holds
`page` (destination, owns resident values' strings) and optional `src_page` (where probe
values' strings live), so `hash`/`eql` deref the right base; `deleted` frees the entry's
strings from the destination page's string allocator. `PageEntry.dupe` (`:94-141`) copies
the strings into the destination page's string_alloc (shallow if same page).

Page-level flow (`page.zig`): `insertHyperlink` allocates URI/ID strings + adds to the
set (`:1326-1401`, errors distinguish which sub-allocator was exhausted so PageList can
grow the right capacity); `setHyperlink` (`:1412-1449`) puts cell→id in the map, and on
overwrite always releases the old id first (caller has pre-counted the new one; the
"same id repaint" case keeps `row.hyperlink` true while cell flag may momentarily be
false — see comment at `:1433-1440`). `lookupHyperlink`/`clearHyperlink`/`moveHyperlink`
are map plumbing; `moveHyperlink` deliberately does NOT touch cell flags (`:1466-1469`).

## Row and Cell (`page.zig:1907-2195`)

Both are `packed struct(u64)` — 8 bytes, zero = valid empty. Bit layouts (LSB first):

- `Row`: `cells: Offset(Cell)` (u32), `wrap`, `wrap_continuation`, `grapheme`, `styled`,
  `hyperlink`, `semantic_prompt: enum(u2)` (none/prompt/prompt_continuation),
  `kitty_virtual_placeholder`, `dirty`, pad u23. The `grapheme`/`styled`/`hyperlink`
  flags may have **false positives, never false negatives** (`:1926-1940`) — they gate
  slow paths (`managedMemory() = styled or hyperlink or grapheme`, `:2001-2004`).
  `dirty` likewise (false positives OK) drives rendering.
- `Cell`: `content_tag: enum(u2)` (codepoint / codepoint_grapheme / bg_color_palette /
  bg_color_rgb), `content: packed union` (u21 codepoint | u8 palette | 24-bit RGB; union
  width 24 bits), `style_id: u16`, `wide: enum(u2)` (narrow/wide/spacer_tail/
  spacer_head), `protected`, `hyperlink`, `semantic_content: enum(u2)`
  (output/input/prompt), pad u16. bg-color-only cells are an optimization to skip the
  style set for pure-background cells (`:2063-2066`). `Cell.init` bit-casts from zero to
  avoid uninitialized union padding (`:2112-2120`).

Cells are stored row-major in one array; each `Row.cells` offset points at its slice —
rows can in principle be permuted without moving cells (PageList relies on this
stability; grapheme/hyperlink maps are keyed by cell offsets which don't change when only
row structs move).

## Page operations and their invariants

- `getRow`/`getCells`/`getRowAndCell` (`:1030-1061`): plain offset math; y/x asserted in
  bounds of `size` (not capacity).
- `clearCells(row, left, end)` (`:1195-1270`): releases graphemes/hyperlinks/styles for
  the range (guarded by the row flags), clears row flags only when the full row was
  cleared (else recompute via `updateRow*Flag`), then zeroes the cells (as u64s).
- `moveCells` (`:1066-1131`): clear destination, copy (fast memcpy when
  `!src_row.managedMemory()`; else per-cell with grapheme/hyperlink map moves), then
  zero the source **without** clearCells (ownership moved, must not release refs).
- `swapCells` (`:1134-1188`): swaps map entries (or moves one-sided), then swaps the 8
  bytes. Styles need no map work (keyed by ID, ref counts unchanged).
- Graphemes: cell stores the **first** codepoint; extras go through the
  bitmap allocator (`setGraphemes`, `appendGrapheme`, `:1486-1582`). Append fast-path
  uses spare space in the last chunk (`len % 4 != 0`); otherwise alloc `len+1`, copy,
  free old (slow path). `clearGrapheme` frees + removes map entry + resets content_tag.
- `cloneFrom`/`cloneRowFrom`/`clonePartialRowFrom` (`:797-1027`): the cross-capacity
  clone used by PageList for resize/compaction. Destination range is cleared first;
  non-managed source rows are memcpy'd (with a debug assertion that no cell has managed
  data); managed rows go cell-by-cell: graphemes re-allocated in dst, hyperlinks
  dedup-looked-up cross-page (or `use`d if same page) with `addWithId` preferring the
  source ID, styles `addWithId` similarly. Partial-row copies preserve dst row flags and
  wrap state (`:861-879`). Errors name the exhausted sub-structure
  (`StyleSetOutOfMemory`, `HyperlinkMapOutOfMemory`, …) so PageList can grow capacity
  and retry. Growing columns clears a stale `spacer_head` on the old last column
  (`:1019-1026`).

### Integrity checking (`verifyIntegrity`, `page.zig:362-623`)

Debug-only (`assertIntegrity` compiles to nothing without slow runtime safety; a
`pause_integrity_checks` counter suspends it mid-operation, e.g. during
`clonePartialRowFrom`). Checks, in order: nonzero size; per-cell — grapheme flag ⇔ map
entry, style_id present in set (via `get`'s ref>0 assertion) and row.styled set,
hyperlink flag ⇒ map entry + row flag + live set entry (and ¬flag ⇒ no map entry);
spacer_tail must follow a wide cell, spacer_head only in the last column of a wrapped
row; per-row — cells with graphemes ⇒ row.grapheme. Then: graphemes seen ≤ map count;
per-ID style/hyperlink ref counts ≥ seen counts (≥, not ==, because fast paths trim rows
without releasing; the "zombie styles" check is disabled upstream for the same reason —
e.g. the cursor style ref).

## Invariants the unsafe Rust code must uphold

Collected here because the port isolates all `unsafe` behind these:

1. **Base validity**: every `Offset(T)`/`OffsetSlice` stored in a page refers into that
   page's own `memory` block, within `total_size`, aligned for `T`. All accessor calls
   must pass the owning page's base. (Enforced structurally: offsets are only minted by
   `layout()`/allocators/`get_offset` against the same base they're used with.)
2. **Zero = valid**: all section types (Cell, Row bit-structs; map metadata; set table)
   treat all-zero bytes as their empty state; page memory is zeroed at alloc/reinit.
   Types stored raw in page memory are `Copy`, contain no pointers/niches that byte
   copies could invalidate, and every slot is initialized (set items are written with
   defaults at init, so no uninitialized reads ever occur).
3. **Bitmap allocator**: a freed slice must be exactly an allocation previously returned
   (same offset, same rounded chunk length) and every chunk index derived from bitmaps
   is within the slab (guaranteed by the exact-coverage layout, see quirk note above).
4. **Set IDs**: `0 < id < layout.cap` for `get`/`use`/`release`; `get` requires
   `ref > 0`. Dead items may hold stale values but are never returned by lookups.
5. **Map-key/cell agreement**: grapheme/hyperlink map keys are exactly the offsets of
   cells whose flags say they have data; movers update maps before/with flags
   (`verifyIntegrity` audits this in debug).
6. **No aliasing across sections**: sections are disjoint ranges of the allocation;
   mutations take `&mut Page` so raw-pointer access within one section never races
   another `&mut` view. Structure handles (offsets) are copied out before use, so no
   Rust reference into `memory` outlives a mutation.

## Port notes (Rust, `crates/ghostty-vt/src/page/`)

- Module layout (as built): `size.rs` (Offset/OffsetSlice/getOffset/OffsetBuf),
  `hash.rs` (SplitMix64 mix + `MapKey`), `bitmap.rs` (`BitmapAllocator<CHUNK>`),
  `offset_map.rs` (`OffsetHashMap<K, V>` handle + `Map<K,V>` live view), `ref_set.rs`
  (`RefCountedSet<T, Id, C>` + a `SetContext<T>` trait supplying hash/eql/deleted),
  `style.rs` (Style/Flags/Color/Underline + `StyleSet`), `hyperlink.rs` (PageEntry/
  EntryId/`HyperlinkSet` + context), `page_impl.rs` (Page, Row, Cell, Capacity, Size,
  Layout — Row/Cell are `#[repr(transparent)]` u64 wrappers *in this file*, not separate
  row.rs/cell.rs), `mod.rs` (module wiring + re-exports). Plus crate-level `color.rs`
  (Rgb, Palette, Name, default palette — a minimal subset; the full color.zig parser is
  deferred to the SGR/OSC chunk).
- `RefCountedSet`'s Zig duck-typed `Context`/`base: anytype` become a `SetContext<T>`
  trait whose methods take an explicit `base: *const/*mut u8`; the context implementor
  carries any extra state. `has_deleted()` stands in for `@hasDecl(Context,"deleted")`.
- **Stacked-Borrows fix (Miri-driven, deviation worth noting):** the hyperlink set
  context must NOT hold a `*mut Page` — the set is a field of that same `Page`, so
  `page.hyperlink_set.add(..)` reborrows `&mut page.hyperlink_set` and invalidates any
  `Page` pointer aliasing it, and the callback's later deref is then UB (Miri flagged it).
  Instead the context holds the *disjoint* pieces the callbacks need: the page's memory
  base (`*mut u8`, a separate allocation) and a pointer to the `string_alloc` *field*
  (`*mut StringAlloc`, obtained via `&raw mut`), rebound before each set op by
  `bind_hyperlink_ctx`. Semantically identical to Zig; SB-clean. Likewise `Page::reinit`
  overwrites `*self` with `ptr::write` (not assignment) so `Drop` doesn't free the memory
  the rebuilt page reuses.
- `Row`/`Cell` are `#[repr(transparent)]` wrappers over `u64` with getter/setter methods
  matching the Zig bit layout exactly (LSB-first as documented above), so zeroing,
  memcpy fast paths, and the C ABI (`cval`) survive.
- Backing memory: `std::alloc::alloc_zeroed` with page-size alignment instead of raw
  mmap. Same guarantees the code relies on (zeroed, page-aligned); keeps the module
  Miri-runnable and platform-independent. Swapping in mmap (and the PageList pool) is a
  PageList-chunk concern; `init_buf`-style construction is kept (`Page` tracks whether
  it owns its memory).
- `u21` codepoints are `u32` in Rust (`size_of` 4 matches Zig's `@sizeOf(u21)`), so
  `grapheme_chunk` stays 16 bytes.
- Hash functions (auto-hash for map keys, Style fold finalizer, hyperlink Wyhash) are
  replaced by a stable SplitMix64-based mix; internal only (see OffsetHashMap section).
- BitmapAllocator slab sizing fixed to exact bitmap coverage (see quirk note); all other
  layout math ported verbatim.
- `slow_runtime_safety` ⇒ `cfg(debug_assertions)`; `build_options.kitty_graphics` ⇒
  always on (the row flag occupies its bit regardless, matching upstream).
- `std_capacity.grapheme_bytes` keeps upstream's test/non-test split (512 under
  `cfg(test)`, 8192 otherwise).
- `PAGE_SIZE_MIN` is pinned to 4096 (matches `std.heap.page_size_min` on target
  platforms) so `Layout` math is host-deterministic (incl. under Miri).

### Deferred to the PageList chunk (not yet ported here)

- **`verifyIntegrity`** (`page.zig:362-623`): the debug-only cross-check suite. The
  operation hooks (`assert_integrity()` call sites) are in place as no-ops; the full
  audit (grapheme⇔map, style ref accounting, spacer_tail/head placement, hyperlink
  flag/map/set agreement) is a follow-on. `pause_integrity_checks` likewise stubbed.
- **`clone`/`cloneBuf`** (whole-page byte-copy relocation) and the mmap/`PageAlloc` pool
  — PageList concerns. `init_buf` + the `owned` flag are in place so a pool can drive
  construction later.
- **`Style::bg`/`bgCell`** (cell-dependent color resolution): `fg`/`underline_color` are
  ported; `bg`/`bgCell` need `Cell` content-tag plumbing and land with the terminal
  layer that consumes them.
- **Full `color.zig`** (parsing, x11, dynamic/special colors, C ABI) — SGR/OSC chunk.
- The kitty-graphics placeholder codepoint is tracked unconditionally (matching
  upstream's always-reserved row-flag bit); the kitty graphics subsystem itself is a
  separate chunk.
