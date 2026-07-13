//! A render pass within a frame: a render-command encoder, its color
//! attachments, and the per-step pipeline/buffer/texture/sampler binding + draw.
//!
//! Port of `src/renderer/metal/RenderPass.zig` (commit `2da015cd6`).
//!
//! [`RenderPass::begin`] builds an `MTLRenderPassDescriptor` from the given
//! attachments (a clear color means `loadAction = clear`, absence means
//! `load`; `storeAction` is always `store`) and opens an encoder on the frame's
//! command buffer. [`RenderPass::step`] binds a pipeline and its resources and
//! issues one instanced draw. [`RenderPass::complete`] (or dropping the pass)
//! ends encoding.
//!
//! **Buffer-index convention** (plan decision 5, `docs/plans/m3-first-pixels.md`;
//! also the frozen convention documented in [`crate::wire`]):
//!
//! - index 0 = vertex/instance data (bound as *both* vertex and fragment
//!   buffer, matching upstream's OpenGL-compatible convention),
//! - index 1 = uniforms (also bound to both stages),
//! - index 2+ = extras (the `buffers[1..]` slice, bound to both stages).
//!
//! Upstream binds buffers/textures/uniforms to both the vertex and fragment
//! stages "consistent and predictable, and we need to treat the uniforms as
//! special because of OpenGL" — this port keeps that exactly.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLLoadAction, MTLPrimitiveType,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLStoreAction,
};

use super::{Metal, MetalError};
use crate::gpu::{Attachment, GpuRenderPass, Primitive, Step};

// `Attachment`, `Step`, `Draw`, and `Primitive` are the backend-neutral types in
// [`crate::gpu`], generic over the backend's own resource types (`Metal` here).
// This module reads them via the `GpuBuffer::handle`/`GpuTexture`/`Pipeline::state`
// accessors, so no raw `objc2_metal` handles leak into the trait surface.

/// Map the backend-neutral [`Primitive`] to its Metal enum.
fn primitive_to_metal(p: Primitive) -> MTLPrimitiveType {
    match p {
        Primitive::Triangle => MTLPrimitiveType::Triangle,
        Primitive::TriangleStrip => MTLPrimitiveType::TriangleStrip,
    }
}

/// A live render pass (open encoder). Port of `RenderPass`.
pub struct RenderPass {
    encoder: Retained<ProtocolObject<dyn MTLRenderCommandEncoder>>,
    /// `true` once `endEncoding` has been sent, so `Drop` doesn't repeat it.
    ended: bool,
}

impl RenderPass {
    /// Begin a render pass on `command_buffer` with the given attachments.
    /// Port of `RenderPass.begin`.
    pub(super) fn begin(
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
        attachments: &[Attachment<'_, Metal>],
    ) -> Result<Self, MetalError> {
        let desc = MTLRenderPassDescriptor::renderPassDescriptor();
        let color_attachments = desc.colorAttachments();

        for (i, at) in attachments.iter().enumerate() {
            // SAFETY: `i` indexes the color-attachment array, which grows on
            // demand; the returned descriptor is owned by the array.
            let attachment = unsafe { color_attachments.objectAtIndexedSubscript(i) };
            attachment.setLoadAction(if at.clear_color.is_some() {
                MTLLoadAction::Clear
            } else {
                MTLLoadAction::Load
            });
            attachment.setStoreAction(MTLStoreAction::Store);
            attachment.setTexture(Some(at.texture.texture()));
            if let Some(c) = at.clear_color {
                attachment.setClearColor(MTLClearColor {
                    red: c[0],
                    green: c[1],
                    blue: c[2],
                    alpha: c[3],
                });
            }
        }

        let encoder = command_buffer
            .renderCommandEncoderWithDescriptor(&desc)
            .ok_or(MetalError::MetalFailed)?;

        Ok(Self {
            encoder,
            ended: false,
        })
    }

    /// Add a step: bind pipeline + resources, then draw. Port of
    /// `RenderPass.step`. A zero-instance draw is skipped entirely (matches
    /// upstream's early return). Reads the generic [`Step`] via the backend's
    /// resource accessors (`Pipeline::state`, `GpuBuffer::handle`,
    /// `GpuTexture`/`Sampler`).
    fn step_impl(&self, step: &Step<'_, Metal>) {
        if step.draw.instance_count == 0 {
            return;
        }

        self.encoder.setRenderPipelineState(step.pipeline.state());

        // Index 0: vertex/instance buffer, bound to both stages (OpenGL-compat
        // convention).
        if let Some(buf) = step.vertex {
            // SAFETY: `buf` is a live MTLBuffer; offset 0 is in bounds.
            unsafe {
                self.encoder.setVertexBuffer_offset_atIndex(Some(buf), 0, 0);
                self.encoder
                    .setFragmentBuffer_offset_atIndex(Some(buf), 0, 0);
            }
        }

        // Index 1: uniforms, bound to both stages.
        if let Some(buf) = step.uniforms {
            // SAFETY: as above.
            unsafe {
                self.encoder.setVertexBuffer_offset_atIndex(Some(buf), 0, 1);
                self.encoder
                    .setFragmentBuffer_offset_atIndex(Some(buf), 0, 1);
            }
        }

        // Indices 2..: extra buffers, bound to both stages.
        for (i, extra) in step.extras.iter().enumerate() {
            if let Some(buf) = extra {
                let index = i + 2;
                // SAFETY: as above.
                unsafe {
                    self.encoder
                        .setVertexBuffer_offset_atIndex(Some(buf), 0, index);
                    self.encoder
                        .setFragmentBuffer_offset_atIndex(Some(buf), 0, index);
                }
            }
        }

        // Textures, bound to both stages by position.
        for (i, tex) in step.textures.iter().enumerate() {
            if let Some(tex) = tex {
                let mtl = tex.texture();
                // SAFETY: `mtl` is a live MTLTexture; `i` is a valid slot.
                unsafe {
                    self.encoder.setVertexTexture_atIndex(Some(mtl), i);
                    self.encoder.setFragmentTexture_atIndex(Some(mtl), i);
                }
            }
        }

        // Fragment samplers, by position.
        for (i, samp) in step.samplers.iter().enumerate() {
            if let Some(samp) = samp {
                let mtl = samp.sampler();
                // SAFETY: `mtl` is a live MTLSamplerState; `i` is a valid slot.
                unsafe {
                    self.encoder.setFragmentSamplerState_atIndex(Some(mtl), i);
                }
            }
        }

        // SAFETY: primitive/counts are valid; buffers bound above cover the
        // shader's declared inputs (the caller's responsibility, as upstream).
        unsafe {
            self.encoder
                .drawPrimitives_vertexStart_vertexCount_instanceCount(
                    primitive_to_metal(step.draw.primitive),
                    0,
                    step.draw.vertex_count,
                    step.draw.instance_count,
                );
        }
    }

    fn end(&mut self) {
        if !self.ended {
            self.encoder.endEncoding();
            self.ended = true;
        }
    }
}

impl GpuRenderPass for RenderPass {
    type Backend = Metal;

    fn step(&self, step: &Step<'_, Metal>) {
        self.step_impl(step);
    }

    /// End encoding. Port of `RenderPass.complete`. The pass must not be used
    /// after this; calling twice (or letting `Drop` run afterward) is a no-op.
    fn complete(mut self) {
        self.end();
    }
}

impl Drop for RenderPass {
    fn drop(&mut self) {
        // A render pass that's dropped without `complete()` still needs its
        // encoder closed, or the command buffer can't be committed.
        self.end();
    }
}

impl std::fmt::Debug for RenderPass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderPass")
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}
