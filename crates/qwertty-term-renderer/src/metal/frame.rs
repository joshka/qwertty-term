//! One in-flight frame: a command buffer, its completion handling, and the
//! present-on-complete → health-report cycle.
//!
//! Port of `src/renderer/metal/Frame.zig` (commit `2da015cd6`).
//!
//! A frame owns one `MTLCommandBuffer`. Render passes are encoded into it via
//! [`Frame::render_pass`]; [`Frame::complete`] commits it and arranges for the
//! target to be presented once the GPU finishes, then reports a [`Health`] to
//! a caller-supplied hook. There are two completion modes:
//!
//! - **async** (`sync = false`): an `addCompletedHandler:` block runs on a
//!   Metal-owned thread when the GPU signals done; it presents (unless the
//!   frame errored) and reports health. This is the steady-state path with
//!   triple buffering (swap-chain permits = 3).
//! - **sync** (`sync = true`): `waitUntilCompleted` blocks the calling thread,
//!   then the same present/health logic runs inline. This is the day-one
//!   degenerate mode (swap-chain permits = 1) the plan
//!   (`docs/plans/m3-first-pixels.md`, decision 3) calls acceptable, and the
//!   mode the resize-driven `display` callback uses.
//!
//! Upstream ties completion to the generic `Renderer` (`present` +
//! `frameCompleted`). The Rust port keeps `Frame` backend-local and takes the
//! present/health work as a boxed [`FrameCompletion`] callback, so the frame
//! lifecycle has no dependency on the (not-yet-ported) generic renderer — the
//! swap-chain and any window host supply the callback.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandQueue, MTLPrimitiveType};

use super::MetalError;
use super::render_pass::{Attachment, RenderPass};

/// Health of a completed frame. Port of `renderer.Health` (the two states
/// upstream distinguishes: a command buffer that finished with `.error`
/// status is `unhealthy`, anything else is `healthy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Healthy,
    Unhealthy,
}

/// What to do when a frame's GPU work finishes: present the target (if the
/// frame is healthy) and record the health status. Port of the
/// `bufferCompleted` body (`api.present(target, sync)` +
/// `renderer.frameCompleted(health)`), lifted out of `Frame` so the frame
/// lifecycle doesn't depend on the generic renderer.
///
/// Invoked exactly once per frame, on the GPU-completion thread in async mode
/// or inline on the committing thread in sync mode. The `bool` is the sync
/// flag, forwarded so the present step can choose sync vs async surface
/// assignment (upstream `present(target, sync)`).
pub type FrameCompletion = Box<dyn Fn(Health, bool) + Send + 'static>;

/// One in-flight frame. Port of `Frame`.
pub struct Frame {
    /// The `MTLCommandBuffer` this frame encodes into.
    buffer: Retained<ProtocolObject<dyn MTLCommandBuffer>>,
    /// The present/health hook, run once on completion. Consumed by
    /// [`Frame::complete`]; `None` afterward so double-completion is a no-op.
    completion: Option<FrameCompletion>,
}

impl Frame {
    /// Begin encoding a frame: grab a command buffer from the queue and stash
    /// the completion hook. Port of `Frame.begin`.
    pub fn begin(
        queue: &ProtocolObject<dyn MTLCommandQueue>,
        completion: FrameCompletion,
    ) -> Result<Self, MetalError> {
        let buffer = queue.commandBuffer().ok_or(MetalError::MetalFailed)?;
        Ok(Self {
            buffer,
            completion: Some(completion),
        })
    }

    /// The underlying `MTLCommandBuffer` (encoding side).
    pub fn command_buffer(&self) -> &ProtocolObject<dyn MTLCommandBuffer> {
        &self.buffer
    }

    /// Begin a render pass into this frame with the given color attachments.
    /// Port of `Frame.renderPass`.
    pub fn render_pass(&self, attachments: &[Attachment<'_>]) -> Result<RenderPass, MetalError> {
        RenderPass::begin(&self.buffer, attachments)
    }

    /// Commit the frame and arrange presentation + health reporting.
    ///
    /// Port of `Frame.complete`. In async mode (`sync = false`) an
    /// `addCompletedHandler:` block carries the completion hook and runs when
    /// the GPU signals done. In sync mode (`sync = true`) we `commit` then
    /// `waitUntilCompleted` and invoke the hook inline — this is the mode the
    /// M3 offscreen-readback tests and the resize `display` callback use.
    ///
    /// Idempotent: a second call is a no-op (the hook was already consumed).
    pub fn complete(&mut self, sync: bool) {
        let Some(completion) = self.completion.take() else {
            return;
        };

        if sync {
            self.buffer.commit();
            self.buffer.waitUntilCompleted();
            let health = health_of(&self.buffer);
            completion(health, true);
        } else {
            // The block owns the completion hook and reads the buffer status
            // when the GPU finishes. `RcBlock` copies the block on registration
            // and the Metal runtime releases the copy after it runs, so we can
            // drop our reference immediately after `addCompletedHandler:`.
            let block = RcBlock::new(move |buf: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                // SAFETY: Metal hands us a valid, live command buffer for
                // the duration of the callback.
                let buf = unsafe { buf.as_ref() };
                completion(health_of(buf), false);
            });
            // `addCompletedHandler:` copies the block, so a pointer to our
            // `RcBlock`'s inner `Block` is fine — Metal owns the copy and
            // releases it after invocation. The `*mut` cast is required by the
            // `MTLCommandBufferHandler` type; the call does not mutate through it.
            let block_ptr: *mut block2::Block<_> = (&*block as *const block2::Block<_>).cast_mut();
            // SAFETY: `block_ptr` points at a live block whose signature matches
            // `MTLCommandBufferHandler` (`Fn(NonNull<MTLCommandBuffer>)`); it
            // stays alive across the call (Metal copies it), and the handler
            // itself is `Send`-safe to run on Metal's completion thread.
            unsafe { self.buffer.addCompletedHandler(block_ptr) };
            self.buffer.commit();
        }
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("completed", &self.completion.is_none())
            .finish_non_exhaustive()
    }
}

/// Map a finished command buffer's status to a [`Health`]. Port of the
/// `switch (status)` in `bufferCompleted`: only `.error` is `unhealthy`.
fn health_of(buffer: &ProtocolObject<dyn MTLCommandBuffer>) -> Health {
    if buffer.status() == MTLCommandBufferStatus::Error {
        Health::Unhealthy
    } else {
        Health::Healthy
    }
}

/// The Metal primitive types the renderer draws. Port of the
/// `mtl.MTLPrimitiveType` values reachable through `RenderPass.Step.Draw`
/// (upstream draws triangles for cells/backgrounds; the full-screen passes use
/// a triangle strip).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primitive {
    Triangle,
    TriangleStrip,
}

impl Primitive {
    pub(super) fn to_metal(self) -> MTLPrimitiveType {
        match self {
            Self::Triangle => MTLPrimitiveType::Triangle,
            Self::TriangleStrip => MTLPrimitiveType::TriangleStrip,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::super::pipeline::{
        ColorAttachment, Options as PipelineOptions, Pipeline, VertexAttribute, VertexFormat,
        VertexLayout, VertexStep, library_from_source,
    };
    use super::super::render_pass::{Attachment, Draw, Step};
    use super::super::test_metal;
    use super::*;
    use crate::gpu::GpuBackend;

    /// A trivial MSL shader pair, private to this test — the production shaders
    /// live with chunk R3 (`src/shaders/`), which this chunk must not touch.
    /// A pass-through vertex that emits one clip-space vertex from `vertex_id`,
    /// and a fragment that returns opaque magenta.
    const TEST_MSL: &str = r"
#include <metal_stdlib>
using namespace metal;
vertex float4 test_vertex(uint vid [[vertex_id]]) {
    float2 p[3] = { float2(-1.0, -1.0), float2(3.0, -1.0), float2(-1.0, 3.0) };
    return float4(p[vid], 0.0, 1.0);
}
fragment float4 test_fragment() { return float4(1.0, 0.0, 1.0, 1.0); }
";

    /// End-to-end offscreen CLEAR: a pipeline-less render pass with a clear
    /// color onto an IOSurface-backed target, completed synchronously, then the
    /// IOSurface bytes are read back and checked. This is the R2 acceptance
    /// test (`docs/plans/m3-first-pixels.md`: "offscreen: render clear-color
    /// frame, read IOSurface pixels back").
    #[test]
    fn clear_color_readback() {
        let Some(metal) = test_metal() else { return };
        let (w, h) = (4usize, 4usize);
        let target = metal.new_target(w, h).expect("target");

        // Clear color as linear RGBA in [0,1]. The target is BGRA8Unorm (no
        // sRGB, since linear_blending defaults false), so each channel stores
        // as round(c * 255) with no gamma curve.
        let clear = [0.25_f64, 0.5, 0.75, 1.0]; // r, g, b, a

        // Record which health we were told about, and that present was invoked.
        let reported = Arc::new(AtomicBool::new(false));
        let reported2 = Arc::clone(&reported);
        let completion: FrameCompletion = Box::new(move |health, sync| {
            assert_eq!(health, Health::Healthy);
            assert!(sync, "clear_color_readback drives sync completion");
            reported2.store(true, Ordering::Release);
        });

        let mut frame = metal.begin_frame(completion).expect("frame");
        {
            let pass = frame
                .render_pass(&[Attachment {
                    texture: target.texture(),
                    clear_color: Some(clear),
                }])
                .expect("render pass");
            // No steps: a pure clear. End encoding.
            pass.complete();
        }
        frame.complete(true);

        assert!(
            reported.load(Ordering::Acquire),
            "completion hook must run in sync mode"
        );

        // Read back. Bytes are B, G, R, A per pixel.
        let pixels = target.read_pixels();
        let to_u8 = |c: f64| (c * 255.0).round() as u8;
        let (rb, gb, bb, ab) = (
            to_u8(clear[0]),
            to_u8(clear[1]),
            to_u8(clear[2]),
            to_u8(clear[3]),
        );
        for px in pixels.chunks_exact(4) {
            // Allow ±1 for any rounding/colorspace nuance in the clear store.
            assert!(
                (px[0] as i16 - bb as i16).abs() <= 1,
                "B: {} vs {bb}",
                px[0]
            );
            assert!(
                (px[1] as i16 - gb as i16).abs() <= 1,
                "G: {} vs {gb}",
                px[1]
            );
            assert!(
                (px[2] as i16 - rb as i16).abs() <= 1,
                "R: {} vs {rb}",
                px[2]
            );
            assert_eq!(px[3], ab, "A");
        }
    }

    /// Pipeline creation succeeds against a trivial runtime-compiled MSL pair.
    /// The pixel format matches what a real target uses so the pipeline is
    /// compatible with the frame's attachment.
    #[test]
    fn pipeline_creation_from_inline_msl() {
        let Some(metal) = test_metal() else { return };
        let library = library_from_source(metal.device(), TEST_MSL).expect("compile MSL");
        let pipeline = Pipeline::new(
            metal.device(),
            &PipelineOptions {
                vertex_fn: "test_vertex",
                fragment_fn: "test_fragment",
                vertex_library: &library,
                fragment_library: &library,
                // This shader generates geometry from vertex_id: no vertex
                // descriptor.
                vertex_layout: None,
                attachments: &[ColorAttachment {
                    pixel_format: metal.target_pixel_format(),
                    blending_enabled: true,
                }],
            },
        )
        .expect("pipeline");
        // The compiled state exists and answers messages.
        let _ = pipeline.state();
    }

    /// A vertex descriptor built from an explicit attribute table compiles into
    /// a pipeline (proving the `autoAttribute` replacement). Uses a CellText-
    /// shaped instance layout (per-instance step) constructed from string/format
    /// names — the frozen-wire coordination point with R3.
    #[test]
    fn pipeline_with_explicit_vertex_layout() {
        let Some(metal) = test_metal() else { return };
        // A vertex shader that reads a per-instance attribute; the fragment is
        // the same magenta.
        const MSL: &str = r"
#include <metal_stdlib>
using namespace metal;
struct VIn { uint2 grid_pos [[attribute(0)]]; uchar4 color [[attribute(1)]]; };
vertex float4 layout_vertex(VIn in [[stage_in]], uint vid [[vertex_id]]) {
    float2 p[3] = { float2(-1.0, -1.0), float2(3.0, -1.0), float2(-1.0, 3.0) };
    return float4(p[vid], 0.0, 1.0);
}
fragment float4 test_fragment() { return float4(1.0, 0.0, 1.0, 1.0); }
";
        let library = library_from_source(metal.device(), MSL).expect("compile MSL");
        // CellText: grid_pos (u16x2) at offset 20, color (u8x4) at offset 24,
        // 32-byte stride. We mirror the shape with ushort2/uchar4 attributes.
        let attrs = [
            VertexAttribute {
                format: VertexFormat::UShort2,
                offset: 20,
            },
            VertexAttribute {
                format: VertexFormat::UChar4,
                offset: 24,
            },
        ];
        let pipeline = Pipeline::new(
            metal.device(),
            &PipelineOptions {
                vertex_fn: "layout_vertex",
                fragment_fn: "test_fragment",
                vertex_library: &library,
                fragment_library: &library,
                vertex_layout: Some(VertexLayout {
                    stride: 32,
                    attributes: &attrs,
                    step: VertexStep::PerInstance,
                }),
                attachments: &[ColorAttachment {
                    pixel_format: metal.target_pixel_format(),
                    blending_enabled: true,
                }],
            },
        )
        .expect("pipeline with vertex layout");
        let _ = pipeline.state();
    }

    /// A render pass encodes a 3-vertex draw (one full-screen triangle) through
    /// a real pipeline without crashing, and the frame completes healthy.
    #[test]
    fn render_pass_encodes_three_vertex_draw() {
        let Some(metal) = test_metal() else { return };
        let (w, h) = (8usize, 8usize);
        let target = metal.new_target(w, h).expect("target");
        let library = library_from_source(metal.device(), TEST_MSL).expect("compile MSL");
        let pipeline = Pipeline::new(
            metal.device(),
            &PipelineOptions {
                vertex_fn: "test_vertex",
                fragment_fn: "test_fragment",
                vertex_library: &library,
                fragment_library: &library,
                vertex_layout: None,
                attachments: &[ColorAttachment {
                    pixel_format: metal.target_pixel_format(),
                    blending_enabled: true,
                }],
            },
        )
        .expect("pipeline");

        let mut frame = metal
            .begin_frame(Box::new(|health, _sync| {
                assert_eq!(health, Health::Healthy)
            }))
            .expect("frame");
        {
            let pass = frame
                .render_pass(&[Attachment {
                    texture: target.texture(),
                    clear_color: Some([0.0, 0.0, 0.0, 1.0]),
                }])
                .expect("render pass");
            pass.step(&Step {
                pipeline_state: pipeline.state(),
                vertex: None,
                uniforms: None,
                extras: &[],
                textures: &[],
                samplers: &[],
                draw: Draw::vertices(Primitive::Triangle, 3),
            });
            pass.complete();
        }
        frame.complete(true);

        // The full-screen triangle covers the whole target with magenta
        // (BGRA bytes: B=255, G=0, R=255, A=255).
        let pixels = target.read_pixels();
        for px in pixels.chunks_exact(4) {
            assert_eq!(px, [255, 0, 255, 255], "magenta fill");
        }
    }
}
