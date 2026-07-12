# Renderer P1: software backend + `Engine`→`GpuBackend` generalization (design)

Design/scoping study for [#41](https://github.com/joshka/qwertty-term/issues/41) — the core
of [ADR 003](../adr/003-linux-strategy.md)'s P1. Written by T7 (Linux) to make the
**ownership/sequencing decision concrete** (ADR open-question 3) before any renderer-core
edits. Commit-stamped survey of our tree at `main` (2026-07-12) + upstream `2da015cd6`.

**This is a proposal, not a merged decision.** `engine.rs`/`present.rs`/`gpu.rs` are T2
(renderer) territory; the trait extension below is the renderer's core architecture. T7 is
not editing them until T2 blesses the shape and the sequencing (see §6). Filed to T2's inbox.

## 1. Goal

A CPU (software) render path so `qwertty-term-renderer` produces terminal frames on Linux
with **no GPU and no window** — the headless artifact P1/P2 deliver and the thing
betamax will consume. Concretely: `Engine::draw_frame` must be able to run against a
`Software` backend that composites to an RGBA buffer, not only against `Metal`.

## 2. The seam today — and the exact gap

`gpu.rs` defines `GpuBackend` (+ `GpuBuffer`, `GpuTexture`). Its own module doc is explicit
about scope (`gpu.rs:21-27`): **"Chunk scope (R1): resource creation only. `Frame`,
`RenderPass` and `Pipeline` are declared as associated types so the trait shape is complete
… but no methods reference them yet."**

So the trait covers **resource creation** — `new_target`, `new_buffer[_with_data]`,
`new_texture`, `new_sampler`, `max_texture_size` (`gpu.rs:74-102`) — and `Engine` uses those
generically (it already imports `GpuBackend, GpuBuffer, GpuTexture`, `engine.rs:26`). But the
**frame / pipeline / render-pass / draw / present** surface is *not* on the trait. `Engine`
reaches those through **concrete `Metal` inherent methods**:

| What Engine does                                                                                                       | Where                                       | On the trait?          |
| ---------------------------------------------------------------------------------------------------------------------- | ------------------------------------------- | ---------------------- |
| `backend: Metal` field, `swap_chain: SwapChain<Metal>`                                                                 | `engine.rs:94,96`                           | ❌ concrete            |
| `Metal::new()` / `with_backend(backend: Metal)` ctors                                                                  | `engine.rs:167,177,183,191`                 | ❌ concrete            |
| `backend.begin_frame(completion) -> Frame`                                                                             | `engine.rs:1198`                            | ❌ inherent on `Metal` |
| `frame.render_pass(&[Attachment{…}]) -> RenderPass`                                                                    | `engine.rs:1200`                            | ❌ inherent on `Frame` |
| pipeline-from-shader-source (`library_from_source`, `PipelineOptions`, raw `objc2_metal::MTLLibrary`/`MTLPixelFormat`) | `engine.rs:1481-1555`                       | ❌ raw Metal           |
| `pass` draws / `Step` / `Primitive` / `Draw`                                                                           | `engine.rs:1478-1555`, `encode_image_steps` | ❌ concrete            |
| atlas texture creation, `Attachment`, `Buffer<T>`, `Pipeline` types                                                    | `engine.rs:27-31` imports                   | ❌ concrete            |
| present + readback via `IOSurfaceLayer`                                                                                | `present.rs:28,44,63`                       | ❌ Metal/IOSurface     |

Upstream models exactly this surface as its comptime `GraphicsAPI` interface
(`generic.zig`, `2da015cd6`): `GraphicsAPI → Target → Frame → RenderPass → Step → Pipeline`,
with methods `initShaders`, `initTarget`, `drawFrameStart/End`, `beginFrame`, `present`,
`presentLastTarget`, `initAtlasTexture`, `surfaceSize` (surveyed in
`docs/analysis/` OpenGL study). Our `gpu.rs` already names the same associated types
(`Target/Frame/RenderPass/Pipeline/Sampler`) — they just carry no methods yet.

**Therefore #41 is a trait-extension, not a find-and-replace.** You cannot make `Engine`
generic over `B: GpuBackend` until the trait grows the frame/pipeline/pass/draw/present
methods that `Engine` currently calls on concrete `Metal`.

## 3. Proposed design

Two coordinated moves:

### 3a. Extend `GpuBackend` to cover the draw surface

Add the missing methods, mirroring the concrete `Metal` inherent methods `Engine` already
calls (so the Metal impl is mostly "move existing code behind a trait method"), and named
after upstream's `GraphicsAPI` surface. New methods (associated traits on `Frame`/
`RenderPass` where the receiver isn't the backend):

- `GpuBackend::build_pipeline(desc, source) -> Self::Pipeline` — folds `library_from_source`
  and `PipelineOptions` (`engine.rs:1481-1555`).
- `GpuBackend::begin_frame(completion) -> Self::Frame` (`engine.rs:1198`);
  `new_atlas_texture(...)`; a present/readback entry (`draw_frame` end + `present.rs`).
- `GpuFrame` (new trait on `Self::Frame`): `render_pass(attachments) -> Self::RenderPass`
  (`engine.rs:1200`); `complete(sync)`; readback (`to_rgba`).
- `GpuRenderPass` (new trait on `Self::RenderPass`): `draw(pipeline, buffers, primitive,
  step/instances)` — the `Step`/`Draw`/`Primitive` surface (`encode_image_steps`,
  `engine.rs:1478`).

Keep `wire` structs frozen — the software backend consumes the same `Uniforms`/`CellText`/
`CellBg`/`Image` and interprets them on the CPU.

### 3b. Implement `Software` backend (new module, no collision)

A `software` module implementing the full trait against a CPU RGBA framebuffer, using
`tiny-skia` (already a workspace dep via `qwertty-term-sprite` — **no new dependency**):

- `Target` = a `tiny-skia::Pixmap` (or raw `Vec<u8>` BGRA to match Metal's readback order).
- `Buffer<T>`/`Texture`/`Sampler` = plain `Vec`-backed structs (trivial).
- `build_pipeline` = a no-op returning a tagged enum (`BgColor`/`CellBg`/`CellText`/`Image`)
  — software "pipelines" are just which CPU rasterizer to run, keyed by
  `PipelineDescription::name`.
- `render_pass.draw` = the actual raster: `bg_color`/`cell_bg` fill the target from
  `Uniforms`; `cell_text` iterates the per-instance `CellText` buffer and alpha-blits each
  glyph from the atlas texture (grayscale/color) at `grid_pos`, applying the same
  premultiplied-over blend upstream's shader does; `image` blits kitty RGBA.
- Reuse the color math already ported and unit-tested in
  `shaders/color_math.rs` (linearize/unlinearize/luminance/contrast) so software output
  matches the MSL's blending, not an ad-hoc approximation.

The `Software` backend is **new code** — it does not touch `engine.rs`. Only step 3a (trait)
and the `Engine` field/ctor genericization touch shared renderer files.

## 4. Evidence plan

- A Linux offscreen readback test: render a known snapshot through `Software` and assert the
  **cell grid** matches what the macOS `Metal` path produces for the same input (mirror the
  existing macOS offscreen-smoke assertions; geometry/coverage, not pixel-exact vs Metal).
- Run the backend-agnostic acceptance tests (`dirty_equality` scenarios, ink placement) on
  Linux via `Software` — the [#42](https://github.com/joshka/qwertty-term/issues/42) un-gate.
- macOS `Metal` path unchanged; full renderer suite still green.

## 5. Sizing

| Piece                                                           | Est.                             | Touches                                 |
| --------------------------------------------------------------- | -------------------------------- | --------------------------------------- |
| Trait extension (`gpu.rs` methods + `GpuFrame`/`GpuRenderPass`) | ~150–250 LoC                     | T2 core (`gpu.rs`)                      |
| Metal impl moved behind the new methods                         | ~200–400 LoC (mostly relocation) | T2 core (`metal/`, `engine.rs` helpers) |
| `Engine` genericized over `B: GpuBackend`                       | ~100–200 LoC diff                | T2 core (`engine.rs`, `present.rs`)     |
| `Software` backend (new module)                                 | ~500–800 LoC                     | **new file** (low collision)            |
| Linux readback test + un-gating                                 | ~150 LoC                         | tests                                   |

The genuinely contentious surface is the trait + Metal relocation + `Engine` genericization
(all T2 core); the software backend itself is additive.

## 6. Ownership & sequencing recommendation

The refactor rewrites the renderer's central abstraction and edits `engine.rs` (1426 LoC) —
which **T2's next slice also touches** (R6 slice 2: scroll/pin tracking + *viewport clip* is
renderer-side). A large generalization landing concurrently with T2's feature work invites
exactly the contention the file-claim protocol warns about. T2 currently holds **no claim**
on these files and has **no open PR** on them, but that is a momentary window, not a
guarantee.

**Recommended split (for T2 to accept/counter):**

1. **T7 authors the whole change** (#41) — trait extension + Metal-behind-trait + `Engine`
   genericization + `Software` backend — as one cohesive PR. It is the Linux thread's
   deliverable and the software backend is the forcing function; splitting the trait work
   from its first non-Metal consumer would land an unused abstraction.
2. **T2 owns the trait *shape*** — reviews/approves the `GpuBackend` extension before T7
   builds on it (it's T2's core contract; T2 also lives with it for OpenGL later).
3. **Sequence after T2's R6 slice 2 merges** (or T7 rebases onto it), so the `engine.rs`
   viewport-clip work and this genericization don't collide mid-flight. T7 does the
   non-colliding prep first: the `Software` backend module + the trait-extension design in
   `gpu.rs` behind review, while slice 2 is in flight.

Alternative if T2 prefers to own its core: **T2 does 3a (trait + Metal)**, T7 does 3b
(`Software`) + the `Engine` genericization on top. Cleaner territory boundary, one more
handoff.

Either way the decision is a two-thread coordination (T7↔T2), surfaced here + in T2's inbox;
Josh only needs to weigh in if T7 and T2 disagree.

## 7. Open questions for T2

1. Accept the trait-extension shape in §3a (names/method split), or counter?
2. Ownership: T7-authors-all (recommended) vs T2-owns-trait+Metal / T7-owns-software?
3. Sequencing vs R6 slice 2's `engine.rs` viewport-clip edits — after, or coordinate a
   shared rebase point?
