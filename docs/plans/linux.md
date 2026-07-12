# Plan: Linux port (phased)

Companion to `docs/adr/003-linux-strategy.md` (**ACCEPTED** 2026-07-11). Decisions are MADE
in the ADR; this plan sizes and sequences them. T7 is un-parked; **P1 is the next chunk**,
gated only on settling P1 ownership with T2 (ADR open-question 3). Upstream refs at pin
`2da015cd6`, cited `file:line`. Chunk sizing uses `wc -l` on upstream dirs (surveyed
2026-07-11).

## Upstream LoC sizing (the Linux surface we are choosing what to mirror)

| Upstream area                                                | LoC         | Notes                                                 |
| ------------------------------------------------------------ | ----------- | ----------------------------------------------------- |
| `src/apprt/gtk` (whole tree)                                 | **23,760**  | GTK4+libadwaita apprt; `gtk-ng` already promoted here |
| `class/` (GObject widget classes)                            | 16,646      | ~70% of the apprt                                     |
| `class/surface.zig`                                          | 4,146       | largest single file                                   |
| `class/application.zig`                                      | 2,960       |                                                       |
| `class/window.zig`                                           | 2,205       |                                                       |
| `class/split_tree.zig`                                       | 1,375       | our app already has splits (native)                   |
| Blueprint `.blp` (`ui/1.0`…`ui/1.5`)                         | 1,455       | dir name = min libadwaita version                     |
| `winproto/` (X11+Wayland)                                    | 1,496       | blur, CSD/SSD, quick-terminal, urgency                |
| `portal/OpenURI.zig`                                         | 565         | XDG desktop portal                                    |
| `class/global_shortcuts.zig`                                 | 635         | GlobalShortcuts portal                                |
| `key.zig` + IME in `class/surface.zig`                       | ~535 + ~200 | GtkIMMulticontext flow                                |
| `src/renderer` OpenGL-specific                               | **2,327**   | `OpenGL.zig` 461 + `opengl/*` + 10 GLSL               |
| `src/renderer` shared/generic                                | ~8,145      | `generic.zig` 3,386 etc. — already ported (macOS)     |
| `src/font/face/freetype.zig`                                 | 1,439       | glyph raster, synthetic bold/italic, color bitmap     |
| `src/font/opentype/glyf*.zig`                                | ~1,871      | pure-Zig glyf decoder (COLR/bitmap edge cases)        |
| `src/font` fontconfig discovery (in `discovery.zig`)         | ~225        | `discovery.zig:114-339`                               |
| `src/font/Metrics.zig` `calc`                                | (shared)    | platform-independent — reused, not reported           |
| Linux OS glue (`os/flatpak,xdg,systemd,desktop,cgroup,dbus`) | ~1,237      | + apprt cgroup/ipc/portal/gsettings ~700              |

**Reading of the sizing:** the *rendering + font* Linux surface (OpenGL 2.3k + FreeType
1.4k + fontconfig 0.2k ≈ 4k) is small and diff-comparable. The *app* surface (~24k GTK +
~2k OS glue) is 6× larger, GObject/Blueprint-shaped, and has no differential oracle. This is
why the ADR sequences rendering/fonts first (headless) and the app last.

## Our current portability seams (what P1/P2 build on)

- **Renderer:** `GpuBackend` trait clean and objc2-free (`renderer/src/gpu.rs:35-103`), but
  `Engine` bypasses it for concrete Metal (`engine.rs:27`, macOS-gated). `backend.rs` is a
  stub. No software/GL backend. → **P1 forces `Engine` through `GpuBackend`.**
- **Fonts:** `atlas`/`metrics`/`tables`/`constraint`/`presentation`/`embedded` are pure
  Rust already; rustybuzz shaping is chosen and portable, macOS-gated only via
  `use crate::coretext::Face` (`shaper.rs:34`). Seam is a concrete `Face` type, not a trait.
  → **P2 adds a FreeType `Face` + fontconfig discovery, un-gates the shaper.**
- **Core:** `qwertty-term-vt` (60,876 LoC), `qwertty-term-sprite` (tiny-skia), input
  encoding are already platform-free. Nothing to do here — keep it that way (the tripwire).
- **App:** `crates/qwertty-term` has no apprt boundary; macOS shell is `cfg`-gated
  module-by-module; `--offscreen-smoke` is macOS+Metal-only (`smoke.rs:15`). → **P4 only.**

## Phases

### P1 — Software rasterizer backend + `Engine` generalization  *(Wave-1, first)*

**Goal:** a CPU render path that turns cell/atlas data into an RGBA buffer with no GPU, no
window — and, in doing so, makes `Engine` render through `GpuBackend` rather than concrete
`crate::metal::*`.

- Introduce a `Software` backend implementing `GpuBackend`/`GpuBuffer`/`GpuTexture`
  (`renderer/src/gpu.rs`), compositing to a CPU framebuffer. Reuse `tiny-skia` (already a
  workspace dep via `qwertty-term-sprite`) for the raster surface; do **not** add
  cosmic-text/softbuffer.
- Refactor `Engine` (`renderer/src/engine.rs`) to be generic over `GpuBackend` where it
  currently names `crate::metal::*` concretely. This is the debt the ADR calls out; it is
  **T2 (renderer) territory** — file-claim `engine.rs`/`present.rs` and coordinate, or hand
  the sub-chunk to T2 (ADR open-question 3).
- Keep wire structs frozen (T2's invariant). The software backend consumes the same
  `Uniforms`/`CellText`/`CellBg` — it just interprets them on the CPU.
- **Evidence:** an offscreen readback test on Linux producing the same cell grid the macOS
  path produces for a known input (mirror the existing macOS offscreen-smoke assertions);
  dirty-equality suite still green.
- **Sizing:** small (single-digit-thousand LoC), dominated by the `Engine` generalization,
  not the CPU compositor.

### P2 — FreeType raster + fontconfig discovery → headless Linux text  *(Wave-1)*

**Goal:** real glyphs on Linux, so P1's software backend renders actual terminal frames.
This + P1 = the betamax headless artifact.

- Add a `freetype` `Face` (raster, synthetic bold via `FT_Outline_Embolden`, synthetic
  italic shear, color-bitmap emoji scaling) mirroring `face/freetype.zig:425+,439-447,524-640`.
  SVG emoji explicitly out of scope (upstream also skips it, `freetype.zig:342-345`).
- Add fontconfig discovery mirroring `discovery.zig:117-155,263-289` (family + mono bias +
  charset/codepoint fallback + sort). Use system fontconfig via a Rust binding
  (`fontconfig`/`yeslogic-fontconfig-sys`) or FreeType via `freetype-rs`/`freetype-sys`.
- Un-gate `shaper.rs`/`grid.rs`/`resolver.rs`/`collection.rs` from `crate::coretext::Face`
  to accept the platform `Face`. `Metrics::calc`, `Atlas`, `tables` reused unchanged
  (upstream proves metrics are platform-independent).
- Non-macOS emoji fallback: upstream loads embedded Noto emoji where macOS uses Apple emoji
  (`collection.rs:182-183` documents the no-op today) — decide bundle vs fontconfig-discover.
- **Evidence:** differential golden atlases (the MB3 vendored set) — FreeType output must be
  pixel-identical to upstream on integer-path glyphs, AA fringes within the existing budgets.
  `+list-fonts`/`+show-face` parity if/when those CLIs exist on Linux.
- **Sizing:** ~1.5–2k LoC (FreeType face ~1.4k analog + fontconfig ~0.2k + un-gating).

### P3 — Linux CI lanes + betamax integration  *(Wave-1, closes the artifact)*

- Add GPU-less Linux CI jobs (coordinate with **T8**, who owns CI): renderer software-raster
  readback tests + font FreeType/fontconfig tests + the existing vt/sprite/input platform-free
  suites. This is the lane that today can only run on a local Mac.
- Wire the betamax consumption: betamax renders through our software backend + FreeType path
  instead of cosmic-text (the MB4-Linux intent; `roadmap.md:133-134`). Report API friction
  back into the embeddability notes (`work/betamax-thread-prompt.md` feedback loop).
- **Evidence:** betamax golden outputs do not regress vs its cosmic-text baseline (or shifts
  are investigated as integration bugs, per the differential-proven engine).

### P4 — GTK4 interactive app + OpenGL backend + OS glue  *(SEPARATELY GREENLIT — not Wave 1)*

Deferred; do not start without Josh's explicit go. Full sizing above. Order within P4:

1. **OpenGL backend** (~2.3k): port `OpenGL.zig` + `opengl/*` + GLSL as a `GpuBackend` impl
   (GL 4.3, GLAD via `glow`/`gl` crate, swap-chain=1, always-sync). Reuse the generic
   `Engine` from P1. Keep GLSL verbatim from upstream (`shaders/glsl/*`).
2. **GTK4 app** (~24k analog): `gtk4-rs` App/Window/Surface mirroring `class/*`, GLArea
   hosting the GL context (main-thread draw, `must_draw_from_app_thread`), GtkIMMulticontext
   IME flow (`class/surface.zig:1275-1449`), Blueprint or programmatic UI, libadwaita chrome.
   Our app already has splits/tabs/search/keybind logic that is platform-free — reuse it;
   only the GObject shell is new.
3. **Winproto + OS glue** (~2.7k): Wayland/X11 (blur, decorations, quick-terminal via
   layer-shell), XDG portals, desktop notifications, DBus single-instance, systemd cgroup
   scopes, flatpak. Each is independently gateable.

P4 is integration-tested, not diff-proven (no UI oracle). Expect it to be the largest and
slowest Linux milestone by far.

## CI implications

- **Wave-1 (P1–P3) needs only a free Linux runner** — no GPU, no display server. This is
  the headline CI win: renderer + font stack finally get a Linux lane.
- **P4 GTK/GL needs a display server** (Xvfb/headless Wayland) and GL drivers — heavier,
  gated behind the P4 greenlight.
- Coordinate all CI with **T8** (owns `.github/workflows` on `joshka/qwertty-term`). T8's
  Linux runner already runs the platform-free majority; P1–P3 extend it to renderer/fonts.

## What T2 / T3 / T5 must keep portable meanwhile (standing constraint, in force now)

- **T2 (renderer):** keep `GpuBackend` clean; do **not** deepen `Engine`'s concrete-Metal
  coupling — ideally reduce it. New GPU features go through buffers/textures, wire structs
  stay frozen. P1's generalization is a shared interest; coordinate ownership.
- **T3 (config/keybinds):** Linux `gtk-*` keys (~10), `linux-cgroup*`, `primary-paste`,
  FreeType flags (`force-autohint`, `freetype-load-flags`) will land later — keep the config
  system open to platform-gated keys without macOS assumptions baked into the option parser.
- **T5 (vt):** keep `qwertty-term-vt` at zero `target_os`. This is the tripwire T8's Linux
  CI protects; any platform `cfg` entering vt is a regression to block.
- **All:** app-only (AppKit) code stays in `crates/qwertty-term`'s macOS-gated modules; the
  portable app logic (splits/tabs/search/keybind/theme) must not acquire AppKit deps, so a
  GTK shell can reuse it in P4.

## Coordination summary

| Item                                | Owner                    | Others involved                                               |
| ----------------------------------- | ------------------------ | ------------------------------------------------------------- |
| Software backend (P1)               | T7                       | T2 (renderer territory — file-claim `engine.rs`/`present.rs`) |
| `Engine`→`GpuBackend` refactor (P1) | T7 or T2 (open question) | ADR Q3                                                        |
| FreeType/fontconfig `Face` (P2)     | T7                       | font-crate seam already exists                                |
| Linux CI lanes (P3)                 | T8 (owns CI)             | T7 supplies the tests                                         |
| betamax integration (P3)            | betamax thread           | T7 supplies render path; feedback → embeddability             |
| GTK app / OpenGL / OS glue (P4)     | T7                       | separately greenlit; T3 for config keys                       |

## Status

ADR 003 **ACCEPTED** 2026-07-11. T7 is un-parked. **P1 is the next coding chunk**, gated
only on settling P1 ownership (`engine.rs`/`present.rs` — T7 file-claim vs T2 owns it) with
T2 before any renderer-territory edits. The standing constraint above remains in force.
