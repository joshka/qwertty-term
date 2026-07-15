//! One in-flight frame.
//!
//! Port of `src/renderer/opengl/Frame.zig` (commit `2da015cd6`). OpenGL has no
//! command buffer, so a frame is just the shared GL state plus the
//! completion hook. Upstream `Frame.complete` always `glFinish`es regardless of
//! the `sync` flag ("For OpenGL, `sync` is ignored and we always block"), then
//! reports [`Health`] from the GL error state and presents; this headless port
//! keeps the `glFinish` + health report and hands presentation off to the
//! caller-supplied [`FrameCompletion`] (there is no window to present to).

use std::rc::Rc;

use glow::HasContext;

use super::render_pass::RenderPass;
use super::{GlError, GlState};
use crate::gpu::{Attachment, FrameCompletion, GpuFrame, Health};

/// One in-flight frame. Port of `Frame`.
pub struct Frame {
    state: Rc<GlState>,
    /// The present/health hook, run once on completion; `None` afterward so
    /// double-completion is a no-op.
    completion: Option<FrameCompletion>,
}

impl Frame {
    /// Begin encoding a frame. Port of `Frame.begin`.
    pub(super) fn begin(state: Rc<GlState>, completion: FrameCompletion) -> Self {
        Self {
            state,
            completion: Some(completion),
        }
    }
}

impl GpuFrame for Frame {
    type Backend = super::OpenGL;

    /// Open a render pass with the given color attachments. Port of
    /// `Frame.renderPass`.
    fn render_pass(
        &self,
        attachments: &[Attachment<'_, super::OpenGL>],
    ) -> Result<RenderPass, GlError> {
        RenderPass::begin(Rc::clone(&self.state), attachments)
    }

    /// Complete the frame: `glFinish`, then report health (unhealthy iff a GL
    /// error is pending) to the completion hook. Port of `Frame.complete`
    /// (`sync` is ignored — GL always blocks). Idempotent.
    fn complete(&mut self, sync: bool) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        let gl = self.state.gl();
        // SAFETY: plain GL calls on the current context.
        let health = unsafe {
            gl.finish();
            if gl.get_error() == glow::NO_ERROR {
                Health::Healthy
            } else {
                Health::Unhealthy
            }
        };
        completion(health, sync);
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("completed", &self.completion.is_none())
            .finish_non_exhaustive()
    }
}
