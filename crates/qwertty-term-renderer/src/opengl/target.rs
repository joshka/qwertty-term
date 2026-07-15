//! A render target: an OpenGL renderbuffer-backed framebuffer.
//!
//! Port of `src/renderer/opengl/Target.zig` (commit `2da015cd6`). Upstream
//! presents a target by blitting its renderbuffer to the default framebuffer
//! (`OpenGL.present`); the headless backend never presents — it reads the
//! renderbuffer's pixels back through the FBO instead
//! ([`Target::read_pixels`]), which is the offscreen-readback seam the M3/ADR
//! verification strategy uses.

use std::rc::Rc;

use glow::HasContext;

use super::{GlError, GlState};
use crate::gpu::GpuTarget;

/// A renderbuffer-backed framebuffer. Port of `Target`.
pub struct Target {
    state: Rc<GlState>,
    framebuffer: glow::Framebuffer,
    renderbuffer: glow::Renderbuffer,
    width: usize,
    height: usize,
}

impl Target {
    /// Port of `Target.init`: create a renderbuffer with the given internal
    /// format and attach it as color-0 of a fresh framebuffer.
    pub(super) fn new(
        state: Rc<GlState>,
        width: usize,
        height: usize,
        internal_format: u32,
    ) -> Result<Self, GlError> {
        let gl = state.gl();
        // SAFETY: create/bind/allocate/attach on the current context; the FBO
        // completeness is checked before use.
        let (framebuffer, renderbuffer) = unsafe {
            let renderbuffer = gl
                .create_renderbuffer()
                .map_err(|e| GlError::GlFailed(format!("glGenRenderbuffers: {e}")))?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(renderbuffer));
            gl.renderbuffer_storage(
                glow::RENDERBUFFER,
                internal_format,
                width as i32,
                height as i32,
            );
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);

            let framebuffer = gl
                .create_framebuffer()
                .map_err(|e| GlError::GlFailed(format!("glGenFramebuffers: {e}")))?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::RENDERBUFFER,
                Some(renderbuffer),
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(framebuffer);
                gl.delete_renderbuffer(renderbuffer);
                return Err(GlError::GlFailed(format!(
                    "incomplete framebuffer (status {status:#x})"
                )));
            }
            (framebuffer, renderbuffer)
        };

        Ok(Self {
            state,
            framebuffer,
            renderbuffer,
            width,
            height,
        })
    }

    /// The framebuffer object (render + readback side).
    pub(super) fn framebuffer(&self) -> glow::Framebuffer {
        self.framebuffer
    }
}

impl GpuTarget for Target {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    /// Read the target's pixels back through its FBO: `height` rows of
    /// `width * 4` **BGRA** bytes, **top-down** (row 0 = top of the image).
    ///
    /// `glReadPixels` returns rows bottom-up (window origin is lower-left),
    /// whereas the Software and Metal `read_pixels` are top-down — so this
    /// flips vertically, giving a byte layout identical to those backends so
    /// consumers stay backend-agnostic. Requesting `GL_BGRA` reorders the
    /// `RGBA8` renderbuffer's channels to B,G,R,A, matching the Metal/Software
    /// BGRA convention. Coherent because the caller completes the frame
    /// (`glFinish`) before reading.
    fn read_pixels(&self) -> Vec<u8> {
        let stride = self.width * 4;
        let mut flipped = vec![0u8; stride * self.height];
        let mut raw = vec![0u8; stride * self.height];
        let gl = self.state.gl();
        // SAFETY: current context; `raw` is exactly `width*height*4` bytes,
        // matching the requested region and `GL_BGRA`/`UNSIGNED_BYTE` format
        // with `PACK_ALIGNMENT = 1`.
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.framebuffer));
            gl.read_buffer(glow::COLOR_ATTACHMENT0);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(
                0,
                0,
                self.width as i32,
                self.height as i32,
                glow::BGRA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut raw)),
            );
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
        }
        // Flip bottom-up → top-down.
        for row in 0..self.height {
            let src = row * stride;
            let dst = (self.height - 1 - row) * stride;
            flipped[dst..dst + stride].copy_from_slice(&raw[src..src + stride]);
        }
        flipped
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        let gl = self.state.gl();
        // SAFETY: current context; both names are live and owned here.
        unsafe {
            gl.delete_framebuffer(self.framebuffer);
            gl.delete_renderbuffer(self.renderbuffer);
        }
    }
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_opengl;
    use crate::gpu::{GpuBackend, GpuTarget};

    #[test]
    fn target_dimensions_and_readback_size() {
        let Some(gl) = test_opengl() else { return };
        let target = gl.new_target(7, 5).expect("target");
        assert_eq!((target.width(), target.height()), (7, 5));
        assert_eq!(target.read_pixels().len(), 7 * 5 * 4);
    }
}
