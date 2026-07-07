# Renderer R0: geometry, cursor style, row heuristics, `RenderSnapshot`

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse --short 2da015cd6`; local working checkout
HEAD at time of port is `38e49a232`, two unrelated `repro:` commits ahead —
the six files below are byte-identical to the baseline). Scope: `src/renderer/
{size,State,cursor,row,Options,backend}.zig` (458+161+154+63+24+23 = 883
lines, 12 inline tests) plus the design of a new `RenderSnapshot` trait that
lets any future `ghostty-renderer` backend consume `ghostty-vt` state without
depending on its internal page/pin representation. Rust ports live at
`crates/ghostty-renderer/src/{size,cursor,row,options,backend,snapshot}.rs`.

This is chunk R0: the foundation every other renderer chunk (cell building,
GPU backends, betamax) imports. It intentionally ports only geometry + cursor
style + row heuristics + the snapshot contract — no GPU code, no font
shaping, no cell/run building. Those are later chunks (see
`docs/roadmap.md`'s R1..R4 sequencing).

## `size.zig` -> `size.rs`

**Purpose**: every coordinate system a renderer needs to convert between —
screen pixels, terminal pixels (screen minus padding), grid cells — plus the
padding-balancing math that centers or near-centers the grid inside a window
that isn't an exact multiple of the cell size.

**Key types**, all `extern struct` in Zig (plain `Copy` structs in Rust, no
`#[repr(C)]` needed since this crate has no C ABI surface yet):

- `CellSize { width: u32, height: u32 }` — pixel dimensions of one cell,
  DPI-already-applied (recalculation on DPI change is the caller's job).
- `ScreenSize { width: u32, height: u32 }` — the terminal-surface pixel size
  (a subset of the window, which may also have other chrome).
- `GridSize { columns: Unit, rows: Unit }` where `Unit` is `CellCountInt`
  (mirrors `crate::page::size::CellCountInt`, i.e. `u16` in this codebase —
  Zig's `terminal_size.CellCountInt`). `GridSize::update` floors
  `screen / cell` per axis and clamps to a **minimum of 1** in each
  dimension (`@max(1, calc_cols)`) — a zero-sized grid is never valid even
  when the screen is 0x0.
- `Padding { top, bottom, right, left: u32 }` (all default 0).
- `Size { screen, cell, padding }` — the aggregate; `.grid()` and
  `.terminal()` are derived (not stored) so they're always consistent with
  the three source fields.
- `PaddingBalance` enum: `False` (explicit only), `True` (balance, but cap
  top padding — see below), `Equal` (balance all sides evenly). Ported as a
  Rust enum with the same three variants (`False`/`True`/`Equal` — not
  `false`/`true`/`equal`; Zig's payload-free enum literals `.false`/`.true`
  collide with keywords, Rust doesn't have that constraint).
- `Coordinate` tagged union over three spaces: `Surface` (0,0 = top-left of
  the padded surface, pixels, negative/overflow allowed), `Terminal` (same
  origin shifted by padding, pixels), `Grid` (0,0 = top-left of the grid,
  *cell* units, x/y clamped into `[0, columns/rows - 1]` — never negative,
  but may need clamping on conversion *into* grid space from an
  out-of-bounds surface point).

**Math worth calling out** (all ported 1:1, including saturating arithmetic):

- `ScreenSize::sub_padding` uses **saturating subtraction** on both axes —
  a window shrunk to near-zero can make `padding.left + padding.right`
  exceed `width`; the Zig code comments this explicitly ("our padding can
  cause the padded sizes to be larger than our real screen"). Rust port
  uses `u32::saturating_sub`.
- `Padding::balanced(screen, grid, cell)`: compute leftover space
  (`screen - grid*cell` per axis, as `f32`), floor-divide by 2, clamp to
  `>= 0.0` **before** the int cast (this is what makes "Padding balanced on
  zero" pass: a 0x0 screen with a nonzero grid would otherwise produce a
  huge negative leftover that casts to a huge `u32`).
- `Size::balance_padding(explicit, mode)`: three-step recipe —
  1. set `self.padding = explicit` (so `self.grid()` reflects the
     explicitly-requested padding),
  2. recompute `self.padding = Padding::balanced(screen, self.grid(),
     cell)` (balancing happens *after* explicit padding has already shrunk
     the grid — the balanced padding is additional slack around that
     grid, not a replacement for the explicit request),
  3. `mode` dispatch: `Equal` is a no-op (keep the symmetric balance from
     step 2); `True` caps top padding at `(explicit.left + explicit.right +
     cell.width) / 2` and shifts any excess down to `bottom` (`vshift =
     padding.top.saturating_sub(max_top)`); `False` is unreachable (callers
     must not invoke balancing in `False` mode — Zig `unreachable`, Rust
     port documents this as a precondition/panics in debug via
     `unreachable!()` or is simply not exposed as a valid input, see
     divergence below).
- `Coordinate::convert` funnels every conversion through `Surface` as a
  pivot to avoid a combinatorial explosion of direct converters (3 spaces
  -> 6 direct pairs; funneling through one pivot needs only 2 functions per
  space). Grid conversion clamps both the *input* (surface/terminal
  coordinates can be negative or huge; clamp to `>= 0` before dividing by
  cell size) and the *output* (`min(col, columns - 1)`, `min(row, rows -
  1)`) — this is what "coordinate conversion" test's `100_000, 100_000`
  case exercises.

**Test count**: 6, all ported 1:1 (see below): `balance_padding_equal_
distributes_whitespace_equally`, `balance_padding_true_shifts_excess_top_to_
bottom`, `padding_balanced_on_zero`, `grid_size_update_exact`, `grid_size_
update_rounding`, `coordinate_conversion`.

**Divergence**: `PaddingBalance::False` is unreachable inside
`balance_padding` in the Zig source (`switch (mode) { .false => unreachable,
... }`) because the *caller* is expected to skip calling `balancePadding` at
all when balancing is disabled (explicit padding is used as-is). The Rust
port keeps the same shape — `balance_padding` takes `PaddingBalance` but
documents that passing `False` panics (`unreachable!()`), matching upstream's
contract rather than silently no-op'ing, so a future caller wiring this to
config doesn't hide a logic bug.

## `State.zig` -> folded into the `RenderSnapshot` design

Not ported as a standalone struct (see below for why).

**What it is upstream**: `State` is the mutable, mutex-guarded handle a
renderer thread holds: a `*std.Thread.Mutex` (guarding everything else in the
struct, NOT the struct itself — the struct's own fields are freely readable,
only what they *point to* needs the lock), `terminal: *Terminal`,
`inspector: ?*Inspector`, `preedit: ?Preedit`, `mouse: Mouse { point, mods }`.

This is upstream's **two-tier lock model**: the app/IO thread mutates the
live `Terminal` under the mutex; the renderer thread periodically locks,
copies out exactly what it needs into transient stack values or its own
owned buffers, unlocks, then spends the rest of the frame building GPU state
from the *copy* — never holding the terminal lock during the (much longer)
GPU-facing work. `State` itself is the bridge: a thin, swappable pointer
bundle handed to the renderer thread at construction; the mutex is a pointer
so the app and renderer threads share literally the same lock, not a copy.

The only *behavior* in `State.zig` beyond field storage is `Preedit`:
`codepoints: []const Codepoint` (each `{ codepoint: u21, wide: bool }`),
`.width()` (sum of 1 or 2 per codepoint), and `.range(start, max)` — given a
cursor cell position and the last grid column, computes where the preedit
text should actually be drawn: if it's wider than the remaining cells to the
right, shift left (never wrap or truncate at the right edge) and report
`cp_offset`, the index of the first codepoint that still fits, so a caller
walking codepoints left-to-right knows which one to start compositing from.
Both tests are pure functions of `Preedit`, no terminal needed:
`preedit_range_covers_exact_cell_width`, `preedit_range_shifts_left_at_right_
edge`.

**Why State isn't ported 1:1**: the two-tier lock model is a *threading*
concern specific to upstream's dedicated-renderer-thread architecture
(`docs/rewrite-prompt.md`'s Phase 4 note: "dedicated threads for termio read,
renderer, and app/UI"). This crate (R0) doesn't yet have a threading model —
that's a later chunk's job once we pick an app-shell architecture. What *is*
timeless and belongs in R0 is the **contract**: what data crosses the
lock/copy boundary from engine to renderer. That's exactly `ghostty-vt`'s
existing `Snapshot`/`SnapshotWindow` (an owned, `Vec`-backed, no-lifetime copy
of visible cells + cursor + palette) plus the fields `State` adds on top
(`preedit`, `mouse.point`/`mods`, `inspector` — inspector is out of scope,
kitty placements are a placeholder). `RenderSnapshot` (below) is the trait
that generalizes "hand a renderer one frame's worth of copied state" so a
future threaded implementation can produce it from a locked `Terminal` +
`State`-equivalent exactly the way `Screen::snapshot_window` does today
synchronously.

`Preedit::range` **is** ported verbatim as free functions/methods on a
`Preedit` type in `crates/ghostty-renderer/src/snapshot.rs` (2 tests), since
it's pure geometry a renderer needs regardless of threading model.

## `cursor.zig` -> `cursor.rs`

**Purpose**: `style(state, opts) -> Option<Style>` resolves the *actual*
cursor visual — or "don't draw a cursor at all" — from terminal mode state
plus renderer-local knowledge (focus, blink phase, IME preedit) that the
terminal itself doesn't track.

**`Style` enum** (renderer superset of terminal `CursorStyle`): `Block`,
`BlockHollow` (renderer-only, shown when unfocused), `Bar`, `Underline`,
`Lock` (renderer-only, password-input indicator). `Style::from_terminal`
maps the terminal's 4-variant `CursorStyle` (`Bar`/`Block`/`BlockHollow`/
`Underline` — already a 1:1 superset in this codebase's `screen::cursor::
CursorStyle`, so the mapping is the identity) — note upstream's terminal
`CursorStyle` also already includes `block_hollow` (reported as plain block
over the wire), matching `ghostty-vt`'s existing type exactly.

**`StyleOptions { preedit, focused, blink_visible }`** — three renderer-only
booleans not derivable from terminal state alone.

**Priority order in `style()` (load-bearing — ported exactly, including
comment)**:

1. `state.cursor.viewport().is_none()` -> `None`. (Cursor must be in the
   visible viewport — e.g. not scrolled into history — full stop, overrides
   everything else including preedit.)
2. `opts.preedit` -> `Some(Block)`, unconditionally (even if the terminal
   says the cursor is invisible) — preedit must always be visible to show
   editing state, but only reachable past check 1 (scrolled-into-history +
   preedit is still `None`; see "always block with preedit" test's second
   half).
3. `state.cursor.password_input` -> `Some(Lock)`.
4. `!state.cursor.visible` (DECTCEM / mode 25 off) -> `None`.
5. `!opts.focused` -> `Some(BlockHollow)` (always visible when unfocused,
   regardless of blink state — a hollow box is the "you're not looking at
   me" signal and must not blink away).
6. `state.cursor.blinking && !opts.blink_visible` -> `None`.
7. Otherwise -> `Some(Style::from_terminal(state.cursor.visual_style))`.

**Test count**: 4, all ported 1:1: `default_uses_configured_style` (all four
focus/blink combinations against a `Bar`-styled, blink-enabled cursor),
`blinking_disabled` (same but `cursor_blinking` mode off — style stays `Bar`
regardless of `blink_visible`), `explicitly_not_visible` (mode 25 off -> null
in all 4 combos), `always_block_with_preedit` (preedit true -> `Block` in all
4 combos while cursor is on-screen; then scroll into history -> `None` in all
4 combos even with preedit true, proving priority #1 over #2).

**What R0's port needs that isn't in `ghostty-vt` yet** (documented as
deferrals, not blockers — `style()` is portable today as a pure function over
a small input struct; only the *source* of two of its fields is missing):

- `cursor.password_input` — no `PasswordInput` terminal mode/flag wired in
  `ghostty-vt` yet (checked: no `password` hit in `modes.rs`; `Flags.
  password_input` exists at the *Terminal* struct level in `terminal/mod.rs:
  146` but nothing sets it from OSC/mode processing yet — plumbing is a
  future chunk, likely alongside OSC 133/prompt or a dedicated input chunk).
  R0's `cursor.rs::style()` still takes a `password_input: bool` field on its
  input struct; it just has no live wiring in the full-copy `RenderSnapshot`
  impl today (hardcoded `false`, documented inline).
- `cursor.blinking` — **is** available: `modes::Mode::CursorBlinking` exists
  and is queryable (`modes.rs:135`). Wired in the full-copy impl.
- `cursor.viewport()` (`Option<...>`, i.e. "is the cursor currently within
  the visible viewport") — `ghostty-vt`'s `Screen`/`Terminal` snapshot APIs
  don't currently expose "cursor is scrolled out of view" as a distinct
  concept; `SnapshotCursor` always reports a `col`/`row` relative to the
  *active area*, and `snapshot_window` always returns a `rows`-tall window
  regardless of scrollback offset. R0's trait models this as `cursor:
  Option<SnapshotCursor>` (`None` when not in the rendered viewport) and the
  full-copy impl always yields `Some` (it snapshots the active area, where
  the live cursor always is) — the `None` arm is exercised structurally but
  not reachable from the full-copy impl until a caller wires actual
  scrollback-offset-aware viewport snapshotting through `snapshot_window`'s
  existing `scrollback_offset` parameter (already present! the gap is purely
  "does the cursor's row fall inside `[window_top, window_top+rows)`", a
  one-line check R1 or the dirty-row impl can add).

## `row.zig` -> `row.rs`

**Purpose**: `never_extend_bg(row, cells, styles, palette, default_background)
-> bool` — heuristics for whether a row's background color should be
"extended" into the padding gutter beyond the last column (a cosmetic
feature: e.g. a solid-color status line looks better continuing into the
right/bottom padding than showing a hard-edged default-bg gutter next to it).
Returns `true` (never extend) when:

1. The row is a semantic-prompt row (`Prompt` or `PromptContinuation`, OSC
   133) — prompts often contain powerline/segment formatting that looks
   wrong stretched into padding.
2. **Any** cell in the row resolves to the terminal's current default
   background (either no explicit bg set, or an explicit bg that happens to
   equal `default_background`) — a default-colored cell already blends with
   default-colored padding, so extending adds nothing and risks looking
   patchy if only *some* cells match.
3. **Any** cell's codepoint falls in the Powerline private-use ranges
   (`U+E0B0..=U+E0C8`, `U+E0CA`, `U+E0CC..=U+E0D2`, `U+E0D4`) — these glyphs
   are drawn to exactly fill their cell edge-to-edge and are visually
   "perfect fit" already; extending the background past them breaks the
   illusion of a continuous shape.

Otherwise (every cell has a non-default explicit background and no
powerline glyphs) -> `false`, meaning the renderer *should* paint the row's
rightmost/bottommost cell's background color into the adjacent padding.

**Test count**: 0 upstream (the Zig source has a `// TODO: Test
neverExtendBg` comment — never implemented upstream). R0's Rust port adds
inline tests anyway (prompt row, default-bg cell, powerline glyph, and the
"extend" negative case) since porting a 0-test function into a stricter
language without any coverage would be a regression versus the rest of this
codebase's testing bar; these are new tests, not ports, and documented as
such in code comments and in the summary table below.

**Divergence — input shape**: upstream takes `row: page.Row, cells: []const
page.Cell, styles: []const Style, palette: *const Palette, default_
background: Rgb` — i.e. raw pointers into a live page. R0's `row.rs` instead
takes the already-*resolved* per-cell view: a row's `&[SnapshotCell]` (via
`ghostty-vt::snapshot::{SnapshotCell, SnapshotRow}`) plus `default_background:
Option<Rgb>` (matching `Snapshot::default_bg`'s `Option`), since
`SnapshotCell::style` already carries a resolved `CellStyle` (fg/bg as
`SnapshotColor`, not a style-table lookup) — there is no separate `styles`
slice to thread through in the snapshot model. `SemanticPrompt` isn't
currently surfaced on `SnapshotRow` (see deferrals) so R0's `row.rs` takes it
as an explicit parameter today rather than reading it off the row, with a
`// TODO` pointing at the same gap noted below.

## `Options.zig` -> `options.rs` (stub)

Upstream bundles everything a concrete renderer impl needs at construction:
derived config, a `*SharedGrid` font handle, `Size`, an apprt mailbox/surface
pointer, and a thread handle. None of font loading, apprt, or threading exist
in this codebase yet. R0 ports this as a **stub** — a `RendererOptions`
struct holding just `pub size: Size` (the one field this chunk can actually
type) with `// TODO(chunk:R1+)` comments enumerating the fields that will be
added once fonts/apprt/threading chunks land (`font_grid`, `surface_mailbox`,
`rt_surface`, `thread`, `config`). No tests upstream (0), none added — it's
inert data, nothing to assert yet beyond what `Default`/construction already
proves at compile time.

## `backend.zig` -> `backend.rs` (stub)

Upstream: `Backend` enum (`opengl`/`metal`/`webgl`) plus a `default(target,
wasm_target)` platform-detection function (Darwin -> Metal, wasm32 -> WebGL,
else OpenGL). R0 ports the enum verbatim (`Backend::{OpenGl, Metal, WebGl}`)
but the platform-`default()` function is deferred — this crate doesn't yet
pick or link against any GPU API, and encoding a "Metal on macOS" default
before any backend actually exists would just be dead code. Stubbed with a
`// TODO(chunk:R2+ GPU backend)` note instead. No tests upstream (0), none
added.

## The two-tier lock + owned-snapshot consumption model

Upstream's per-frame data path (`State.zig` + the renderer thread's use of
it, cross-referenced with `docs/rewrite-prompt.md`'s threading note) is:

1. **Tier 1 — the terminal lock.** The app/IO thread holds `*Terminal`
   behind `state.mutex` and mutates it continuously as bytes arrive. The
   renderer thread, once per frame, takes the *same* lock, and while holding
   it: reads cursor position/style/visibility, walks the *visible* rows only
   (not full scrollback) building its own cell/run representation, reads
   `preedit`/`mouse`/`inspector`, then **releases the lock**. This is
   `Terminal.render_state`-style access in spirit, though the concrete Zig
   renderer inlines the walk rather than calling a single "snapshot" method
   — there is no equivalent of `ghostty-vt`'s owned `Snapshot` struct
   upstream; each renderer backend (Metal/OpenGL) does its own walk-and-copy
   directly into GPU-shaped buffers (cell arrays, glyph atlas keys) while
   holding the lock, which is *more* backend-specific coupling than this
   Rust rewrite wants at the boundary.
2. **Tier 2 — the copy.** Everything produced during tier 1 (cell/run
   buffers, cursor style, preedit range) is now plain owned data with no
   borrow of the terminal. The (much slower) GPU work — texture uploads,
   draw call submission, vsync wait — happens entirely against this copy,
   so the terminal lock is held only for the O(visible-rows) walk, not for
   the O(frame-time) GPU work. This is the whole point of the split: without
   it, a slow GPU frame would stall the input-reading thread.

**Why `ghostty-vt`'s existing `Snapshot`/`SnapshotWindow` already model tier
1's *output* faithfully, and why R0 formalizes it as a trait instead of
inlining the walk per-backend:** `Screen::snapshot_window` / `Terminal::
snapshot_window` (see `crates/ghostty-vt/src/snapshot.rs`) already do
exactly the tier-1 job — walk only the visible window (cost proportional to
`rows`, not total scrollback), copy cells with fully-resolved styles, copy
cursor position/style/visibility, copy the palette + dynamic fg/bg — and
return a plain, `Vec`-backed, lifetime-free struct. That *is* tier 2's input.
What upstream does ad-hoc per-GPU-backend (walk once, build backend-specific
buffers directly), this rewrite splits into two composable seams: (a)
`ghostty-vt` produces a generic, backend-agnostic owned snapshot (already
built, R0 adds the trait around it), (b) each renderer backend consumes that
snapshot to build its own cell/run/GPU buffers (a later chunk, not R0).
This keeps `ghostty-vt` renderer-agnostic (it has zero renderer-specific
types) while still giving every future backend the same cheap,
lock-minimal, allocation-shaped-once frame data.

The mutex itself (tier 1's synchronization primitive) is explicitly **not**
part of this contract — `RenderSnapshot` describes *what* a renderer needs
per frame, not *how* it's produced under a lock. A future threaded
implementation obtains a `Terminal` lock, calls `terminal.snapshot_window(...)`
(or the future dirty-row equivalent), and hands the result across the
tier-1/tier-2 boundary; R0 doesn't need to model the lock/thread machinery to
specify that contract.

## The `RenderSnapshot` trait

```rust
/// Everything a renderer needs to draw one frame, decoupled from
/// `ghostty-vt`'s internal page/pin representation.
///
/// Two planned implementations:
/// - `FullSnapshot` (implemented now): wraps `ghostty_vt::snapshot::
///   SnapshotWindow`, built fresh (a full visible-window copy) every frame.
///   Simple, correct, and already cheap (O(visible rows), not O(scrollback))
///   thanks to `Screen::snapshot_window`'s backward-walk-from-bottom
///   design — but still re-copies every cell every frame even when only
///   one row changed.
/// - `DirtySnapshot` (contract only, lands with a future chunk once
///   `PageList`'s existing per-row dirty bit — see `Row::dirty()` /
///   `Row::set_dirty()` at `crates/ghostty-vt/src/page/page_impl.rs:187-195`,
///   bit 40 of the packed row header, already ported from `row.zig`'s Zig
///   bit layout but not yet surfaced through `snapshot`/`snapshot_window` —
///   is threaded through to report exactly which visible rows changed since
///   the last frame, and the palette/cursor/preedit are only recopied when
///   their own dirty flags (`Terminal.flags.dirty`, `Screen.dirty`, both
///   already ported) are set.
pub trait RenderSnapshot {
    /// Number of columns / visible rows this snapshot covers. Always
    /// matches the renderer's current grid size for this frame.
    fn cols(&self) -> usize;
    fn rows(&self) -> usize;

    /// What changed since the renderer's last consumed frame.
    fn dirty(&self) -> DirtyStatus;

    /// One visible row, 0-indexed from the top of the rendered window.
    /// Panics (or returns a blank row — impl's choice, documented per-impl)
    /// if `dirty()` is `Partial` and `row` isn't in `dirty_rows()`; callers
    /// that honor partial dirty must skip un-dirtied rows rather than
    /// calling this for every row every frame.
    fn row(&self, row: usize) -> &[SnapshotCell];

    /// Cursor to draw this frame, or `None` if it's outside the visible
    /// viewport, invisible, or otherwise suppressed (see `cursor::style`'s
    /// priority order in this same doc for the *style* decision — this is
    /// just position/raw-visibility data; `cursor::style()` is applied on
    /// top by the caller, which also supplies focus/blink/preedit opts that
    /// have no terminal-state source).
    fn cursor(&self) -> Option<SnapshotCursor>;

    /// Current 256-color palette + dynamic default fg/bg, resolved through
    /// exactly like `ghostty_vt::snapshot::Snapshot::palette/default_fg/
    /// default_bg`.
    fn palette(&self) -> &Palette;
    fn default_fg(&self) -> Option<Rgb>;
    fn default_bg(&self) -> Option<Rgb>;

    /// IME composition text to render over/near the cursor, if active.
    /// Placeholder: mirrors `State.Preedit` (codepoints + width/range
    /// helpers, ported in `snapshot.rs`) but no `ghostty-vt` producer wires
    /// it yet (no OSC/input-layer preedit state lands until an input
    /// chunk) — `FullSnapshot` always returns `None`.
    fn preedit(&self) -> Option<&Preedit>;

    /// Kitty graphics placements visible this frame. Placeholder: the
    /// `kitty::Placement`/`ImageStorage` model exists in `ghostty-vt`
    /// (`crates/ghostty-vt/src/kitty/storage.rs`) but isn't threaded
    /// through `Snapshot`/`SnapshotWindow` yet — `FullSnapshot` always
    /// returns an empty slice.
    fn kitty_placements(&self) -> &[KittyPlacement];
}

/// What changed since the last frame this renderer consumed.
pub enum DirtyStatus {
    /// Everything must be repainted (first frame, resize, palette swap,
    /// clear, etc).
    Full,
    /// Only the rows in `dirty_rows` changed; cursor/palette/preedit may
    /// also have changed independently — check those dirty bits/values
    /// directly rather than assuming they track row dirtiness.
    Partial { dirty_rows: Vec<usize> },
}
```

`FullSnapshot` (implemented now, `snapshot.rs`): wraps a `ghostty_vt::
snapshot::SnapshotWindow` (built via `Terminal::snapshot_window`), always
reports `DirtyStatus::Full`, and answers every accessor straight from the
wrapped fields — `row()` indexes `window.window[row].cells`, `cursor()` is
always `Some` (see the cursor.rs deferral above re: viewport clamping),
`preedit()`/`kitty_placements()` always empty. This is correct (byte-for-byte
matches what a full repaint would show) but O(visible cells) per frame
regardless of how much actually changed — acceptable for R0 and for any
renderer backend that repaints every frame anyway (e.g. while GPU backends
don't yet exist to make redraw cost matter), not acceptable long-term for a
60-120fps target with mostly-static screens (a shell prompt, a pager, etc.)
where most rows don't change between frames.

### What the future `DirtySnapshot` impl needs from `ghostty-vt` (precise list)

None of this blocks R0 — it's the exact scope for the chunk that wires
dirty-row tracking through to the renderer boundary:

1. **Surface `Row::dirty()` through `snapshot_window`.** The bit already
   exists (`page_impl.rs` bit 40, `Row::dirty()`/`set_dirty()` at lines
   187-195) and is already set/cleared by mutation paths (confirmed: used in
   resize/reflow at `page_impl.rs:1996` and asserted in tests at `:2325`),
   but `Screen::snapshot_window` currently always does a full row-copy walk
   and never reads or clears the bit. Needs: (a) a way to query "is row N
   (by absolute/window-relative index) dirty" without copying its cells, and
   (b) a way to **clear** the dirty bit for rows the renderer has now
   consumed — upstream's dirty-bit lifecycle is presumably
   set-on-mutation / cleared-on-render, which implies the clearing needs to
   happen either in `snapshot_window` itself (as a side effect — arguably
   wrong, since `snapshot_window` today is a read-only `&self` method with
   no mutation) or via a new explicit `&mut self` method the renderer calls
   after consuming a frame (e.g. `PageList::clear_dirty_in_window(...)` or
   similar) — this design choice is exactly what the dirty-row chunk needs
   to make, not something R0 should preempt.
2. **A cheap "which of the `rows` visible rows changed" query** scoped to
   the *current viewport window* (mirroring `snapshot_window`'s existing
   backward-walk-from-bottom cost model) rather than a scan of the whole
   page list — otherwise the dirty-row impl's "figure out what's dirty" step
   costs more than the "just copy everything" step it's trying to avoid.
3. **Terminal/Screen-level dirty flags surfaced as queryable, not just
   internally consumed.** `Terminal.flags.dirty` (`palette`, `reverse_colors`,
   `clear`, `preedit`, `glyph_glossary`) and `Screen.dirty` (`selection`,
   `hyperlink_hover`) already exist and are set by mutation paths (confirmed:
   `stream.rs:1760,1773,1785` set `dirty.palette` on OSC 4/104/etc) but
   aren't currently read by anything outside `ghostty-vt` — no public
   accessor exists to ask "has the palette changed since I last checked".
   `DirtySnapshot` needs read (and renderer-side "I've consumed this, clear
   it") access to at least `dirty.palette`, `dirty.clear`, `dirty.preedit`,
   and `Screen.dirty.selection` to decide whether non-row state
   (palette/cursor/preedit/selection highlight) needs recopying even when no
   *row* is dirty.
4. **Cursor-moved-without-a-row-dirty tracking.** The cursor can move
   between two cells without either cell's *content* changing (plain cursor
   motion), which today wouldn't set either row's dirty bit (dirty is a
   content-mutation signal, not a cursor-position signal upstream). The
   dirty-row impl needs either: cursor position included unconditionally in
   every partial frame (simplest — cursor is cheap, always send it), or an
   explicit "cursor moved" flag alongside the row dirty set. Recommend the
   former (always include cursor in `DirtyStatus::Partial`'s payload or as a
   separate always-present accessor, which the trait sketch above already
   does — `cursor()` is unconditional, not gated by dirty status) so this
   isn't actually a new requirement on `ghostty-vt`, just a design note for
   whoever implements `DirtySnapshot`.
5. **Preedit state producer.** Not `ghostty-vt`'s job per se (preedit is
   IME/input-layer state, arguably belongs on an app-shell / input chunk
   above both `ghostty-vt` and `ghostty-renderer`), but whatever chunk adds
   it needs to decide where it lives so `RenderSnapshot::preedit()` has a
   real source; flagging here so R0's placeholder isn't forgotten.
6. **Kitty placement geometry resolved against current grid.** `kitty::
   storage::Placement` already carries `location`/offsets/source rect/
   columns/rows/z (confirmed at `kitty/storage.rs:120-140`), and `kitty::
   mod.rs` documents "the terminal geometry the placement model needs to
   compute pixel/grid sizes and rects" as an existing seam — the dirty-row
   (or even the full-copy) impl eventually needs a way to enumerate
   placements whose top-left pin falls within the current visible window,
   analogous to how cells are windowed today. Not needed until a chunk
   actually renders images; flagged for completeness since the trait already
   reserves the accessor.
7. **`CursorStyle`/mode plumbing already sufficient for cursor.rs.**
   Confirmed no gap here beyond `password_input` (item redundant with the
   cursor.zig section above, listed for completeness of "everything
   DirtySnapshot needs" in one place): `modes::Mode::CursorBlinking` /
   `CursorVisible` exist and are queryable now; only `password_input` needs
   a producer.

## Summary: Zig vs Rust test counts

| File          | Zig lines | Zig tests | Rust file     | Rust tests |
| ------------- | --------: | --------: | ------------- | ---------: |
| `size.zig`    |       458 |         6 | `size.rs`     |          6 |
| `State.zig`   |       161 |         2 | `snapshot.rs` |          2 |
| `cursor.zig`  |       154 |         4 | `cursor.rs`   |          4 |
| `row.zig`     |        63 |         0 | `row.rs`      |          4 |
| `Options.zig` |        24 |         0 | `options.rs`  |          0 |
| `backend.zig` |        23 |         0 | `backend.rs`  |          0 |
| *(new)*       |         — |         — | trait + impl  |         6+ |

Notes on the table: `size.rs` and `cursor.rs` are 1:1 ports (same test
count). `State.zig`'s 2 tests are `Preedit::range`'s tests, ported 1:1 into
`snapshot.rs`; the rest of `State` is reframed as the `RenderSnapshot` trait
rather than ported as a struct (see above). `row.rs` has 0 upstream tests
(documented `TODO` in the Zig source) but gains 4 new tests in this port.
`Options.zig`/`backend.zig` are inert stubs with nothing to test yet. The
`snapshot.rs` trait + `FullSnapshot` impl is new: 6+ tests exercising frame
coherence over a live `Terminal` across writes/resizes.

## Deferrals (out of scope for R0, tracked above)

- `cursor.password_input` source (no mode/OSC wiring in `ghostty-vt`).
- `Row::dirty()` not surfaced through `snapshot`/`snapshot_window`; no clear-on-consume API.
- `Terminal.flags.dirty` / `Screen.dirty` have no public read accessor outside the crate.
- Preedit producer (IME/input-layer, likely a different future chunk entirely).
- Kitty placement windowing against the visible grid.
- Cursor viewport-membership check in `snapshot_window` (currently always
  "in view" since only the active area is windowed today;
  scrollback-offset-aware cursor visibility is a one-line addition once
  needed).
- `Backend::default()` platform detection (no GPU backend exists yet to default to).
- `Options` fields for font grid / apprt surface / thread handle (later chunks own those subsystems).
