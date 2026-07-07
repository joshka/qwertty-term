# Plan: M3 first pixels (fonts + Metal renderer)

Decisions are MADE here; executing agents implement, they don't re-litigate. Zig refs at
commit `2da015cd6`. Chunk IDs match `docs/roadmap.md`. F1/R0 are in flight as this is
written — their outputs (`ghostty-font`, `ghostty-renderer` crates) are the substrate.

## Decisions (locked unless evidence overturns them — record an ADR if so)

1. **Shaping: rustybuzz first.** Upstream's default macOS shaper is CoreText
   (`shaper/coretext.zig`, XL: CFRelease thread, forced-LTR BiDi, cluster heuristics), but
   upstream's own `coretext_harfbuzz` backend proves HarfBuzz semantics are viable on macOS.
   First pixels use rustybuzz (pure Rust) + the ported run-segmentation logic. The CoreText
   shaper is a FIDELITY PASS later (M6-adjacent), compared via glyph-index/position dumps.
2. **Presentation: IOSurface-on-CALayer, not CAMetalLayer.** Upstream renders into an
   IOSurface-backed MTLTexture (Display P3, 32BGRA) and assigns the IOSurface to a plain
   CALayer's `contents` (async main-thread dispatch; sync during resize). Port this exactly
   — do NOT introduce CAMetalLayer/nextDrawable or wgpu. Crates: objc2-metal,
   objc2-quartz-core, objc2-io-surface (or io-surface), block2 for completion handlers,
   objc2 `declare_class!` for the CALayer subclass (hooks `display` + `actionForKey:` to
   disable implicit animations).
3. **Pacing, day one: plain 8–16ms timer** driving draw; CVDisplayLink (objc2-core-video)
   added after pixels exist. Two pacing sources ultimately: display-link tick (steady
   state) + CALayer `display` callback (resize-driven synchronous redraw). Triple buffering
   via semaphore permits=3; day-one degenerate mode permits=1 + `waitUntilCompleted` is
   acceptable.
4. **Damage tracking, day one: full redraw** (`dirty = full` every frame) behind the
   `RenderSnapshot` trait from R0. Dirty-row impl comes later by wiring PageList's existing
   per-row dirty flags — renderer code must not change when it does.
5. **Wire structs are a frozen contract.** `Uniforms`/`CellText`(32 bytes, has a sizeof
   test upstream)/`CellBg`([4]u8)/`Image`/`BgImage` layouts must match `shaders.metal`
   bit-for-bit; buffer index convention: 0=vertex data, 1=uniforms, 2+=extras. R1 review
   freezes these before R4 emits into them.
6. **Shaders: port MSL verbatim** (embed source; runtime `newLibraryWithSource` first,
   build-time metallib via `xcrun metal` in build.rs later). The color math
   (linearize/unlinearize, sRGB↔P3, WCAG contrast, luminance-based alpha remap) must be
   numerically exact — golden-value unit tests on the helper functions.
7. **Discovery, first cut: load-by-name only.** CoreText descriptor for the config
   font-family (fall back to embedded JetBrains Mono). The full `Score` ranking (XL) and
   fallback chains are completeness passes.
8. **Collection index: slotmap-style arena, not the packed u16 bitfield** — upstream packs
   3-bit style + 13-bit index and aliases SegmentedList pointers; Rust wants generational
   indices. Keep the 4-style grouping semantics.

## Chunk sequence (after F1+R0 land)

| Chunk                         | Model                         | Scope (Zig refs)                                                                                                                                                                                                                            | Key acceptance                                                                                                                                       |
| ----------------------------- | ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| R1 GpuBackend + Metal context | Opus                          | trait w/ associated types Target/Frame/RenderPass/Pipeline/Buffer/Texture/Sampler (validated by upstream's exact metal/-opengl/ file mirror); Metal.zig, metal/{Target,Texture,Sampler,buffer}.zig; api.zig DELETED in favor of objc2-metal | headless test: create device/queue, IOSurface target, upload a texture, readback                                                                     |
| F5-reduced CoreText face      | Opus                          | face/coretext.zig rasterization + metrics extraction; load-by-name discovery only                                                                                                                                                           | rasterize 'A' from JetBrains Mono + a system font; alpha bitmap snapshot test; metrics parity with ghostty-font's table-derived values               |
| R2 frame/present/pacing       | Opus                          | metal/{Frame,IOSurfaceLayer,RenderPass,Pipeline}.zig + SwapChain semaphore (generic.zig:230-430)                                                                                                                                            | offscreen: render clear-color frame, read IOSurface pixels back                                                                                      |
| R3 shaders + wire structs     | Sonnet                        | shaders.metal (cell_text/cell_bg/bg_color + full_screen vertex + helpers ~450 lines MSL); shaders.zig struct defs                                                                                                                           | sizeof/layout tests incl. CellText==32; MSL compiles at runtime; color-math golden values                                                            |
| F6-reduced + F7-reduced       | Opus                          | single-style Collection, minimal resolver (one font + sprite dispatch to ghostty-sprite), run.zig reduced (single-font runs, no fallback/ligature-split), rustybuzz shaping, glyph->atlas upload via ghostty-font Atlas                     | shape+rasterize "hello", an em dash, a CJK char, a symbol into atlas; positions verified                                                             |
| R4 cell engine                | Opus (large; priority ladder) | cell.zig + generic.zig rebuildCells/addGlyph/addCursor/underline family + updateFrame/drawFrame cores (~2.6k)                                                                                                                               | FIRST PIXELS: offscreen frame of a live Terminal snapshot with real glyphs, readback-compared against per-cell expectations; then wire into a window |
| R5 window swap                | Opus                          | Thread.zig-lite (plain thread+channel first) + replace egui host in spike with an NSView/winit hosting the CALayer                                                                                                                          | `--window` runs on Metal; egui path kept behind a flag until parity, then deleted                                                                    |

Completeness follow-ons (parallel, post-pixels): R6 kitty/bg images, R7 links+overlay+
min-contrast edges, F5-full discovery Score, F7-full run splitting (fallback, bad-ligature,
emoji presentation), F2 glyf rasterizer (glyph protocol), nerd-font constraints codegen
(companion to ghostty-sprite), R8 shadertoy, CoreText-shaper fidelity pass.

## Verification strategy

Offscreen-readback tests at every step (no human eyes needed until the window swap): render
into the IOSurface target, read pixels, assert. Reuse `--render-probe` habits from the
spike. Side-by-side vs real ghostty is the human checkpoint at R5. Keep the egui frontend
runnable until R5 passes it visually.
