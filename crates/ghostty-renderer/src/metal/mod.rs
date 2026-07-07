//! Metal backend: device/queue context and GPU resources.
//!
//! Port of Ghostty's `src/renderer/Metal.zig` + `src/renderer/metal/`
//! (commit `2da015cd6`), reduced to chunk R1 scope: context init
//! (`chooseDevice`, `newCommandQueue`, storage-mode/max-texture-size
//! queries) and the resource wrappers (`Target`, `Texture`, `Sampler`,
//! `Buffer(T)`). Upstream's `metal/api.zig` (hand-written selector
//! bindings) is deliberately NOT ported — the `objc2-metal` crate provides
//! the same bindings, generated from the headers.
//!
//! Chunk R2 adds the frame lifecycle and presentation: [`Frame`] (command
//! buffer + completion → present → health), [`RenderPass`] (encoder +
//! instanced draws), [`Pipeline`] (runtime-compiled shader library + vertex
//! descriptor + premultiplied-alpha blending), and [`IOSurfaceLayer`] (the
//! `CALayer` subclass presentation target). The R1 placeholder enums for
//! `Frame`/`RenderPass`/`Pipeline` are replaced by these real types.
//!
//! Out of R2 scope, landing later: `shaders.zig`'s production pipeline table
//! (R3, lives with the shaders), the cell engine that drives rebuild/draw
//! (R4), and the view-attachment/`contentsScale` wiring in upstream
//! `Metal.init` (R5, needs a window).

mod buffer;
mod frame;
mod layer;
mod pipeline;
mod render_pass;
mod sampler;
mod target;
mod texture;

use std::fmt;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLCommandQueue, MTLCopyAllDevices, MTLDevice, MTLGPUFamily, MTLPixelFormat,
    MTLResourceOptions, MTLStorageMode,
};

pub use self::buffer::Buffer;
pub use self::frame::{Frame, FrameCompletion, Health, Primitive};
pub use self::layer::{DisplayCallback, IOSurfaceLayer};
pub use self::pipeline::{
    ColorAttachment, Options as PipelineOptions, Pipeline, VertexAttribute, VertexFormat,
    VertexLayout, VertexStep, library_from_source,
};
pub use self::render_pass::{Attachment, Draw, RenderPass, Step};
pub use self::sampler::Sampler;
pub use self::target::Target;
pub use self::texture::Texture;
use crate::gpu::{GpuBackend, SamplerOptions, TextureFormat, TextureOptions};

/// Errors from the Metal backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetalError {
    /// No usable (non-headless) Metal device on this machine. Upstream
    /// `error.NoMetalDevice`.
    NoDevice,
    /// A Metal (or IOSurface/CoreGraphics) API call returned nil. Upstream
    /// `error.MetalFailed`.
    MetalFailed,
}

impl fmt::Display for MetalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => write!(f, "no Metal device available"),
            Self::MetalFailed => write!(f, "a Metal API call failed"),
        }
    }
}

impl std::error::Error for MetalError {}

/// The Metal graphics API context. Port of the R1 subset of the `Metal`
/// struct in `Metal.zig`: device + queue + device metadata. The
/// presentation layer (`layer: IOSurfaceLayer`) and config-derived
/// `blending` land in R2+/R5.
pub struct Metal {
    /// MTLDevice.
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    /// MTLCommandQueue.
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    /// Default storage mode for resources created with this device:
    /// `Shared` with unified memory, `Managed` on discrete GPUs (which
    /// don't support `Shared` for GPU-visible resources).
    default_storage_mode: MTLStorageMode,
    /// Maximum 2D texture width/height supported by the device.
    max_texture_size: u32,
    /// Whether render targets/textures should use `*_srgb` pixel formats so
    /// blending happens in linear space. Stands in for upstream's
    /// `blending: AlphaBlending` config field until config plumbing lands
    /// (R2+); upstream default (`.native`) is non-linear, i.e. `false`.
    linear_blending: bool,
}

impl Metal {
    /// Create the Metal context: choose a device, create its command queue,
    /// and query device metadata. Headless-friendly (no window/layer
    /// involved) — the R1 subset of upstream `Metal.init`.
    pub fn new() -> Result<Self, MetalError> {
        let device = Self::choose_device().ok_or(MetalError::NoDevice)?;
        let queue = device.newCommandQueue().ok_or(MetalError::MetalFailed)?;

        // Managed mode is what discrete GPUs (no unified memory) require;
        // shared is both sufficient and faster on Apple Silicon.
        let default_storage_mode = if device.hasUnifiedMemory() {
            MTLStorageMode::Shared
        } else {
            MTLStorageMode::Managed
        };
        let max_texture_size = Self::query_max_texture_size(&device);

        Ok(Self {
            device,
            queue,
            default_storage_mode,
            max_texture_size,
            linear_blending: false,
        })
    }

    /// Port of `Metal.chooseDevice` (macOS arm): prefer a GPU that's
    /// connected to a display; if the user has an eGPU (removable) they
    /// probably want it, otherwise integrated (low-power) GPUs are better
    /// for battery and thermals.
    fn choose_device() -> Option<Retained<ProtocolObject<dyn MTLDevice>>> {
        let mut chosen: Option<Retained<ProtocolObject<dyn MTLDevice>>> = None;
        for device in MTLCopyAllDevices() {
            // We want a GPU that's connected to a display.
            if device.isHeadless() {
                continue;
            }
            let stop = device.isRemovable() || device.isLowPower();
            chosen = Some(device);
            if stop {
                break;
            }
        }
        chosen
    }

    /// Port of `Metal.queryMaxTextureSize`, per Apple's Metal feature set
    /// tables.
    fn query_max_texture_size(device: &ProtocolObject<dyn MTLDevice>) -> u32 {
        if device.supportsFamily(MTLGPUFamily::Apple10) {
            return 32768;
        }
        if device.supportsFamily(MTLGPUFamily::Apple3) {
            return 16384;
        }
        8192
    }

    /// The MTLDevice.
    pub fn device(&self) -> &ProtocolObject<dyn MTLDevice> {
        &self.device
    }

    /// The MTLCommandQueue.
    pub fn queue(&self) -> &ProtocolObject<dyn MTLCommandQueue> {
        &self.queue
    }

    /// The default storage mode for resources created with this device.
    pub fn default_storage_mode(&self) -> MTLStorageMode {
        self.default_storage_mode
    }

    /// Resource options every upstream call site uses: CPU writes but never
    /// reads (`write_combined`) + the device's default storage mode.
    /// (Upstream `Metal.bufferOptions` / the `resource_options` fields of
    /// `textureOptions`/`initTarget`.)
    fn resource_options(&self) -> MTLResourceOptions {
        MTLResourceOptions::CPUCacheModeWriteCombined
            | Self::storage_mode_options(self.default_storage_mode)
    }

    /// Convert an `MTLStorageMode` into its `MTLResourceOptions` bits.
    fn storage_mode_options(mode: MTLStorageMode) -> MTLResourceOptions {
        match mode {
            MTLStorageMode::Managed => MTLResourceOptions::StorageModeManaged,
            MTLStorageMode::Private => MTLResourceOptions::StorageModePrivate,
            MTLStorageMode::Memoryless => MTLResourceOptions::StorageModeMemoryless,
            // Shared (and anything unexpected) → shared, the default.
            _ => MTLResourceOptions::StorageModeShared,
        }
    }

    /// The pixel format for render targets and custom-shader textures:
    /// `*_srgb` iff linear blending, so Metal gamma-encodes after blending.
    /// (Upstream `initTarget`/`textureOptions`/`initShaders` all make this
    /// same choice from `self.blending.isLinear()`.)
    ///
    /// Public because pipeline color attachments (R2/R3) must match the target
    /// format the frame renders into (upstream `initShaders` passes the same
    /// choice into pipeline construction).
    pub fn target_pixel_format(&self) -> MTLPixelFormat {
        if self.linear_blending {
            MTLPixelFormat::BGRA8Unorm_sRGB
        } else {
            MTLPixelFormat::BGRA8Unorm
        }
    }

    /// Begin encoding a frame on this context's command queue. Port of
    /// `Metal.beginFrame` / `Frame.begin`. The `completion` hook runs once the
    /// frame finishes (present + health report); the swap chain supplies it.
    pub fn begin_frame(&self, completion: FrameCompletion) -> Result<Frame, MetalError> {
        Frame::begin(&self.queue, completion)
    }

    /// Compile a render pipeline against this device. Port of `Pipeline.init`
    /// dispatched through `Metal`. R3 owns the production shader source +
    /// pipeline table; this is the device-bound entry point they call.
    pub fn new_pipeline(&self, opts: &PipelineOptions<'_>) -> Result<Pipeline, MetalError> {
        Pipeline::new(&self.device, opts)
    }
}

impl fmt::Debug for Metal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Metal")
            .field("device", &self.device.name().to_string())
            .field("default_storage_mode", &self.default_storage_mode)
            .field("max_texture_size", &self.max_texture_size)
            .field("linear_blending", &self.linear_blending)
            .finish_non_exhaustive()
    }
}

impl GpuBackend for Metal {
    /// Triple buffering (upstream `Metal.swap_chain_count = 3`).
    const SWAP_CHAIN_COUNT: usize = 3;

    type Error = MetalError;
    type Target = Target;
    type Frame = Frame;
    type RenderPass = RenderPass;
    type Pipeline = Pipeline;
    type Buffer<T: Copy + 'static> = Buffer<T>;
    type Texture = Texture;
    type Sampler = Sampler;

    fn max_texture_size(&self) -> u32 {
        self.max_texture_size
    }

    /// Upstream `Metal.initTarget`.
    fn new_target(&self, width: usize, height: usize) -> Result<Target, MetalError> {
        Target::new(&target::Options {
            device: &self.device,
            width,
            height,
            pixel_format: self.target_pixel_format(),
            storage_mode: self.default_storage_mode,
        })
    }

    /// Upstream `Buffer.init` with `Metal.bufferOptions`.
    fn new_buffer<T: Copy + 'static>(&self, len: usize) -> Result<Buffer<T>, MetalError> {
        Buffer::new(&self.device, self.resource_options(), len)
    }

    /// Upstream `Buffer.initFill` with `Metal.bufferOptions`.
    fn new_buffer_with_data<T: Copy + 'static>(&self, data: &[T]) -> Result<Buffer<T>, MetalError> {
        Buffer::new_with_data(&self.device, self.resource_options(), data)
    }

    /// Upstream `Texture.init` with `Metal.textureOptions` /
    /// `imageTextureOptions` / `initAtlasTexture` (the format/usage split
    /// those helpers encode is carried by `options` here).
    fn new_texture(
        &self,
        options: TextureOptions,
        width: usize,
        height: usize,
        data: Option<&[u8]>,
    ) -> Result<Texture, MetalError> {
        Texture::new(
            &texture::Options {
                device: &self.device,
                pixel_format: pixel_format_for(options.format),
                resource_options: self.resource_options(),
                usage: texture::usage_bits(options.usage),
            },
            width,
            height,
            data,
        )
    }

    /// Upstream `Sampler.init` with `Metal.samplerOptions`.
    fn new_sampler(&self, options: SamplerOptions) -> Result<Sampler, MetalError> {
        Sampler::new(&self.device, options)
    }
}

/// Map the backend-agnostic [`TextureFormat`] to the Metal pixel format,
/// exactly as upstream's `initAtlasTexture`/`ImageTextureFormat.toPixelFormat`
/// do.
fn pixel_format_for(format: TextureFormat) -> MTLPixelFormat {
    match format {
        TextureFormat::R8Unorm => MTLPixelFormat::R8Unorm,
        TextureFormat::R8UnormSrgb => MTLPixelFormat::R8Unorm_sRGB,
        TextureFormat::Rgba8Unorm => MTLPixelFormat::RGBA8Unorm,
        TextureFormat::Rgba8UnormSrgb => MTLPixelFormat::RGBA8Unorm_sRGB,
        TextureFormat::Bgra8Unorm => MTLPixelFormat::BGRA8Unorm,
        TextureFormat::Bgra8UnormSrgb => MTLPixelFormat::BGRA8Unorm_sRGB,
    }
}

#[cfg(test)]
pub(crate) fn test_metal() -> Option<Metal> {
    match Metal::new() {
        Ok(metal) => Some(metal),
        Err(err) => {
            // CI machines may have no (non-headless) Metal device; tests
            // must skip rather than fail. This machine class (dev Macs) has
            // one.
            eprintln!("SKIP: no usable Metal device ({err}); skipping GPU test");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{GpuBuffer, GpuTexture, TextureUsage};

    #[test]
    fn context_init_device_and_queue() {
        let Some(metal) = test_metal() else { return };
        // Feature-set table values only.
        assert!(matches!(metal.max_texture_size(), 8192 | 16384 | 32768));
        // Device/queue exist and answer messages.
        assert!(!metal.device().name().to_string().is_empty());
        let _ = metal.queue();
        // Storage mode matches the unified-memory rule.
        let expected = if metal.device().hasUnifiedMemory() {
            MTLStorageMode::Shared
        } else {
            MTLStorageMode::Managed
        };
        assert_eq!(metal.default_storage_mode(), expected);
    }

    #[test]
    fn backend_creates_all_resource_types() {
        let Some(metal) = test_metal() else { return };

        let target = metal.new_target(4, 4).expect("target");
        assert_eq!((target.width(), target.height()), (4, 4));

        let buffer = metal.new_buffer::<u32>(4).expect("buffer");
        assert_eq!(buffer.len(), 4);

        let texture = metal
            .new_texture(
                TextureOptions {
                    format: TextureFormat::R8Unorm,
                    usage: TextureUsage::SHADER_READ,
                },
                8,
                8,
                None,
            )
            .expect("texture");
        assert_eq!((texture.width(), texture.height()), (8, 8));

        metal
            .new_sampler(SamplerOptions::default())
            .expect("sampler");
    }
}
