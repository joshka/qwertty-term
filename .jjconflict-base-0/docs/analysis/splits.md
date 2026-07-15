# Splits slice 1: a surface tree per tab

Terminal splits (panes within a tab) for `qwertty-term`. Upstream Ghostty
implements splits **entirely in the macOS Swift layer**; this chunk ports that
model natively to Rust/AppKit (objc2), design-guided rather than transliterated.

Upstream reference (all citations pinned to Ghostty commit `2da015cd6`):

- `macos/Sources/Features/Splits/SplitTree.swift` — the tree model + focus /
  spatial navigation / resize algorithms.
- `macos/Sources/Features/Splits/SplitView.swift` + `SplitView.Divider.swift` —
  upstream's own **hand-rolled** SwiftUI split container (not `NSSplitView`).
- `macos/Sources/Features/Splits/TerminalSplitTreeView.swift` — recursive
  tree → view rendering, resize/drop operations.
- `macos/Sources/Features/Terminal/BaseTerminalController.swift` (~236-270,
  `newSplit`) — controller wiring: create surface, insert, move focus.
- `src/config/Config.zig` (~6625-6667) — the default split keybinds.

## Upstream's model (what we mirrored)

### Tree shape

`SplitTree` is an **immutable value tree**: `root: Node?`, `zoomed: Node?`,
where `Node = leaf(view) | split(Split)` and
`Split { direction, ratio: Double, left: Node, right: Node }`
(SplitTree.swift 5-25). `direction` is `horizontal` (children left|right) or
`vertical` (children top/bottom). `ratio` is the fraction of the container the
left/top child occupies. Every mutation (`inserting`, `removing`, `replacing`,
`resizing`) returns a new tree.

Our port: `crates/qwertty-term/src/splits.rs` — `SplitTree { root: Node,
focused: SurfaceId }`, `Node = Leaf(SurfaceId) | Split { axis, ratio, first,
second }`. Two adaptations:

- **Leaves are ids, not views.** Upstream's leaves hold the `SurfaceView`
  directly; ours hold an opaque `SurfaceId` keying a `HashMap<SurfaceId,
  Surface>` on the `Tab`, so the tree stays pure Rust (AppKit-free,
  unit-tested — 18 tests in `splits.rs`).
- **Root is never empty and mutation is in-place.** Upstream's `root: nil`
  empty state maps to "the tab is closed" for us; `SplitTree::close` returns
  `None` for the last leaf and the caller closes the tab. Rust ownership makes
  in-place mutation the natural fit; the *semantics* (what each operation
  produces) match the immutable versions.

### Insert (new split)

Upstream `inserting(view:at:direction:)` (SplitTree.swift 501-549): replace the
target **leaf** with a split whose children are the old leaf and the new leaf,
always at `ratio: 0.5`. `NewDirection` decides the slot: `left`/`up` put the
*new* view in the left/top slot, `right`/`down` in the right/bottom slot.
Inserting always resets `zoomed`. The controller (`newSplit`,
BaseTerminalController.swift 236-270) creates the surface, inserts, and moves
focus to the **new** view.

Ours is identical (`SplitTree::split` + `Controller::new_split`), including
new-pane-gets-focus. The new surface spawns its own shell via the existing
`TabIo::spawn`, inheriting the focused pane's OSC 7 pwd through the same
`tabs::inherit_pwd` path `new_tab_in` uses (upstream inherits via
`window-inherit-working-directory` on the surface config).

### Close / collapse

Upstream `removing(_:)` / `Node.remove` (SplitTree.swift 141-157, 594-630):
removing a leaf makes its **sibling take the parent split's place** — the
sibling absorbs the parent's whole rect, no ratio redistribution anywhere else.
Removing the root empties the tree.

Ours: `SplitTree::close` collapses identically; focus moves to the collapsing
sibling's leftmost leaf if the closed pane was focused (upstream picks the next
focus target from the surviving neighbourhood; same effect for slice-1 trees).
`cmd+w` and shell-exit both route through `Controller::close_surface`; the last
pane's close becomes today's `close_tab` (single-pane tabs behave exactly as
before). The close-tab re-entrancy rule is respected: the AppKit
`NSWindow::close` happens with no controller borrow held.

### Focus

- **Previous/next** (`focusTarget`, SplitTree.swift 177-200): flatten the
  leaves in-order, step with **wrap-around** (`indexWrapping`). Ours:
  `SplitTree::adjacent`.
- **Directional/spatial** (`focusTarget` + `Spatial.slots(in:from:)`,
  SplitTree.swift 202-232 and the `slots` filter): compute every node's pixel
  rect by recursive ratio division (top-left origin), keep only slots strictly
  on the far side (`bounds.maxX <= ref.minX` for left, etc.), sort by Euclidean
  distance and take the nearest leaf. Ours (`SplitTree::neighbor`) is the same
  family with two small deviations: we measure distance **centre-to-centre**
  (upstream: top-left-corner-to-corner) and require cross-axis overlap, which
  avoids selecting a corner-touching pane; for slice-1 tree shapes the results
  agree.
- **One focused surface per tab**, tracked in the tree. Click-to-focus: the
  pane view's `mouseDown:` calls `focus_surface_in_tab` before routing the
  press. Focus = AppKit first responder: keystrokes and IME land on the focused
  pane's `TerminalView` (`NSTextInputClient`) and nowhere else — input
  isolation falls out of the responder chain rather than needing routing
  checks. Mouse coordinates are inherently per-pane (each view converts
  `locationInWindow` into its own flipped space), so mouse reporting offsets
  are relative to the pane's grid.

### Divider ratio + resize

Upstream clamps ratios to `[0.1, 0.9]` (`resizing`, SplitTree.swift 305-315)
and converts a pixel offset to a ratio delta against the **split's own slot
extent** (`pixelOffset / splitSlot.bounds.width`). Window resize keeps ratios
(they're fractions; the SwiftUI layout re-divides new bounds).

Ours: same clamp constants (`MIN_RATIO`/`MAX_RATIO`), same geometry.
`SplitTree::split_rect(path)` reports the split's own container rect;
`Controller::drag_divider` maps the pointer's absolute position within that
rect to a ratio, sets it, and re-lays-out. Re-layout resizes **both** adjacent
panes: each `Surface::reflow` re-fits its grid to its new view bounds and posts
`TabIo::resize` (pty WINCH), the exact single-pane resize path multiplied.
Window resize calls the same layout with new bounds → proportional
redistribution.

## Layout mechanism: hand-rolled container (not `NSSplitView`)

`crates/qwertty-term/src/splitview.rs`. A tab's window content view is a plain
flipped `SplitContainer` (`NSView`) holding one `TerminalView` per pane at an
explicit frame plus a thin `SplitDivider` view per split (4 pt, draggable,
resize cursor). Frames come from the pure `SplitTree::layout`; the controller
applies them.

Why not `NSSplitView`:

- Each pane is a layer-backed view whose layer is the renderer's
  `IOSurfaceLayer` with carefully tuned `contentsScale` + `contentsGravity`
  (`pin_surface_to_top`, the R5 dark-band fix). `NSSplitView` owns its
  subviews' frames and inserts its own divider chrome, fighting that geometry
  and the flipped top-left coordinate space the mouse-report pixel math uses.
- **Upstream doesn't use `NSSplitView` either** — `SplitView.swift` is their
  own SwiftUI container with a custom divider, for the same control reasons.
- One pure function (`SplitTree::layout`) drives both the single-pane case
  (byte-identical to the pre-splits `Tab`: one leaf, whole container, no
  dividers) and the n-pane case.

One AppKit subtlety the container absorbs: the native tab bar
appearing/disappearing resizes the content area **without** firing
`windowDidResize:`. `SplitContainer` overrides `setFrameSize:` to trigger a
relayout (with a `try_borrow_mut` re-entrancy guard), so pane frames track the
content area exactly — verified by the geometry smoke's 2-tab phase.

## The Surface refactor

`app.rs`: the old `Tab` (one window = one engine + pty + renderer + view) split
into:

- **`Surface`** — the multiplied unit: `Arc<Mutex<Engine>>` + `TabIo` +
  `RenderEngine` + `FontGrid`/`FontSize` + `TerminalView` + per-pane
  grid/scale/selection/mouse state. All prior per-tab behaviour became
  per-surface unchanged (pump, render, reflow, cell_at, font rebuild).
- **`Tab`** — the window bundle: `SplitTree` + `HashMap<SurfaceId, Surface>` +
  `NSWindow` + `SplitContainer` + divider views. Window title reflects the
  *focused* pane's title (+ password marker).

The pace tick pumps + renders **every** surface of every tab (unfocused panes
keep updating); a pane whose shell exits closes just that pane
(collapse), and the last pane's exit closes the tab — same as today for
single-pane tabs.

Unfocused panes render with `FrameOptions { focused: false }`, which the
renderer already maps to the **hollow cursor** (upstream's unfocused cursor
treatment) — free, no renderer changes. Upstream's additional
`unfocused-split-opacity` dimming overlay is deferred (needs a per-pane overlay
or shader work; not cheap in the IOSurface presentation path).

## Keybinds (`splitkeys.rs`)

Same shape as `tabkeys.rs`: a static table + pure `resolve`, matched in the
view's `performKeyEquivalent:` **before** the tab table (tables are asserted
disjoint), consumed chords never reach the PTY encoder.

| Chord              | Action               | Upstream (Config.zig `2da015cd6`)        |
| :----------------- | :------------------- | :--------------------------------------- |
| `cmd+d`            | new_split right      | maintainer binding (see note)            |
| `cmd+shift+d`      | new_split down       | maintainer binding (see note)            |
| `ctrl+shift+o`     | new_split right      | upstream default, 6625-6628              |
| `ctrl+shift+e`     | new_split down       | upstream default, 6629-6633              |
| `ctrl+cmd+[`       | goto_split previous  | 6634-6639                                |
| `ctrl+cmd+]`       | goto_split next      | 6640-6645                                |
| `ctrl+alt+arrows`  | goto_split direction | 6646-6669                                |

Note: upstream's `new_split` defaults are `ctrl+shift+o`/`ctrl+shift+e` on
**all** platforms (no macOS override exists in Config.zig). The maintainer
asked for the macOS-conventional `cmd+d`/`cmd+shift+d` as the primary chords;
both sets are bound (the same pattern as the `cmd+shift+[`/`]` maintainer alias
in `tabkeys.rs`). `goto_split` chords match upstream exactly. Bare arrows /
unmodified keys never resolve here — they reach the PTY encoder untouched
(asserted in tests).

## Evidence

- `splits.rs` unit tests (18): split/collapse/ratio-clamp/layout/hit-test/
  directional-neighbour/adjacent-wrap/split_rect as pure functions.
- `splitkeys.rs` unit tests (7): chord table, disjointness from tab table,
  encoder fall-through.
- `QWERTTY_TERM_SMOKE_SPLITS=1` (+ `QWERTTY_TERM_ASSERT_PRESENT=1`), wrapped by
  `tests/splits_smoke.rs` (`--ignored`, needs a windowserver): split right then
  down → 3 panes; three **isolated** live shells proven by writing a distinct
  marker to each pane's pty and asserting each marker appears **only** in its
  own pane's engine (isolation + liveness in one probe; pty fds aren't exposed
  by `qwertty-term-termio`, and distinct-marker-per-shell is the stronger check);
  directional focus walk (left/up/down); per-pane presented-pixel ink (frame
  readback per pane, 3 distinct regions via per-surface `last_present_delta`);
  divider move shrinks the left pane's columns and grows the right column's
  (engine grids re-fit + `TabIo::resize` WINCH); closing the middle pane
  collapses to 2 with the sibling absorbing the space; closing every pane
  closes the tab.
- All pre-existing smokes pass unchanged: offscreen, geometry (1→2→1 tab
  chrome), typing (+ presented pixels), tab-keys.

## Slice 2 (landed)

All verified against upstream `2da015cd6`; see `splits.rs` / `selection.rs` /
`splitkeys.rs` / `config.rs` doc comments for per-item citations.

- **Unfocused dimming** (`unfocused-split-opacity` default 0.7, clamped
  `[0.15,1.0]` at Config.zig:4684; `unfocused-split-fill` default `null` →
  background). Upstream draws a `Rectangle().fill(fill).opacity(1 - opacity)`
  over an unfocused split pane (`SurfaceView.swift` 193-200, gate `isSplit &&
  !isFocusedSurface`). Replicated CPU-side in `selection::dim_window`: every
  cell's *fully-resolved* fg/bg (Default→renderer's `0xd8d8d8`/bg,
  Palette→palette, Rgb→self — matching the renderer's `resolve_color`) is
  blended toward `fill` by `overlay_alpha = 1 - opacity`. Applied in
  `Surface::render` only when the tab has >1 pane and the pane is unfocused; the
  focused pane and single-pane tabs never dim. Config keys parse + clamp in
  `config.rs`.
- **Zoom** (`toggle_split_zoom`, `cmd+shift+enter` = upstream
  `ctrlOrSuper(shift)+enter`, Config.zig:6857). Added `zoomed: Option<SurfaceId>`
  to `SplitTree`; `layout` renders only the zoomed leaf (upstream
  `TerminalSplitTreeView` `zoomed ?? root`). `relayout` hides the other panes'
  views. Unzoom triggers verified & ported: **insert resets zoom**
  (SplitTree.swift:124-129), **resize resets zoom** (:250/332), **close clears
  zoom iff the removed node was zoomed** (:152-153), **equalize preserves zoom**
  (:239), **directional navigation unzooms** (`ghosttyDidFocusSplit`, default
  `split-preserve-zoom` off — we don't expose that config). `isSplit` required
  to zoom (single pane can't zoom).
- **`resize_split`** (`cmd+ctrl+shift+arrows`, 10pt step, Config.zig:6671-6695).
  `SplitTree::resize_dir` walks up from the focused leaf to the nearest ancestor
  split of the matching axis (horizontal for left/right, vertical for up/down),
  moves its ratio by the pixel delta against that split's own slot extent,
  clamped `[0.1,0.9]`; resets zoom. No-op if no matching-axis ancestor.
- **`equalize_splits`** (`cmd+ctrl+=`, Config.zig:7050-7054). `SplitTree::equalize`
  is a verbatim port of `equalizeWithWeight`+`weightForDirection`
  (SplitTree.swift:685-729): each split's ratio = leftWeight/(leftWeight+rightWeight)
  where a subtree's weight *for an axis* is a leaf=1, a same-axis split=sum of
  children, a cross-axis split=1. Preserves zoom.

## Still deferred

- **Drag-to-reparent** (upstream's `TerminalSplitOperation.drop` zones) and
  session save/restore (upstream's `Codable` tree).
- **`split-preserve-zoom` config** (Config.zig:1102): navigation currently
  always unzooms (upstream's default); the `navigation` opt-in isn't exposed.
- **Zoom titlebar indicator**: upstream shows a reset-zoom accessory button
  (`TerminalWindow.swift` `surfaceIsZoomed`); we hide the other panes but add no
  titlebar chrome (consistent with our minimal chrome — deviation documented).
