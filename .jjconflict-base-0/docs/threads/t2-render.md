# T2 — Renderer completeness thread

**Model:** Opus · **Wave:** 1 · **Workspace:** `work/t2` · **Status:** `status/t2.md`
**Territory:** `crates/qwertty-term-renderer`, renderer-facing additions in
`qwertty-term-font` (glyph/atlas APIs). App-side wiring (view/layer plumbing) via
file-claim — T4 owns the app in Wave 1. Rules: `docs/threads/README.md`.

## Mission

Close the renderer feature gaps vs upstream (`src/renderer/*` at `2da015cd6`): kitty image
rendering, link/URL overlay, background image, custom shaders — while preserving the two
invariants this renderer is built on: **frozen wire structs** and **dirty-tracking
equality vs full redraw** (there is an equality property test; every feature must keep it
green or extend it).

## Context you inherit

Metal backend with IOSurface-on-CALayer presentation, verbatim upstream MSL, grayscale +
color atlases (sRGB), run-based shaping w/ cache, per-row dirty tracking with upstream's
full-rebuild conditions, snapshot-offset scrollback, cursor-hide-when-scrolled. The engine
(vt) already parses/stores kitty images (transmit/place/delete, storage limits) — the
renderer just never draws them. `docs/analysis/` has per-subsystem notes; `docs/plans/
m3-first-pixels.md` holds the locked renderer decisions (IOSurface not CAMetalLayer, wire
freeze).

## Backlog (ordered)

- [ ] **Kitty image rendering — R6** (L, the big one): upstream `renderer/generic.zig`
      image layer + `src/renderer/image.zig`* (verify paths in source). Slices: (1) RGBA
      transmit→texture upload + placement quads for visible placements, unicode
      placeholders (U=1 cells) included; (2) scroll/pin tracking so images move with
      scrollback and clip to viewport; (3) delete/eviction + storage-limit interplay;
      (4) z-ordering vs text/bg per upstream. Evidence per slice: offscreen readback test
      with a known image (icat-style), dirty-equality extended to image scenarios,
      differential corpus untouched (engine side is done — if you need engine data not
      exposed, file-claim a minimal accessor, don't fork logic).
- [ ] **Link/URL detection + overlay — R7** (M) — **PRIORITY (Josh, 2026-07-13): the top
      everyday gap; do this next now that kitty R6 is complete.** upstream uses an OSC8 layer plus regex
      detection over rows with hover underline + cmd+click open. Engine has OSC8
      hyperlinks stored. Slices: (1) render underline-on-hover for OSC8 links (app sends
      hover cell via existing mouse plumbing — file-claim the small hook); (2) regex
      detection (upstream's `url.zig` pattern, `regex` crate is acceptable — document);
      (3) cmd+click → open (app-side handler, coordinate with T4 via claim/Inbox).
- [ ] **background-image** (M): image load + fit/position/repeat/opacity modes behind the
      bg pass, per upstream compositing order; config keys land via T3 later — implement
      with programmatic options now, config wiring later (leave a `## Inbox` note for T3).
- [ ] **background-blur / opacity-cells / glass** (S/M, macOS): `background-blur` via the
      private-API route upstream uses if acceptable, else document deviation ADR.
- [ ] **Custom shaders — R8** (M, LAST, optional this wave): shadertoy-compatible pipeline
      (naga translation), `custom-shader` + animation flag. De-scope to an ADR if effort
      balloons — polish elsewhere matters more.
- [ ] **Scrolled-back rendering polish** (S): background for partial top rows, any
      remaining viewport-edge artifacts found while in here.

## Method rules

Wire structs stay frozen — new GPU data = new buffers/textures, never repacked existing
structs. Every feature extends `tests/` offscreen readback + the dirty-equality suite.
Shaders: keep upstream MSL verbatim where one exists; new shaders get golden-image tests
with tolerance bands. Perf: run `scripts/bench-quick.sh` (T1's fence, once it exists)
before merging anything touching the frame path; a >3% doom-fire/vtebench regression
blocks merge (coordinate with T1 via status files).

## Definition of done

`icat`/kitty-image demos render correctly incl. scrollback; OSC8 links hover+open;
background-image works; dirty-equality suite covers all of it; feature-coverage.md
renderer section flipped to `[x]` accordingly (update it in your PRs).
