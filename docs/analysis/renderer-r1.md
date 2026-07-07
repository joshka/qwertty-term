# Renderer R1: `GpuBackend` trait, Metal context/resources, frozen wire structs

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse --short 2da015cd6`). Scope: the *implicit*
`GraphicsAPI` interface upstream's generic renderer requires
(`src/renderer/generic.zig`), the Metal implementation of its resource half
(`src/renderer/Metal.zig` 458 lines + `src/renderer/metal/{Target,Texture,
Sampler,buffer}.zig` = 108+202+64+185 lines), and the GPU wire structs
(`src/renderer/metal/shaders.zig`, the subset defining `Uniforms`/`CellText`/
`CellBg`/`Image`/`BgImage`). Rust ports live at `crates/ghostty-renderer/src/
{gpu,wire}.rs` + `crates/ghostty-renderer/src/metal/{mod,target,texture,
sampler,buffer}.rs`.

This is chunk R1: resource creation only. The frame lifecycle
(`Frame`/`RenderPass`/`Pipeline`, present, swap-chain pacing), the
presentation layer (`IOSurfaceLayer`), shader/pipeline construction, and
threading hooks are all R2+ — declared here as trait-shape placeholders but
not implemented (see "Deferrals to R2+").

## The `GpuBackend` trait (`gpu.rs`) — making an implicit interface explicit

Upstream's `generic.zig` is a comptime-generic renderer parameterized over a
`GraphicsAPI` type. That type is never written down as an interface; it's
validated *by convention* — `metal/` and `opengl/` contain the exact same
file set (`Target`, `Frame`, `RenderPass`, `Pipeline`, `Sampler`, `Texture`,
`buffer`, `shaders`), and `generic.zig` consumes them as `GraphicsAPI.Target`,
`GraphicsAPI.Buffer(T)`, etc. Rust can't do structural comptime duck-typing,
so R1 makes the contract explicit as [`GpuBackend`] with associated types —
one per mirrored file:

- `type Target` / `type Frame` / `type RenderPass` / `type Pipeline` —
  the four presentation/encoding types. Only `Target` has methods in R1; the
  other three are declared for trait-shape completeness (they *are* 3 of the 8
  mirrored files, so leaving them off would mis-shape the trait), and the
  Metal impls are **uninhabited placeholder enums** (`pub enum Frame {}`)
  until R2 gives them constructors.
- `type Buffer<T: Copy + 'static>: GpuBuffer<T>` — a *generic* associated
  type, mirroring `buffer.zig`'s `Buffer(T)`. The `GpuBuffer<T>` supertrait
  bound carries the growth/sync API.
- `type Texture: GpuTexture` / `type Sampler` — the sampled-resource types.
- `type Error` — one error type for all fallible ops (Metal's is
  `MetalError`).
- `const SWAP_CHAIN_COUNT: usize` — upstream `Metal.swap_chain_count = 3`
  (triple buffering; consumed by R2's SwapChain).

**Resource creation folds upstream's `*Options` factory methods into
constructor methods.** Upstream expresses "how to make a buffer/texture/
sampler for *this* device" as per-backend factory methods on the API struct
(`bufferOptions()`, `textureOptions()`, `samplerOptions()`,
`imageTextureOptions(format, srgb)`) — each just bundles the device pointer
with backend-specific enum values. The Rust trait folds those into
`new_buffer`/`new_texture`/`new_sampler`/`new_target` on the backend itself:
the backend already *is* the device handle, and the remaining knobs are
backend-agnostic option structs ([`TextureOptions`], [`SamplerOptions`]). The
format/srgb split that `imageTextureOptions` encodes is carried by
[`TextureFormat`] (6 variants: the exact set upstream constructs —
`initAtlasTexture`'s `r8unorm`/`bgra8unorm_srgb`, `imageTextureOptions`'s
gray/rgba/bgra × srgb, and render targets' `bgra8unorm[_srgb]`).

**`GpuBuffer<T>` growth contract** (the seam R4's cell engine relies on):
`sync(data)` treats `data` as the complete new contents, growing (never
shrinking) at double the required size if it doesn't fit; `sync_from_slices`
is the gather variant of upstream `syncFromArrayLists` (the renderer's
per-row cell lists). `GpuTexture::replace_region` is the CPU streaming path
(upstream `replaceRegion`).

## `Metal.zig` -> `metal/mod.rs` (context init, R1 subset)

**`Metal` struct** carries the R1 subset: `device` + `queue` +
`default_storage_mode` + `max_texture_size` + `linear_blending`. The
presentation layer (`layer: IOSurfaceLayer`) and the config-derived
`blending: AlphaBlending` field are R2+/R5 — until config plumbing lands,
`linear_blending: bool` stands in for `blending.isLinear()` (upstream default
`.native` is non-linear, i.e. `false`).

**`chooseDevice`** (macOS arm) ported 1:1: iterate `MTLCopyAllDevices`, skip
headless GPUs (not connected to a display), and stop at the first removable
(eGPU — user probably wants it) or low-power (integrated — better for
battery/thermals) device. Falls back to the last non-headless device.

**`queryMaxTextureSize`** ported from Apple's feature-set tables: `Apple10`
-> 32768, `Apple3` -> 16384, else 8192.

**`api.zig` is deliberately NOT ported.** Upstream's `metal/api.zig` is
hand-written Objective-C selector bindings (msgSend wrappers). `objc2-metal`
provides the same bindings generated from the SDK headers, so R1 uses those
directly and drops `api.zig` entirely (the chunk spec called for this:
"`api.zig` DELETED in favor of objc2-metal").

**Storage mode + resource options.** `hasUnifiedMemory()` -> `Shared`
(Apple Silicon), else `Managed` (discrete GPUs, which can't use `Shared` for
GPU-visible resources). Every upstream resource call site uses
`CPUCacheModeWriteCombined | <storage mode>` (CPU writes, never reads) —
folded into `Metal::resource_options()`.

## `metal/Target.zig` -> `metal/target.rs` (IOSurface-backed render target)

The load-bearing decision (plan decision 2): render into an IOSurface-backed
`MTLTexture`, present by assigning the IOSurface to a plain `CALayer`'s
`contents` — **not** CAMetalLayer/nextDrawable. R1 builds the target half;
presentation (`IOSurfaceLayer`) is R2.

`Target::new` ports `Target.init`:

1. Create the backing IOSurface via `IOSurfaceRef::new` with a
   `{width, height, pixelFormat=32BGRA, bytesPerElement=4}` property
   dictionary. `32BGRA` is the fourcc `'BGRA'` (`kCVPixelFormatType_32BGRA`),
   computed as `u32::from_be_bytes(*b"BGRA")`.
2. Tag the surface with the **Display P3** color space
   (`CGColorSpace::with_name(kCGColorSpaceDisplayP3)`, serialized to a
   property list via `property_list()` and set as `kIOSurfaceColorSpace`).
   This is the "Apple-style" alpha blending upstream comments on: rendering
   text in the display's color space with converted colors reduces blending
   artifacts.
3. Create an `MTLTextureDescriptor` (RenderTarget usage, write-combined +
   storage mode) and back it with the IOSurface via
   `newTextureWithDescriptor:iosurface:plane:` (plane 0). Rendering goes
   through the Metal texture view; presentation through the surface — same
   pixels, no copy.

**Crate/feature note (build fix from the partial state):** the
`newTextureWithDescriptor:iosurface:plane:` binding is gated behind
`objc2-metal`'s `objc2-io-surface` feature (plus `MTLResource`/`MTLTexture`,
both default), which is **not** in `objc2-metal`'s default feature set. R1
enables it explicitly in `Cargo.toml`
(`objc2-metal = { features = ["objc2-io-surface"] }`). `objc2-io-surface` was
chosen over the servo `io-surface` crate because its `IOSurfaceRef` is the
exact parameter type of that binding (zero conversion glue) and it shares
objc2's memory-management model.

**`read_pixels` (new, no upstream equivalent).** The M3 verification strategy
is offscreen readback ("render into the IOSurface target, read pixels,
assert") — upstream verifies with eyes on a window, so there's no port
source. `read_pixels` locks the IOSurface read-only, copies `height` rows of
`width*4` BGRA bytes from the base address (stripping row-stride padding), and
unlocks. Correct for CPU writes (`replaceRegion`) immediately, and for GPU
renders after `waitUntilCompleted` (R2's concern) since IOSurface/shared/
managed textures are coherent once GPU work completes.

## `metal/Texture.zig` -> `metal/texture.rs`

`Texture::new` ports `Texture.init`: descriptor (format/width/height/usage/
resource-options) -> `newTextureWithDescriptor` -> optional initial upload via
`replace_region`. `replace_region` ports `replaceRegion` (CPU streaming path).

**Divergence — bounds check.** Upstream documents `replaceRegion` "does NOT
check the dimensions of the data". The port asserts `data.len() >= width *
height * bpp` before handing Metal a raw pointer, because an out-of-bounds
read here is UB in Rust (not just a rendering glitch as it is in the Zig,
which still passes a valid-if-wrong pointer).

**`bpp_of` ports `bppOf`** with the same size-class groups and the same
"could be memory corruption" panic for unknown formats. **Divergence:**
upstream returns `128` for the 128-bit formats (bits, not bytes — an upstream
bug, harmless there because no call site uses those formats); this port
returns `16` (bytes), the evident intent.

## `metal/Sampler.zig` -> `metal/sampler.rs`

Straight port of `Sampler.init`: descriptor with min/mag filter +
s/t address mode -> `newSamplerStateWithDescriptor`. The one upstream call
site (custom shaders) uses linear/linear + clamp-to-edge ("match Shadertoy
behaviors"), exercised by the test.

## `metal/buffer.zig` -> `metal/buffer.rs` (growable typed buffer)

`Buffer(T)` ports the "prealloc/grow/sync" storage. `sync` copies `data` as
the complete new contents; `ensure_capacity` reallocates at `req_bytes * 2`
on overflow; managed-storage buffers get `didModifyRange` after CPU writes.
`sync_from_slices` ports `syncFromArrayLists` (gather from per-row cell
lists).

**Two deliberate divergences from the Zig, both fixes:**

1. **Growth allocation size.** Upstream computes `size = req_bytes * 2` then
   passes `size * @sizeOf(T)` to `newBufferWithLength:` — a double
   multiplication that over-allocates by an extra `sizeOf(T)` factor. The port
   allocates `req_bytes * 2` bytes (the evident intent).
2. **`len` accuracy.** Upstream never updates its `len` field after a growth
   reallocation (it reads the true capacity back from the MTLBuffer's
   `length` property when needed). The port keeps `len` accurate across
   reallocation.

A build fix from the partial state: `device.retain()` requires `objc2::
Message` in scope (the method comes from that trait, not inherent).

## Wire structs (`wire.rs`) — FROZEN after this chunk

Every struct is a bit-for-bit port of the `extern struct` definitions in
`shaders.zig`, themselves the mirror of the MSL argument structs in
`shaders.metal`. R1 freezes them; R3 ports the MSL that reads them; R4 emits
into them. The layout tests are the executable form of the freeze.

Buffer index convention (plan decision 5): index 0 = vertex/instance data,
index 1 = uniforms, 2+ = extras.

**These live in one backend-agnostic module**, not duplicated per backend.
Upstream duplicates them in `metal/shaders.zig` and `opengl/shaders.zig`; the
Rust port shares one definition (a future OpenGL backend must keep matching,
which upstream's GLSL mirrors already do by construction).

Zig -> Rust layout translation notes:

- `math.Mat` = `[4]@Vector(4, f32)` (each column a 16-byte-aligned `float4`)
  -> [`Mat`], `#[repr(C, align(16))]` over `[[f32; 4]; 4]` (size 64, align 16).
- `grid_padding: [4]f32 align(16)` -> [`AlignedF32x4`], an explicit aligned
  wrapper (bare `[f32; 4]` is only 4-aligned and would land at offset 84
  instead of 96 inside [`Uniforms`]).
- `packed struct(u8)` bitfields -> `#[repr(transparent)]` newtypes over `u8`
  with LSB-first bit constants ([`PaddingExtend`], [`CellTextBools`],
  [`BgImageInfo`]).
- `bool` in an `extern struct` is 1 byte; Rust `bool` matches.

**Frozen layouts (asserted by `wire::tests`):**

| Struct     | Size | Align | Notes                                         |
| ---------- | ---: | ----: | --------------------------------------------- |
| `Uniforms` |  144 |    16 | offsets 0/64/72/80/96/112/116/120/124/128/132 |
| `CellText` |   32 |     8 | **upstream's own `@sizeOf == 32` assertion**  |
| `CellBg`   |    4 |     1 | `= [4]u8`                                     |
| `Image`    |   40 |     4 | offsets 0/8/16/32                             |
| `BgImage`  |    8 |     4 | `opacity: f32` @0, `info: Info` @4            |
| `Mat`      |   64 |    16 | column-major `float4x4`                       |

`CellText` is the size-critical one — upstream carries an inline
`expectEqual(32, @sizeOf(CellText))` test ("minimizing the size of this struct
is important"); the port carries that assertion forward, plus a full
per-field offset test. `Mat::ortho2d` ports `math.ortho2d` with a golden-value
test on the projection formula.

## Tests (44 total in the crate; R1 adds the GPU + wire coverage)

Metal-touching tests **skip gracefully** when no non-headless Metal device is
present (`test_metal()` returns `None`, prints `SKIP:`, the test returns
early) — required for CI machines without a GPU. On a dev Mac they run:

- `metal::context_init_device_and_queue` — device/queue exist and answer
  messages; `max_texture_size` is a feature-set value; storage mode matches
  the unified-memory rule.
- `metal::backend_creates_all_resource_types` — target + buffer + texture +
  sampler all construct via the `GpuBackend` methods.
- `target::target_surface_properties` — IOSurface is 32BGRA, 4 bpe, width/
  height correct, row stride ≥ packed width; texture view matches.
- `target::target_upload_and_readback_roundtrip` — **the R1 acceptance
  test**: upload distinct-per-byte BGRA pixels via `replaceRegion`, read them
  back through the IOSurface base address, assert byte-for-byte equality
  (catches any stride/offset mistake).
- `texture::texture_upload_readback_roundtrip` +
  `texture::replace_region_partial_update` — grayscale (atlas-style) upload
  and partial-region update, read back via `getBytes` (shared storage only;
  skips on managed/discrete where readback needs a blit sync — R2).
- `texture::bpp_of_matches_upstream_groups` + `bpp_of_invalid_panics`.
- `buffer::{buffer_init_and_fill, sync_grows_at_double_required_size,
  sync_smaller_leaves_remainder_untouched_and_never_shrinks,
  sync_from_slices_concatenates_in_order, sync_wire_structs}` — the growth
  semantics + the R4 hot path (instance buffers of `CellText`, asserting
  capacity stays a multiple of the frozen 32-byte stride).
- `sampler::sampler_creation`.
- `wire::*` (7 tests) — the layout freeze + `ortho2d` golden values.

## Deferrals to R2+

- **`Frame`/`RenderPass`/`Pipeline`** — declared as `GpuBackend` associated
  types and uninhabited placeholder enums in the Metal backend; no
  constructors/methods until R2 (frame lifecycle, present, swap-chain pacing)
  and R2/R3 (pipeline + shader library).
- **`IOSurfaceLayer`** (presentation: assign IOSurface to a `CALayer`'s
  `contents`, `declare_class!` subclass hooking `display`/`actionForKey:` to
  disable implicit animations) — R2.
- **SwapChain semaphore** (permits=3 steady state; day-one degenerate
  permits=1 + `waitUntilCompleted`) — R2.
- **Shader library** (`newLibraryWithSource` at runtime, metallib via
  `xcrun metal` in build.rs later) + pipeline state construction — R3.
- **Config-derived `blending: AlphaBlending`** — currently the
  `linear_blending: bool` stand-in; real config plumbing R2+.
- **View-attachment / `contentsScale` wiring** in upstream `Metal.init` —
  needs a window, R5.
- **Threading hooks** (`loopEnter`, `threadEnter`, …) — R2+.
- **GPU-render readback** (as opposed to CPU-write readback, which works
  today) — needs `waitUntilCompleted` from the R2 frame path; for managed/
  discrete storage it additionally needs a blit sync, which is why the
  `getBytes`-based texture readback tests skip on non-shared storage.
