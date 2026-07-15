# T7 — Linux thread (PARKED — ADR gate before any code)

**Model:** Opus · **Wave:** 3 (open only when Josh says so AND Waves 1–2 have drained
enough to free a seat) · **Workspace:** `work/t7` · **Status:** `status/t7.md`
**Territory (once open):** Linux apprt, `qwertty-term-renderer` OpenGL backend, font
fontconfig/FreeType backend, CI Linux-GUI lanes. Rules: `docs/threads/README.md`.

## Why parked

Linux is a platform port, not a feature: GTK4 apprt (upstream `src/apprt/gtk*` — large),
OpenGL backend (R9, ~1.9k), FreeType+fontconfig discovery/rasterization (F-deferred),
Wayland/X11 integration, `linux-cgroup*`. Zero exploratory work exists on our side. The
first deliverable is an ADR + plan, not code — and the cross-platform seams the macOS
code preserved (GpuBackend trait, discovery abstraction, apprt boundary) must be
validated as actually sufficient before committing to an approach.

## Session 1 deliverable (the un-parking work)

`docs/adr/00X-linux-strategy.md` (PROPOSED) + `docs/plans/linux.md` covering:

1. **Toolkit decision**: GTK4 mirroring upstream vs a leaner path (winit was rejected for
   macOS — reasons in the spike notes; re-examine for Linux where the calculus differs).
   Recommendation with trade table.
2. **Renderer**: port upstream's OpenGL backend vs wgpu-behind-GpuBackend vs Metal-only-
   for-now + software raster for betamax CI (that software-raster ADR is referenced in
   the roadmap M6 — fold it in here).
3. **Fonts**: fontconfig discovery + FreeType rasterization scope; what stays shared with
   the CoreText path (Metrics, Atlas, shaping are already portable).
4. **Betamax angle**: betamax's Linux CI currently uses its cosmic-text path; decide
   whether headless-Linux rendering (software or GL) is this thread's Wave-1 deliverable
   — it's likely the highest-value Linux artifact and much smaller than the full app.
5. Phased plan with LoC/complexity sizing (upstream `wc -l` per dir), CI implications,
   and what T2/T3/T5 must keep portable meanwhile.

Present the ADR to Josh; the thread stays parked until he accepts a direction.

## Standing constraint on other threads (in force NOW, while parked)

Nothing merges that hard-couples portable layers to macOS: vt stays platform-free;
font keeps the discovery/rasterize trait seam; renderer keeps GpuBackend clean; app-only
code stays in the app crate. T8's CI running vt tests on Linux is the tripwire.
