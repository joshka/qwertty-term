//! Renderer core, ported from Ghostty (`~/local/ghostty/src/renderer/`).
//!
//! This is renderer chunk R0: the foundation every other renderer chunk
//! imports. It provides:
//!
//! - [`size`]: screen/cell/grid geometry, padding-balance math, and
//!   coordinate conversion between surface/terminal/grid spaces.
//! - [`cursor`]: cursor style resolution (focus/blink/preedit priority).
//! - [`row`]: row background-extension heuristics.
//! - [`options`] / [`backend`]: construction-option and GPU-backend stubs,
//!   filled in by later chunks (fonts, apprt, threading, GPU backends).
//! - [`snapshot`]: the [`snapshot::RenderSnapshot`] trait — the contract
//!   between `ghostty-vt` and any renderer backend — plus
//!   [`snapshot::FullSnapshot`], a full-copy implementation backed by
//!   `ghostty-vt`'s existing `Terminal::snapshot_window`.
//!
//! Chunk R1 adds the GPU-backend layer:
//!
//! - [`gpu`]: the [`gpu::GpuBackend`] trait (associated types for
//!   target/frame/pass/pipeline/buffer/texture/sampler) plus the
//!   backend-agnostic resource-creation options.
//! - [`wire`]: the FROZEN CPU↔shader wire structs (`Uniforms`, `CellText`,
//!   `CellBg`, `Image`, `BgImage`), bit-for-bit matches of upstream
//!   `shaders.zig`.
//! - [`metal`] (macOS only): the concrete Metal backend — device/queue
//!   context, IOSurface-backed render target, textures, samplers, growable
//!   buffers.
//!
//! Chunk R3 adds the first-pixels shader set:
//!
//! - [`shaders`]: the embedded MSL source for `bg_color`/`cell_bg`/
//!   `cell_text` (ported verbatim from `shaders.metal`) plus a
//!   backend-agnostic [`shaders::PipelineDescription`] table pinned to the
//!   frozen [`wire`] struct layouts.
//!
//! This crate depends on `ghostty-vt` (read-only use of its snapshot APIs)
//! and never the reverse.
//!
//! See `docs/analysis/renderer-r0.md` for the survey of the R0 Zig source,
//! `docs/analysis/renderer-r1.md` for the R1 GPU-backend survey
//! (`src/renderer/Metal.zig` + `src/renderer/metal/`, commit `2da015cd6`), and
//! `docs/analysis/renderer-r3.md` for the R3 shader-port survey.

pub mod backend;
pub mod cursor;
pub mod gpu;
pub mod options;
pub mod row;
pub mod shaders;
pub mod size;
pub mod snapshot;
pub mod swap_chain;
pub mod wire;

/// The Metal GPU backend. Chunk R1: context + resources. Chunk R2: frame
/// lifecycle (`Frame`/`RenderPass`/`Pipeline`), the `IOSurfaceLayer`
/// presentation target, and the swap chain (see [`swap_chain`]). macOS only;
/// the trait surface in [`gpu`] is platform-agnostic so other backends can
/// slot in later.
#[cfg(target_os = "macos")]
pub mod metal;
