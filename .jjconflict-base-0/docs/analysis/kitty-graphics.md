# Kitty graphics protocol (`src/terminal/kitty/graphics_*.zig`)

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse --short HEAD`). This is ghostty's implementation of the
[Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol): transmitting
images to the terminal (in several encodings and transport mediums), storing them, and
placing them on the screen at pin-tracked positions with z-layers and unicode placeholders.

The subsystem is ~6.3k LOC across eight Zig files. This document maps the **model** —
command grammar, image storage, placement tracking — and the **exec** and **unicode
placeholder** layers, all now ported in `crates/ghostty-vt/src/kitty/`. Only the actual GPU
texture upload/draw of a resolved `RenderPlacement` remains outside this crate's scope (that's
an embedder/renderer concern, not a `ghostty-vt` one).

## File inventory (Zig)

| file                    | LOC      | role                                                                   | ported?                                    |
| ----------------------- | -------- | ---------------------------------------------------------------------- | ------------------------------------------ |
| `graphics.zig`          | 38       | namespace re-exports + `refAllDecls` test                              | n/a (module root)                          |
| `graphics_command.zig`  | 1333     | APC command grammar: KV parser + command tree + `Response`             | **yes** → `command.rs`                     |
| `graphics_image.zig`    | 1050     | `Image`, `LoadingImage` (chunked transfer, decode), `Rect`             | **yes** → `image.rs`                       |
| `graphics_storage.zig`  | 1601     | `ImageStorage`: image map, placement map, eviction, generation, delete | **yes (model)** → `storage.rs`             |
| `graphics_exec.zig`     | 658      | `execute(alloc, *Terminal, *Command)` → `Response`; needs `Terminal`   | **yes** → `exec.rs`                        |
| `graphics_render.zig`   | 27       | `render.Placement`: renderer-facing placement struct                   | **yes** → `unicode.rs::RenderPlacement`    |
| `graphics_unicode.zig`  | 1347     | unicode placeholder (`U+10EEEE`) placement resolution                  | **yes** → `unicode.rs`                     |
| `color.zig` / `key.zig` | 77 / 169 | kitty color protocol / keyboard flags                                  | out of scope (other chunks)                |

`testdata/` holds `@embedFile`-d raw image fixtures used by `graphics_image.zig` tests.

## Command grammar (`graphics_command.zig`)

### Wire format

A kitty graphics command arrives as an APC sequence `ESC _ G <control> ; <payload> ESC \`.
The DCS/APC sibling strips the `ESC _ G` prefix and `ESC \` terminator and hands the parser
the bytes starting immediately after the `G`. `<control>` is comma-separated `key=value`
pairs; `<payload>` is base64. Keys are always a single ASCII character; values are either a
single printable ASCII char (stored as its byte) or a decimal integer (`u32`, or `i32` for
the signed keys `z`, `H`, `V`).

### `Parser` (`:19-279`) — a byte-at-a-time state machine

Fields: an arena (for the KV map + temp), `kv: AutoHashMap(u8, u32)`, an 11-byte `kv_temp`
scratch (max u32 is 10 digits + sign), `kv_current` (the key being parsed), a `data`
ArrayList for the raw payload, `max_bytes` cap, and `state`.

States (`:48-58`): `control_key`, `control_key_ignore`, `control_value`,
`control_value_ignore`, `data`. The `_ignore` variants are entered on malformed KV (multi-char
key, or an over-long value) and skip bytes until the next delimiter — the command is still
completed with whatever KV pairs did parse (kitty-compatible leniency).

`feed(c)` (`:103-150`) transitions:

- In `control_key`: `=` finishes the key (only if exactly one char accumulated, else go to
  `control_value_ignore`); `;` with no key means "payload only, no control" → `data`
  (`ESC_G;<data>` is valid per kitty); anything else accumulates into `kv_temp`.
- In `control_value`: `,` finishes the value and returns to `control_key`; `;` finishes the
  value and moves to `data`; else accumulate.
- In `data`: append to `data`, erroring `OutOfMemory` once `max_bytes` is reached.

`accumulateValue` (`:240-249`): pushes a char into `kv_temp`; on overflow (>11 chars) drops
to the overflow-ignore state and resets — this is how "ignore very long values" works.

`finishValue` (`:251-278`): if the value is a single non-digit ASCII char, store its byte
directly; otherwise parse as `i32` (bitcast to u32) for keys `z`/`H`/`V`, else `u32`.
`parseInt` overflow propagates as `error.Overflow`.

`complete(alloc)` (`:157-208`): flushes a trailing value; errors `InvalidFormat` if we ended
mid-key. Reads action from key `a` (default `t`), dispatches to the per-action `parse(kv)`
that projects the flat KV map into a typed struct, reads `quiet` from key `q` (0/1/2 →
`no`/`ok`/`failures`), and base64-decodes the payload.

`decodeData` (`:213-238`): decodes base64 **in place** on top of `self.data` (encoded size ≥
decoded size), truncates to the decoded length. Empty payload → empty string. On decode
failure → `error.InvalidData`. The Rust port cannot decode in-place safely with the `base64`
crate's slice API, so it decodes into a fresh `Vec` (behavior-identical).

### Command tree (`:325-970`)

`Command = { control: Control, quiet: Quiet, data: []const u8 }`. `Control` is a tagged union
over `Action` (`query` q, `transmit` t, `transmit_and_display` T, `display` p, `delete` d,
`transmit_animation_frame` f, `control_animation` a, `compose_animation` c).

Payload structs and their KV keys:

- **`Transmission`** (`:393-514`): `f`ormat (24=rgb, 32=rgba, 100=png; plus internal
  gray/gray_alpha that png decodes to), `t` medium (d=direct, f=file, t=temporary_file,
  s=shared_memory), `s`/`v` width/height, `S`ize, `O`ffset, `i`mage_id, `I`mage_number,
  `p`lacement_id, `o` compression (z=zlib_deflate), `m`ore_chunks. **Security-relevant
  quirk** (`:497-510`): `m` is only honored when medium is `direct`; kitty and mpv rely on
  this for shared-memory transfers. `formatBpp` gives bytes-per-pixel (gray=1, gray_alpha=2,
  rgb=3, rgba=4; png unreachable — must be decoded first).
- **`Display`** (`:516-629`): `i`/`I`/`p` ids, `x`/`y` source-rect origin, `w`/`h`
  source-rect size, `X`/`Y` pixel offsets, `c`/`r` columns/rows, `C` cursor_movement (0=after,
  1=none), `U` virtual_placement (unicode placeholder), `P`/`Q` parent id/placement (relative
  placements), `H`/`V` signed relative offsets, `z` signed z-index.
- **`Delete`** (`:791-965`): a big union keyed by `d` (default `a`). Lower/upper case of the
  key selects "delete placements only" vs "delete placements + underlying image data"
  (`delete = what == UPPER`). Variants: `all` (a/A), `id` (i/I), `newest` (n/N by image
  number), `intersect_cursor` (c/C), `animation_frames` (f/F), `intersect_cell` (p/P),
  `intersect_cell_z` (q/Q), `range` (r/R — `x>y` errors, both required), `column` (x/X),
  `row` (y/Y), `z` (z/Z).
- **`AnimationFrameLoading`** / **`AnimationFrameComposition`** / **`AnimationControl`**
  (`:631-789`): animation model; parsed but exec is unimplemented upstream.

### `Response` (`:282-323`)

`{ id, image_number, placement_id, message="OK" }`. `encode(writer)` emits
`ESC_G i=..,I=..,p=..;<message> ESC\` but **only if** id or image_number is non-zero (else
nothing). `ok()` = message is `"OK"`; `empty()` = no id and no number.

### Inline tests: **21** (all in `graphics_command.zig`)

transmission command / ignores-m-if-not-direct / respects-m-if-direct / query / display /
delete / no-control-data / ignore-unknown-keys / ignore-very-long-values /
large-negative-values / overflow-u32 / overflow-i32 / all-i32-values (z/H/V) /
response-encode {nothing, id-only, number-only, id+number} / delete-range {1..5}.

## Image loading (`graphics_image.zig`)

### `LoadingImage` (`:31-498`) — chunked, multi-medium assembly

Holds the in-progress `Image` (metadata from the first chunk's `Transmission`), a growing
`data` buffer, an optional `Display` (for `T` transmit-and-display), the initial `quiet`, and
`Limits` (which mediums are permitted). `Limits` is a 3-bit set (`file`/`temporary_file`/
`shared_memory`); `.direct` = all false (direct is always allowed), `.all` = all true.

`init(alloc, cmd, limits)` (`:74-153`):

1. Build `Image` from the transmission metadata (id/number/width/height/compression/format).
2. **Direct medium**: append `cmd.data` directly (base64 already decoded by the parser).
3. Otherwise (file/temp/shm): validate capabilities (png without a decoder → `UnsupportedMedium`),
   check the medium is in `limits`, then treat the payload as a **path** and load it.

**Security handling** (this is the sensitive part):

- Reject paths containing embedded NUL (`:125-132`) — `realpath` would assert.
- `readFile` (`:251-326`): rejects `/proc/`, `/sys/`, and `/dev/` (except `/dev/shm/`).
  For `temporary_file`: the path must be inside a temp dir (`isPathInTempDir`) **and** contain
  the literal `tty-graphics-protocol`, else `TemporaryFileNotInTempDir` /
  `TemporaryFileNotNamedCorrectly`. Temporary files are **unlinked after reading** (a `defer`).
  Requires a regular file; honors `O`ffset (seek) and `S`ize (read cap, ≤ `max_size` 400MB).
- `isPathInTempDir` (`:330-345`): accepts `/tmp`, `/dev/shm`, the OS temp dir, and its
  realpath (macOS `/tmp` → `/private/var/...`).
- `readSharedMemory` (`:156-245`): `shm_open` + `fstat` + `mmap`; validates the segment is at
  least the expected size (`width*height*bpp`, or the stat size for png); honors offset/size;
  `shm_unlink`s after. Android/Windows/no-libc → `UnsupportedMedium`.

`addData(alloc, data)` (`:359-376`): append a chunk (the `m=1` continuation path); errors past
`max_size`.

`complete(alloc)` (`:379-410`): decompress (zlib inflate if `compression==.zlib_deflate`),
decode PNG if `format==.png` (updates width/height, sets format `.rgba`), validate dimensions
(`> 0`, `≤ max_dimension` 10000), and assert `data.len == width*height*bpp`. Produces a final
`Image` with `compression=.none` and a non-png `format`.

### `Image` (`:507-553`)

`{ id, number, width, height, format, compression, data, generation, implicit_id }`. Post-
`complete` invariant: data is fully-decoded raw pixels, `compression=.none`, `format!=.png`,
`data.len == width*height*bpp`. `generation` is a monotonic content-mutation stamp (see
storage). `implicit_id` marks images whose transmit lacked an id/number (should not be
responded to). `withoutData` clones with data cleared (for logging).

### `Rect` (`:558-561`)

`{ top_left: PageList.Pin, bottom_right: PageList.Pin }` — the grid-cell rect a placement
occupies. **This leaks `PageList.Pin`** into the image module's API (see extraction notes).

### PNG decode seam

Upstream calls `sys.decode_png` — a function pointer, null when the decoder isn't linked
(wuffs). The decision table says the Rust port replaces wuffs with the `image`/`png` crates.
Per the port's scope, PNG decode is behind a **seam**: `LoadingImage::complete` takes an
optional decoder (`PngDecoder` trait / fn), and a stored image can hold encoded png bytes +
format tag until a decoder is supplied. This matches upstream where `decode_png == null`
short-circuits png handling.

### Inline tests: **15** (all in `graphics_image.zig`)

invalid-RGB-allowed / too-wide / too-tall / rgb-zlib-direct / rgb-none-direct /
rgb-zlib-chunked / rgb-zlib-chunked-zero-initial / temp-file-wrong-path / rgb-temp-file /
rgb-regular-file / png-regular-file / limits {direct-always, file-blocked, file-allowed,
temp-blocked, temp-allowed}. Several depend on `@embedFile` fixtures and OS temp dirs.

## Image storage & placement (`graphics_storage.zig`)

### Generation counter (`:30-64`, `:35`)

Process-global monotonic `u64` counter (`nextGeneration()`); atomic on 64-bit, mutex on
32-bit. Starts at 1 (0 = "never stamped"). Global (not per-storage) so a generation value is a
unique cache key across every storage/screen/terminal in the process. The Rust port uses a
single `static AtomicU64` (`fetch_add`).

### `ImageStorage` (`:69-902`)

One per screen (main/alt). Fields:

- `dirty` — set on any placement/image change **and** on scroll/resize (geometry). Renderer
  clears it. Informational only.
- `generation` — stamp of the last **content** mutation (transmit/replace/placement/delete).
  NOT bumped by geometry events. Written only via `markMutated` (`:162-165`, which sets both
  `dirty` and a fresh `generation`). Invariant: dirty is always set when generation changes.
- `next_image_id = 2147483647` (mid-u32, to avoid collisions with client-chosen ids).
- `next_internal_placement_id = 0` (internal placements for `p=0`).
- `images: HashMap<u32, Image>`, `placements: HashMap<PlacementKey, Placement>`.
- `loading: ?*LoadingImage` (in-progress transfer).
- `image_limits: LoadingImage.Limits` (default `.direct`).
- `total_bytes` / `total_limit = 320MB`. `enabled()` = `total_limit != 0`.

`PlacementKey` (`:708-714`): `{ image_id: u32, placement_id: { tag: internal|external, id:
u32 } }`. `p=0` → an auto-incremented **internal** id (allows many placements per image);
`p>0` → **external** id (one placement per (image_id, p)).

`Placement` (`:716-901`): `{ location: Location, x_offset, y_offset, source_x/y/width/height,
columns, rows, z }`. `Location` is `pin: *PageList.Pin` (tracked) **or** `virtual` (unicode
placeholder — has no rect). `deinit(screen)` untracks the pin. **This leaks
`PageList.Pin`.**

Geometry methods on `Placement` (need terminal px/cell geometry, **not** a full Terminal):

- `pixelSize(image, t)` (`:758-834`): image px size honoring source rect, cols/rows, and
  aspect ratio. Uses `t.width_px/t.cols` and `t.height_px/t.rows` as cell size.
- `gridSize(image, t)` (`:837-868`): cols/rows in cells (divCeil of pixel size + offset by
  cell size; 0 on zero cell size).
- `rect(image, t)` (`:873-900`): the `Rect` from the pin using `downOverflow(rows-1)` for the
  bottom and `min(pin.x + cols-1, t.cols-1)` for the right. `virtual` → null.

The Rust port introduces a small POD `TerminalGeometry { cols, rows, width_px, height_px }`
carrying exactly the four fields these methods read — no `Terminal` dependency.

### Operations

- `addImage(alloc, img)` (`:199-238`): reject if single image > limit; evict if over limit;
  `getOrPut` (freeing an existing same-id image and adjusting `total_bytes`); `markMutated`
  and stamp the stored image's `generation`.
- `addPlacement(alloc, image_id, placement_id, p)` (`:242-279`): asserts the image exists;
  builds the `PlacementKey` (internal id if `p=0`); inserts; `markMutated`.
- `imageById` (`:288-290`) / `imageByNumber` (`:293-308`): by-id lookup; by-number returns the
  **newest generation** among images sharing that number.
- `setLimit(alloc, screen, limit)` (`:171-195`): `limit=0` fully resets storage (disabling the
  protocol) preserving `image_limits`, and marks mutated; lowering below `total_bytes` evicts.
- `evictImage(alloc, req)` (`:602-703`): builds candidates `{id, generation, used}`, sorts
  unused-first then by generation (transmit time), tie-break by id; evicts placements+image
  until `req` bytes freed. Marks mutated if anything evicted.
- `delete(alloc, *Terminal, cmd)` (`:311-519`): the big dispatch over `command.Delete`. Counts
  placements/images before/after and only `markMutated`s if something actually changed (a
  delete-all runs on every `ESC[2J`, so empty clears must not dirty). Sub-helpers: `deleteById`
  (`:521-551`), `deleteIfUnused` (`:554-568`), `deleteIntersecting` (`:571-594`, uses
  `target_pin.isBetween(rect.top_left, rect.bottom_right)`). Column/row/intersect variants need
  `t.screens.active.pages.pin(...)` and `Placement.rect` (terminal geometry + pagelist), plus
  the cursor position for `intersect_cursor`.

The Rust port decouples `delete` from `Terminal`: it takes `&mut PageList`, the
`TerminalGeometry`, and a cursor `(x, y)` — precisely the pieces the Zig `delete` reads out of
`Terminal`. The pin-untracking on placement deinit goes through `PageList::untrack_pin`.

### Inline tests: **25** (all in `graphics_storage.zig`)

add-placement-zero-id / delete-all(+preserves-limit) / delete-all-placements /
delete-by-image-id(+unused) / delete-by-specific-id / intersect-cursor(+unused, +multiple) /
by-column(+1x1) / by-row(+1x1) / by-range{1..4} / aspect-ratio / generation{add-replace,
placement-delete, setLimit-evict} / imageByNumber-newest / nextGeneration-monotonic /
no-op-delete-no-mutation.

Most tests build a `Terminal` for `trackPin` + geometry + cursor. The Rust port drives them
against a `PageList` directly (via a small test helper mirroring `trackPin`) plus a
`TerminalGeometry` and explicit cursor coords, so they port 1:1 semantically without a
`Terminal` type existing yet.

## exec (`graphics_exec.zig`) — PORTED (`kitty/exec.rs`)

`execute(alloc, *Terminal, *Command) ?Response` (`:23-91`) is the top of the subsystem. It
checks `storage.enabled()`, dispatches on `cmd.control`:

- `query` (`:97-`): `LoadingImage.init` + `complete` a throwaway image to validate, respond
  with id/number/placement, never persists.
- `transmit`/`transmit_and_display`: manage the `loading` state across chunks (the `q`
  inheritance rule at `:56-67`), on the final chunk `complete` → `addImage`, then for `T` also
  add a placement at the cursor and advance the cursor.
- `display` (p): resolve image by id/number, `trackPin` at the cursor, `addPlacement`, handle
  `C` cursor movement.
- `delete` (d): `storage.delete`.

The quiet filter (`:78-88`) decides whether to actually emit the `Response`. Exec is where the
cursor advances and where `trackPin` happens against the live screen.

**Rust port** (`crates/ghostty-vt/src/kitty/exec.rs`, all 10 inline tests ported 1:1):
`kitty::exec::execute(&mut Terminal, &Command) -> Option<Response>` mirrors the dispatch
exactly. The in-progress `loading` state (which the Rust `ImageStorage` model deliberately
omits) lives on `Screen.kitty_loading: Option<LoadingImage>`, alongside the new
`Screen.kitty_images: ImageStorage`. Placement pins are tracked against the active screen's
`PageList` (`track_pin(*cursor.page_pin)`); the delete path split-borrows the `Screen` so
`ImageStorage::delete` can hold `&mut PageList` + geometry + cursor at once. Cursor advance for
`T`/`p` uses `Terminal::index` (scroll-region-aware) then `set_cursor_pos`. The stream's
`TerminalHandler::apc_end` delivers the raw `apc::Command::KittyRaw` payload to
`kitty::CommandParser::parse_string` → `execute`, pushing `Response::encode()` onto the reply
queue (port of `stream_terminal.Handler.apcEnd` + `Terminal.kittyGraphics`).

Two seams remain inside exec: file/shm/png/zlib media (the default `image_limits = .direct` +
`NoDecoder` restrict it to direct-medium uncompressed non-png transfers; `execute_with` accepts
a real decoder + medium reader), and animation actions (`f`/`a`/`c`, which return
`"ERROR: unimplemented action"` exactly as upstream).

## Unicode placeholders (`U=1` virtual placements, `graphics_unicode.zig` + `graphics_render.zig`)

Ported to `crates/ghostty-vt/src/kitty/unicode.rs`. This is the mechanism kitty uses to make
graphics scroll, reflow, and copy/paste like ordinary text: instead of a screen-anchored pin
(the `Location::Pin` case in `storage.rs`), a `U=1` display command creates a
`Location::Virtual` placement, and the *client* (e.g. a shell prompt or `icat`-style tool)
paints ordinary-looking cells that happen to hold the codepoint `U+10EEEE` — the "unicode
placeholder". These cells carry the placement's identity and the row/column fragment they
represent entirely in existing cell attributes, so no new cell-storage fields are needed:

- **Image ID**: the low 24 bits live in the cell's **foreground color** — `colorToId`
  (ported as `color_to_id`) reads the palette index directly, or packs an RGB triple as
  `(r<<16)|(g<<8)|b`. This lets any of the three SGR foreground forms (256-color palette or
  24-bit RGB) carry an id, matching how kitty-compatible clients (e.g. shell integration
  scripts) already set colors.
- **Image ID, high byte**: an optional 4th byte, packed via the **3rd combining diacritic**
  on the placeholder cell (`image_id = low24 | (high8 << 24)`), for image ids that don't fit
  in 24 bits.
- **Placement ID**: the low 24 bits of the cell's **underline color**, via the same
  `colorToId` mechanism (`p=0` if the underline color is unset/none).
- **Row/column fragment**: two more **combining diacritics** (row first, then column) from a
  fixed 297-entry table of Unicode combining marks
  ([kitty's `rowcolumn-diacritics.txt`](https://sw.kovidgoyal.net/kitty/_downloads/f0a0de9ec8d9ff4456206db8e0814937/rowcolumn-diacritics.txt)),
  ported verbatim as `DIACRITICS` (binary-searched by `get_index`, since it's sorted). These
  say *which row/column of the image* this particular placeholder cell represents — a large
  image spans many placeholder cells, each showing one "tile".
- Missing diacritics are **not** errors — they mean "continue the previous placement": a
  cell missing its column diacritic implicitly continues at `prev.col + 1` (see
  `IncompletePlacement::can_append`, "converted from Kitty's logic, don't @ me" per upstream's
  own comment). Invalid diacritic codepoints are likewise tolerated (logged upstream, silently
  ignored here) rather than erroring.

### Print-path half: recognizing the placeholder

`crate::terminal::print::print_cell` (the single choke point every print path funnels
through) checks the mapped codepoint against `kitty::unicode::PLACEHOLDER` and sets
`Row::kitty_virtual_placeholder` — a bit already present on `Page`/`Row` (exercised by
`pagelist` reflow/resize tests) but never previously set by the print path (the
`TODO(chunk:kitty-gfx)` seam this chunk closes). Two narrow-batching fast paths
(`print_slice_fast_narrow`'s first-codepoint check, and `print_slice_fill_narrow`'s
run-continuation scan) must *not* swallow the placeholder codepoint into a batched cell
write, since only `print_cell` sets the row flag; both now explicitly exclude
`kitty::unicode::PLACEHOLDER` before consulting `codepoint_width` (which returns 1 for it,
same as any other narrow codepoint) — mirroring upstream's `printSliceEligible` guard. The
run-continuation gap (a placeholder appearing after the first codepoint of an already-batched
narrow run) was a real latent bug caught while porting: `codepoint_width(0x10EEEE) == 1`
made it look eligible for the batch, silently dropping the row flag for any placeholder that
wasn't the very first codepoint the print path saw.

### Read-path half: resolving placements at render-collection time

`kitty::unicode::placement_iterator(pin, limit)` returns a `PlacementIterator` that walks
rows via the existing `Pin::row_iterator` (right-down direction), skipping any row whose
`kitty_virtual_placeholder` flag is unset (an O(1) per-row check — this is *why* the flag
exists: without it, resolving placements would mean inspecting every cell's codepoint on
every frame). Within a flagged row it scans cells left-to-right, decoding each placeholder
cell into an `IncompletePlacement` (`IncompletePlacement::init`, reading the cell's style via
`Page::style_by_id` and its diacritics via `Page::lookup_grapheme`) and merging contiguous,
compatible cells into a single run (`IncompletePlacement::append`/`canAppend`) — "a run is
always only a single row" (never wraps across rows), matching upstream. A run ends when a
cell breaks compatibility (different image/placement id, or a non-continuing row/col), and
the iterator resumes from that exact cell on the next call — achieved in Rust by writing the
iterator's resume pin (`self.row`) in lockstep with the scan index, since Zig's version
aliases a local `row: *Pin` directly against the iterator's stored pin.

`Placement::render_placement(storage, image, cell_width, cell_height)` resolves one
iterator-yielded `Placement` (a run: image id, placement id, row/col fragment, width) plus the
*stored* `storage::Placement` (which carries the requested `columns`/`rows` for the whole
virtual placement, looked up by placement id or, if unset, "the first virtual placement for
this image") into a `RenderPlacement` — the flat, `Pin`-plus-pixel-rects struct a GPU renderer
consumes (port of `graphics_render.zig`'s 27-line `render.Placement`, folded into this module
since nothing else produces one). The math is aspect-ratio-preserving letterboxing: the
image's native size fits into the requested grid's pixel size (scale-and-center on whichever
axis is the tighter constraint), then the specific row/col fragment's source and destination
sub-rects are carved out of that centered, scaled image — including the "this fragment is
entirely in the letterboxed margin, render nothing" degenerate case. This is straight
floating-point geometry ported 1:1 from `graphics_unicode.zig:130-351`; the three `dog.png`
fixture tests (4x2, 2x2, 1x1 grids) pin the exact rounding behavior.

### Storage interplay (verified, no changes needed)

A `U=1` display command (`kitty::exec::display`) already builds a `Location::Virtual`
placement — `exec.rs` was ported (a prior chunk) against the same `storage::Location` enum
this module reads, and the wiring needed no changes: `display` skips `PageList::track_pin`
entirely for `U=1` (a virtual placement has no rect to track) and rejects `P=`/parent-id
placements as `EINVAL` (matching upstream, since a virtual placement can't be a relative-offset
child). The only remaining consumer-side task was the read path this chunk adds.

## Extraction-readiness (library candidate)

The command grammar (`command.rs`) and `Response` are fully ghostty-free — pure `u8`/`u32`
data, publishable as-is. The blockers for a standalone crate are:

1. **`PageList.Pin` leaks** into `Placement.location`, `Rect`, the storage delete/rect API,
   and now `unicode::Placement`/`RenderPlacement` (the placeholder-resolution read path also
   walks and returns pins). A standalone crate would need to abstract "a tracked screen
   position" behind a trait or generic parameter. For now the port keeps `Pin` (this is a
   `ghostty-vt` module, not yet a split crate), matching the prompt's "design the seam, split
   later" guidance.
2. **`TerminalGeometry`** was introduced precisely to *avoid* leaking `Terminal`; it is a POD
   the model owns, so it is not a leak.
3. **PNG decode** is behind a seam, so the model doesn't force a decoder dependency.

Recommended eventual split: parametrize storage (and the unicode-placement read path) over a
`Pin`/`Position` trait and a `PinTracker` (track/untrack) trait, plus a small "row flag +
style/grapheme lookup" trait for `unicode.rs`; the command grammar and `Image` (sans `Rect`)
can go in a lower crate with zero ghostty types.

## Pin-API gaps

The existing `PageList` Pin API (`crates/ghostty-vt/src/pagelist/pin.rs`) provides everything
the storage model needs: `pin(Point) -> Option<Pin>`, `track_pin(Pin) -> *mut Pin`,
`untrack_pin(*mut Pin)`, `count_tracked_pins()`, and `Pin::is_between` / `Pin::down_overflow`
(both `pub(crate)`, accessible from the in-crate kitty module). No additions were required.
`Pin::x()`/`y()` are public accessors. The `Coordinate`/`Point`/`Tag` types are public.
