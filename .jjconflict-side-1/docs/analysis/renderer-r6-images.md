# R6 — Kitty image rendering (renderer side)

Authored 2026-07-11 (T2 thread). Upstream refs at ghostty `2da015cd6` (a pinned
worktree lives at `/private/tmp/ghostty-2da015`; `~/local/ghostty` itself has
drifted to `38e49a232`). This documents the design of the renderer-side kitty
image path — the engine already parses/stores images (`docs/analysis/kitty-graphics.md`),
this is about drawing them.

## The core architectural question: how image data crosses to the renderer

Upstream resolves image placements **inside the renderer** (`renderer/image.zig`
`kittyUpdate`/`prepKittyPlacement`), which runs under the `draw_mutex` with direct
access to the live `Terminal`. Our port splits differently:

- The renderer consumes a captured [`RenderSnapshot`], not a live `&Terminal`.
- The **live-app render path** (`crates/qwertty-term/src/app.rs`) captures a
  `SnapshotWindow` under a lock, releases the lock, then renders on that snapshot
  (`FullSnapshot::from_window`). It never holds `&Terminal` on the render thread.
- The **test path** (`FullSnapshot::capture(&Terminal, …)`) *does* hold `&Terminal`.

`SnapshotWindow` (in `qwertty-term-vt`) carries **no kitty data** today. So there
are two ways to get images to the renderer:

1. Thread resolved kitty data through `SnapshotWindow` (needed for the live app —
   the data must be captured under the lock and outlive its release).
2. Resolve at capture time from `&Terminal` (only possible on the test/`capture`
   path).

**Slice 1 takes path (2)**: it delivers a fully-tested image pipeline via the
`capture` path without touching `SnapshotWindow`. The live-app path (`from_window`)
carries empty kitty data for now; wiring it (path 1) is a later slice that needs a
vt claim on `snapshot.rs`.

## Where placement resolution lives (and why it's in vt)

Resolving a placement to a drawable quad requires dereferencing its tracked
`*mut Pin` and walking the `PageList` (`point_from_pin`, the virtual-placeholder
iterator). Those are `qwertty-term-vt` internals; doing the deref from the renderer
crate would be an `unsafe` reach into another crate's representation and would fork
`prepKittyPlacement`'s math.

So the resolver is a **new vt module** (`crates/qwertty-term-vt/src/kitty/render.rs`,
a minimal additive file-claim): `resolve_placements(storage, pages, geo) ->
Vec<RenderImagePlacement>` returns flat, `Pin`-free, viewport-relative data the
renderer converts directly into the frozen `wire::Image` GPU struct. It also hosts
`image_rgba(image)` (the RGB/gray→RGBA swizzle, port of `image.zig`'s `convert`).
This same resolver is what the live-app slice will call from inside
`snapshot_window`, so no logic is duplicated later — only relocated to the capture
site.

## GPU pipeline (renderer)

- **Shader**: `image_vertex`/`image_fragment` ported **verbatim** from upstream
  `shaders.metal` into `shaders/ghostty.metal`. `bg_image` remains skipped.
- **Wire struct**: the frozen `wire::Image` (40 bytes: grid_pos/cell_offset f32×2,
  source_rect f32×4, dest_size f32×2) already existed and is unchanged; a new
  `IMAGE_ATTRIBUTES` vertex layout + `image_layout_pins_match_wire_offsets` test
  pin it, mirroring the cell_text pinning.
- **Pipeline**: registered as `"image"` in `PIPELINE_DESCRIPTIONS`, premultiplied
  "over" blend, per-instance step. `VertexFormat` gained `Float2`/`Float4`.
- **Textures**: an Engine-level `HashMap<u32, ImageEntry>` (id → generation +
  texture). `rgba8unorm_srgb` (srgb=true, matching upstream), so the GPU
  linearizes on sample and the shader unlinearizes back when blending isn't
  linear. Re-upload only when an id's `generation` changes (upstream's staleness
  protocol). This is content, not per-slot — safe under day-one `Sync` pacing.
- **Instances**: one `Buffer<Image>` per placement (pool grown on demand),
  matching upstream's one-buffer-per-image draw. Each placement is a 4-vertex
  triangle-strip quad with its own texture bound — one `pass.step` per placement.
- **Draw order**: after `cell_text`, in **both** draw bodies (`engine::draw_frame`
  and `present::draw_and_present_inner` — these are duplicated and must stay in
  sync). Slice 1 draws every placement above text; z-order buckets are slice 4.

## Slice boundaries

1. **(this)** RGBA transmit→texture→placement quads, pin-anchored + virtual (U=1)
   placements, via the capture path. Offscreen readback test + dirty-equality
   image scenario + GPU-less vt resolver unit tests.
2. Scroll/pin tracking + pixel-accurate viewport clipping (partial images at the
   top/bottom edge; negative `grid_row`). Reconcile the snapshot's
   `scrollback_offset` with the pagelist viewport.
3. Delete/eviction + `image-storage-limit` interplay: drop GPU textures when the
   engine deletes an image; honor `storage.dirty`.
4. Z-order buckets (below-bg / below-text / above-text) per upstream, sorted by z
   with the image-id tiebreak.
5. Live-app `SnapshotWindow` threading (needs a vt `snapshot.rs` claim): carry the
   resolved placements + Arc-shared RGBA so `from_window` (the real render loop)
   draws images too.

## Invariants preserved

- **Frozen wire structs**: `wire::Image` unchanged; new GPU data uses new
  buffers/textures. Layout re-pinned by test.
- **Dirty-equality**: extended with a kitty-image scenario
  (`dirty_tracking_equals_full_redraw_with_image`). Image resolution is stateless
  (rebuilt each frame from storage) and texture upload is generation-keyed, so the
  incremental and full-redraw paths stay byte-identical.
- **Engine semantics untouched**: no parser/vt-diff changes (the resolver only
  reads storage).
