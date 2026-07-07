# Selection: the highlight region (`src/terminal/Selection.zig`)

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse --short HEAD`). `Selection.zig` is ~1.6k lines
(1604) with **18** inline `test` blocks. It models a single highlight region on a
[`Screen`](screen.md) as a pair of pins (start/end), plus the `select*` query
family that lives on `Screen` itself. The Rust port lives in
`crates/ghostty-vt/src/screen/selection.rs`, and the `Screen`-side integration
replaces the `SelectionPlaceholder` scaffold in `screen/mod.rs`.

`SelectionGesture.zig` (input-phase drag/click state) is **out of scope** — it is
frontend/input territory that sits *above* `Screen`; the engine only exposes the
`Selection` value and the `select*`/`selectionString`/`adjust` primitives it drives.

## The model: two pins, in any order

A `Selection` is `{ bounds: Bounds, rectangle: bool }` (`Selection.zig:24-30`).

`Bounds` (`:42-52`) is a tagged union:

- `untracked: { start: Pin, end: Pin }` — plain pins, valid only until the next
  screen mutation.
- `tracked: { start: *Pin, end: *Pin }` — pins vended by `PageList.trackPin`,
  kept valid across scroll/prune/resize, but each costs an allocation + a fixup
  walk on every mutating op.

**Key invariant**: `start` and `end` are in *no guaranteed order*. If the user
drags backwards, `start` is after `end`. All ordering is recovered on demand via
`order()` (below); callers never assume start≤end. `startPtr`/`endPtr`/`start`/
`end` (`:92-119`) just deref the union without ordering.

`init(start, end, rect)` (`:56-68`) always builds an **untracked** selection.
`deinit(s)` (`:70-82`) untracks both pins if tracked (no-op if untracked).
`track(s)` (`:131-149`) asserts untracked, tracks both pins, returns a new tracked
selection (errdefer-untracks on partial failure — infallible in Rust).
`tracked()` (`:122-127`) reports the variant. `eql` (`:85-89`) compares
start/end/rectangle.

### The `Screen` field lifecycle

`Screen.selection: ?Selection` (`Screen.zig:53-57`) **MUST** be a tracked
selection (else it dangles). The only sanctioned mutators are:

- `Screen.select(sel_)` (`Screen.zig:2424-2438`): `null` → `clearSelection`.
  Otherwise track it if untracked (`errdefer` untracks on failure); **untrack the
  prior selection's pins** (`if (self.selection) |*old| old.deinit(self)`); store
  the tracked selection; set `dirty.selection = true`. The untrack-old step is
  what keeps the tracked-pin count constant on replace (test: "select replaces
  existing pins").
- `Screen.clearSelection()` (`:2441-2447`): deinit (untrack) the selection if
  present, set `dirty.selection`, null it out.

`dirty.selection` is set on *any* set-or-unset (even a no-op clear of an already-
null selection is guarded to only dirty when something was present).

## Ordering: `order` / `topLeft` / `bottomRight` / `ordered`

Ordering is computed lazily from screen points, because `pointFromPin(.screen, …)`
requires a list traversal (the `NOTE(mitchellh)` at `:14-22` laments this cost but
keeps it for caller compatibility).

`Order` (`:199-204`) = `forward | reverse | mirrored_forward | mirrored_reverse`.
`order(s)` (`:206-229`):

- **Non-rectangle**: `start.y < end.y` → forward; `>` → reverse; same row →
  `start.x <= end.x` ? forward : reverse.
- **Rectangle** (`:210-222`): the two mirrored orders exist because a rectangle
  flips only one axis at a time.
  - reverse if (`sy>ey && sx>=ex`) or (`sy>=ey && sx>ex`) — also covers single-col.
  - `mirrored_reverse` if `sy>ey && sx<ex` (bottom-left → top-right).
  - `mirrored_forward` if `sy<ey && sx>ex` (top-right → bottom-left).
  - else forward.

`topLeft(s)` / `bottomRight(s)` (`:152-185`) switch on `order`: forward returns
start/end; reverse swaps; the mirrored cases build a copy of one pin with the
*other* pin's x (flipping the single axis). `ordered(s, desired)` (`:237-251`)
returns a fresh untracked selection reordered to `forward`/`reverse` (any other
desired order behaves as forward), using topLeft/bottomRight.

## Containment: `contains` / `containedRow`

`contains(s, pin)` (`:258-286`) resolves tl/br/p to screen points, then:

- rectangle: `p.y in [tl.y,br.y] and p.x in [tl.x,br.x]`.
- else: single-row → x between tl.x/br.x; on tl row → `p.x >= tl.x`; on br row →
  `p.x <= br.x`; strictly between → always contained.

`containedRow(s, pin)` (`:295-315`) → `containedRowCached` (`:319-395`): given a
row-pin, returns a *single-row sub-selection* clipped to that row, or `null` if the
row is outside `[tl.y, br.y]`. Rectangle: returns `[pin.x=tl.x .. pin.x=br.x]`
rectangle. Non-rectangle: on the tl row (and it's the only row) → the whole tl..br;
tl row only → `tl .. (cols-1)`; br row → `0 .. br`; middle → `0 .. (cols-1)`. This
is the per-row driver `selectionString` uses to walk a multi-line selection.

## `adjust`: keyboard/mouse expansion of `end`

`Adjustment` (`:398-409`) = `left right up down home end page_up page_down
beginning_of_line end_of_line`. `adjust(s, a)` (`:413-510`) **always moves the
`end` pin** (end = the last mouse point, so up/down drags both work):

- `up`: `end.up(1)` or fall back to `beginning_of_line`.
- `down`: walk `end.down(1)` to the next row with any text (`Cell.hasTextAny`);
  if none, `end_of_line`.
- `left`: `cellIterator(.left_up)`, skip self, stop at first `cell.hasText()`.
- `right`: `cellIterator(.right_down)`, skip self, stop at first `hasText()`.
- `page_up`/`page_down`: `end.up/down(s.pages.rows)` or fall back to `home`/`end`.
- `home`: `end = pin(.screen{0,0})`.
- `end`: rowIterator `.left_up` from screen top, first row with `hasTextAny`, set
  `end` to that row's last text cell (`end.x = cells.len-1`).
- `beginning_of_line`: `end.x = 0`.
- `end_of_line`: `end.x = size.cols - 1`.

Tests (`:512-991`) cover right/left/left-skips-blanks/up/down/down-not-full/home/
end-not-full/beginning-of-line/end-of-line.

## `Screen`-side query family (`Screen.zig`)

These build **untracked** selections (the caller passes them to `select`, which
tracks). They read the pagelist directly; none needs a formatter.

- `selectAll()` (`:2702-2754`): first/last text cell via cellIterator
  right_down/left_up over `.screen`, skipping whitespace `{0,' ','\t'}`. `null` if
  empty.
- `selectLine(opts)` (`:2538-2698`): the largest one. Walks the soft-wrap run
  containing `opts.pin` up (start) and down (end), honoring `semantic_prompt_
  boundary` (any semantic-content change is a boundary — issue #1329) and trimming
  leading/trailing `whitespace` (default `selection_codepoints.default_line_
  whitespace`; `null` disables trim). Returns `null` if no non-whitespace found.
- `selectWord(pin, boundary_cps)` (`:2795-2877`): expands over cells that are
  *all* boundary or *all* non-boundary (matching the clicked cell's class), across
  soft-wraps, stopping at empty cells / non-wrap row ends. `null` if clicked cell
  is empty. `selectWordBetween` (`:2764-2784`) scans a range for the nearest word
  (marked `TODO: test this` upstream — no test to port).
- `selectOutput(pin)` (`:2886-2937`): `null` unless clicked cell is `.output`.
  Finds the enclosing prompt via `promptIterator(.left_up)`; if none, captures
  from screen top to just-before the next prompt. Uses
  `pages.highlightSemanticContent(prompt_pin, .output)` (landed — see
  [highlight.md](highlight.md)) then trims trailing whitespace via a left_up
  cellIterator.
- `lineIterator(start)` (`:2939-2966`): yields successive full soft-wrapped lines
  (selectLine with whitespace=null, boundary=false), advancing to `result.end.
  down(1)`.
- `promptClickMove` (`:2988-…`): **out of scope** (needs SelectionGesture click
  state + OSC133 click semantics — a later chunk; screen.md already defers its 17
  tests).

### `selectionString` — local plain-text path, NO formatter needed

`selectionString(alloc, {sel, trim, map})` (`Screen.zig:2471-2518`) builds a
`ScreenFormatter{emit=.plain, unwrap=true, trim}` over `content=.{selection}`.
In the Rust port there is **no `ScreenFormatter`** (a sibling chunk owns it), but
the `.plain` selection emit is fully reproducible with the same machinery the
existing `dump_string` uses: iterate the selection's rows via `containedRow`, emit
each contained row's cell codepoints (skipping spacer tail/head, appending
graphemes), join with `\n` at non-soft-wrap boundaries (unwrap), and optionally
trim trailing whitespace per row + trailing blank lines. Rectangle mode emits each
row's `[tl.x..br.x]` slice with a hard `\n` between rows (no unwrap). The
`map`/`StringMap`/pin-map parameter (used only by search) is **not exercised by any
ported test** and is deferred with `TODO(chunk:formatter)` — it needs the pin_map
plumbing that lives with the formatter. The 15 `selectionString*` tests only use
`{sel, trim}`, so the local path covers them all.

**Decision**: `selectionString` is implemented **locally** on `Screen` (no
formatter seam for the tested surface); only the optional string-map is seamed out.

## Lifecycle across scroll / resize / prune

Because the stored selection is *tracked*, its two pins ride the same fixup
machinery as the cursor pin (see [pagelist.md](pagelist.md) "Every pin fixup
site"): `cursorDownScroll`/`grow` prune move/garbage them, `eraseRow`/`resize`
reflow relocate them. No selection-specific code runs on scroll/resize — the pin
set does the work. Test "Screen: scrolling moves selection" asserts a selected
active row `y=1` becomes `y=0` after `cursorDownScroll` (the row moved into
scrollback-adjacent position), purely via pin tracking.

`clone` (`Screen.zig:442-567`) is the one place selection needs bespoke handling
because pins are duplicated into a fresh pagelist via a `TrackedPinsRemap`
(`:451-458`). Selection carry-over (`:488-555`):

1. order the selection (forward/mirrored_forward → tl=start,br=end; reverse/
   mirrored_reverse → tl=end,br=start).
2. `remap.get(tl)`; if missing, the tl pin fell outside the clone. Then, if
   `remap.get(br)` is *also* missing, the whole selection may be out of bounds:
   compare br.y/tl.y against the clone top's screen y — if br is above or tl is
   below the clone, `sel = null`. Otherwise clip tl to the clone's first row
   (`x = rectangle ? tl.x : 0`).
3. `remap.get(br)`; if missing, clip br to the clone's last row (`x = rectangle ?
   br.x : cols-1`, `y = last page rows-1`).
4. build a tracked selection from the (possibly clipped) start/end pins.

The 7 clone-selection tests cover: full/none/start-cutoff/end-cutoff/end-cutoff-
reversed/subset/subset-rectangle.

## Test inventory (exact, to port 1:1)

**`Selection.zig` own tests (13):** adjust right, adjust left, adjust left skips
blanks, adjust up, adjust down, adjust down with not full screen, adjust home,
adjust end with not full screen, adjust beginning of line, adjust end of line,
`Selection: order standard`, `Selection: order rectangle`, `topLeft`,
`bottomRight`, `ordered`, `Selection: contains`, `Selection: contains rectangle`,
`Selection: containedRow`. **Exactly 18 `test` blocks** (`grep -c '^test "'`),
all ported 1:1 into `screen/selection.rs`'s `#[cfg(test)] mod tests`.

**Deferred Screen selection tests (from `Screen.zig`, grep `test "…select`):**

- `Screen: scrolling moves selection` (`:4501`)
- clone: full / none / start cutoff / end cutoff / end cutoff reversed / subset /
  subset rectangle selection (`:5295-5544`, 7)
- `Screen: select untracked`, `select replaces existing pins` (`:7641`, `:7661`)
- `Screen: selectAll` (`:7687`)
- `Screen: selectLine` + across soft-wrap / across full soft-wrap / ignores blank
  lines / disabled whitespace trimming / with scrollback / semantic prompt
  boundary / prompt-to-input / input-to-output / mid-row / soft-wrap mid-row /
  boundary disabled / boundary first cell / all same content (`:7723-8409`, 14)
- `Screen: selectWord` + across soft-wrap / whitespace across soft-wrap / with
  character boundary (`:8411-8775`, 4)
- `Screen: selectOutput` (`:8777`)
- `Screen: selectionString` basic / start outside / end outside / trim space /
  trim empty line / soft wrap / wide char / wide char with header / empty with
  soft wrap / with zero width joiner / rectangle basic / rectangle w/EOL /
  rectangle more complex / multi-page (`:8873-9336`, 14)
- `Screen: lineIterator`, `lineIterator soft wrap` (`:9338`, `:9369`, 2)

Total deferred Screen selection tests: **46** (screen.md's "~45"): scrolling(1) +
clone(7) + select untracked/replaces(2) + selectAll(1) + selectLine(14) +
selectWord(4) + selectOutput(1) + selectionString(14) + lineIterator(2).
`selectionString map allocation failure cleanup` (`:9966`) is a tripwire alloc-
failure test — Rust is infallible-alloc, so it is not ported (same policy as the
PageList tripwire tests).
