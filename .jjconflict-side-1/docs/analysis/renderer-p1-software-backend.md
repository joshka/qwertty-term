# Renderer P1: software backend + `Engine`→`GpuBackend` generalization (design)

Execution-ready design/sizing for [#41](https://github.com/joshka/qwertty-term/issues/41) —
the core of [ADR 003](../adr/003-linux-strategy.md)'s P1. Written by T7 (Linux) so the work
is mechanical the moment T2 accepts the trait shape (ADR open-question 3). Survey of our tree
at `main` (2026-07-12, full method-signature inventory) + upstream `2da015cd6`.

**This is a proposal, not a merged decision.** `gpu.rs`/`engine.rs`/`present.rs` are T2
(renderer) territory; the trait extension is the renderer's core contract. T7 will not edit
those files until T2 blesses the shape and sequencing (§7). Filed to T2's inbox.

## 1. Goal

A CPU (software) render path so `qwertty-term-renderer` produces terminal frames on Linux
with **no GPU and no window** — the headless artifact P1/P2 deliver and the thing betamax
consumes. Concretely: `Engine::draw_frame` (and the readback in `Frame::to_rgba`) must run
against a `Software` backend that composites to an RGBA buffer, not only `Metal`.

## 2. The seam today — the exact gap (from the signature inventory)

`GpuBackend` (`gpu.rs:35-103`) abstracts **resource creation only** (its own docs,
`gpu.rs:21-27`). The associated types `Target`/`Frame`/`RenderPass`/`Pipeline` exist
(`gpu.rs:45-57`) but **no trait method references them**, so the entire frame/pipeline/
pass/draw/present path is hard-wired to concrete `Metal`.

**Already generic (a Software backend reuses as-is):** `new_target`/`new_buffer[_with_data]`/
`new_texture`/`new_sampler`/`max_texture_size` (`gpu.rs:74-102`); `GpuBuffer`
(`len`/`sync`/`sync_from_slices`) and `GpuTexture` (`width`/`height`/`replace_region`);
`SwapChain<B>`/`FrameSlot<B>`/`FrameGuard<B>` (`swap_chain.rs`, fully generic); the
backend-agnostic `shaders::PipelineDescription` table (`shaders/mod.rs:117-221`).

**Hard-wired to `Metal` (the extension surface), with the crux in bold:**

| Cluster          | Concrete API (file:line)                                                                                                                                                                                                                                              | What the trait needs                                                                                                            |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| Backend ctor     | `Metal::new` (`metal/mod.rs:106`), called `engine.rs:208,218`                                                                                                                                                                                                         | a fallible `Backend::new()` seam (or backend-supplied constructor)                                                              |
| Frame start      | `Metal::begin_frame(FrameCompletion) -> Frame` (`metal/mod.rs:214`)                                                                                                                                                                                                   | `GpuBackend::begin_frame`                                                                                                       |
| Pipeline build   | `Metal::new_pipeline(&PipelineOptions)` (`:221`) + `library_from_source(&MTLDevice, &str)` (`pipeline.rs:209`) + `device()` (`:161`) + `target_pixel_format()` (`:203`) — **`PipelineOptions` leaks `MTLLibrary`; the whole path leaks `MTLDevice`/`MTLPixelFormat`** | `GpuBackend::build_pipeline(&PipelineDescription, source)` that hides library/format                                            |
| Frame            | `Frame::render_pass(&[Attachment]) -> RenderPass` (`frame.rs:88`), `Frame::complete(sync)` (`:101`)                                                                                                                                                                   | `GpuFrame` trait                                                                                                                |
| Render pass      | `RenderPass::step(&Step)` (`render_pass.rs:147`), `RenderPass::complete()` (`:225`)                                                                                                                                                                                   | `GpuRenderPass` trait                                                                                                           |
| **Draw binding** | **`Step<'a>` (`render_pass.rs:56-71`) holds raw `&MTLRenderPipelineState`, `&MTLBuffer`×N, `&MTLTexture`×N, `&MTLSamplerState`×N**; `Attachment` holds `&MTLTexture`; `Draw`/`Primitive` are already plain data                                                       | **rebind `Step` through the trait's own `Pipeline`/`Buffer`/`Texture`/`Sampler` associated types — this is the whole ballgame** |
| Target use       | `Target::texture()`→`&MTLTexture` (`target.rs:98`), `surface()`→`&IOSurfaceRef` (`:93`), `read_pixels()->Vec<u8>` BGRA (`:113`), `width/height`                                                                                                                       | `GpuTarget` trait: draw-dest handle + `read_pixels`                                                                             |
| Presentation     | `IOSurfaceLayer` + `set_surface_sync` (`layer.rs:177`), `present.rs` `draw_and_present*`                                                                                                                                                                              | **stays macOS/Metal-only** — headless needs no window; see §3.4                                                                 |
| `Engine` fields  | `backend: Metal`, `swap_chain: SwapChain<Metal>`, four `Pipeline`, `Vec<Buffer<Image>>`, `HashMap<u32, ImageEntry{texture: Texture}>` (`engine.rs:135-199`); `present_parts`/`build_pipeline`/`encode_image_steps` all name `Metal`                                   | make `Engine<B: GpuBackend>`                                                                                                    |

`Draw` (`render_pass.rs:77`, `{primitive, vertex_count, instance_count}`) and `Primitive`
(`frame.rs:159`, `{Triangle, TriangleStrip}`) are already backend-neutral plain data — keep
them. Upstream models exactly this as `GraphicsAPI → Target → Frame → RenderPass → Step →
Pipeline` (`generic.zig`), and its `RenderPass.step` binds the backend's own buffer/texture
types — i.e. the fix below is what upstream already does.

## 3. Recommended design

### 3.1 Rebind `Step` through the trait's associated types (the crux)

The trait *already declares* `Pipeline`, `Buffer<T>`, `Texture`, `Sampler`. Make `Step`
generic and reference **those** instead of raw `objc2_metal`:

```rust
// gpu.rs — new
pub struct Draw { pub primitive: Primitive, pub vertex_count: usize, pub instance_count: usize }
pub enum Primitive { Triangle, TriangleStrip }

pub struct Step<'a, B: GpuBackend + ?Sized> {
    pub pipeline: &'a B::Pipeline,
    pub vertex:   Option<&'a B::BufferHandle>,   // untyped bindable handle (see 3.2)
    pub uniforms: Option<&'a B::BufferHandle>,
    pub extras:   &'a [Option<&'a B::BufferHandle>],
    pub textures: &'a [Option<&'a B::Texture>],
    pub samplers: &'a [Option<&'a B::Sampler>],
    pub draw:     Draw,
}
pub struct Attachment<'a, B: GpuBackend + ?Sized> {
    pub texture: &'a B::Target,          // render dest is a Target, not a Texture
    pub clear_color: Option<[f64; 4]>,
}

pub trait GpuFrame {
    type Backend: GpuBackend;
    type Error: Error + Send + Sync + 'static;
    fn render_pass(&self, attachments: &[Attachment<'_, Self::Backend>])
        -> Result<<Self::Backend as GpuBackend>::RenderPass, Self::Error>;
    fn complete(&mut self, sync: bool);
}
pub trait GpuRenderPass {
    type Backend: GpuBackend;
    fn step(&self, step: &Step<'_, Self::Backend>);
    fn complete(self);
}
// GpuBackend — new methods
fn new() -> Result<Self, Self::Error>;
fn begin_frame(&self, completion: FrameCompletion) -> Result<Self::Frame, Self::Error>;
fn build_pipeline(&self, desc: &shaders::PipelineDescription, source: ShaderSource<'_>)
    -> Result<Self::Pipeline, Self::Error>;
// new bounds: type Frame: GpuFrame<Backend=Self>; type RenderPass: GpuRenderPass<Backend=Self>;
//             type Target: GpuTarget; and a new `type BufferHandle;`
```

`FrameCompletion = Box<dyn Fn(Health, bool) + Send + 'static>` (already `frame.rs:56`; move
`Health` to `gpu.rs`). `ShaderSource<'a>` is backend-chosen: Metal wraps the MSL `&str`
(`shaders::SOURCE`) and compiles it in `build_pipeline`; Software takes `()` and keys the
pipeline off `desc.name`. This deletes `PipelineOptions`/`library_from_source`/`device()`/
`target_pixel_format()` from the *trait* surface — they become Metal-internal to its
`build_pipeline` impl.

### 3.2 The one real decision for T2: buffer binding handle

`Step` binds buffers **untyped** (Metal binds `MTLBuffer` regardless of `T`). Our
`GpuBuffer<T>` is typed. Two options — recommend **A**:

- **A. `type BufferHandle` + `GpuBuffer::handle(&self) -> &B::BufferHandle`.** Metal:
  `BufferHandle = ProtocolObject<dyn MTLBuffer>` (returns what `buffer()` returns today,
  `buffer.rs:110`). Software: `BufferHandle = SoftBuffer` (a `Rc<RefCell<Vec<u8>>>` view or an
  arena id). Minimal churn: call sites change `slot.vertex.buffer()` → `slot.vertex.handle()`.
- **B.** Make `Step` bind `&dyn Any` / an enum. Rejected: loses type-checking, uglier.

`GpuTexture` already exposes `width/height/replace_region`; add `type TextureHandle`? Not
needed if `Step.textures` binds `&B::Texture` directly and the Metal impl's `step()` calls
`.texture()` internally. Same for `Sampler`. So **only buffers need the extra handle type**
(they're bound untyped); textures/samplers/pipelines bind by their associated type directly.

### 3.3 `GpuTarget` (draw dest + readback)

```rust
pub trait GpuTarget {
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn read_pixels(&self) -> Vec<u8>;   // BGRA, tight rows — matches Target::read_pixels
}
```

Metal maps to `Target::{width,height,read_pixels}` (`target.rs:84-113`); `surface()` stays a
Metal-inherent extra used only by presentation (§3.4), not on the trait. Software: the Target
*is* the `Vec<u8>` framebuffer; `read_pixels` clones it (already BGRA).

### 3.4 Presentation stays out of the trait

`IOSurfaceLayer`/`set_surface_sync`/`present.rs::draw_and_present*` are macOS-only and
**headless needs none of it** — the P1 artifact reads pixels back, it doesn't present to a
window. So: leave `present.rs` exactly as-is (`#![cfg(target_os = "macos")]`), keep its
`Engine<Metal>`-specialized `impl`, and make only `draw_frame`/`update_frame`/`sync_atlas`/
readback generic. The Software backend never touches presentation. (A Linux windowed present
is P4/GTK, a separate seam.)

### 3.5 `Software` backend module (new, non-colliding)

New `renderer/src/software/` implementing the full extended trait against a CPU `Vec<u8>`
BGRA framebuffer, reusing `tiny-skia` only if a fill/blit primitive helps (already a
workspace dep via `qwertty-term-sprite`; no new dep). Per-pipeline CPU raster, keyed by
`PipelineDescription::name`, consuming the frozen wire structs — see §4. Associated types:
`Target = SoftTarget(Vec<u8>, w, h)`; `Buffer<T> = SoftBuffer(Vec<T>)` with a byte-view
`BufferHandle`; `Texture = SoftTexture{fmt, w, h, Vec<u8>}`; `Sampler = ()`;
`Pipeline = SoftPipeline(enum {BgColor, CellBg, CellText, Image})`; `Frame`/`RenderPass`
accumulate steps then execute on `complete`.

## 4. Software rasterization spec (per pipeline, exact)

Reference: `shaders/ghostty.metal` (function lines below) + the **already-ported, unit-tested
color math** in `shaders/color_math.rs` (`linearize`/`unlinearize`/`luminance`/
`contrast_ratio`/`contrasted_color`) — reuse it verbatim so software output matches the MSL,
not an approximation. Uniforms/instance layouts are frozen in `wire.rs`.

- **`bg_color`** (`ghostty.metal:245`): fill the whole target with `Uniforms.bg_color`
  (`wire.rs:136`). Honor `bools.use_linear_blending`/`use_display_p3` exactly as the MSL
  `load_color` helper does (`ghostty.metal:147-195`).
- **`cell_bg`** (`ghostty.metal:261`): per-pixel, compute the cell `(col,row)` from
  `Uniforms.cell_size`/`grid_padding`; blend `CellBg[row*cols+col]` (`wire.rs:207`,
  `[u8;4]`) premultiplied-over the target. Padding pixels use `padding_extend`
  (`wire.rs:129`, LSB left/right/up/down) to pick the nearest edge cell's color.
- **`cell_text`** (vtx `ghostty.metal:366`, frag `:488`): for each `CellText` instance
  (`wire.rs:170`): source rect = `glyph_pos..glyph_pos+glyph_size` in the atlas
  (`atlas` field picks grayscale vs color, `wire.rs:144`); dest top-left =
  `grid_pos*cell_size + (bearings.x, cell_baseline-bearings.y)` (bearings `wire.rs:176` are
  i16 px). Grayscale glyph: coverage α from the atlas × `contrasted_color(min_contrast,
  color, cell_bg_at_that_cell)` (`ghostty.metal:465`), premultiplied-over, in linear space
  when `use_linear_blending`. Color glyph (emoji): sample BGRA directly, premultiplied-over
  (`ghostty.metal:513`). `bools.is_cursor_glyph`/`no_min_contrast` (`wire.rs:157`) gate the
  contrast step.
- **`image`** (kitty, vtx `ghostty.metal:596`, frag `:640`): for each `Image` instance
  (`wire.rs:213`), blit `source_rect` of the image texture to `grid_pos*cell_size +
  cell_offset`, scaled to `dest_size`, premultiplied-over. R6-slice-1 scope; can land last.

Blend/color invariants come straight from `color_math.rs`; the atlas textures are the same
grayscale+color `GpuTexture`s the engine already syncs (`engine.rs:1346,1358`).

## 5. Mechanical migration checklist (`engine.rs`/`present.rs` — T2/authored-by-T7)

Each concrete site → generic form. Line numbers from the 2026-07-12 inventory.

1. `Engine` struct (`engine.rs:135-199`) → `Engine<B: GpuBackend>`; fields:
   `backend: B`, `swap_chain: SwapChain<B>`, four `B::Pipeline`, `Vec<B::Buffer<Image>>`,
   `ImageEntry{ texture: B::Texture }`.
2. Ctors (`engine.rs:208,218`): replace `Metal::new()` with `B::new()`; keep the
   `with_backend(backend: B, …)` variants (already take a backend by value).
3. `build_pipeline` (`engine.rs:1551-1594`) + `map_vertex_format`: collapse into
   `B::build_pipeline(desc, ShaderSource::…)`; delete the `library_from_source`/`device()`/
   `target_pixel_format()`/`PipelineOptions` plumbing (moves into Metal's impl).
4. `draw_frame` (`engine.rs:1239-1303`): `backend.begin_frame` → `B::begin_frame`;
   `frame.render_pass(&[Attachment{ texture: slot.target… }])` (now `Attachment<B>`);
   `pass.step(&Step{ pipeline.state()→&B::Pipeline, …buffer()→…handle(), …texture()→&B::Texture })`;
   `slot.target.texture()` dest → `Attachment.texture: &B::Target`; `slot.target.read_pixels()`
   (`engine.rs:1303`) via `GpuTarget`.
5. `encode_image_steps` (`engine.rs:1519-1534`): params `pass: &B::RenderPass`,
   `pipeline: &B::Pipeline`, `uniforms: &B::BufferHandle`, `images: &HashMap<u32, ImageEntry<B>>`,
   `image_instances: &[B::Buffer<Image>]`.
6. `present_parts` (`engine.rs:1441-1453`) + all of `present.rs`: **do not genericize** —
   they stay in the `#[cfg(target_os="macos")] impl Engine<Metal>` block (§3.4). Move
   `present_parts` and the `present` module under that specialized impl.
7. `Frame` readback type (`engine.rs:1477-1512`, the `{width,height,bgra}` struct — distinct
   from `metal::Frame`) is already backend-neutral; keep. `to_rgba` (`:1505`) unchanged.

## 6. Sizing (tightened, per file)

| Piece                                                                                                                                                        | File(s)                                                   | Est. LoC                                          | Notes                                                                      |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------- | ------------------------------------------------- | -------------------------------------------------------------------------- |
| Trait extension (`Step<B>`/`Attachment<B>`/`Draw`/`Primitive`, `GpuFrame`/`GpuRenderPass`/`GpuTarget`, `BufferHandle`, `begin_frame`/`build_pipeline`/`new`) | `gpu.rs`                                                  | ~180–240                                          | additive; T2 reviews the shape                                             |
| Metal moved behind the new methods                                                                                                                           | `metal/{mod,frame,render_pass,pipeline,target,buffer}.rs` | ~250–400 (mostly relocation, near-zero new logic) | `Step`/`Attachment` become `<Metal>`; `handle()`/`GpuTarget` thin wrappers |
| `Engine<B>` genericization                                                                                                                                   | `engine.rs`                                               | ~150–220 diff                                     | §5 items 1–5,7                                                             |
| Keep present.rs Metal-specialized                                                                                                                            | `engine.rs`, `present.rs`                                 | ~30                                               | move `present_parts` into the macOS impl                                   |
| **Software backend**                                                                                                                                         | `software/*` (new)                                        | ~550–800                                          | §3.5 + §4; additive, low collision                                         |
| Linux readback test + `#42` un-gates                                                                                                                         | `tests/`                                                  | ~150                                              | §8                                                                         |

Contentious surface = trait + Metal-relocation + `Engine<B>` (all T2 core). The Software
backend and its tests are additive.

## 7. Ownership & sequencing (unchanged recommendation, now concrete)

`engine.rs` (1426 LoC) is also touched by T2's **R6 slice 2** (viewport clip; PR #64 open as
of 2026-07-12). Landing this genericization concurrently invites the contention the file-claim
protocol warns about. Recommended split (T2 to accept/counter):

1. **PR-1 — trait extension + Metal behind it** (`gpu.rs` + `metal/*`). No behavior change;
   the Metal path still works. **T2 reviews/owns the trait shape.** This is the reviewable
   contract change; everything downstream is mechanical.
2. **PR-2 — `Software` backend** (new `software/*` + Linux readback test). Additive, no
   `engine.rs` edits beyond what PR-3 needs; can be built against PR-1's trait.
3. **PR-3 — `Engine<B>` genericization** (`engine.rs`, `present_parts` into the macOS impl).
   **Sequence after T2's R6 slice-2 (#64) merges** (or a shared rebase point).

T7 authors all three; T2 owns the PR-1 trait shape. Alternative if T2 prefers: T2 does PR-1,
T7 does PR-2+PR-3.

## 8. Verification plan

- **PR-1**: `cargo test -p qwertty-term-renderer` on macOS stays fully green (pure
  refactor); `--all-targets --target x86_64-unknown-linux-gnu` still compiles.
- **PR-2**: a Linux (and macOS) readback test drives a known snapshot through `Software` and
  asserts the **cell grid** matches the `Metal` path's for the same input (geometry/coverage;
  not pixel-exact vs Metal). Software backend has no GPU/display → runs on any CI runner.
- **PR-3**: the backend-agnostic acceptance suite (`dirty_equality` scenarios, ink placement)
  runs on Linux via `Software` — the [#42](https://github.com/joshka/qwertty-term/issues/42)
  un-gate. macOS Metal path unchanged and green throughout.

## 9. Open questions for T2 (the go/no-go)

1. Accept the **`Step<B>` associated-type rebinding** (§3.1) and the **`BufferHandle`** choice
   (§3.2 option A)? This is the one load-bearing API decision.
2. Accept **presentation stays Metal-specialized** (§3.4) — `Engine<B>` generic only for
   draw/update/readback, `present.rs` unchanged?
3. Ownership: T7-authors-all with T2 owning PR-1's shape (recommended), or T2 owns PR-1?
4. Sequencing vs R6 slice-2 (#64): after it merges, or a coordinated rebase point?
