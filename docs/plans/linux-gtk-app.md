# Plan: Linux GTK4 app (P4 slice 2) — a user-testable, typeable, rendering window

Implementation plan for **P4 slice 2**: a GTK4 + libadwaita window that hosts the terminal via
the OpenGL backend, that a user can launch, type into, and see render. Companion to
`docs/adr/005-linux-windowed-app.md` (**toolkit = GTK4 + libadwaita via `gtk4-rs`**, ACCEPTED by
Josh 2026-07-15) and the P4 section of `docs/plans/linux.md`. Slice 1 (the OpenGL `GpuBackend`,
headless/offscreen) is being built in parallel by a separate agent; this plan consumes it.

Upstream reference: `/Users/joshka/local/ghostty` at pin `2da015cd6` (`file:line` cited
throughout; a couple of upstream reads were against descendant `38e49a2`, an confirmed-ancestor
of the pin — line numbers are within a few lines).

The guiding priority from ADR 005: **the typeable window lands first**, then layers. And
**do not over-defer** — port mechanically-easy areas (mouse, scroll, selection, basic clipboard)
inline rather than skipping and returning.

## 1. What we reuse (platform-free) vs what is new

The single most important reuse fact: **the pty driver, the VT engine, the snapshot contract,
the cell `Engine<B>`, and all input encoding are already platform-free.** The GTK crate wires
these together; it must not reimplement them. Upstream's ~24k-LoC GTK apprt shrinks dramatically
for us because the equivalent of upstream's `core Surface`/`termio`/`renderer` threads already
exist as our reusable crates.

### Reuse directly, unchanged (core crates)

- `qwertty-term-termio` — the pty/termio driver. Platform-free (rustix + POSIX; only
  `cfg!(target_os = "macos")` *runtime* branches, all with Linux fallbacks). Entry points:
  `Termio::spawn` (`crates/qwertty-term-termio/src/hub.rs:339`), `Pty::open`
  (`crates/qwertty-term-termio/src/pty.rs:162`), the cloneable `Writer`
  (`hub.rs:275-317`: `write`/`resize`/`focus`), and the read-loop seam
  `Sink = Box<dyn FnMut(&[u8]) + Send>` (`crates/qwertty-term-termio/src/exec.rs:678`) — the
  closure that feeds raw pty bytes into the VT. This is the key seam the GTK shell supplies.
- `qwertty-term-vt` — the `Terminal` (`crates/qwertty-term-vt/src/terminal/mod.rs:322`), a state
  machine fed via `Stream<TerminalHandler>::feed` (`stream.rs:689`), producing `SnapshotWindow` via
  `Terminal::snapshot_window[_tracking]`. Zero `target_os`. The tripwire.
- `qwertty-term-input` — key/mouse/paste encoding, deliberately freestanding
  (`crates/qwertty-term-input/src/lib.rs:1-28`). `key_encode::encode(&KeyEvent, &Options)`
  (`key_encode.rs:136`), `mouse_encode::encode` (`mouse_encode.rs:148`), `paste::encode`
  (`paste.rs:62`), the `binding::Set` keybind system (`src/binding/`). The GTK shell builds
  `KeyEvent` (`key.rs:24`) from GDK keyvals via `Key::from_name`/`from_w3c` (`key.rs:871-875`)
  and **does not** use the macOS `keymap` module.
- `qwertty-term-renderer` — the generic cell `Engine<B: GpuBackend>`
  (`crates/qwertty-term-renderer/src/engine.rs:158`), the `RenderSnapshot` trait +
  `FullSnapshot` (`src/snapshot.rs:199-380`), and `Software`/`Metal`/`OpenGL` backends. The GTK
  shell drives `Engine<OpenGL>`.
- `qwertty-term-font` — FreeType raster + fontconfig discovery (P2, already landed) gives the
  `Grid` the GTK shell passes to `update_frame`.

### Reuse the platform-free logic modules in `crates/qwertty-term`

`crates/qwertty-term/src/lib.rs:13-39` declares these **without** a cfg gate; objc2 is pulled in
only under `[target.'cfg(target_os = "macos")'.dependencies]`
(`crates/qwertty-term/Cargo.toml:44`). So the `qwertty_term` *library* is expected to compile on
Linux exposing:

- `engine` (`src/engine.rs`) — the VT wrapper (feed/snapshot/resize/replies) the app layer uses.
- `theme` (`src/theme.rs`) — Ghostty theme-file parse (palette, fg/bg). Zero objc.
- `config` (`src/config.rs`) — TOML config; one `#[cfg(target_os="macos")]` path branch at
  `:1248`, else portable.
- `keybind` (`src/keybind.rs`), `selection`, `scroll`, `geometry`, `font_size`, `progress`,
  `bell`, `notify`, `session`, `menu` — AppKit appears only in doc comments.
- `splits`/`tabs`/`search`/`searchkeys`/`splitkeys`/`tabkeys` — "pure, AppKit-free logic".
- `termio` (`src/termio.rs`, `#[cfg(unix)]`) — `TabIo` owns the `Termio` hub + `Writer` and
  builds the `Sink` that feeds `Arc<Mutex<Engine>>`; **reusable essentially as-is** (names no
  AppKit type). The threading model is documented at `termio.rs:9-36`.

**Decision point (verify early, PR-B):** run `cargo check -p qwertty-term` on Linux. If the lib
builds cleanly, `qwertty-term-gtk` depends on `qwertty-term` and imports these modules directly.
If some module transitively drags in a macOS-gated symbol, extract the platform-free set into a
new `qwertty-term-app-core` crate that both the macOS app and the GTK app depend on. Prefer the
direct-dependency path first (no premature extraction, per ADR 005 "don't over-defer").

### New (the AppKit-gated set, to be written for GTK)

`crates/qwertty-term/src/lib.rs:47-60` (macOS-gated) has no reusable analog — these are what the
GTK crate replaces:

- `app` (`src/app.rs`) — the controller + the `CADisplayLink`-driven render/update loop
  (`app.rs:4305-4390`) + the `Surface`/window abstraction (`app.rs:245`,
  `Surface::render` `:675`, frame build `app.rs:711-719`).
- `view` (`src/view.rs`) — the NSView/Metal surface host.
- `clipboard`, `search_overlay`, `splitview`, `smoke`, and `input/keymap.rs` + `NSEvent` field
  extraction (keep the `input/translate.rs` *shape*).

The offscreen, platform-free `smoke.rs:95-101` (`FullSnapshot::from_window` → `update_frame` →
`draw_frame`) is the reference for how to wire the GTK render callback.

## 2. The OpenGL backend seam — THE key integration risk

**Finding: our on-screen present path is currently macOS/Metal-only, and generalizing it is the
one real integration risk for slice 2. It must be settled with T2 before slice-2 rendering code.**

How a frame is drawn today (`crates/qwertty-term-renderer/src/engine.rs:1400` `draw_frame`) is
**fully generic over `B: GpuBackend`**: it gathers uniforms/bg-cells/fg-lists, acquires a
swap-chain slot, syncs buffers, opens a render pass against `slot.target` (a `B::Target`), encodes
bg → cell-bg → text (+ kitty image) steps, completes the frame, and ends with
`slot.target.read_pixels()` (`engine.rs:1549`). That trailing readback is the **offscreen** path —
exactly what slice 1's GL readback test wants.

An **on-screen** frame differs from `draw_frame` in exactly one place: instead of reading pixels
back, it must put the drawn target on screen. The three backends present differently:

- Metal (`crates/qwertty-term-renderer/src/present.rs`, `#![cfg(target_os = "macos")]`,
  `Engine<Metal>::draw_and_present`): assigns the target's IOSurface to a `CALayer`'s `contents`
  (`present.rs:202` `layer.set_surface_sync`).
- OpenGL (upstream `src/renderer/OpenGL.zig:299-333` `present`): **blits the target's FBO to the
  default framebuffer** (`glBlitFramebuffer`, FBO 0 is what `GtkGLArea` binds), with
  `GL_FRAMEBUFFER_SRGB` disabled during the blit (`OpenGL.zig:307-317`).
- Software: no-op (nothing to present).

The blit target — the GLArea's default framebuffer — is only bound and current **inside the
GLArea `render` signal on the GTK main thread** (upstream `glareaRender`
`src/apprt/gtk/class/surface.zig:3347-3363` calls `renderer.drawFrame(true)` there; GTK cannot
draw GL from another thread — `must_draw_from_app_thread = true`,
`src/apprt/gtk/App.zig:20-23`). So the GL on-screen present must run in our render callback.

The blocker: `present.rs`'s Metal present relies on `pub(crate)` accessors that are
`#[cfg(target_os = "macos")] impl Engine<Metal>` only (`engine.rs:1649-1735`): `present_parts()`,
`uniforms_snapshot()`, `bg_cells_snapshot()`, `fg_count()`, `fg_lists_snapshot()`,
`screen_width()`/`screen_height()`. An `Engine<OpenGL>` on-screen present cannot reuse them as-is.

**Recommended resolution (upstream-faithful, smallest surface): add a generic present seam to the
trait.** Add `fn present(&self, target: &Self::Target) -> Result<(), Self::Error>` to
`GpuBackend` (`crates/qwertty-term-renderer/src/gpu.rs`): Metal blits/assigns IOSurface, OpenGL
does the `glBlitFramebuffer`-to-FBO-0, Software is a no-op. Then add a thin generic
`Engine::<B>::draw_and_present()` that runs the *same body* as `draw_frame` but calls
`backend.present(&slot.target)` in place of `read_pixels()`. This mirrors upstream `generic.zig`
calling `api.present(target)` and keeps the encode logic single-sourced. Alternative (if T2
prefers no trait change): un-gate the six present helpers to be generic and add a parallel
`present_gl.rs` with `impl Engine<OpenGL>::draw_and_present` — smaller trait, but duplicates
encoding.

**T2 coordination is mandatory here.** `gpu.rs`, `engine.rs`, and `present.rs` are T2 core
(file-claim). This present-seam generalization (call it **PR-A**) is a prerequisite of the
on-screen window and should be agreed with T2 before slice-2 rendering code is written. It is a
small, contained change — the draw body is already generic; only the final step moves behind a
trait method.

Secondary risks to coordinate with the slice-1 OpenGL backend author (all cheap if flagged now):

- The GL `B::Target` must be an FBO+renderbuffer (upstream `src/renderer/opengl/Target.zig:44-50`)
  whose FBO handle stays reachable for the present blit — not only `glReadPixels` for the
  offscreen test. Ensure the GL `Target` exposes what `present` needs.
- sRGB: our Metal targets are `bgra8unorm_srgb`; the GL present must disable `GL_FRAMEBUFFER_SRGB`
  during the blit to avoid double-linearization (upstream `OpenGL.zig:302-317`).
- GLArea config: `has-stencil-buffer=false`, `has-depth-buffer=false`, `allowed-apis=gl`
  (upstream `ui/1.2/surface.blp:34-36`). GL 4.3 core context.
- Swap chain: OpenGL `swap_chain_count = 1`, always-sync (upstream `OpenGL.zig:32`); our
  `SwapChain` already supports `SwapChainMode::Sync` (one live slot) —
  `crates/qwertty-term-renderer/src/swap_chain.rs`.

## 3. Minimal upstream GTK path to a rendering window

Upstream runs three threads per surface (main/app, renderer, io). **We do not need to mirror that
threading**: our `TabIo`/`Termio` already own the pty read/write threads, and the VT+Engine live
behind an `Arc<Mutex<>>`. The GTK shell only needs the GTK main thread plus termio's threads, and
a main-thread "redraw now" bounce. The minimal lifecycle to extract:

### Application

- `adw::Application` subclass; `run` is a hand-rolled loop upstream
  (`src/apprt/gtk/class/application.zig:477`, loop at `:548-552`: iterate GTK context, tick core).
  In `gtk4-rs` we use the stock `Application::run`.
- `activate` (`application.zig:1459-1473`) → request one window.
- `newWindow`/`initAndShowWindow` (`application.zig:2252-2329`): build the window, create the
  first surface (`:2309-2313`), `present` it (`:2328`).
- `wakeup` (`application.zig:1286-1288`) `glib.MainContext.wakeup(null)` — how a background
  thread kicks the GTK loop to service a redraw. Our analog: a `glib` channel / `idle_add` from
  the termio read `Sink` to the main thread.

### Window

- `adw::ApplicationWindow` (`window.zig:38`), `new`/`init` (`window.zig:272-352`). For the minimal
  window, set a **single `Surface` widget as the window child** — bypass `AdwTabView`/`Tab`/
  `SplitTree` (`window.zig:265,393-502`), which are the tabs/splits LATER layer.

### Surface + GLArea (the critical widget)

- `GtkGLArea` (upstream builds it in the Blueprint `ui/1.2/surface.blp:23-37`; we create it in
  Rust and connect the same signals). Field/bind: `surface.zig:623,3627`.
- `realize` (`glareaRealize` `surface.zig:3247-3282`): `gl_area.make_current()`, check
  `gl_area.error()`, init the GL context / renderer.
- `render` (`glareaRender` `surface.zig:3347-3363`): the frame draw, on the main thread, into the
  bound default FBO. Our callback calls `Engine::<OpenGL>::draw_and_present` (PR-A).
- `resize` (`glareaResize` `surface.zig:3365-3423`): store size; **lazily init the surface on the
  first resize** (`surface.zig:3419-3422,3430-3508`) so the terminal gets correct initial
  dimensions — replicate this ordering. On later resizes, resize the engine target + pty winsize.
- `redraw` (`surface.zig:818-825`): `gl_area.queue_render()` — the only thing that schedules a
  frame. Our redraw bounce: termio `Sink` (termio thread) feeds the VT, then signals the main
  thread (glib channel) which calls `queue_render()`. Upstream does the equivalent via its
  `.redraw_surface` mailbox bounce (`renderer/Thread.zig:500-517` → core `App.zig:252,277-289` →
  `application.zig:2481-2486` → `surface.redraw()`).

### Input (keyboard = minimal; mouse/scroll = next, easy)

- `GtkEventControllerKey` (`surface.zig:2714-2744`) → `keyEvent` (`surface.zig:1240-1463`) →
  `surface.keyCallback(...)` (`surface.zig:1428-1439`). Our path: map GDK keyval/keycode/state →
  `qwertty_term_input::KeyEvent` → `key_encode::encode` → `TabIo::write` (pty).
- **IME is woven into `keyEvent` even for ASCII**: upstream routes every key through
  `im_context.filterKeypress` (`surface.zig:1303`) and commits characters via the IM commit
  signal. For the *first* typeable window we bypass the IM gate and encode directly from the
  keyval (loses dead-keys/CJK); the `GtkIMMulticontext` flow (`surface.zig:1253-1343,3661-3664`)
  is its own early-but-gated step (PR-G).
- Mouse/scroll/focus: `GestureClick`/`EventControllerMotion`/`EventControllerScroll`
  (`surface.zig:2785-3050`), focus (`surface.zig:2746-2783`). Feed `mouse_encode`/`scroll`. Easy,
  port inline (PR-E), not deferred.

### The pty → vt → redraw loop (our shape)

1. `TabIo::spawn` builds the read `Sink`: on the termio read thread, lock `Arc<Mutex<Engine>>`,
   `Engine::write(bytes)` (feeds `Stream<TerminalHandler>`), then signal the main thread.
2. Main thread (glib channel handler) calls `gl_area.queue_render()`.
3. GTK invokes the `render` callback → `FullSnapshot::capture_tracking(&mut terminal)` →
   `Engine::update_frame(&snapshot, &mut grid, opts)` → `Engine::sync_atlas(&grid)` →
   `Engine::<OpenGL>::draw_and_present()` (blit to FBO 0).
4. Keystrokes: GTK key controller, encode, `TabIo::write` to pty; the child's echo returns via
   step 1.

## 4. Crate shape

**New crate `crates/qwertty-term-gtk`** (a Linux-gated `lib` + `bin`), a sibling to
`crates/qwertty-term`. Rationale:

- The macOS app crate stays **untouched** (ADR 005: strictly additive). A new crate is the
  cleanest additive boundary and matches how `qwertty-term-termio`/`-renderer` are separate crates.
- It depends on the core crates directly (`qwertty-term-vt`, `-input`, `-termio`, `-renderer`,
  `-font`) and on `qwertty-term` for the platform-free logic modules (theme/config/keybind/…),
  pending the `cargo check -p qwertty-term` verification in §1 (fall back to extracting
  `qwertty-term-app-core` only if needed).
- It plugs into the workspace via `members = ["crates/*"]` (`Cargo.toml:3`) automatically — no
  edit to the macOS app crate, no change to the default GL/vt tripwires.

Manifest sketch (`crates/qwertty-term-gtk/Cargo.toml`), all GTK deps Linux-gated so the workspace
still builds on macOS:

```toml
[package]
name = "qwertty-term-gtk"
version.workspace = true
edition.workspace = true

[dependencies]
qwertty-term = { version = "0.3.0", path = "../qwertty-term" }
qwertty-term-vt = { version = "0.3.0", path = "../qwertty-term-vt" }
qwertty-term-input = { version = "0.3.0", path = "../qwertty-term-input" }
qwertty-term-termio = { version = "0.3.0", path = "../qwertty-term-termio" }
qwertty-term-renderer = { version = "0.3.0", path = "../qwertty-term-renderer" }
qwertty-term-font = { version = "0.3.0", path = "../qwertty-term-font" }

[target.'cfg(target_os = "linux")'.dependencies]
gtk4 = { version = "0.9", package = "gtk4", features = ["v4_12"] }  # imported as `gtk`
libadwaita = { version = "0.7", features = ["v1_5"] }               # imported as `adw`
glow = "0.16"        # GL calls; already in Cargo.lock via the egui spike
epoxy = "0.1"        # GLArea GL function loader (libepoxy)
libloading = "0.8"   # dlopen libepoxy for epoxy::get_proc_addr
```

Notes on deps:

- `gtk4-rs` re-exports `gdk4`/`glib`/`gio`/`cairo`/`pango` transitively; pull `gdk4` explicitly
  only if a type is needed directly. Pin the `v4_x`/`v1_x` feature to the minimum libadwaita the
  Blueprint dir names imply (upstream `ui/1.5` → libadwaita 1.5; start conservative).
- GL loading is the standard `gtk4-rs` GLArea+glow pattern: `epoxy::load_with(|s| unsafe {
  library.get(s) })` on a `libloading`-opened `libepoxy.so`, then
  `glow::Context::from_loader_function(|s| epoxy::get_proc_addr(s))` inside `realize` after
  `gl_area.make_current()`. `glow`, `glutin`, `khronos-egl` are **already in `Cargo.lock`** (via
  the egui spike), so no new fetch for the GL layer.
- System libraries required to *build/run* (Docker/CI, coordinate with T8):
  `libgtk-4-dev libadwaita-1-dev libepoxy-dev` + Mesa (`libgl1-mesa-dri`).
- **No new deps in `crates/qwertty-term`.**

## 5. Validation strategy (CI/Docker, no human)

Slice 1's offscreen GL readback (separate agent) already runs headless under Mesa `llvmpipe` +
EGL surfaceless (`LIBGL_ALWAYS_SOFTWARE=1`, no display) — that is where **pixel-correctness** is
proven differentially against the Software/Metal backends. Slice 2 does not re-prove pixels; it
proves **windowing/GL-hosting/input plumbing**, which needs a display server.

Two headless display options (pick one for CI; both work under Mesa software GL):

- **Xvfb + X11** (simplest, most reliable in CI):

```sh
export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe
export GDK_BACKEND=x11
Xvfb :99 -screen 0 1280x800x24 &
export DISPLAY=:99
cargo test -p qwertty-term-gtk
```

- **Headless Wayland (weston)** (closer to the primary target backend):

```sh
export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe
weston --backend=headless-backend.so --socket=wayland-99 &
export WAYLAND_DISPLAY=wayland-99 GDK_BACKEND=wayland
cargo test -p qwertty-term-gtk
```

Assertable headlessly (turn each into a test hook / env-gated smoke):

- Window realizes: the GLArea `realize` signal fires and `gl_area.error()` is `None` (assert a
  flag set in the realize handler).
- A frame renders: from within the `render` callback, `glReadPixels` the presented default FBO
  (or reuse the offscreen readback) and assert non-empty glyph coverage — the analog of the macOS
  `Engine::draw_and_present_readback` smoke (`crates/qwertty-term-renderer/src/present.rs:64`).
- A scripted keypress reaches the pty: the strongest assertion needs no GTK at all — a pure
  integration test drives `TabIo::write(key_encode::encode(...))` and asserts the child echoed
  (VT snapshot shows the text). For the GTK layer specifically, synthesize a key event
  (`gtk::prelude` event emission on the `EventControllerKey`, or invoke our keyval→encode→write
  path directly) and assert pty bytes / snapshot echo.
- No-crash lifecycle: open window, feed `printf 'hello\n'` to the pty, render N frames, close —
  under Xvfb, asserting clean exit.

Needs a human's eyes (cannot be asserted headlessly):

- Real-compositor behavior on Wayland and X11 (CSD/SSD, HiDPI fractional scaling, resize feel).
- IME popups (GtkIMMulticontext with ibus/fcitx), dead-keys, CJK composition.
- Clipboard interaction with the real desktop (primary + clipboard selections).
- Cursor blink cadence, scroll momentum, visual glyph fidelity at the desktop's actual DPI (the
  pixels themselves are already differentially proven headless).

## 6. Slice-2 PR breakdown (ordered; typeable window ASAP)

Ordered so the **user-testable typeable window (PR-D)** lands as early as possible. PRs A–D are
the critical path to that milestone; E onward layer on top. Each keeps the macOS build and the
vt/GL tripwires green.

- **PR-A — generalize the on-screen present seam (T2-coordinated, prerequisite).** Add
  `GpuBackend::present(&self, &Target)` (or un-gate the present helpers) so a generic
  `Engine::draw_and_present` can blit `Engine<OpenGL>`'s target to FBO 0. File-claim `gpu.rs`/
  `engine.rs`/`present.rs`; agree the approach with T2. Depends on slice-1 GL backend existing.
  This is §2, the key risk — do it first, small and contained.
- **PR-B — `qwertty-term-gtk` scaffold + GL-clearing window.** New crate, `gtk4`+`libadwaita`
  deps, `Application`→`activate`→`ApplicationWindow` with a `GtkGLArea` child, epoxy+glow GL
  loader, realize/render/resize wired to an `Engine<OpenGL>` that clears to a solid color (no
  terminal yet). Verify `cargo check -p qwertty-term` on Linux (§1 decision). Milestone: a window
  opens and GL-clears. Headless test: realizes + renders a frame.
- **PR-C — wire the terminal (text renders).** Lazy `initSurface` on first resize spawns `TabIo`
  (termio) + vt `Terminal` + `Engine<OpenGL>`; read `Sink` → glib channel → `queue_render`;
  render callback runs `capture_tracking`, `update_frame`, `sync_atlas`, then `draw_and_present`.
  Reuse `qwertty_term::{theme, config, font_size}` + the FreeType/fontconfig `Grid` (P2).
  Milestone: **the shell prompt renders**. No input yet.
- **PR-D — keyboard input (THE milestone).** `EventControllerKey` → GDK keyval/keycode/mods →
  `qwertty_term_input::KeyEvent` → `key_encode::encode` → `TabIo::write`. Reuse
  `qwertty_term::keybind` for keybind actions. Milestone: **launch, type, see it render and
  echo** — the first thing worth a human's eyes. Ships the ADR-005 target.
- **PR-E — mouse, scroll, selection (port inline, not deferred).** `GestureClick`/
  `EventControllerMotion`/`EventControllerScroll` → `mouse_encode` + `qwertty_term::scroll`;
  selection via `qwertty_term::selection`. Mechanically easy — the encoders exist.
- **PR-F — clipboard (basic, inline).** `gdk::Clipboard` copy/paste with
  `qwertty_term_input::paste::encode` (bracketed paste). Gate the trickier bits (primary-paste,
  paste-safety prompt) as follow-ups.
- **PR-G — IME (gated, hard).** `GtkIMMulticontext` preedit/commit flow
  (`surface.zig:1253-1343,3661-3664`) — woven into key handling even for ASCII; its own step with
  its own testing. Uncertain: input-method quirks (ibus vs fcitx commit ordering).
- **PR-H — chrome / tabs / splits (incremental).** libadwaita headerbar + `AdwTabView`, reusing
  `qwertty_term::{tabs, splits, tabkeys, splitkeys}` logic. Each sub-feature independently
  shippable.
- **PR-I — winproto + OS glue (slice 3, separate).** Wayland/X11 specifics (blur, CSD/SSD,
  quick-terminal/layer-shell), XDG portals, notifications, DBus single-instance, systemd cgroups,
  flatpak. Out of slice 2.

Hard/uncertain parts flagged for their own gated steps: **IME (PR-G)**, **Wayland-vs-X11 backend
differences** (GDK_BACKEND, GL context creation, fractional HiDPI — surface early via the two CI
backends in §5), and **clipboard primary selection (PR-F follow-ups)**.

## Coordination

- **T2 (renderer core):** PR-A touches `gpu.rs`/`engine.rs`/`present.rs` — file-claim and agree
  the present-seam shape before slice-2 render code. Coordinate with the slice-1 OpenGL backend
  author on the GL `Target` FBO exposure + sRGB blit (§2).
- **T8 (CI):** add a Linux GTK CI lane with a display server (Xvfb or weston) + GTK/libadwaita/
  epoxy/Mesa system deps (§5). The offscreen GL readback stays on the existing GPU-less lane.
- **T3 (config):** the `gtk-*` keys, `linux-cgroup*`, `primary-paste`, FreeType flags land as the
  GTK app grows — keep the option parser open to platform-gated keys.
- macOS app crate (`crates/qwertty-term`) stays untouched except (optionally) extracting
  `qwertty-term-app-core` if the Linux `cargo check` in PR-B forces it.
