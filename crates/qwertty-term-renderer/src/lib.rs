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
//!   between `qwertty-term-vt` and any renderer backend — plus
//!   [`snapshot::FullSnapshot`], a full-copy implementation backed by
//!   `qwertty-term-vt`'s existing `Terminal::snapshot_window`.
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
//! Chunk R4 adds the cell engine — the first pixels:
//!
//! - [`cells`]: the [`cells::Contents`] cell store (flat bg array + per-row
//!   fg lists, cursor at `fg[0]`) plus the codepoint-classification helpers
//!   (`is_symbol`/`is_covering`/`no_min_contrast`/`constraint_width`), a port
//!   of `src/renderer/cell.zig`.
//! - [`engine`] (macOS only): [`engine::Engine`], which turns a
//!   [`snapshot::RenderSnapshot`] + a `qwertty-term-font` `Grid` into GPU buffers
//!   ([`engine::Engine::update_frame`]) and draws them through the R2/R3
//!   pipelines ([`engine::Engine::draw_frame`]) — a port of the load-bearing
//!   subset of `generic.zig`'s `updateFrame`/`drawFrame`/`rebuildCells`/
//!   `addGlyph`/`addCursor`/`syncAtlasTexture`.
//!
//! This crate depends on `qwertty-term-vt` (read-only use of its snapshot APIs)
//! and never the reverse.
//!
//! For the end-to-end "VT bytes in, pixels out" embedding flow — feed a
//! terminal, [`snapshot::FullSnapshot::capture_live`] it, and
//! [`engine::Engine::render`] one offscreen frame — see the quickstart in the
//! [`engine`] module docs (macOS only) and the `examples/frame-capture` crate.
//!
//! See `docs/analysis/renderer-r0.md` for the survey of the R0 Zig source,
//! `docs/analysis/renderer-r1.md` for the R1 GPU-backend survey
//! (`src/renderer/Metal.zig` + `src/renderer/metal/`, commit `2da015cd6`), and
//! `docs/analysis/renderer-r3.md` for the R3 shader-port survey.

pub mod backend;
pub mod cells;
pub mod cursor;
pub mod gpu;
pub mod options;
pub mod present_stats;
pub mod row;
pub mod shaders;
pub mod size;
pub mod snapshot;
/// The software (CPU) render backend: a platform-free implementation of the
/// [`gpu::GpuBackend`] trait that composites the frozen wire structs into a BGRA
/// framebuffer — the headless render path (ADR 003 P1, PR-2). Available on all
/// targets (no GPU / `objc2`), so it builds and tests on macOS and Linux.
pub mod software;
pub mod swap_chain;
pub mod wire;

/// The Metal GPU backend. Chunk R1: context + resources. Chunk R2: frame
/// lifecycle (`Frame`/`RenderPass`/`Pipeline`), the `IOSurfaceLayer`
/// presentation target, and the swap chain (see [`swap_chain`]). macOS only;
/// the trait surface in [`gpu`] is platform-agnostic so other backends can
/// slot in later.
#[cfg(target_os = "macos")]
pub mod metal;

/// The cell engine: builds GPU buffers from a [`snapshot::RenderSnapshot`] and
/// a `qwertty-term-font` `Grid`, and draws them via the Metal backend. Chunk R4
/// (first pixels). macOS only (it drives the Metal backend and the CoreText
/// font stack).
#[cfg(target_os = "macos")]
pub mod engine;

/// Presentation wiring for a window host: draw a frame and assign its IOSurface
/// to an [`metal::IOSurfaceLayer`]. Chunk R5 (additive over R4's offscreen
/// [`engine::Engine::draw_frame`]). macOS only.
#[cfg(target_os = "macos")]
pub mod present;
