//! Wrapper for handling render pipelines.
//!
//! Port of `src/renderer/opengl/Pipeline.zig` (commit `2da015cd6`): a linked
//! GLSL program plus a VAO describing the per-instance vertex layout.
//!
//! # Vertex layout: explicit table, not comptime reflection
//!
//! Upstream derives the VAO from a Zig struct type via `autoAttribute`, a
//! comptime loop over the struct's fields that picks a GL attribute format from
//! each field type and uses `@offsetOf` for the relative offset. Rust has no
//! comptime field reflection, so this port consumes the backend-agnostic
//! [`crate::shaders::VertexAttribute`] table (format + offset) that
//! [`crate::shaders`] already pins to the frozen wire structs — the same table
//! the Metal backend maps to `MTLVertexDescriptor`. All attributes bind to
//! vertex buffer binding 0 (upstream `attributeBinding(i, 0)`); the per-binding
//! step divisor (`per_instance` → 1) is set once on binding 0.

use std::rc::Rc;

use glow::HasContext;

use super::shaders::ShaderSet;
use super::{GlError, GlState};
use crate::shaders::{PipelineDescription, StepFunction, VertexFormat};

/// A compiled/linked GL pipeline. Port of `Pipeline`.
pub struct Pipeline {
    state: Rc<GlState>,
    pub(super) program: glow::Program,
    pub(super) vao: glow::VertexArray,
    /// Byte stride of one instance record (`@sizeOf(V)`; 0 when there is no
    /// per-instance vertex buffer), for `glBindVertexBuffer`.
    pub(super) stride: usize,
    pub(super) blending_enabled: bool,
}

impl Pipeline {
    /// Compile + link the vendored GLSL and build the VAO. Port of
    /// `Pipeline.init` (`opengl/Pipeline.zig`) driven by the backend-agnostic
    /// [`PipelineDescription`].
    pub(super) fn new(
        state: Rc<GlState>,
        desc: &PipelineDescription,
        set: &ShaderSet,
    ) -> Result<Self, GlError> {
        let gl = state.gl();
        // SAFETY: all calls run on the current context; shader/program objects
        // are checked for compile/link success and cleaned up on failure.
        let (program, vao) = unsafe {
            let vertex = compile_shader(gl, glow::VERTEX_SHADER, &set.vertex, desc.name)?;
            let fragment = match compile_shader(gl, glow::FRAGMENT_SHADER, &set.fragment, desc.name)
            {
                Ok(f) => f,
                Err(e) => {
                    gl.delete_shader(vertex);
                    return Err(e);
                }
            };

            let program = gl
                .create_program()
                .map_err(|e| GlError::GlFailed(format!("glCreateProgram: {e}")))?;
            gl.attach_shader(program, vertex);
            gl.attach_shader(program, fragment);
            gl.link_program(program);
            // Shaders can be deleted once linked into the program.
            gl.delete_shader(vertex);
            gl.delete_shader(fragment);
            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                gl.delete_program(program);
                return Err(GlError::GlFailed(format!(
                    "linking `{}` program failed: {log}",
                    desc.name
                )));
            }

            let vao = gl
                .create_vertex_array()
                .map_err(|e| GlError::GlFailed(format!("glGenVertexArrays: {e}")))?;
            if let Some(attrs) = desc.vertex_attributes {
                gl.bind_vertex_array(Some(vao));
                let divisor = match desc.step_fn {
                    StepFunction::PerVertex => 0,
                    StepFunction::PerInstance => 1,
                };
                for attr in attrs {
                    gl.enable_vertex_attrib_array(attr.index);
                    gl.vertex_attrib_binding(attr.index, 0);
                    let (size, gl_type, integer) = gl_format(attr.format);
                    if integer {
                        gl.vertex_attrib_format_i32(
                            attr.index,
                            size,
                            gl_type,
                            u32::try_from(attr.offset).unwrap_or(0),
                        );
                    } else {
                        gl.vertex_attrib_format_f32(
                            attr.index,
                            size,
                            gl_type,
                            false,
                            u32::try_from(attr.offset).unwrap_or(0),
                        );
                    }
                }
                // One buffer binding (0) carries every attribute; its divisor
                // controls per-vertex vs per-instance advance.
                gl.vertex_binding_divisor(0, divisor);
                gl.bind_vertex_array(None);
            }

            (program, vao)
        };

        Ok(Self {
            state,
            program,
            vao,
            stride: desc.stride,
            blending_enabled: desc.blending.enabled,
        })
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        let gl = self.state.gl();
        // SAFETY: current context; both names are live and owned here.
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
        }
    }
}

impl std::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("stride", &self.stride)
            .field("blending_enabled", &self.blending_enabled)
            .finish_non_exhaustive()
    }
}

/// Compile one GLSL stage, returning a compile error (with the info log) on
/// failure.
///
/// # Safety
/// The GL context must be current.
unsafe fn compile_shader(
    gl: &glow::Context,
    stage: u32,
    source: &str,
    name: &str,
) -> Result<glow::Shader, GlError> {
    unsafe {
        let shader = gl
            .create_shader(stage)
            .map_err(|e| GlError::GlFailed(format!("glCreateShader: {e}")))?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            let stage_name = if stage == glow::VERTEX_SHADER {
                "vertex"
            } else {
                "fragment"
            };
            return Err(GlError::GlFailed(format!(
                "compiling `{name}` {stage_name} shader failed: {log}"
            )));
        }
        Ok(shader)
    }
}

/// Map a backend-agnostic [`VertexFormat`] to `(component_count, gl_type,
/// is_integer)`. Integer formats use `glVertexAttribIFormat` (they stay integer
/// in the shader — `uvec`/`ivec`); float formats use `glVertexAttribFormat`.
/// Mirrors `autoAttribute`'s per-field `attributeIFormat`/`attributeFormat`
/// dispatch.
fn gl_format(format: VertexFormat) -> (i32, u32, bool) {
    match format {
        VertexFormat::UInt2 => (2, glow::UNSIGNED_INT, true),
        VertexFormat::UShort2 => (2, glow::UNSIGNED_SHORT, true),
        VertexFormat::Short2 => (2, glow::SHORT, true),
        VertexFormat::UChar4 => (4, glow::UNSIGNED_BYTE, true),
        VertexFormat::UChar => (1, glow::UNSIGNED_BYTE, true),
        VertexFormat::Float2 => (2, glow::FLOAT, false),
        VertexFormat::Float4 => (4, glow::FLOAT, false),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_opengl;
    use crate::gpu::{GpuBackend, ShaderSource};
    use crate::shaders::PIPELINE_DESCRIPTIONS;

    /// Every first-pixels pipeline compiles + links its vendored GLSL.
    #[test]
    fn all_pipelines_compile_and_link() {
        let Some(gl) = test_opengl() else { return };
        for desc in PIPELINE_DESCRIPTIONS {
            gl.build_pipeline(desc, ShaderSource::None)
                .unwrap_or_else(|e| panic!("pipeline `{}` failed: {e}", desc.name));
        }
    }
}
