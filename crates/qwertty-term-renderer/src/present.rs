//! Presentation wiring for a window host (chunk R5, additive).
//!
//! R4's [`Engine::draw_frame`](crate::engine::Engine::draw_frame) renders a
//! frame into the swap chain's IOSurface-backed target and reads the pixels
//! back — exactly what the offscreen acceptance tests need, but a *window* host
//! wants those pixels on screen instead. This module adds the on-screen path
//! without touching the R4 draw core:
//!
//! - [`Engine::draw_and_present`](crate::engine::Engine::draw_and_present): draw one
//!   frame (identical GPU work to `draw_frame`) and assign the drawn target's IOSurface
//!   to an [`IOSurfaceLayer`](crate::metal::IOSurfaceLayer)'s `contents`, presenting it.
//!   Sync mode: the frame is
//!   completed with `waitUntilCompleted` before the surface is attached, so the
//!   layer never shows a half-drawn surface.
//!
//! The pacing side is deliberately left to the host: R2 already ships
//! [`TimerPacer`](crate::swap_chain::TimerPacer) (the "tick a draw" shape
//! CVDisplayLink later swaps into), but AppKit requires the actual draw to run
//! on the main thread (Metal command submission + CoreAnimation `contents`
//! assignment are main-thread affairs), so `qwertty-term` drives the tick from
//! an `NSTimer` on the main run loop rather than the background-thread
//! `TimerPacer`. This module therefore only supplies the *draw+attach* half; the
//! *when-to-draw* half lives in the host next to its run loop.

#![cfg(target_os = "macos")]

use crate::engine::Engine;
use crate::gpu::GpuBuffer;
use crate::metal::{Attachment, Draw, IOSurfaceLayer, MetalError, Primitive, Step};

impl Engine {
    /// Draw one frame and present it by attaching the drawn target's IOSurface
    /// to `layer`.
    ///
    /// This is [`Engine::draw_frame`](crate::engine::Engine::draw_frame)'s GPU
    /// body (bg_color → cell_bg → cell_text, sync completion), except that after
    /// the frame completes it assigns the target's surface to the layer's
    /// `contents` — the presentation step the offscreen path skips. Must be
    /// called on the main thread (CoreAnimation requirement); the sync-mode
    /// `waitUntilCompleted` guarantees the surface is fully rendered before it
    /// is shown.
    ///
    /// Returns `Ok(false)` (nothing presented) when the target has zero area
    /// (no `update_frame` has sized it yet), matching `draw_frame`'s early-out.
    pub fn draw_and_present(&mut self, layer: &IOSurfaceLayer) -> Result<bool, MetalError> {
        self.draw_and_present_inner(layer, false).map(|r| r.0)
    }

    /// Like [`Engine::draw_and_present`], but also reads the freshly presented
    /// surface's pixels back and returns them (BGRA, `screen_width × screen_height`,
    /// row padding stripped — same layout as [`Engine::draw_frame`]).
    ///
    /// This is the presented-pixel verification seam the windowed typing smoke
    /// uses to assert that what was *attached to the layer* actually contains
    /// glyph coverage — not just that the engine's text buffer does. It reads
    /// from the same IOSurface that was handed to the layer, after the sync
    /// frame completed, so the bytes are exactly what CoreAnimation will show.
    /// `None` (nothing presented) when the target has zero area.
    ///
    /// Slightly more expensive than [`Engine::draw_and_present`] (a full-frame
    /// CPU readback), so it's for smoke/debug paths, not the steady render loop.
    pub fn draw_and_present_readback(
        &mut self,
        layer: &IOSurfaceLayer,
    ) -> Result<Option<Vec<u8>>, MetalError> {
        let (presented, pixels) = self.draw_and_present_inner(layer, true)?;
        if presented {
            // Present-smoothness measurement (#141): feed the presented frame to
            // the env-gated recorder (no-op unless QWERTTY_TERM_PRESENT_STATS).
            self.record_present(&pixels);
        }
        Ok(if presented { Some(pixels) } else { None })
    }

    /// Shared body of the present paths. When `readback` is set, the presented
    /// surface's pixels are returned in the second tuple slot (empty otherwise).
    fn draw_and_present_inner(
        &mut self,
        layer: &IOSurfaceLayer,
        readback: bool,
    ) -> Result<(bool, Vec<u8>), MetalError> {
        if self.screen_width() == 0 || self.screen_height() == 0 {
            return Ok((false, Vec::new()));
        }

        // Upload kitty image textures + sync placement instance buffers before
        // the disjoint field borrows below (needs `&mut self`).
        self.prepare_image_frame()?;

        let uniforms = self.uniforms_snapshot();
        let bg_cells = self.bg_cells_snapshot();
        let fg_count = self.fg_count();
        let fg_lists_owned = self.fg_lists_snapshot();
        let fg_lists: Vec<&[_]> = fg_lists_owned.iter().map(Vec::as_slice).collect();
        let (sw, sh) = (self.screen_width(), self.screen_height());

        let (
            backend,
            swap_chain,
            bg_pipe,
            cell_bg_pipe,
            cell_text_pipe,
            image_pipe,
            images,
            image_instances,
            placements,
            img_bg_end,
            img_text_end,
        ) = self.present_parts();

        let mut guard = swap_chain.next_frame().ok_or(MetalError::MetalFailed)?;
        let slot = guard.slot();

        if slot.target.width() != sw || slot.target.height() != sh {
            slot.resize(backend, sw, sh)?;
        }

        slot.uniforms.sync(&[uniforms])?;
        slot.cells_bg.sync(&bg_cells)?;
        slot.cells.sync_from_slices(&fg_lists)?;

        let mut frame = backend.begin_frame(Box::new(|_health, _sync| {}))?;
        {
            let pass = frame.render_pass(&[Attachment {
                texture: slot.target.texture(),
                clear_color: Some([0.0, 0.0, 0.0, 0.0]),
            }])?;

            pass.step(&Step {
                pipeline_state: bg_pipe.state(),
                vertex: None,
                uniforms: Some(slot.uniforms.buffer()),
                extras: &[Some(slot.cells_bg.buffer())],
                textures: &[],
                samplers: &[],
                draw: Draw::vertices(Primitive::Triangle, 3),
            });
            // Kitty images below the cell backgrounds (R6 slice 4).
            crate::engine::encode_image_steps(
                &pass,
                image_pipe,
                slot.uniforms.buffer(),
                images,
                image_instances,
                placements,
                0..img_bg_end,
            );
            pass.step(&Step {
                pipeline_state: cell_bg_pipe.state(),
                vertex: None,
                uniforms: Some(slot.uniforms.buffer()),
                extras: &[Some(slot.cells_bg.buffer())],
                textures: &[],
                samplers: &[],
                draw: Draw::vertices(Primitive::Triangle, 3),
            });
            // Kitty images below text.
            crate::engine::encode_image_steps(
                &pass,
                image_pipe,
                slot.uniforms.buffer(),
                images,
                image_instances,
                placements,
                img_bg_end..img_text_end,
            );
            pass.step(&Step {
                pipeline_state: cell_text_pipe.state(),
                vertex: Some(slot.cells.buffer()),
                uniforms: Some(slot.uniforms.buffer()),
                extras: &[Some(slot.cells_bg.buffer())],
                textures: &[Some(slot.grayscale.texture()), Some(slot.color.texture())],
                samplers: &[],
                draw: Draw {
                    primitive: Primitive::TriangleStrip,
                    vertex_count: 4,
                    instance_count: fg_count,
                },
            });

            // Kitty images above text.
            crate::engine::encode_image_steps(
                &pass,
                image_pipe,
                slot.uniforms.buffer(),
                images,
                image_instances,
                placements,
                img_text_end..placements.len(),
            );

            pass.complete();
        }
        frame.complete(true);

        // Present: hand the freshly rendered surface to the layer. Sync path —
        // we're on the main thread and the frame has completed, so a direct
        // assignment (no dispatch, no size guard) is correct and jank-free.
        // SAFETY: called on the main thread (documented precondition).
        unsafe { layer.set_surface_sync(slot.target.surface()) };

        // Optional presented-pixel readback (smoke/debug). The frame completed
        // synchronously above, so the IOSurface is coherent; read the exact
        // bytes just attached to the layer before releasing the slot (which may
        // reuse the target on the next frame).
        let pixels = if readback {
            slot.target.read_pixels()
        } else {
            Vec::new()
        };

        guard.release();
        Ok((true, pixels))
    }
}
