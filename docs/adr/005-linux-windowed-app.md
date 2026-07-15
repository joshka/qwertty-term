# ADR 005: Linux windowed app (P4) — sequencing, toolkit, and PR slices

**Status:** PROPOSED (Josh greenlit P4 on 2026-07-15; this records how it is sliced and
surfaces the one decision that is Josh's — the windowing toolkit).

Companion to `docs/adr/003-linux-strategy.md` (ACCEPTED — headless-first; P4 deferred behind a
separate greenlight, now given) and the P4 section of `docs/plans/linux.md`. Upstream reference
at pin `2da015cd6`, cited `file:line`.

## Context

Wave 1 (P1–P3) is complete and merged: `Engine<Software>` renders terminal frames headless on
Linux over the FreeType + fontconfig font stack, validated end-to-end (CI + a local Docker run
over native FreeType, incl. visual PNG confirmation). P4 is the **windowed** Linux terminal —
the analog of the macOS AppKit app crate. Upstream's Linux app is **GTK4 + libadwaita** (`src/
apprt/gtk`, 23,760 LoC — a full GObject-subclass design; `class/` alone is ~16.6k) plus an
**OpenGL 4.3** renderer (`OpenGL.zig` 461 + `opengl/*` + 10 GLSL files, ~2,327 LoC) and ~2.7k
of OS glue (Wayland/X11 winproto, XDG portals, notifications, DBus single-instance, systemd
cgroups, flatpak).

Two facts shape the slicing:

1. **The on-screen renderer is toolkit-independent.** Whatever hosts the window (a GTK `GLArea`
   or a `winit` surface), it needs an **OpenGL `GpuBackend`** driving the existing generic
   `Engine<B>` (P1). So the OpenGL backend is the first slice and does **not** depend on the
   toolkit decision below.
2. **Embeddability is unaffected by the app toolkit.** qwertty-term's co-equal embeddability
   goal (betamax) is served by the *library* crates (`qwertty-term-vt`/`-renderer`/`-font`),
   which embedders consume directly and which stay platform-free. The windowed app is the
   *standalone* terminal; its toolkit choice does not constrain embedders.

## Decision

**Sequence P4 as OpenGL-first, then the app shell, then OS glue** — each slice independently
gated, each ported with upstream inline tests where an oracle exists, each keeping the macOS
build and the platform-free tripwire (vt = zero `target_os`) green.

### Slice order

1. **OpenGL `GpuBackend` (`renderer`, ~2.3k)** — port `OpenGL.zig` + `opengl/*` + the GLSL
   shaders as a second `GpuBackend` impl behind the P1 `Engine<B>` seam. GL 4.3 core, loader
   via the `glow` crate (safe-ish GL wrapper) or raw `gl`; swap-chain = 1; always-sync
   (`OpenGL.zig:36-38,141-149`). **GLSL kept verbatim** from upstream (`shaders/glsl/*`) — the
   frozen wire structs (T2's invariant) feed the same uniforms/vertices, the GPU just
   interprets them via GL instead of Metal. **Toolkit-independent; start here.**
   - Evidence: an **offscreen GL readback** test (EGL surfaceless / pbuffer, headless) that
     reproduces the same cell grid the Software + Metal backends produce for a known input —
     the differential parity the ADR-003 methodology uses. Runs in CI/Docker under Mesa
     software GL (`LIBGL_ALWAYS_SOFTWARE=1` / `llvmpipe`), **no display server, no GPU**.
2. **App shell (toolkit TBD — see Open Question, ~24k or ~2k analog)** — window / event loop /
   input / IME / clipboard / config, reusing the **platform-free** splits/tabs/search/keybind/
   theme logic that already exists in the app crate. **Additive**: a new crate (or a
   `cfg(target_os="linux")` apprt module), **never** edits to the macOS AppKit code.
3. **Winproto + OS glue (~2.7k)** — Wayland/X11 specifics (blur, CSD/SSD, quick-terminal via
   layer-shell), XDG portals (OpenURI, GlobalShortcuts), desktop notifications, DBus
   single-instance, systemd cgroup scopes, flatpak. Each independently gateable and mostly
   only relevant once slice 2 runs a real window.

### Open Question — the windowing toolkit (Josh's call; recommendation below)

This is the one genuine decision P4 forces, and it only gates **slice 2+** (slice 1 proceeds
regardless). Two viable paths:

- **(A) GTK4 + libadwaita via `gtk4-rs`** — mirror upstream `class/*`. **Pros:** feature parity
  (native tabs/headerbar/CSD, libadwaita theming, the ~10 `gtk-*` config keys actually mean
  something, XDG portal integration is idiomatic), and the differential-port methodology carries
  over (port `class/*` GObject-by-GObject). Hosts GL via `GtkGLArea` (main-thread draw,
  `must_draw_from_app_thread`). **Cons:** ~24k-LoC analog, GObject/Blueprint-shaped, a heavy
  dependency tree, no differential *oracle* (UI isn't diff-testable), slow.
- **(B) `winit` + `glutin`/`glow`** — a minimal portable window + GL context, no GTK. **Pros:**
  ~10× smaller, no GObject, fast to a first runnable window, closer in spirit to the macOS app
  (which is native, not a cross-platform toolkit), trivially headless-testable. **Cons:** loses
  native GTK chrome / libadwaita / tabs-in-headerbar / the `gtk-*` keys / idiomatic portal &
  quick-terminal integration — we'd rebuild a bare shell and those features become non-goals or
  bespoke.

**Recommendation: (A) GTK4-rs**, because the app is the *standalone* terminal where Linux users
expect native integration, embeddability is already covered by the library crates (so B's
"leaner/more embeddable" edge does not apply to the app), and mirroring upstream preserves the
port methodology and feature parity that has driven every other chunk. **(B) is a legitimate
"lean standalone" alternative** if parity with upstream's GTK feature set is explicitly *not* a
goal for the Linux app — that is the trade Josh should confirm. Until confirmed, slice 1 ships
and slice 2 is scoped-not-started.

### What we port faithfully vs defer

- **Port:** OpenGL backend + GLSL verbatim; the GObject window/surface/app lifecycle (path A);
  GtkIMMulticontext IME flow (`class/surface.zig:1275-1449`); clipboard; the event/keymap
  translation into the existing platform-free input encoder.
- **Reuse (already platform-free):** splits, tabs, search, keybind resolution, theme/color,
  the `RenderSnapshot` contract, the cell `Engine<B>`.
- **Defer (independently gateable, behind their own follow-ups):** background blur, custom
  shaders, quick-terminal/layer-shell, GlobalShortcuts portal, flatpak packaging, systemd
  cgroup scopes, tmux control-mode UI (ADR 004 is the engine). SVG emoji stays out (upstream
  also skips it).

## CI implications

- **Slice 1 (OpenGL) needs only a headless GL context** — Mesa `llvmpipe` (software GL) under
  EGL surfaceless, no display server, no GPU. This runs on the free Linux runner and in the
  local Docker harness already used for the P1–P3 validation. So slice 1 keeps the "prove it in
  a container, no human needed" property.
- **Slice 2+ (a real window)** needs a display server for interactive tests — headless Wayland
  (`weston --backend=headless`) or `Xvfb` — and is where **human visual testing** finally
  enters. The pixel-correctness of what's drawn is still coverable headlessly (offscreen GL
  readback from slice 1); only genuine windowing/input/IME behavior needs a display.
- Coordinate all CI with **T8** (owns `.github/workflows`).

## Resolution (Josh confirmed 2026-07-15)

- **Toolkit = (A) GTK4 + libadwaita via `gtk4-rs`.** winit was rejected — it has known problems
  in practice, and GTK gives the native parity the standalone Linux terminal wants. The Open
  Question above is settled; slice 2 is unblocked.
- **Prioritize a real, user-testable window first**, then layer everything else on top. The
  target milestone is a GTK4 window hosting the terminal via the OpenGL backend that a user can
  launch, type into, and see render — the first thing worth a human's eyes.
- **Don't over-defer: port mechanically-easy areas inline.** When an upstream area (e.g.
  background blur, custom shaders, quick-terminal/layer-shell) is a straightforward mechanical
  conversion, port it in place rather than skipping and returning later — that is more efficient
  than re-establishing context on a second pass. Only genuinely hard/uncertain areas get
  deferred behind their own gate.
- **Execute with subagents, in parallel where safe.** Independent areas (OpenGL backend vs GTK
  shell scaffolding vs winproto/OS-glue) can progress concurrently; the orchestrator sequences
  the integration. **Defer the *merge*** of any slice that would block the user-visible window
  from landing — parallel work continues, but the window milestone ships first.

## Consequences

- The renderer gains a second `GpuBackend` (`OpenGL`) alongside `Metal`/`Software`, exercising
  the P1 generalization for real — validating that `Engine<B>` was the right seam.
- P4 remains **strictly additive** to the macOS app; the AppKit code and the vt tripwire are
  untouched.
- The GTK-vs-winit decision is deferred to a Josh confirmation but does **not** block slice 1;
  if Josh picks (B), only slice 2's shape changes, not slice 1 or the overall sequencing.
- This is the largest and slowest Linux milestone; expect it to span many sessions, integration-
  tested rather than diff-proven for the UI, with the OpenGL backend being the one diff-provable
  piece.
