//! A compiled render pipeline: vertex + fragment functions, an optional vertex
//! descriptor, and premultiplied-alpha color-attachment blending.
//!
//! Port of `src/renderer/metal/Pipeline.zig` (commit `2da015cd6`).
//!
//! # Vertex descriptor: explicit tables, not comptime reflection
//!
//! Upstream derives the `MTLVertexDescriptor` from a Zig struct type via
//! `autoAttribute`, a `comptime` loop over the struct's fields that maps each
//! field type to an `MTLVertexFormat` and uses `@offsetOf` for the attribute
//! offset. Rust has no comptime field reflection, so the port takes an explicit
//! [`VertexLayout`] — a stride plus a table of [`VertexAttribute`]s (format +
//! offset) — that the caller builds. R3 owns the production layouts (they live
//! with the shaders); tests here build trivial ones from [`VertexFormat`] to
//! prove the machinery. All attributes come from buffer index 0 (upstream
//! hard-codes `bufferIndex = 0` in `autoAttribute`), matching the RenderPass
//! index-0-is-vertex-data convention.
//!
//! # Blending
//!
//! Every color attachment uses premultiplied-alpha blending when enabled
//! (upstream: "We always use premultiplied alpha blending for now."):
//! `add` op, `source = one`, `dest = one_minus_source_alpha`, for both RGB and
//! alpha. When disabled (the custom-shader full-screen passes) no blend state
//! is set.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLDevice, MTLLibrary, MTLPixelFormat,
    MTLRenderPipelineColorAttachmentDescriptor, MTLRenderPipelineDescriptor,
    MTLRenderPipelineState, MTLVertexDescriptor, MTLVertexFormat, MTLVertexStepFunction,
};

use super::MetalError;

/// A vertex attribute format. The subset of `MTLVertexFormat` upstream's
/// `autoAttribute` maps from Zig field types (see that fn's `switch`); named
/// after the wire-struct field shapes they serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexFormat {
    UChar,
    UChar4,
    UShort2,
    Short2,
    Float,
    Float2,
    Float4,
    Int,
    Int2,
    UInt,
    UInt2,
    UInt4,
}

impl VertexFormat {
    fn to_metal(self) -> MTLVertexFormat {
        match self {
            Self::UChar => MTLVertexFormat::UChar,
            Self::UChar4 => MTLVertexFormat::UChar4,
            Self::UShort2 => MTLVertexFormat::UShort2,
            Self::Short2 => MTLVertexFormat::Short2,
            Self::Float => MTLVertexFormat::Float,
            Self::Float2 => MTLVertexFormat::Float2,
            Self::Float4 => MTLVertexFormat::Float4,
            Self::Int => MTLVertexFormat::Int,
            Self::Int2 => MTLVertexFormat::Int2,
            Self::UInt => MTLVertexFormat::UInt,
            Self::UInt2 => MTLVertexFormat::UInt2,
            Self::UInt4 => MTLVertexFormat::UInt4,
        }
    }
}

/// One vertex attribute: its format and byte offset within the vertex struct.
/// The explicit-table equivalent of one iteration of upstream `autoAttribute`
/// (which reads `@offsetOf` and the field type). The attribute's shader index
/// is its position in the [`VertexLayout::attributes`] slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexAttribute {
    pub format: VertexFormat,
    pub offset: usize,
}

/// A full vertex layout: the per-instance/per-vertex struct stride plus its
/// attributes. Replaces upstream's `comptime VertexAttributes: ?type`.
#[derive(Debug, Clone, Copy)]
pub struct VertexLayout<'a> {
    /// Byte stride between consecutive vertices/instances (`@sizeOf(V)`).
    pub stride: usize,
    /// Attributes in shader-index order.
    pub attributes: &'a [VertexAttribute],
    /// How the vertex buffer advances. Upstream default `per_vertex`; the cell
    /// shaders use `per_instance`.
    pub step: VertexStep,
}

/// Vertex step function. Port of `mtl.MTLVertexStepFunction` (the two upstream
/// uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VertexStep {
    #[default]
    PerVertex,
    PerInstance,
}

impl VertexStep {
    fn to_metal(self) -> MTLVertexStepFunction {
        match self {
            Self::PerVertex => MTLVertexStepFunction::PerVertex,
            Self::PerInstance => MTLVertexStepFunction::PerInstance,
        }
    }
}

/// A color attachment description for a pipeline. Port of
/// `Pipeline.Options.Attachment`.
#[derive(Debug, Clone, Copy)]
pub struct ColorAttachment {
    pub pixel_format: MTLPixelFormat,
    /// Whether premultiplied-alpha blending is enabled (upstream default
    /// `true`; the full-screen custom-shader pass sets `false`).
    pub blending_enabled: bool,
}

/// Options for building a [`Pipeline`]. Port of `Pipeline.Options` minus the
/// device (folded into [`Pipeline::new`]'s receiver).
pub struct Options<'a> {
    /// Name of the vertex function within `vertex_library`.
    pub vertex_fn: &'a str,
    /// Name of the fragment function within `fragment_library`.
    pub fragment_fn: &'a str,
    /// Library holding the vertex function.
    pub vertex_library: &'a ProtocolObject<dyn MTLLibrary>,
    /// Library holding the fragment function.
    pub fragment_library: &'a ProtocolObject<dyn MTLLibrary>,
    /// Optional vertex layout (`None` = no vertex descriptor, e.g. shaders that
    /// generate geometry from `vertex_id`).
    pub vertex_layout: Option<VertexLayout<'a>>,
    /// Color attachments, in index order.
    pub attachments: &'a [ColorAttachment],
}

/// A compiled render pipeline. Port of `Pipeline` (`state: MTLRenderPipelineState`).
pub struct Pipeline {
    state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
}

impl Pipeline {
    /// Compile a pipeline. Port of `Pipeline.init`.
    ///
    /// Looks up the vertex/fragment functions by name, builds a vertex
    /// descriptor from `opts.vertex_layout` if present, configures each color
    /// attachment (premultiplied-alpha blending when enabled), and asks the
    /// device for the pipeline state. Returns [`MetalError::MetalFailed`] if a
    /// function name is missing or pipeline compilation fails.
    pub fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        opts: &Options<'_>,
    ) -> Result<Self, MetalError> {
        let desc = MTLRenderPipelineDescriptor::new();

        let vertex_fn = function(opts.vertex_library, opts.vertex_fn)?;
        let fragment_fn = function(opts.fragment_library, opts.fragment_fn)?;
        desc.setVertexFunction(Some(&vertex_fn));
        desc.setFragmentFunction(Some(&fragment_fn));

        if let Some(layout) = opts.vertex_layout {
            let vertex_desc = build_vertex_descriptor(&layout);
            desc.setVertexDescriptor(Some(&vertex_desc));
        }

        let color_attachments = desc.colorAttachments();
        for (i, at) in opts.attachments.iter().enumerate() {
            // SAFETY: `i` indexes the color-attachment array (grows on demand);
            // the returned descriptor is owned by the array.
            let attachment = unsafe { color_attachments.objectAtIndexedSubscript(i) };
            configure_attachment(&attachment, at);
        }

        let state = device
            .newRenderPipelineStateWithDescriptor_error(&desc)
            .map_err(|err| {
                // Surface the localized description like upstream's checkError.
                eprintln!("metal pipeline error: {err:?}");
                MetalError::MetalFailed
            })?;

        Ok(Self { state })
    }

    /// The compiled `MTLRenderPipelineState` (bound in `RenderPass::step`).
    pub fn state(&self) -> &ProtocolObject<dyn MTLRenderPipelineState> {
        &self.state
    }
}

impl std::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline").finish_non_exhaustive()
    }
}

/// Compile MSL `source` into an `MTLLibrary` at runtime. Port of upstream's
/// `newLibraryWithSource:options:error:` usage (`initPostPipeline` /
/// `initLibrary`'s fallback). R3 supplies the production shader source; tests
/// here compile a trivial inline library to exercise pipeline creation.
pub fn library_from_source(
    device: &ProtocolObject<dyn MTLDevice>,
    source: &str,
) -> Result<Retained<ProtocolObject<dyn MTLLibrary>>, MetalError> {
    let ns_source = NSString::from_str(source);
    device
        .newLibraryWithSource_options_error(&ns_source, None)
        .map_err(|err| {
            eprintln!("metal library compile error: {err:?}");
            MetalError::MetalFailed
        })
}

/// Look up a named function in a library. Port of `newFunctionWithName:`.
fn function(
    library: &ProtocolObject<dyn MTLLibrary>,
    name: &str,
) -> Result<Retained<ProtocolObject<dyn objc2_metal::MTLFunction>>, MetalError> {
    let ns_name = NSString::from_str(name);
    library
        .newFunctionWithName(&ns_name)
        .ok_or(MetalError::MetalFailed)
}

/// Build an `MTLVertexDescriptor` from an explicit layout. Port of
/// `autoAttribute` + the layout-0 setup in `Pipeline.init`, but table-driven
/// instead of comptime-reflected.
fn build_vertex_descriptor(layout: &VertexLayout<'_>) -> Retained<MTLVertexDescriptor> {
    let desc = MTLVertexDescriptor::vertexDescriptor();

    let attrs = desc.attributes();
    for (i, attr) in layout.attributes.iter().enumerate() {
        // SAFETY: `i` indexes the attribute-descriptor array (grows on demand).
        let a = unsafe { attrs.objectAtIndexedSubscript(i) };
        a.setFormat(attr.format.to_metal());
        // SAFETY: offset/bufferIndex are plain scalar setters; all attributes
        // come from buffer index 0 (upstream `autoAttribute` hard-codes this).
        unsafe {
            a.setOffset(attr.offset);
            a.setBufferIndex(0);
        }
    }

    // Layout 0 describes buffer 0's stride + step function.
    let layouts = desc.layouts();
    // SAFETY: index 0 is always valid.
    let l = unsafe { layouts.objectAtIndexedSubscript(0) };
    l.setStepFunction(layout.step.to_metal());
    // SAFETY: plain scalar setter.
    unsafe {
        l.setStride(layout.stride);
    }

    desc
}

/// Configure one color attachment: pixel format and premultiplied-alpha
/// blending. Port of the attachment loop in `Pipeline.init`.
fn configure_attachment(
    attachment: &MTLRenderPipelineColorAttachmentDescriptor,
    at: &ColorAttachment,
) {
    attachment.setPixelFormat(at.pixel_format);
    attachment.setBlendingEnabled(at.blending_enabled);
    if at.blending_enabled {
        // Premultiplied alpha: out = src + dst * (1 - src.a), for RGB and A.
        attachment.setRgbBlendOperation(MTLBlendOperation::Add);
        attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
        attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
        attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
        attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    }
}
