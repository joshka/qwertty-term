# Highlights + semantic-content highlighting (`src/terminal/highlight.zig` + `PageList.highlightSemanticContent`)

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse --short HEAD`). This chunk ports the standalone
`highlight.zig` module (~213 lines, 0 inline tests) plus the `highlightSemanticContent`
method of `PageList.zig` (`:4340-4470`) and its 17 inline tests (`:8313-9374`), which the
[PageList port](pagelist.md) deliberately deferred because it depends on `highlight.zig`.
The Rust port lands `highlight.zig` at `crates/ghostty-vt/src/highlight.rs` and
`highlightSemanticContent` additively in `crates/ghostty-vt/src/pagelist/ops.rs`.

## What `highlight.zig` models

A **highlight** is any contiguous run of cells that should be "called out" — text
selection is the headline use, but search results and (here) semantic prompt/input/output
zones are the same concept. The module header notes the long-term plan is for highlights to
*replace* `Selection` entirely; today it is a generic range-of-cells representation with
three storage flavors, differing only in how the endpoints are stored and how robust they
are to terminal mutation.

The invariant across all three: **`start` MUST be before-or-equal-to `end`** (top-left
before bottom-right in screen order). For rectangular selections, the *consumer* reinterprets
the two pins as x-bounds per row — the module itself never encodes shape.

### The three representations

| Type                     | Endpoints stored as                       | Survives mutation?                                       | Purpose                                                                                     |
| ------------------------ | ----------------------------------------- | -------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| `Untracked` (`:31-49`)   | two `Pin` values                          | No — valid only for current terminal state               | cheap, transient result (what `highlightSemanticContent` returns)                           |
| `Tracked` (`:62-105`)    | two `*Pin` (tracked pins in a `Screen`)   | Yes — `PageList` fixes them on every mutation            | long-lived highlights (a persistent selection)                                              |
| `Flattened` (`:112-213`) | `MultiArrayList(Chunk)` + `top_x`/`bot_x` | Yes for traversal (holds node+serial, no live pin deref) | iterate the whole area without reading terminal state or dereferencing possibly-pruned pins |

- **`Untracked`** — `{ start: Pin, end: Pin }`. `track(screen)` promotes it to a `Tracked`
  by tracking both pins; `eql` is pin-equality on both endpoints.
- **`Tracked`** — `{ start: *Pin, end: *Pin }`. `init` tracks both pins (with `errdefer`
  untrack on the second failing); `initAssume` wraps already-tracked pins with no allocation
  (caller must not `deinit`); `deinit` untracks both. More overhead, more operations.
- **`Flattened`** — `{ chunks: MultiArrayList(Chunk), top_x, bot_x }`, where
  `Chunk = { node: *Node, serial: u64, start: u16, end: u16 }`. The chunk list carries the
  y-bounds (`chunks[0].start` = first row, `chunks[len-1].end` = last row exclusive) so the
  whole area can be walked without pin math. It flattens the page *serial* alongside the node
  so validity/equality checks against the `PageList` are robust to node reuse.
  `bot_x` may be numerically *less* than `top_x`: a left-to-right selection can start to the
  right of its end on a higher row. Helpers: `init` (build from a `start`/`end` pin pair via
  `pageIterator(.right_down)`), `clone`, `startPin`/`endPin`, and `untracked` (collapse back
  to an `Untracked`; note `end.y = ends[last] - 1` because chunk end is exclusive).

### Upstream bug noted during the port

`Flattened.init` (`:155-159`) constructs its result with `.end_x = end.x`, but the struct
field is named `bot_x` (`top_x`/`bot_x` everywhere else). This does not compile as written —
it is almost certainly dead/untested code upstream (`Flattened` has no inline tests and no
in-tree consumer; see below). The Rust port uses the field name the struct actually declares
(`bot_x = end.x`) and leaves a code comment. Flagged, not "fixed" in Zig — out of scope.

## `PageList.highlightSemanticContent` (`:4340-4470`)

Given a pin `at` on a **prompt row** and a `SemanticContent` selector
(`.prompt` | `.input` | `.output`), returns an `Untracked` highlight covering that kind of
content within the current command "zone", or `null` when there is none. Consumers call it,
e.g., on a click in the prompt gutter to select the whole command, its input, or its output.

### Step 1 — bound the zone by the next prompt (`:4349-4367`)

Walk prompts forward from `at` with `promptIterator(.right_down, null)`:

1. The first prompt returned must be `at` itself (`assert(it.next().?.y == at.y)`) — a safety
   assertion that `at` is genuinely a prompt row.
2. If a *second* prompt exists, the zone `end` is the last cell of the row **just above** it:
   `next.up(1)`, x set to `cols - 1`.
3. If there is no further prompt, the zone `end` is `getBottomRight(.screen)` — the last cell
   of the screen.

`PromptIterator` treats the first line of a prompt as the prompt and skips continuation lines,
so a multi-row prompt is one logical prompt; the "next prompt" is the start of the *next*
command.

### Step 2 — scan cells within `[at, end]` by `semantic_content` (`:4369-4468`)

A single forward `cellIterator(.right_down, end)` pass, dispatched on the selector. Each cell
carries its own `semantic_content` (`.prompt`/`.input`/`.output`); rows carry
`semantic_prompt` (used only for zone bounding in step 1).

- **`.prompt`** — start at `at.left(at.x)` (column 0 of `at`'s row), end initially at `at`.
  Extend `end` across every `.prompt` **or** `.input` cell; **break on the first `.output`**.
  (Selecting a prompt selects the prompt text and its typed input, stopping at command output.)
- **`.input`** — find the first `.input` cell → that is `start` (and initial `end`); `.prompt`
  cells before it are skipped; hitting `.output` before any input → `return null` (also `null`
  if the scan ends with no input). Then extend `end` over `.input` cells, **skipping `.prompt`
  cells** (continuation prompts nest inside multi-line input), **breaking on `.output`**.
- **`.output`** — find the first `.output` cell **that `hasText()`** → `start`/initial `end`
  (`.prompt`/`.input` skipped; empty `.output` cells skipped — see below); no such cell →
  `return null`. Then extend `end` to each subsequent `.output` cell **that `hasText()`**,
  **breaking on any `.prompt`/`.input`**.

### Edge case the tests pin down: empty cells default to `.output`

A cell's default `semantic_content` is `.output` (enum value 0). A short prompt or input line
leaves its trailing cells empty-but-`.output`. The `.output` branch therefore requires
`cell.hasText()` both to *start* and to *extend* the highlight, so those trailing blanks are
not mistaken for real command output (test *"output skips empty cells"*: output is found on
row 7, not the blank tail of rows 5-6). The `.prompt`/`.input` branches do **not** gate on
`hasText` — they extend over whatever the cell's semantic tag says.

### Behaviors the 17 tests lock in

`prompt`: basic prompt+input stops at output; multiline prompt→input spans two rows; prompt
with trailing output excluded; prompt-only (no input); prompt with no following prompt runs to
end of screen. `input`: basic; stops at output; multiline with continuation-prompt nesting;
`null` when no input / when only prompts; runs to end of screen. `output`: basic (bounded by a
following prompt on the same row); multiline; stops at next prompt; runs to end of screen;
`null` when no output; skips empty default-`.output` cells.

## Consumers of highlights (surveyed `src/`)

> Filled from a grep of `~/local/ghostty/src/` for `highlightSemanticContent`, `highlight.zig`,
> `highlight.Untracked/Tracked/Flattened`, `.untracked(`, `.track(`. `highlight.zig` is exported
> as `terminal.highlight` (`terminal/main.zig:17`).

- **`highlightSemanticContent` → `Screen.selectOutput`** (`Screen.zig:2923`). The one direct
  caller: `selectOutput(pin)` calls `highlightSemanticContent(pin, .output)`, then trims
  trailing whitespace from the returned `Untracked` range and converts it into a `Selection`.
  This is the click-on-a-prompt → "select this command's output" path — exactly the
  Selection-bridging use the module header foreshadows. (The port lands the query but not
  `selectOutput`, which belongs to the not-yet-ported `Screen` and is a sibling agent's
  territory this chunk must not touch.)
- **The `Flattened` type feeds the search + renderer pipeline.** `highlight.Flattened` is the
  currency of the whole search subsystem (`terminal/search/*.zig` — sliding-window matcher,
  active/history/viewport/pagelist searches, and `search/Thread.zig`), which produces
  `Flattened` match ranges that survive scrollback pruning. Those flow: search engine →
  `ScreenSearch` (which `.untracked()`s then `.track()`s the user's *selected* match into a
  `Tracked` for cross-frame persistence) → `renderer/message.zig` `SearchMatches`/`SearchMatch`
  (carrying `[]const highlight.Flattened`) → `Surface.zig:1435` (clone into arena, post to the
  render thread) → `render.zig:658` `updateHighlightsFlattened` (walk `Flattened.chunks` by
  node, mark rows dirty/highlighted for the GPU). `Untracked` is the ephemeral current-state
  form; `Flattened` is the mutation-robust form; `Tracked` bridges an ephemeral snapshot into a
  persistent reference.
- **Port scope.** This chunk lands the three data types (`Untracked`/`Tracked`/`Flattened`) and
  `highlight_semantic_content` for parity and to close the 17 tests. It does **not** wire the
  downstream consumers: `Screen.selectOutput`, the search subsystem, and the renderer all live
  in not-yet-ported (or sibling-owned) modules. When those chunks land they consume
  `highlight::Untracked`/`Flattened` exactly as the Zig graph above describes.

## Rust port notes

- **Module** `crates/ghostty-vt/src/highlight.rs`, registered `pub mod highlight;` in `lib.rs`.
  Ports `Untracked`, `Tracked`, `Flattened` (+ `Flattened::Chunk`). Infallible-alloc idiom
  (matching the PageList port): `Tracked::init`/`Untracked::track` do not return `Result` since
  `track_pin` is infallible in the Rust model; the Zig `errdefer` untrack-on-failure logic is
  therefore moot. `MultiArrayList(Chunk)` → `Vec<Chunk>` (the SoA layout is a Zig micro-opt with
  no Rust equivalent needed at this size). Raw `*mut Node` / `*mut Pin` mirror the pagelist
  unsafe boundary; the module is `#![allow(clippy::not_unsafe_ptr_arg_deref)]` for the same
  handle-vended-by-this-PageList reason documented in `pagelist/mod.rs`.
- **`highlight_semantic_content`** added to `pagelist/ops.rs` (alongside `PromptIterator` and
  `get_cell`, which it reuses). Returns `Option<highlight::Untracked>`. Uses the existing
  `Pin::cell_iterator`, `Pin::left`, `Pin::up`, `get_bottom_right`, and the ported
  `PromptIterator`.
- **PromptIterator caveat.** The pre-existing `PromptIterator` port (`ops.rs`) is the simplified
  variant used by `scrollPrompt`: it yields only rows whose `semantic_prompt == .prompt`
  (skipping continuations by advancing one row at a time) and takes no `limit`. `Untracked`
  zone-bounding only needs `next()` twice with a `null` limit (self, then the next distinct
  prompt), and every test's "next prompt" is a `.prompt` row, so the simplified iterator is
  behaviorally equivalent here. The full `nextRightDown`/`nextLeftUp` continuation-skip +
  `limit` machinery is only needed by consumers not in this chunk; noted in a code comment.
- **Tests** ported 1:1 into `pagelist/tests.rs` (all 17). Test scaffolding writes cells with
  `Cell::init(cp)` + `set_semantic_content`, and rows with `set_semantic_prompt`, via
  `get_cell(...).cell` / `row_and_cell`, mirroring the Zig `page.getRowAndCell(x,y).cell.* = …`
  setup. Assertions compare `point_from_pin(Tag::Screen, hl.start/end)` against expected
  `Point::screen(x, y)`, matching upstream. `highlight.zig` itself contributes **0** tests.
