# Renderer R2: frame lifecycle, presentation, pipelines, swap chain

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse --short 2da015cd6`). Scope: the frame
lifecycle (`src/renderer/metal/Frame.zig`, 131 lines), render-pass encoding
(`RenderPass.zig`, 229), pipeline construction (`Pipeline.zig`, 204), the
presentation layer (`IOSurfaceLayer.zig`, 183), and the `SwapChain` +
`FrameState` machinery inside the comptime-generic renderer
(`src/renderer/generic.zig`, ~lines 230-430), plus the `present` /
`loopEnter` / `displayCallback` / `beginFrame` hooks in `Metal.zig`. Rust ports
live at `crates/ghostty-renderer/src/metal/{frame,render_pass,pipeline,layer}.rs`
and `crates/ghostty-renderer/src/swap_chain.rs`, with frame-lifecycle entry
points (`begin_frame`, `new_pipeline`, `target_pixel_format`) added to
`metal/mod.rs`.

This is chunk R2: it replaces R1's uninhabited placeholder enums
(`pub enum Frame {}` / `RenderPass {}` / `Pipeline {}`) with the real Metal
types, and adds the presentation layer + swap chain. It builds strictly on
R1's resources (`Target`, `Buffer(T)`, `Texture`, `Sampler`) and the frozen
wire structs; it does **not** touch `src/shaders/` (chunk R3 owns the MSL
sources + production pipeline table), `ghostty-vt` tests, or `ghostty-font`.

## `Frame.zig` -> `frame.rs` (command buffer + completion cycle)

Upstream `Frame` owns one `MTLCommandBuffer` and a completion block. `begin`
grabs a command buffer from the queue and constructs the block (bundling
`renderer`, `target`, and a `sync` flag). `complete(sync)` has two paths:

- **async** (`sync = false`): register the block via `addCompletedHandler:`,
  then `commit`. When the GPU finishes, `bufferCompleted` runs on a
  Metal-owned thread: it reads the command-buffer `status` (→ `Health`:
  `.error` is `unhealthy`, else `healthy`), presents the target *if healthy*
  (`renderer.api.present(target, sync)`), and calls `renderer.frameCompleted`.
- **sync** (`sync = true`): `commit`, then `waitUntilCompleted`, then invoke
  the block directly (with `sync = true`) on the calling thread.

**Port shape.** The Rust `Frame` keeps the command buffer but drops the direct
coupling to the generic `Renderer` (not ported yet). Instead of holding
`renderer` + `target`, it holds a boxed [`FrameCompletion`] =
`Box<dyn Fn(Health, bool) + Send>` — the present-and-report work, supplied by
the swap chain / window host. This is the one deliberate structural
divergence: it keeps the frame lifecycle self-contained so R2 can land and be
tested before R4's `Renderer` exists. The two completion paths are otherwise
1:1:

- async uses `block2::RcBlock` for the `addCompletedHandler:` block. `RcBlock`
  copies on registration and Metal releases the copy after invocation, so the
  Rust-side reference is dropped immediately. The handler reads
  `buffer.status()` → `Health` and calls the completion with `sync = false`.
- sync does `commit` + `waitUntilCompleted` + inline completion with
  `sync = true`.

`complete` is idempotent (takes the hook out of an `Option`), so a
double-complete is a no-op — matching the "can no longer be used after"
contract without a type-state.

**`Health`** is ported as a 2-variant enum (upstream's `renderer.Health` has
exactly the two states `Frame` distinguishes here).

## `RenderPass.zig` -> `render_pass.rs` (encoder + instanced draws)

`begin` builds an `MTLRenderPassDescriptor`: for each attachment set
`loadAction` (`clear` iff a clear color is present, else `load`),
`storeAction = store`, the destination `texture`, and (if clearing) the
`clearColor`. Then it opens an `MTLRenderCommandEncoder`.

`step` binds a pipeline state and its resources, then issues
`drawPrimitives:vertexStart:vertexCount:instanceCount:`. The **buffer-index
convention** is load-bearing and matches plan decision 5 / the frozen
`wire` convention:

- **index 0** = the vertex/instance buffer, set as **both** a vertex buffer
  and a fragment buffer,
- **index 1** = uniforms, also set on both stages,
- **indices 2+** = the extras (`s.buffers[1..]`), both stages.

Upstream binds to both stages deliberately ("consistent and predictable, and
we need to treat the uniforms as special because of OpenGL"); the port keeps
this exactly. Textures bind to both stages by position; samplers to fragment
sampler slots by position. A zero-instance draw is skipped (upstream's early
return). `complete` sends `endEncoding`.

**Port shape.** Upstream's attachment `target` is a tagged union of
`Texture`/`Target`; both expose an `MTLTexture`, so the Rust `Attachment`
collapses to a `&ProtocolObject<dyn MTLTexture>` (the caller passes
`target.texture()`). The three buffer roles become named fields
(`vertex` / `uniforms` / `extras`) rather than one slice with index magic, so
the convention is enforced by the type rather than by comment. `RenderPass`
gets a `Drop` that ends encoding if `complete` wasn't called — a dropped pass
must still close its encoder or the command buffer can't be committed
(no upstream analogue; Zig's `defer` covers this at call sites).

## `Pipeline.zig` -> `pipeline.rs` (explicit vertex tables, premult-alpha)

`init` looks up the vertex/fragment functions by name in their libraries,
optionally builds a vertex descriptor, configures color-attachment blending,
and asks the device for an `MTLRenderPipelineState`.

**Vertex descriptor: explicit tables, no comptime reflection.** Upstream's
`autoAttribute` is a `comptime` loop over a Zig struct's fields, mapping each
field type to an `MTLVertexFormat` and using `@offsetOf` for the offset. Rust
has no comptime field reflection, so the port takes an explicit
[`VertexLayout`] = stride + a slice of [`VertexAttribute`] (format + offset),
with an attribute's shader index being its slice position. All attributes come
from buffer index 0 (upstream hard-codes `bufferIndex = 0`). [`VertexFormat`]
enumerates exactly the `autoAttribute` `switch` targets the renderer uses
(`uchar`/`uchar4`/`ushort2`/`short2`/`float`/`float2`/`float4`/`int`/`int2`/
`uint`/`uint2`/`uint4`). The step function is `per_vertex` (default) or
`per_instance` (the cell shaders).

**Blending: premultiplied alpha.** When an attachment enables blending
(upstream default `true`), both RGB and alpha use `add` with
`source = one`, `dest = one_minus_source_alpha` — upstream's "we always use
premultiplied alpha blending for now." When disabled (the full-screen
custom-shader passes), no blend state is set. The attachment pixel format must
match the frame's render target (`Metal.target_pixel_format`, now `pub`), which
upstream threads through `initShaders` from `blending.isLinear()`.

**Library from source.** `library_from_source` ports
`newLibraryWithSource:options:error:` — the runtime-compile path (upstream uses
it for custom shaders; R3 will use it or the build-time metallib for the
production shaders). R2's tests compile a private inline MSL pair to exercise
pipeline creation without depending on R3's shaders. `checkError` becomes
`Result<_, Retained<NSError>>` handling with the localized description logged.

## `IOSurfaceLayer.zig` -> `layer.rs` (`define_class!` CALayer subclass)

Presentation is plan decision 2: assign the target's IOSurface to a plain
`CALayer`'s `contents`. Upstream subclasses `CALayer` at runtime
(`allocateClassPair`) to override two methods; the objc2 0.6 declarative
equivalent is **`define_class!`** (the successor to `declare_class!`, which the
chunk brief names). The subclass:

- **overrides `display`** — CoreAnimation's redraw hook (fired during a live
  resize). It forwards to a caller-installed display callback. Upstream stores
  a `display_cb` fn pointer + a `display_ctx` void pointer in two ivars; the
  port stores a single `Box<dyn Fn()>` behind a `Cell<Option<*mut …>>` ivar
  (the closure subsumes the context). The pointer is only touched on the main
  thread (where `display` and `set_display_callback` run), so the `Cell` is
  sound; `Drop` reclaims the box.
- **overrides `actionForKey:`** — returns `NSNull` for every key, disabling all
  implicit CALayer animations so a `contents` swap shows immediately with no
  cross-fade (upstream returns `[NSNull null]`).

`contentsGravity` is set to top-left on init (no stretch during resize before a
new frame — upstream comment).

**Threading of `contents` (main-thread dispatch).** Upstream `setSurface`
retains the surface, checks `NSThread.isMainThread`, and either runs the
assignment inline or `dispatch_async`s it to the main queue; the dispatched
callback re-checks the surface size against `bounds * contentsScale` and
discards a mismatched surface (a late async frame vs. a sync resize frame —
jank guard). `setSurfaceSync` assigns directly (resize path, already
main-thread). The port mirrors this: `MainThreadMarker::new().is_some()` for
the thread check, `dispatch2`'s `DispatchQueue::main().exec_async` for the
dispatch, and the same size guard. Because `exec_async` requires `Send` but
`Retained<CALayer>` / `CFRetained<IOSurfaceRef>` are not `Send`, the retained
handles are ferried in a `MainThreadHandles` wrapper with a hand-written
`unsafe impl Send` — sound because the block runs *only* on the main thread and
does nothing with the handles off it (objc/CF refcounting is itself
thread-safe; this mirrors upstream passing raw `id`/`*IOSurface` into the
block). The handles are captured *whole* (via a `run(self)` method) to defeat
Rust 2021 disjoint closure capture, which would otherwise bypass the wrapper's
`Send`.

## `SwapChain` (generic.zig) -> `swap_chain.rs`

Upstream's `SwapChain` lives inside the comptime-generic `Renderer`; the port
lifts it into a standalone type parameterized over the [`GpuBackend`] trait.

**Slots + semaphore.** `buf_count = GraphicsAPI.swap_chain_count` (Metal: 3)
`FrameState` slots, each owning the per-frame state that would race between CPU
and GPU: the uniform/cell/cell-bg buffers, the grayscale + color atlas
textures, and the render target. A `std.Thread.Semaphore` with `buf_count`
permits gates availability: `nextFrame` waits a permit and advances the
round-robin index; `releaseFrame` posts. `deinit` drains all permits
(waits `buf_count` times) before freeing GPU state, guarded by a `defunct`
flag against double-free.

**Port shape.** `FrameSlot<B: GpuBackend>` holds the same fields reduced to
what exists after R1/R2 (the custom-shader state and bg-image buffer are R6+;
they slot in later without changing the API). Everything starts at size 1 and
resizes on demand (upstream). `std` has no stable semaphore, so a
`Mutex<usize>` + `Condvar` stands in. `next_frame` returns a `FrameGuard` that
borrows the slot and carries an `Arc<Semaphore>`; `release`/`detach` control
where the permit is returned. A `Drop` safety net posts the permit if a
sync-mode caller forgets, so the chain can't deadlock.

**Two modes behind one API (plan decision 3).** `SwapChainMode` selects live
permits: `Sync` = 1 (day-one degenerate double-buffering: one slot, each frame
completed with `waitUntilCompleted`, permit released inline) and `Async` =
`SWAP_CHAIN_COUNT` (real triple buffering: the frame's completion handler posts
the permit via `SwapChain::release_hook`, and the guard is `detach`ed so its
`Drop` doesn't double-release). *All* slots are always allocated so a mode
switch needs no reallocation. The slot-handout API is identical across modes, so
a window host flips modes without renderer changes — exactly what plan decision
3 ("day-one degenerate mode permits=1 + `waitUntilCompleted` is acceptable")
requires, with the async path already behind the same shape.

**Timer pacing hook.** `TimerPacer` (plan decision 3, day one) ticks a draw
callback every 8-16ms on a background thread — deliberately backend-agnostic and
thread-based (no run loop), so it works headless and in tests. `CVDisplayLink`
(`objc2-core-video`) is the later swap-in behind this same "tick a draw" shape;
the second pacing source is the CALayer `display` callback (resize-driven), which
lives in `layer.rs`.

## `Metal.zig` present/loop hooks -> `metal/mod.rs` additions

- `begin_frame(completion)` ports `Metal.beginFrame` / `Frame.begin`.
- `new_pipeline(opts)` dispatches `Pipeline::new` against the device (R3 owns
  the production pipeline table + shader source; this is the device-bound entry
  point they call).
- `target_pixel_format` is promoted to `pub` so pipeline color attachments can
  match the target format (upstream threads the same choice through
  `initShaders`).

`present` itself is folded into the [`FrameCompletion`] hook (the swap chain /
window host calls `IOSurfaceLayer::set_surface[_sync]`); `loopEnter` /
`displayCallback` wiring and `contentsScale` from a live view are R5 (need a
window).

## Coordination with R3 (frozen wire structs only)

R2 and R3 share `crates/ghostty-renderer`, split strictly by file: R3 owns
`src/shaders/` (MSL sources + the production pipeline-description table +
color-math tests), R2 owns `metal/{frame,render_pass,pipeline,layer}.rs` +
`swap_chain.rs` + `gpu.rs`/`mod.rs` additions. The contract between them is the
frozen `wire` structs. Pipeline **descriptions** — vertex-function names,
per-struct vertex layouts — can be constructed from string names + the frozen
struct field offsets, which is what R2's `pipeline_with_explicit_vertex_layout`
test does (a `CellText`-shaped layout: `ushort2` at offset 20, `uchar4` at 24,
32-byte stride). The production table that names the real shader functions lives
with R3's shaders.

## Tests (11 added; 54 in the crate on macOS)

All GPU-touching tests reuse R1's `test_metal()` skip-gracefully pattern
(`SKIP:` + early return when no non-headless Metal device). New:

- `frame::clear_color_readback` — **the R2 acceptance test**: a pipeline-less
  render pass with a clear color onto an IOSurface target, `complete(true)`
  (sync / `waitUntilCompleted`), then read the IOSurface bytes back and assert
  the clear color (BGRA order, ±1 tolerance) on every pixel. Also asserts the
  completion hook ran with `Health::Healthy` and `sync = true`.
- `frame::render_pass_encodes_three_vertex_draw` — a real runtime-compiled
  pipeline draws one full-screen triangle (3 vertices) over the target; readback
  asserts a uniform magenta fill (proves encode + pipeline + draw end-to-end).
- `frame::pipeline_creation_from_inline_msl` — pipeline compiles against a
  private 5-line inline MSL pair (no dependency on R3's `src/shaders/`).
- `frame::pipeline_with_explicit_vertex_layout` — a vertex descriptor built from
  an explicit `CellText`-shaped attribute table compiles into a pipeline
  (proves the `autoAttribute` replacement).
- `layer::layer_creates_and_disables_animations` — the `define_class!` subclass
  registers, is a `CALayer`, and `actionForKey:` returns `NSNull`.
- `layer::display_callback_fires_and_clears` — invoking `display` runs the
  installed callback; clearing it silences further ticks.
- `swap_chain::sync_mode_serializes_frames` — sync mode has one live permit; a
  second `next_frame` blocks until the first guard is released (proven by racing
  a background acquire against the release).
- `swap_chain::async_mode_has_full_permits_and_release_hook_posts` — async mode
  exposes `SWAP_CHAIN_COUNT` permits and its release hook posts the semaphore.
- `swap_chain::semaphore_blocks_until_posted` + `timer_pacer_ticks_then_stops` —
  the semaphore + timer primitives (no GPU needed).

## Deferrals to R3+

- **Production shader library + pipeline table** — R3 owns `src/shaders/`
  (MSL sources, the pipeline-description table naming real functions, color-math
  golden tests). R2 proves the machinery with private inline shaders.
- **Cell engine** (`rebuildCells`/`addGlyph`/`updateFrame`/`drawFrame` cores)
  that drives the swap chain and populates `FrameSlot` buffers — R4.
- **Window host** — `loopEnter`/`displayCallback` wiring, `contentsScale` from a
  live `NSView`, and the `IOSurfaceLayer` mounted in an `NSView` — R5.
- **CVDisplayLink pacing** (`objc2-core-video`) behind `TimerPacer`'s "tick a
  draw" shape — later.
- **Config-derived `blending: AlphaBlending`** — still the R1 `linear_blending`
  stand-in.
- **Async-mode present** wiring: the [`FrameCompletion`] hook is the seam where
  `IOSurfaceLayer::set_surface` gets called; R2 exercises the sync path end to
  end (readback), with the async path structurally present and unit-tested at
  the swap-chain level.
