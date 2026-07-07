# Renderer R5: the window swap — native AppKit host

Chunk R5 of `docs/plans/m3-first-pixels.md`: replace the egui host with a native
macOS `NSApplication`/`NSWindow` that renders the terminal through the Metal
stack (R1–R4), with **native window tabs** (each tab its own engine + PTY), a
menu bar, and the `NSTextInputClient` input path proven by the R5 de-risk spike
(`docs/analysis/appkit-input.md`). It lands a new crate, `crates/ghostty-app`
(binary `ghostty-app`), plus additive-only presentation wiring in
`crates/ghostty-renderer`. The egui spike (`crates/spike`) is untouched
reference material.

The host choice is **raw AppKit via `objc2`, not winit** — the PROPOSED verdict
of `docs/analysis/appkit-input.md`, proceeded on here. The decisive factors were
`performKeyEquivalent` access, native per-tab windows, an owned `NSMenu` +
`AppDelegate`, and IME correctness edges — all of which winit either hides or
fights for `NSApplication` ownership.

## The app architecture

### Object graph

- **`Controller`** (`app.rs`) — `Rc<RefCell<ControllerState>>`, the shared brain.
  Owns the `TabRegistry` and a `HashMap<TabId, Tab>`, the config, and the input
  config. Menu actions and view events call into it. It is single-threaded
  (main thread): everything terminal-side lives on the run loop, so `Rc`/`RefCell`
  rather than `Arc`/`Mutex`. The *only* off-thread work is each PTY's background
  reader thread (inside `PtySession`), which communicates through an mpsc channel
  the pace loop drains.

  > Departure from the brief's "engine behind a mutex, render on a background
  > pace tick" shape: because the appkit-input verdict has AppKit own
  > `NSApplication.run`, and Metal command submission + CoreAnimation `contents`
  > assignment must run on the main thread, the pace tick is an `NSTimer` on the
  > main run loop and the engine is simply main-thread-owned. No mutex is needed
  > or wanted; the mutex would only serialize the render thread against a PTY
  > thread that never touches the engine. This is simpler and race-free.

- **`Tab`** (`app.rs`) — one terminal: a vt `Engine` (`engine.rs`, a thin
  `ghostty-vt` `Stream`+`Terminal` wrapper mirroring the spike's engine adapter),
  a `PtySession`, a render `Engine` (R4), a `FontGrid` (`font.rs`), a `FontSize`
  (`font_size.rs`), the owning `NSWindow` + `TerminalView`, current grid dims,
  backing scale, and mouse-report dedup state.

- **`AppDelegate`** (`app.rs`, `define_class!` NSObject) — builds the menu, opens
  the first window on `applicationDidFinishLaunching:`, starts the pace timer,
  and (smoke) schedules an auto-exit. It is also the menu/key-equivalent target:
  a single `ghosttyMenuAction:` selector reads the sender's `tag`, maps it back
  to a `MenuAction` (`menu.rs`), and dispatches.

- **`TerminalView`** (`view.rs`, `define_class!` NSView) — hosts the renderer's
  `IOSurfaceLayer` (R2) as its layer, conforms to `NSTextInputClient`, accepts
  first responder, and is coordinate-flipped (top-left origin, matching terminal
  mouse-report pixel space and the grid's row order). keyDown runs the upstream
  `interpretKeyEvents` dance; committed/encoded bytes route to the tab's PTY via
  the controller. Mouse events encode to the PTY when the program enabled mouse
  reporting.

### The render + present path

Per tab, per pace tick (`Tab::pump` then `Tab::render`):

1. `PtySession::try_read` drains the reader-thread channel; bytes → `Engine::write`.
2. Engine reply bytes (`take_output`) → PTY (DA/DSR handshakes, etc.).
3. `Engine::snapshot_window(0)` → `FullSnapshot::from_window` (an additive R5
   constructor so the host wraps its already-captured window rather than calling
   `snapshot_window` twice).
4. `RenderEngine::update_frame` → `sync_atlas` → **`draw_and_present`** (the new
   additive method): draws exactly R4's `draw_frame` GPU body, then assigns the
   drawn target's IOSurface to the view's `IOSurfaceLayer` (sync mode:
   `waitUntilCompleted` before attach, on the main thread — no dispatch, no size
   guard, no jank).

### Resize

`Tab::reflow` reads the view's device-pixel bounds (`bounds × backingScaleFactor`),
maps to `(cols, rows)` via `geometry::grid_size` (pure floor-division, one-cell
floor), and resizes the vt engine + PTY when the grid changes. The render engine
rebuilds its target automatically inside `draw_frame`/`draw_and_present` when the
snapshot's grid size changes (R4 `update_frame` resizes `Contents`, which drives
the target size). `contentsScale` is read from the window; a font grid is built
at `font_size × scale` device pixels so glyphs rasterize at native resolution.

### Native tabs

Each tab is a real `NSWindow` with `tabbingMode = .preferred`. New tabs
(`new_tab_in`) are added to the parent window's tab group via
`addTabbedWindow:ordered:`, so macOS groups them into one window's tab bar; a tab
dragged out becomes its own window (native behavior, free). Each window hosts its
own `TerminalView` → its own engine + PTY, per the R5 deliverable.

### Working-directory inheritance

The vt engine tracks OSC 7 pwd (`Terminal::get_pwd`, a `file://host/path` URL).
`Engine::pwd` extracts the local path (`pwd_path_from_osc7`, minimal `%20`
decoding). `new_tab_in` reads the active tab's pwd and, when it names an existing
directory (`tabs::inherit_pwd`), spawns the new tab's shell there
(`PtySession::spawn_in_dir` → `CommandBuilder::cwd`).

### Menu

`menu.rs` is the platform-independent action model: `MenuAction` (New Window /
New Tab / Close Tab / Copy / Paste / Font Size ±/Reset / Quit), each with a title,
a Cmd-key equivalent, a top-menu grouping (App / Shell / Edit / View), and a
stable tag. `build_menu` (`app.rs`) constructs the four `NSMenu`s from
`MenuAction::ALL`, wiring each item's Cmd equivalent + tag + `ghosttyMenuAction:`
target. `MenuAction::for_key` resolves a Cmd-key press (incl. the `+`/`=` synonym)
to an action for `performKeyEquivalent` routing. This single definition is what
both the menu and the key-equivalent path dispatch through, and it is fully
unit-tested.

## Verification (no GUI required)

1. **Pure-logic unit tests** (49, all off the main thread / no AppKit): config
   parse, engine (incl. OSC 7 pwd extraction), font-size clamp/step/reset,
   grid geometry, keymap, key translate (plain/ctrl/cmd-swallow/option-as-alt),
   preedit state machine, mouse encode, menu action round-trips + grouping, tab
   registry + pwd inheritance.
2. **`--offscreen-smoke`** (`smoke.rs`): spawns a real PTY + shell, drives a
   scripted `printf` marker through it, feeds the output into a real `ghostty-vt`
   engine, renders through the R4 cell engine into an IOSurface, reads the pixels
   back, and asserts real glyph coverage over the default background (and that
   the readback size matches the geometry math). Exits 0 on success, 0 on a
   graceful no-Metal skip, non-zero on failure. **Runs green on this machine.**
3. **Windowed auto-exit**: `GHOSTTY_APP_SMOKE_MS=<ms>` launches the real
   `NSApplication`, opens a window+tab (spawning a PTY+shell, building the Metal
   renderer, running the pace loop, constructing the menu), and cleanly
   terminates after `<ms>`. **Runs green (exit 0) repeatedly**, proving
   startup/teardown of the whole window path.

### Windowed synthetic-input smoke (needs a GUI session)

`GHOSTTY_APP_SMOKE_TYPE="echo <marker>\n"` launches the real window and, after
the shell draws its prompt, delivers **synthetic `NSEvent` keystrokes through
the AppKit responder chain** (`app.sendEvent`) — exercising the full
frontmost/key → `keyDown:` → `NSTextInputClient`/encode → PTY → engine → screen
round-trip in-process (no accessibility permissions needed for the app's own
events). It then asserts the marker appears in the engine's screen text and
exits `0`/`1`. This is the regression guard for **"the window renders and tabs
show, but typing is dead"**: that symptom is an app-activation failure — a
terminal-launched build that never becomes frontmost has no key window, so
hardware `keyDown:` never fires. The fix is `activateIgnoringOtherApps(true)`
alongside the cooperative `activate()` in `applicationDidFinishLaunching`.

Wired as an `#[ignore]`d cargo test (needs a windowserver session):

```sh
cargo test -p ghostty-app --test typing_smoke -- --ignored --nocapture
```

Or run the binary directly:

```sh
GHOSTTY_APP_SMOKE_TYPE='echo zz-marker\n' cargo run -p ghostty-app -- --window
```

### Manual test steps (needs a human at a GUI session)

```sh
cargo run -p ghostty-app --bin ghostty-app          # or: --window
```

Then try:

- **Typing / a shell**: run `ls`, `vim`, `htop` — text renders via Metal, theme
  colors show, the cursor draws.
- **Theme**: with `theme = "Aardvark Ink"` (or any installed ghostty theme) in
  `~/.config/ghostty-rs/config.toml`, the window opens with that theme's
  background/foreground/palette instead of the built-in default.
- **Selection + copy**: click-drag over text to select it (highlighted via
  inverse video, or the theme's `selection-background`/`selection-foreground`
  if it sets them); `Cmd-C` copies it; a plain click elsewhere clears the
  selection. With `copy-on-select = true`, releasing the drag copies
  immediately without `Cmd-C`. In `vim`/`htop` (mouse reporting on), hold
  Shift while dragging to select instead of sending the drag to the program.
- **Native tabs**: `Cmd-T` opens a new tab (in the current tab's working
  directory — `cd /tmp` then `Cmd-T` and check `pwd`). Drag a tab out of the tab
  bar → it becomes its own window. `Cmd-W` closes the tab/window; `Cmd-N` opens a
  new window.
- **Menu**: the App / Shell / Edit / View menus and their Cmd equivalents
  (New Window/Tab, Close, Copy/Paste, Font Size ±/0, Quit).
- **Font size**: `Cmd-+` / `Cmd--` / `Cmd-0` re-rasterize the grid.
- **Paste**: `Cmd-V` pastes (bracketed if the program enabled bracketed paste).
- **Mouse reporting**: in `vim`/`htop`, clicks and scroll are reported to the
  program.
- **IME / dead keys**: option-e then e → é; a CJK input source composes inline
  (preedit is stored; inline preedit *rendering* is deferred — see below).

## Deferrals (documented, not blocking)

- **Selection rendering has no dedicated renderer surface.** `ghostty-vt`'s
  `Screen::select`/`selection_string` (the engine-side selection model) is
  wired end to end — left-button drag creates a selection, Cmd-C and
  `copy-on-select` copy `selection_string` to the clipboard, a plain click
  clears it — but neither `RenderSnapshot` nor `ghostty-renderer`'s cell
  engine carry any selection state (no selection colors in `FrameOptions`, no
  selection branch in `Contents::rebuild_row`). Rather than extend those two
  additive-only crates for a single consumer, `crate::selection::tint_selection`
  overlays the selection CPU-side on the app's own `SnapshotWindow` (swapping
  each selected cell's fg/bg `SnapshotColor`s) before wrapping it in a
  `FullSnapshot`. Shift-click-to-extend and rectangle selection are not wired
  (the engine supports both; only the mouse-drag gesture that would drive them
  isn't built) — bonus items, not required for the brief.
- **Inline preedit rendering**: the preedit state machine is wired and stored;
  drawing the marked text over the cursor needs a `RenderSnapshot::preedit`
  producer (none exists yet) and IME-box geometry — a follow-on.
- **Theme-file → colors**: now wired (`crate::theme`, a copy of the spike's
  `theme_file.rs` parser — the spike's module is `pub(crate)` in a different
  crate, so it can't be reused via a path dep without modifying read-only spike
  material; flagged for a later dedup). `config.theme` resolves via
  `~/.config/ghostty/themes/` then `$GHOSTTY_RS_THEMES_DIR`/a hardcoded shared
  themes dir, seeding the engine's startup palette + default fg/bg/cursor
  (`Engine::with_colors`) and the selection tint's colors. OSC 4/10/11 dynamic
  colors and the renderer's built-in default theme remain the fallback when no
  theme is configured or it fails to load.
- **CVDisplayLink**: pacing is the timer-first path (plan decision 3). The
  `NSTimer` tick has the same "tick a draw" shape CVDisplayLink swaps into later.
- **Full legacy key encoder**: CLOSED 2026-07-08 — chunk M2-J landed the full
  legacy encoder underneath the unchanged seam. R5 calls the existing
  `key_encode::encode` seam, which improves underneath it; under kitty-protocol
  apps (the common negotiated default) input is fully correct today.
- **Damage tracking**: full redraw every frame (plan decision 4).

## What surprised me about AppKit hosting

- **No mutex needed.** The plan sketched an engine-behind-a-mutex with a
  background render thread. Because AppKit owns the run loop and Metal +
  CoreAnimation are main-thread-bound, the clean shape is: engine main-thread-owned,
  an `NSTimer` pace tick, and only the PTY reader off-thread (behind a channel).
  The mutex would have added contention for zero benefit.
- **The renderer's `IOSurfaceLayer` is already the whole presentation story.**
  R2 built the `CALayer` subclass with the `display`/`actionForKey:` overrides
  and the main-thread `set_surface` guard; R5's presentation is a one-liner
  (`set_surface_sync` after a completed sync frame). The additive `draw_and_present`
  is R4's `draw_frame` body plus that one attach.
- **`FullSnapshot` needed a `from_window` seam.** `FullSnapshot::capture` takes a
  `&Terminal`, but a host holding its engine wrapper wants to snapshot once and
  wrap — so a small additive constructor avoided a double `snapshot_window`.
- **objc2 0.6 safety annotations are uneven.** Some AppKit setters
  (`setTitle`, `makeKeyAndOrderFront`, `close`, `setLayer`) are safe;
  neighbors (`interpretKeyEvents:`, `newTextureWithDescriptor:iosurface:plane:`)
  are `unsafe`. Clippy's `unused_unsafe` is the reliable guide to which is which.
