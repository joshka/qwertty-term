//! Wrapper for handling render passes.
//!
//! Port of `src/renderer/opengl/RenderPass.zig` (commit `2da015cd6`).
//!
//! [`RenderPass::begin`] binds the target's framebuffer, sets the viewport, and
//! (like upstream's "clear on step 0") clears when the attachment carries a
//! clear color. [`RenderPass::step`] binds a pipeline and its resources and
//! issues one instanced draw. [`RenderPass::complete`] `glFlush`es.
//!
//! **Binding convention** — the backend-neutral [`Step`] (`crate::gpu`)
//! separates `vertex` / `uniforms` / `extras`, exactly as upstream's abstract
//! `RenderPass.Step` separates `buffers[0]` (vertex) from `uniforms` and
//! `buffers[1..]` (storage). This port maps them onto the bindings the vendored
//! GLSL declares:
//!
//! - `vertex` → vertex buffer binding 0 (`glBindVertexBuffer`), the per-instance
//!   `CellText`/`Image` attributes.
//! - `uniforms` → uniform buffer binding **1** (`glBindBufferBase`), the
//!   `layout(binding = 1, std140) uniform Globals` block in `common.glsl`
//!   ("we bind at index 1 to align with Metal", upstream `RenderPass.zig`).
//! - `extras[i]` → shader-storage binding **i + 1**, so `extras[0]` (the
//!   per-cell background array) lands at `layout(binding = 1, std430)` — the
//!   `bg_cells` SSBO `cell_bg.f.glsl` / `cell_text.v.glsl` read.
//! - `textures[i]` → texture unit `i` (`sampler2DRect atlas_grayscale` at
//!   binding 0, `atlas_color` at binding 1).

use std::rc::Rc;

use glow::HasContext;

use super::{GlError, GlState, OpenGL};
use crate::gpu::{Attachment, GpuRenderPass, GpuTarget, Primitive, Step};

/// A live render pass bound to one framebuffer target. Port of `RenderPass`.
pub struct RenderPass {
    state: Rc<GlState>,
}

impl RenderPass {
    /// Begin a render pass: bind the (single) color attachment's framebuffer,
    /// size the viewport to it, and clear if a clear color was given. Port of
    /// `RenderPass.begin` + the step-0 clear.
    pub(super) fn begin(
        state: Rc<GlState>,
        attachments: &[Attachment<'_, OpenGL>],
    ) -> Result<Self, GlError> {
        let at = attachments
            .first()
            .ok_or_else(|| GlError::GlFailed("render pass has no attachments".into()))?;
        let target = at.texture;
        let gl = state.gl();
        // SAFETY: plain GL state changes on the current context; the target's
        // framebuffer is a complete FBO (checked at creation).
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.framebuffer()));
            gl.viewport(0, 0, target.width() as i32, target.height() as i32);
            if let Some(c) = at.clear_color {
                gl.clear_color(c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
        }
        Ok(Self { state })
    }

    fn step_impl(&self, step: &Step<'_, OpenGL>) {
        if step.draw.instance_count == 0 {
            return;
        }
        let gl = self.state.gl();
        // SAFETY: every binding/draw call runs on the current context; the
        // handles come from live resources sharing this state, and the buffers
        // bound below cover the shader's declared inputs (the caller's
        // responsibility, as upstream).
        unsafe {
            gl.use_program(Some(step.pipeline.program));
            gl.bind_vertex_array(Some(step.pipeline.vao));

            // Uniforms → UBO binding 1 (Metal-aligned; see module docs).
            if let Some(ubo) = step.uniforms {
                gl.bind_buffer_base(glow::UNIFORM_BUFFER, 1, Some(*ubo));
            }

            // Textures → units 0.. (atlas grayscale/color, GL_TEXTURE_RECTANGLE).
            for (i, tex) in step.textures.iter().enumerate() {
                if let Some(tex) = tex {
                    gl.active_texture(glow::TEXTURE0 + i as u32);
                    gl.bind_texture(glow::TEXTURE_RECTANGLE, Some(tex.texture()));
                }
            }

            // Samplers → units 0.. (unused in first-pixels; bound for parity).
            for (i, samp) in step.samplers.iter().enumerate() {
                if let Some(samp) = samp {
                    gl.bind_sampler(i as u32, Some(samp.sampler()));
                }
            }

            // Vertex/instance buffer → binding 0, with the pipeline's stride.
            if let Some(vbo) = step.vertex {
                gl.bind_vertex_buffer(0, Some(*vbo), 0, step.pipeline.stride as i32);
            }

            // Extras → SSBO storage bindings 1.. (extras[0] = bg_cells at 1).
            for (i, extra) in step.extras.iter().enumerate() {
                if let Some(buf) = extra {
                    gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, (i + 1) as u32, Some(**buf));
                }
            }

            // Premultiplied "over" blending, else disabled (upstream: enable +
            // `glBlendFunc(ONE, ONE_MINUS_SRC_ALPHA)`, or disable).
            if step.pipeline.blending_enabled {
                gl.enable(glow::BLEND);
                gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            } else {
                gl.disable(glow::BLEND);
            }

            gl.draw_arrays_instanced(
                primitive_to_gl(step.draw.primitive),
                0,
                step.draw.vertex_count as i32,
                step.draw.instance_count as i32,
            );
        }
    }
}

impl GpuRenderPass for RenderPass {
    type Backend = OpenGL;

    fn step(&self, step: &Step<'_, OpenGL>) {
        self.step_impl(step);
    }

    /// End the pass. Port of `RenderPass.complete` (`glFlush`).
    fn complete(self) {
        // SAFETY: plain GL call on the current context.
        unsafe { self.state.gl().flush() };
    }
}

impl std::fmt::Debug for RenderPass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderPass").finish_non_exhaustive()
    }
}

/// Map the backend-neutral [`Primitive`] to its GL enum.
fn primitive_to_gl(p: Primitive) -> u32 {
    match p {
        Primitive::Triangle => glow::TRIANGLES,
        Primitive::TriangleStrip => glow::TRIANGLE_STRIP,
    }
}
