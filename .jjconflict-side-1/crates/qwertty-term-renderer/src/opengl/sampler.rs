//! Wrapper for handling samplers.
//!
//! Port of `src/renderer/opengl/Sampler.zig` (commit `2da015cd6`). The
//! first-pixels path binds no explicit samplers (the atlas is read with the
//! texture's own nearest/clamp state), but the trait requires the type and the
//! custom-shader path (a follow-up) uses linear/clamp samplers.

use std::rc::Rc;

use glow::HasContext;

use super::{GlError, GlState};
use crate::gpu::{SamplerAddressMode, SamplerFilter, SamplerOptions};

/// A GL sampler object. Port of `Sampler`.
pub struct Sampler {
    state: Rc<GlState>,
    sampler: glow::Sampler,
}

impl Sampler {
    /// Initialize a sampler. Port of `Sampler.init`.
    pub(super) fn new(state: Rc<GlState>, opts: SamplerOptions) -> Result<Self, GlError> {
        let gl = state.gl();
        // SAFETY: create/parameterize on the current context.
        let sampler = unsafe {
            let sampler = gl
                .create_sampler()
                .map_err(|e| GlError::GlFailed(format!("glGenSamplers: {e}")))?;
            gl.sampler_parameter_i32(
                sampler,
                glow::TEXTURE_MIN_FILTER,
                filter_bits(opts.min_filter) as i32,
            );
            gl.sampler_parameter_i32(
                sampler,
                glow::TEXTURE_MAG_FILTER,
                filter_bits(opts.mag_filter) as i32,
            );
            gl.sampler_parameter_i32(
                sampler,
                glow::TEXTURE_WRAP_S,
                wrap_bits(opts.s_address_mode) as i32,
            );
            gl.sampler_parameter_i32(
                sampler,
                glow::TEXTURE_WRAP_T,
                wrap_bits(opts.t_address_mode) as i32,
            );
            sampler
        };
        Ok(Self { state, sampler })
    }

    /// The underlying GL sampler name (bound by [`super::RenderPass`]).
    pub(super) fn sampler(&self) -> glow::Sampler {
        self.sampler
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        // SAFETY: current context; sampler name is live and owned here.
        unsafe { self.state.gl().delete_sampler(self.sampler) };
    }
}

impl std::fmt::Debug for Sampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sampler").finish_non_exhaustive()
    }
}

fn filter_bits(filter: SamplerFilter) -> u32 {
    match filter {
        SamplerFilter::Nearest => glow::NEAREST,
        SamplerFilter::Linear => glow::LINEAR,
    }
}

fn wrap_bits(mode: SamplerAddressMode) -> u32 {
    match mode {
        SamplerAddressMode::ClampToEdge => glow::CLAMP_TO_EDGE,
        SamplerAddressMode::Repeat => glow::REPEAT,
        SamplerAddressMode::MirrorRepeat => glow::MIRRORED_REPEAT,
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_opengl;
    use crate::gpu::{GpuBackend, SamplerAddressMode, SamplerFilter, SamplerOptions};

    #[test]
    fn sampler_creation() {
        let Some(gl) = test_opengl() else { return };
        gl.new_sampler(SamplerOptions {
            min_filter: SamplerFilter::Linear,
            mag_filter: SamplerFilter::Linear,
            s_address_mode: SamplerAddressMode::ClampToEdge,
            t_address_mode: SamplerAddressMode::ClampToEdge,
        })
        .expect("sampler");
    }
}
