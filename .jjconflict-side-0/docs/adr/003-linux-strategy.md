# ADR 003: Linux strategy — toolkit, renderer, fonts, and the headless-first path

- Status: **ACCEPTED** (ratified by Josh 2026-07-11 — "approve recommended approach").
  The recommended direction stands: headless-first sequencing (P1→P3 Wave 1, GTK app P4
  deferred) and FreeType for font rasterization. T7 is un-parked; P1 is the next chunk.
  See "Open questions" for the one item still needing a coordination call (P1 ownership).
- Date: 2026-07-11
- Thread: T7 (Linux), Wave 3 · Spec: `docs/threads/t7-linux.md`
- Companion: `docs/plans/linux.md` (the phased plan, LoC sizing, and CI implications)
- Supersedes: nothing. Absorbs the never-written "M6 software-raster ADR" referenced in
  `docs/roadmap.md:133-134`.
- Confidence: **high** on the sequencing (headless-first) and the font/renderer seams;
  **medium** on the GTK-vs-lean-toolkit call for the eventual interactive app, which is the
  one genuinely reversible-later decision here.

## A note on ADR numbering

The directory is inconsistent: `0001-slow-runtime-safety-default-off.md` (4 digits) and
`002-termio-runtime.md` (3 digits). This ADR uses **3 digits (`003`)** to match the most
recent file, and we recommend 3-digit zero-padding as the going-forward convention. No
renumbering of the existing two is proposed — not worth the churn.

## Context

Linux is the last unstarted platform. Upstream Ghostty's Linux surface is large and
multi-layered, and **zero exploratory work existed on our side** before this ADR. The T7
spec requires the un-parking deliverable to settle five questions before any product code:
toolkit, renderer path, font scope, the betamax-headless angle, and a phased plan. This ADR
answers them from a survey of upstream source at pin `2da015cd6` (cited `file:line`
throughout) and of our own cross-platform seams.

### What upstream's Linux stack actually is (surveyed)

- **Apprt: GTK4 + libadwaita, 23,760 LoC.** The `gtk-ng` rewrite has already been promoted
  into `src/apprt/gtk` at this pin (there is no `src/apprt/gtk-ng`; `src/apprt/apprt.zig:42-46`
  offers only `.none` and `.gtk`). It is a full GObject-subclass design: `class/` alone is
  16,646 LoC (~70% of the tree), Blueprint `.blp` UI templates (1,455 LoC across
  `ui/1.0`…`ui/1.5`, dir names = minimum libadwaita version), a `Common(Self, Private)`
  GObject-boilerplate mixin (`class.zig`), and version-gated libadwaita feature use
  (`adw_version.atLeast(1,5,0)`). The three biggest files —
  `class/surface.zig` (4,146), `class/application.zig` (2,960), `class/window.zig` (2,205)
  — are 39% of the apprt on their own.
- **Renderer: OpenGL 4.3, 2,327 LoC.** `OpenGL.zig` (461) + `opengl/*` + 10 GLSL files.
  GLAD loader (`OpenGL.zig:134`), min GL 4.3 enforced (`OpenGL.zig:36-38,141-149`). The GTK
  **`GLArea` owns the context** and drawing happens **on the app/main thread**, not the
  render thread (`apprt/gtk/App.zig:22-23` `must_draw_from_app_thread = true`;
  `renderer/Thread.zig:508-511`; draw runs in `glareaRender` →
  `renderer.drawFrame(true)` at `class/surface.zig:3357`). Upstream already abstracts
  Metal/OpenGL behind a **comptime generic** `GenericRenderer(GraphicsAPI)`
  (`renderer.zig:38-42`, `generic.zig:81`); the two backends implement an identical decl set
  (`OpenGL.zig:16-32` vs `Metal.zig:22-37`). Swap-chain depth is a backend constant: GL=1
  (always-sync `glFinish`, `OpenGL.zig:30-32`), Metal=3 (triple-buffered). Wire structs
  (`Uniforms`/`CellText`/`CellBg`/…) are **hand-duplicated** between GLSL and MSL, not
  codegen'd (`opengl/shaders.zig:163-293` vs `metal/shaders.zig:192-324`).
- **Fonts: fontconfig discovery + FreeType raster + HarfBuzz shaping** on Linux
  (`font/backend.zig:39-61` → `fontconfig_freetype`). FreeType face is 1,439 LoC
  (`face/freetype.zig`); the fontconfig-specific discovery is ~225 LoC inside
  `discovery.zig` (`:114-339`). Crucially, **the grid/metrics math is shared and
  platform-independent**: both backends only read face values then call the same
  `Metrics.calc` (`Metrics.zig:227`; FreeType at `freetype.zig:1135+`, CoreText at
  `coretext.zig:995+`). Upstream vendors freetype/harfbuzz/fontconfig as static builds by
  default, each swappable to the system lib (`SharedDeps.zig:185-254`).
- **Linux OS glue outside the apprt**: cgroup transient scopes via systemd DBus
  (`apprt/gtk/cgroup.zig`, `os/cgroup.zig`), XDG portals (`portal/OpenURI.zig` 565 LoC,
  GlobalShortcuts 635 LoC), desktop notifications (`class/application.zig:1957-1991`),
  single-instance DBus IPC, flatpak (`os/flatpak.zig` 520), Wayland/X11 winproto (blur,
  CSD/SSD decorations, quick-terminal via gtk4-layer-shell) — ~1,496 LoC in `winproto/`.

### What our side already has (surveyed)

- **`GpuBackend` trait is clean and objc2-free** (`renderer/src/gpu.rs:35-103`) — resource
  creation only, no Metal types in signatures. **But the `Engine` bypasses it** and is
  written against concrete Metal (`engine.rs:27`, `#[cfg(target_os = "macos")]`), and no
  software or GL backend exists (`backend.rs` is a `{OpenGl, Metal, WebGl}` stub with a
  `TODO(chunk:R2+)`). Presentation (IOSurface-on-CALayer) is macOS-only and app-side.
- **Font portability is most of the way there.** `atlas`, `metrics`, `tables`
  (ttf-parser), `constraint`, `nerd_font_constraints`, `presentation`, `embedded` are
  already `cfg`-free pure Rust. **Shaping is rustybuzz** (pure Rust, decision 1 in
  `m3-first-pixels.md`), already the chosen path on both platforms — it's macOS-gated today
  only because it imports `crate::coretext::Face` (`shaper.rs:34`). The missing Linux piece
  is a **concrete non-CoreText `Face`** (raster) + **fontconfig discovery**; the seam is a
  concrete type, not a trait.
- **The VT engine (60,876 LoC), sprite rasterizer (tiny-skia), and input encoding are
  already platform-free** (`qwertty-term-vt`: zero `target_os`; input uses runtime
  `cfg!` behavior branches, not compile gates). Nothing in the core blocks Linux.
- **betamax already renders on Linux CI with cosmic-text** (`~/local/betamax` renderer,
  `cosmic-text = "0.19"`, `crates/betamax-core/src/ghostty/renderer.rs`). The MB track wants
  betamax to consume **our** stack instead, and `roadmap.md:133-134` explicitly parks
  betamax's Linux rendering on cosmic-text "until the software-raster ADR (M6)" — i.e. this
  ADR is the thing unblocking it.

## Decision

### Overarching principle: headless-first, interactive-app-last

**Sequence Linux by value density, not by mirroring upstream's layering.** The full GTK
interactive app is ~30k LoC of net-new platform code (apprt + GL + fonts + OS glue) that
serves the *smallest* near-term audience. The *headless software-rendered* path is a few
thousand LoC, unblocks the active MB/betamax track today, forces the `GpuBackend`/`Engine`
generalization that every later renderer benefits from, and gives us a free Linux GPU-less
CI lane for the renderer. So we deliver Linux **bottom-up through the portable seams**, and
treat the GTK app as a distinct, later, separately-greenlit milestone.

### Q1 — Toolkit: **GTK4 mirroring upstream, but deferred to the last phase**

For the eventual *interactive* Linux app, adopt **GTK4 + libadwaita via `gtk4-rs`**,
mirroring upstream. Reject winit and the "lean custom-chrome" path *for the app*. Reasons:

1. **IME fidelity is the same trap winit sprung on macOS.** Our macOS ADR
   (`docs/analysis/appkit-input.md`, ACCEPTED) rejected winit largely because its cooked
   `Ime` events drop selected/replacement ranges and mishandle dead-keys-under-modifiers.
   The Linux equivalent is real: upstream leans on `GtkIMMulticontext` with careful
   commit/preedit ordering (ibus vs simple, `class/surface.zig:1275-1340`) and a keyval→
   printable fallback (`:1420-1435`). winit's Linux IME is a thinner abstraction over the
   same ibus/fcitx machinery and would re-introduce exactly the fidelity gaps we refused on
   macOS. GTK gives us upstream's flow verbatim.
2. **The Linux value surface *is* GTK-shaped.** libadwaita chrome, Blueprint templates,
   quick-terminal via gtk4-layer-shell (`winproto/wayland.zig:72-87`), Wayland/X11 blur and
   decorations, XDG portals, GlobalShortcuts, desktop notifications, single-instance DBus,
   systemd cgroup scopes. winit hosts *none* of this; we'd rebuild it all, worse, and
   diverge from the source we differential-check against.
3. **Upstream is our oracle.** The port's entire method is "verify semantics in upstream,
   cite file:line." On Linux upstream *is* GTK. A winit app throws that away.

The winit calculus genuinely *does* differ from macOS in one way — on Linux winit would give
real cross-platform portability we don't otherwise have — but that upside only matters for a
windowed app, and it's outweighed by the IME + native-surface losses above. **This is the
one medium-confidence call**; it is also the most deferrable and the most reversible, since
nothing before Phase 4 depends on it. See the trade table.

> Note on cost: `gtk4-rs` is mature but binds a large GObject surface, and there is **no
> differential oracle for UI behavior** the way there is for the VT engine. Budget the GTK
> app as the single largest Linux chunk and expect it to be integration-tested, not
> diff-proven. It is explicitly *out of scope until Josh greenlights it separately*.

### Q2 — Renderer: **software-raster first, then port upstream's OpenGL; reject wgpu**

Three sub-decisions:

1. **Add a software rasterizer backend first** (Phase 1), as the headless Linux render
   path and the thing that makes `Engine` flow through `GpuBackend` instead of concrete
   Metal. This is the artifact `roadmap.md:133-134` is waiting on. It rasterizes our
   existing cell/atlas model to a CPU buffer (reuse `tiny-skia`, already a workspace dep for
   `qwertty-term-sprite`) and reads back RGBA — the same shape as the macOS offscreen-smoke
   readback, minus the GPU. It needs no window, no GL, no C GPU driver, so it runs on any
   free Linux CI runner.
2. **When GPU rendering is wanted on Linux, port upstream's OpenGL backend** (Phase 4,
   with the GTK app), not wgpu. Upstream's GL path is only ~2,327 LoC, is a proven 1:1 peer
   of the Metal backend behind the same generic renderer, and keeps us diff-comparable to
   the source. Its constraints are known and portable: GL 4.3, GLAD, GTK `GLArea`-owned
   context, main-thread draw, swap-chain=1.
3. **Reject wgpu-behind-`GpuBackend`.** It's tempting (one backend for GL/Vulkan/Metal),
   but: it's a very large new dependency, it would mean *rewriting* the working Metal path
   to gain nothing on macOS, upstream has no wgpu path so we lose differential comparability,
   and its abstraction fights the "port MSL/GLSL verbatim" invariant the renderer is built
   on. If a unified backend is ever wanted, that's its own future ADR with its own spike —
   not a Linux prerequisite.

Net renderer order: **software (headless/CI) → [GTK app] OpenGL**. The software backend is
the forcing function that pays down the `Engine`-bypasses-`GpuBackend` debt for everyone.

### Q3 — Fonts: **FreeType raster + fontconfig discovery as a concrete `Face` backend**

Keep metrics/atlas/shaper/tables shared.

Implement a Linux font backend as a **concrete non-CoreText `Face`** (FreeType
rasterization) plus **fontconfig discovery**, slotting into the *existing* module seam
(`cfg`-gated `Face` type, exactly as upstream keys its backend enum off one comptime value,
`font/backend.zig`). Keep shared and untouched: `Metrics`/`Metrics::calc` (upstream proves
this is platform-independent — both backends call the same function), `Atlas`, `tables`,
`constraint`, `nerd_font_constraints`, and **rustybuzz shaping** (already the chosen shaper;
it only needs to accept the new `Face` type instead of being hard-wired to
`crate::coretext::Face`).

Sub-decision — **FreeType (C) over a pure-Rust rasterizer** (swash/skrifa/ab_glyph):
FreeType matches upstream pixel-for-pixel and is therefore differential-testable against the
same golden atlases the MB3 work vendored (pixel-identical on integer-path glyphs). The
port's whole ethos is upstream fidelity, and font rasterization is where "close enough"
silently rots into visible divergence. **Tension to flag honestly:** the betamax track
celebrates "pure cargo, no toolchain" after dropping the pinned-Zig `libghostty-vt-sys`
(`work/betamax-thread-prompt.md`). FreeType via `freetype-sys` still needs a **C compiler**
(universally present, unlike Zig 0.15.2) and can build vendored, so "pure cargo" survives in
spirit; but it is not zero-C. If the pure-cargo constraint is later deemed hard, the
fallback is a pure-Rust rasterizer accepted as a *documented pixel deviation* from upstream
— that would be its own follow-up ADR. Recommendation stands at FreeType for fidelity.

### Q4 — Betamax headless Linux is the **Wave-1 first artifact**

Make **headless software rendering on Linux the first Linux deliverable**, ahead of any
windowing. It is the highest-value, smallest-surface Linux artifact:

- It lets betamax drop `cosmic-text` and render through *our* stack on Linux CI (completes
  the MB4 intent for Linux, which `roadmap.md:133-134` currently parks on cosmic-text).
- It is a small superset of work that must happen anyway: the software renderer (Q2.1) +
  the FreeType/fontconfig `Face` (Q3). No apprt, no GL, no toolkit.
- It gives the whole project a **GPU-less Linux CI lane** for the renderer and font stack,
  which today can only be exercised on a local Mac.
- It de-risks the `GpuBackend`/`Engine` generalization on the cheapest possible surface
  before the expensive GL/GTK work leans on it.

### Q5 — Phased plan + standing constraints

Full sizing, phase boundaries, CI implications, and what T2/T3/T5 must keep portable live in
`docs/plans/linux.md`. Phases in brief: **P1** software-raster backend (+ `Engine`
generalization); **P2** FreeType+fontconfig `Face` → headless Linux text render = the
betamax artifact; **P3** Linux CI lanes + betamax integration; **P4 (separately greenlit)**
GTK4 app + OpenGL backend + OS glue.

## Trade table — toolkit (the load-bearing, medium-confidence call)

| Factor                                                                 | GTK4 + libadwaita (gtk4-rs) — **chosen**                         | winit + custom chrome                              | Lean (softbuffer/wgpu, no toolkit)        |
| ---------------------------------------------------------------------- | ---------------------------------------------------------------- | -------------------------------------------------- | ----------------------------------------- |
| IME fidelity                                                           | Upstream's `GtkIMMulticontext` flow verbatim; ibus/fcitx correct | winit cooked-IME drops ranges (same trap as macOS) | Would hand-roll ibus/fcitx — worst option |
| Native surface (blur, CSD/SSD, quick-terminal, portals, notifications) | All present upstream; we mirror                                  | Absent — rebuild all, worse                        | Absent                                    |
| Differential comparability to upstream                                 | High — upstream *is* GTK on Linux                                | Low                                                | None                                      |
| Cross-platform reuse of the app shell                                  | None (GTK is Linux/BSD)                                          | High (winit is cross-platform)                     | High                                      |
| New-dependency weight                                                  | Large GObject surface (`gtk4-rs`)                                | Moderate                                           | Small                                     |
| UI-behavior test oracle                                                | None (integration-tested)                                        | None                                               | None                                      |
| **Needed before Phase 4?**                                             | **No — fully deferrable**                                        | No                                                 | No                                        |

winit's only real win (portability) serves a windowed app we build once; it costs the IME
and native-surface fidelity we already refused to trade away on macOS. Deferred to Phase 4,
so cheap to revisit if the calculus changes.

## Trade table — renderer path

| Option                    | LoC (new)                | Diff-comparable to upstream | Runs headless / GPU-less CI | Pays down `Engine`→`GpuBackend` debt | Verdict     |
| ------------------------- | ------------------------ | --------------------------- | --------------------------- | ------------------------------------ | ----------- |
| **Software raster first** | ~small (reuse tiny-skia) | n/a (our own)               | **Yes**                     | **Yes**                              | **Phase 1** |
| **Port upstream OpenGL**  | ~2.3k                    | **Yes** (1:1 Metal peer)    | No (needs GL/GLArea)        | Yes                                  | **Phase 4** |
| wgpu behind GpuBackend    | Large + big dep          | No (upstream has none)      | Partial                     | Rewrites working Metal               | **Reject**  |

## Consequences

- **Positive:** first Linux value ships in a few-thousand-LoC headless artifact that
  unblocks an active track (betamax/MB4-Linux); the `GpuBackend`/`Engine` seam gets
  validated and generalized cheaply; a GPU-less Linux CI lane appears for renderer+fonts;
  every heavy Linux decision (GTK, OpenGL) is deferred behind a separate greenlight with
  full sizing already in hand.
- **Negative / accepted:** FreeType reintroduces a C build dependency (mitigated: C
  compilers are universal; "pure cargo" survives in spirit). The GTK app remains a large,
  oracle-less, deferred chunk. The `Engine`-bypasses-`GpuBackend` refactor (P1) touches
  T2's territory and must be coordinated (file-claim; see the plan).
- **Standing constraint reaffirmed (in force throughout):** nothing merges that
  hard-couples portable layers to macOS — vt stays platform-free; font keeps the
  discovery/rasterize seam; renderer keeps `GpuBackend` clean (and P1 should *reduce*, not
  add, `Engine`'s concrete-Metal coupling); app-only code stays in the app crate. T8's CI
  running vt tests on Linux is the tripwire.

## Open questions — resolution (ratified 2026-07-11)

1. **Greenlight the headless-first sequencing?** — **YES.** P1→P2→P3 is Wave-1 Linux work;
   the GTK app (P4) is deferred behind a separate greenlight.
2. **FreeType vs pure-Rust rasterizer?** — **FreeType**, for upstream pixel fidelity and
   differential-testability. The C build dependency is accepted (mitigated: C compilers are
   universal; betamax's "pure cargo" survives in spirit). A pure-Rust rasterizer remains the
   documented fallback if the pure-cargo constraint is ever made hard — its own follow-up ADR.
3. **Does the P1 `Engine`→`GpuBackend` generalization belong to T7 or T2?** — **still open;
   coordination call, not a direction call.** Must be settled with T2 *before* P1 edits
   `engine.rs`/`present.rs`. Default: T7 file-claims it and coordinates; T2 may elect to own
   the sub-chunk. This is the one gate remaining before P1 touches renderer territory.
4. **Is betamax-consumes-our-Linux-render still wanted?** — **YES** (implied by approving the
   headless-first approach, whose payoff is exactly this). P3 wires it and reports API
   friction back into the embeddability notes.

T7 is un-parked. Next: settle Q3 with T2, then open P1.
