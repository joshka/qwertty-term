//! The GPU backend abstraction: what a renderer needs from a graphics API.
//!
//! Port of the *implicit* interface Ghostty's generic renderer
//! (`src/renderer/generic.zig`, commit `2da015cd6`) requires of its comptime
//! `GraphicsAPI` parameter. Upstream validates this interface by convention:
//! `metal/` and `opengl/` contain the exact same file set
//! (`Target/Frame/RenderPass/Pipeline/Sampler/Texture/buffer/shaders`), and
//! `generic.zig` consumes them via `GraphicsAPI.Target`, `GraphicsAPI.Buffer`
//! etc. Rust makes that contract explicit as [`GpuBackend`] with associated
//! types.
//!
//! What upstream expresses as per-backend `*Options` factory methods
//! (`bufferOptions()`, `textureOptions()`, `samplerOptions()`,
//! `imageTextureOptions(format, srgb)` — all of which just bundle the
//! device pointer with backend-specific enum values) the Rust trait folds
//! into constructor methods on the backend itself (`new_buffer`,
//! `new_texture`, `new_sampler`, `new_target`): the backend already *is* the
//! device handle, and the remaining knobs ([`TextureOptions`],
//! [`SamplerOptions`]) are backend-agnostic.
//!
//! Chunk scope (R1): resource creation only. `Frame`, `RenderPass` and
//! `Pipeline` are declared as associated types so the trait shape is
//! complete (they are 3 of upstream's 8 mirrored files), but no methods
//! reference them yet — frame lifecycle (`beginFrame`, present, swap-chain
//! pacing) is chunk R2, shader/pipeline construction is R2/R3. Threading
//! hooks (`loopEnter`, `threadEnter`, …) and the presentation layer
//! (`IOSurfaceLayer`) are likewise R2+.

use std::error::Error;

/// A GPU graphics API backend (Metal now; OpenGL/WebGL are future ports).
///
/// Mirrors upstream `GraphicsAPI` (see module docs). One value of this type
/// owns the device/queue context; all resources are created through it.
pub trait GpuBackend: Sized {
    /// Number of frames in flight. Upstream: `Metal.swap_chain_count = 3`
    /// (triple buffering; consumed by `generic.zig`'s SwapChain in R2).
    const SWAP_CHAIN_COUNT: usize;

    /// Error type for all fallible backend operations.
    type Error: Error + Send + Sync + 'static;

    /// A presentable render target (upstream `metal/Target.zig`: an
    /// IOSurface-backed `MTLTexture`).
    type Target;

    /// One in-flight frame's encoding context (upstream `metal/Frame.zig`).
    /// Declared for trait-shape completeness; methods land in chunk R2.
    type Frame;

    /// A single render pass within a frame (upstream
    /// `metal/RenderPass.zig`). Methods land in chunk R2.
    type RenderPass;

    /// A compiled render pipeline (upstream `metal/Pipeline.zig`). Methods
    /// land in chunks R2/R3 together with the shader library.
    type Pipeline;

    /// A typed, growable GPU buffer (upstream `metal/buffer.zig`
    /// `Buffer(T)`).
    ///
    /// `T: Copy + 'static` — plain bytes-copyable instance/uniform data; in
    /// practice the frozen wire structs from [`crate::wire`].
    type Buffer<T: Copy + 'static>: GpuBuffer<T, Error = Self::Error>;

    /// A sampled texture (upstream `metal/Texture.zig`).
    type Texture: GpuTexture<Error = Self::Error>;

    /// A texture sampler (upstream `metal/Sampler.zig`).
    type Sampler;

    /// Maximum 2D texture width/height supported by the device. Surface
    /// sizes must be clamped to this (upstream `queryMaxTextureSize`).
    fn max_texture_size(&self) -> u32;

    /// Create a render target which can be presented by this API
    /// (upstream `Metal.initTarget`).
    fn new_target(&self, width: usize, height: usize) -> Result<Self::Target, Self::Error>;

    /// Create a buffer with room for `len` values of `T`, contents
    /// uninitialized (upstream `Buffer.init`).
    fn new_buffer<T: Copy + 'static>(&self, len: usize) -> Result<Self::Buffer<T>, Self::Error>;

    /// Create a buffer initialized with `data` (upstream `Buffer.initFill`).
    fn new_buffer_with_data<T: Copy + 'static>(
        &self,
        data: &[T],
    ) -> Result<Self::Buffer<T>, Self::Error>;

    /// Create a texture, optionally uploading initial `data`
    /// (`width * height * format.bytes_per_pixel()` bytes; upstream
    /// `Texture.init`).
    fn new_texture(
        &self,
        options: TextureOptions,
        width: usize,
        height: usize,
        data: Option<&[u8]>,
    ) -> Result<Self::Texture, Self::Error>;

    /// Create a sampler (upstream `Sampler.init`).
    fn new_sampler(&self, options: SamplerOptions) -> Result<Self::Sampler, Self::Error>;
}

/// Typed GPU data storage that can be preallocated, grown, and synced from
/// CPU-side slices. Port of the `Buffer(T)` wrapper in upstream
/// `metal/buffer.zig`.
pub trait GpuBuffer<T: Copy> {
    type Error: Error + Send + Sync + 'static;

    /// Allocated capacity, in number of `T`s (upstream field `len`; kept
    /// up to date across reallocation — see the Metal impl's divergence
    /// note).
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Sync new contents to the buffer; `data` is the complete new
    /// contents. Grows (never shrinks) the underlying allocation if `data`
    /// doesn't fit — see upstream's growth semantics: reallocate at double
    /// the required size. If `data` is smaller than the buffer, the
    /// remaining contents are left untouched. (Upstream `Buffer.sync`.)
    fn sync(&mut self, data: &[T]) -> Result<(), Self::Error>;

    /// Like [`GpuBuffer::sync`] but gathers from multiple lists,
    /// concatenated in order. Returns the total number of items synced.
    /// (Upstream `Buffer.syncFromArrayLists`, which takes
    /// `[]const ArrayListUnmanaged(T)` — the renderer's per-row cell
    /// lists.)
    fn sync_from_slices(&mut self, lists: &[&[T]]) -> Result<usize, Self::Error>;
}

/// A 2D texture whose contents can be streamed from the CPU. Port of
/// upstream `metal/Texture.zig`.
pub trait GpuTexture {
    type Error: Error + Send + Sync + 'static;

    fn width(&self) -> usize;
    fn height(&self) -> usize;

    /// Replace a region of the texture with `data` (tightly packed,
    /// `width * height * bpp` bytes). Upstream `Texture.replaceRegion`.
    fn replace_region(
        &self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        data: &[u8],
    ) -> Result<(), Self::Error>;
}

/// Texture pixel formats actually used by the renderer. Named after the
/// Metal formats they map to; the set is exactly what upstream constructs:
/// `initAtlasTexture` (`r8unorm`, `bgra8unorm_srgb`), `imageTextureOptions`
/// (`gray`/`rgba`/`bgra` × srgb), and render targets (`bgra8unorm[_srgb]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    /// 1 byte per pixel grayscale/alpha (glyph atlas).
    R8Unorm,
    R8UnormSrgb,
    /// 4 bytes per pixel RGBA (kitty images).
    Rgba8Unorm,
    Rgba8UnormSrgb,
    /// 4 bytes per pixel BGRA (color atlas, render targets).
    Bgra8Unorm,
    Bgra8UnormSrgb,
}

impl TextureFormat {
    /// Bytes per pixel (the used subset of upstream `Texture.bppOf`; the
    /// Metal backend carries the full mapping).
    #[must_use]
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::R8Unorm | Self::R8UnormSrgb => 1,
            Self::Rgba8Unorm | Self::Rgba8UnormSrgb | Self::Bgra8Unorm | Self::Bgra8UnormSrgb => 4,
        }
    }
}

/// What a texture may be used for. Mirrors the `MTLTextureUsage` subset
/// upstream sets: `shader_read` for atlas/image textures, plus
/// `render_target` for custom-shader intermediates and targets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextureUsage {
    pub shader_read: bool,
    pub shader_write: bool,
    pub render_target: bool,
}

impl TextureUsage {
    /// Sampled-only texture (atlas, images): upstream's
    /// `.{ .shader_read = true }`.
    pub const SHADER_READ: Self = Self {
        shader_read: true,
        shader_write: false,
        render_target: false,
    };

    /// Custom-shader intermediate: read in the next pass, rendered to in
    /// this one.
    pub const SHADER_READ_RENDER_TARGET: Self = Self {
        shader_read: true,
        shader_write: false,
        render_target: true,
    };
}

/// Options for creating a texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureOptions {
    pub format: TextureFormat,
    pub usage: TextureUsage,
}

/// Min/mag sampler filter (upstream `MTLSamplerMinMagFilter` /
/// `GL_NEAREST`/`GL_LINEAR`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SamplerFilter {
    #[default]
    Nearest,
    Linear,
}

/// Sampler texture addressing (upstream `MTLSamplerAddressMode` subset with
/// GL analogues).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SamplerAddressMode {
    #[default]
    ClampToEdge,
    Repeat,
    MirrorRepeat,
}

/// Options for creating a sampler. Upstream `Sampler.Options` minus the
/// device pointer. The one call site so far (custom shaders) uses
/// linear/linear + clamp-to-edge ("match Shadertoy behaviors").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SamplerOptions {
    pub min_filter: SamplerFilter,
    pub mag_filter: SamplerFilter,
    pub s_address_mode: SamplerAddressMode,
    pub t_address_mode: SamplerAddressMode,
}
