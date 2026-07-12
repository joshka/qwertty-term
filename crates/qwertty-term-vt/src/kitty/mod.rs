//! Kitty graphics protocol model (port of `src/terminal/kitty/graphics_*.zig`,
//! commit `2da015cd6`).
//!
//! This is a **renderer-independent** port of the kitty graphics protocol *model*:
//!
//! - [`command`]: the APC command grammar — a byte-at-a-time key=value parser, the typed
//!   command tree (transmit / display / delete / query / animation), and the [`Response`]
//!   encoder.
//! - [`image`]: [`Image`] (a fully-decoded raw-pixel image) and [`LoadingImage`] (chunked
//!   multi-medium transfer assembly, base64/zlib/png handling), plus [`Rect`].
//! - [`storage`]: [`ImageStorage`] — the per-screen image map, placement map, byte-limit
//!   eviction, dirty/generation tracking, and the delete dispatch.
//!
//! - [`exec`]: [`execute`] — applies a parsed [`Command`] to a live
//!   [`crate::terminal::Terminal`] (cursor-tracked placements, chunked-transfer
//!   `q` inheritance, delete against the real cursor, quiet-mode reply filter).
//!
//! - [`unicode`]: the `U=1` unicode-placeholder mechanism —
//!   [`unicode::placement_iterator`] walks flagged rows back into
//!   [`unicode::Placement`]s, and [`unicode::Placement::render_placement`]
//!   resolves one against stored [`storage::Placement`]/[`Image`] data into a
//!   renderer-facing [`unicode::RenderPlacement`] (port of
//!   `graphics_unicode.zig` and `graphics_render.zig`).
//!
//! Deferred (documented in `docs/analysis/kitty-graphics.md`): the GPU-side
//! consumption of [`unicode::RenderPlacement`] (Phase 4; the struct and its
//! math are ported here, only the actual texture upload/draw is deferred).
//!
//! # Extraction
//!
//! This is a flagged library-extraction candidate. The [`command`] grammar and [`Response`]
//! are entirely ghostty-free. The blocker for a clean split is that [`storage`] and [`image`]
//! reference [`crate::pagelist::Pin`] for placement positions; the terminal *geometry* they
//! need is carried in a POD [`TerminalGeometry`] instead of a `Terminal` to keep that surface
//! minimal. See the analysis doc for the recommended trait-based split.

pub mod command;
pub mod exec;
pub mod image;
pub mod render;
pub mod storage;
pub mod unicode;

pub use command::{Command, Parser as CommandParser, Response};
pub use exec::{execute, execute_with};
pub use image::{Image, LoadingImage, Rect};
pub use render::{RenderImagePlacement, image_rgba, resolve_placements};
pub use storage::{
    AddImageError, ImageStorage, Location, Placement, PlacementId, PlacementKey, PlacementTag,
    next_generation,
};
pub use unicode::{PlacementIterator, RenderPlacement, RenderPlacementError};

use crate::page::size::CellCountInt;

/// The terminal geometry the placement model needs to compute pixel/grid sizes and rects.
///
/// This is a plain-old-data snapshot of the four `Terminal` fields the kitty model reads
/// (`cols`, `rows`, `width_px`, `height_px`). It exists so the model does not depend on a
/// `Terminal` type (which is owned by a sibling chunk and does not yet exist), keeping the
/// extraction surface free of qwertty-term-vt-internal aggregate types. Port of the ad-hoc
/// `t.cols` / `t.rows` / `t.width_px` / `t.height_px` reads in `graphics_storage.zig`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalGeometry {
    /// Active-area columns.
    pub cols: CellCountInt,
    /// Active-area rows.
    pub rows: CellCountInt,
    /// Screen width in pixels.
    pub width_px: u32,
    /// Screen height in pixels.
    pub height_px: u32,
}

impl TerminalGeometry {
    pub fn new(cols: CellCountInt, rows: CellCountInt, width_px: u32, height_px: u32) -> Self {
        Self {
            cols,
            rows,
            width_px,
            height_px,
        }
    }
}
