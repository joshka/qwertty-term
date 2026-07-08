//! Procedural glyph rasterizer for terminal "sprite" fonts.
//!
//! This crate draws the built-in glyphs that a terminal renders itself rather
//! than pulling from a font file: box drawing (`U+2500`), block elements,
//! braille, geometric shapes, powerline separators, git-branch symbols, and the
//! Symbols for Legacy Computing blocks. It also draws the pseudo-glyphs a
//! terminal needs for its own rendering model — cursors and text decorations
//! (underlines, strikethrough, overline).
//!
//! It is a standalone port of Ghostty's `src/font/sprite/` subsystem
//! (commit `2da015cd6`). The public API deliberately contains **no**
//! terminal-emulator types: you provide a [`Metrics`] struct describing the
//! target cell in pixels, and receive an [`Glyph`] holding an 8-bit alpha
//! bitmap plus placement offsets.
//!
//! # Seam-free rendering
//!
//! The whole point of drawing these procedurally (instead of relying on a
//! font) is that adjacent cells line up perfectly at *any* cell size. That
//! property comes from the [`Fraction`] rounding rules in [`common`]: a
//! fraction used as a min (left/top) edge rounds differently than the same
//! fraction used as a max (right/bottom) edge, so a line ending at `half` in
//! one cell begins at exactly the same pixel in the next. Preserve those rules
//! exactly if you touch that code.
//!
//! # Example
//!
//! ```
//! use qwertty_term_sprite::{Metrics, render};
//!
//! let metrics = Metrics::simple(9, 18);
//! // U+2500 BOX DRAWINGS LIGHT HORIZONTAL
//! let glyph = render(0x2500, &metrics).expect("box drawing is a sprite glyph");
//! assert!(glyph.width > 0 && glyph.height > 0);
//! assert_eq!(glyph.alpha.len(), (glyph.width * glyph.height) as usize);
//! ```

mod canvas;
mod common;
mod dispatch;
mod draw;
mod sprite;

pub use canvas::Canvas;
pub use common::{Alignment, Corner, Edge, Fraction, Quads, Shade, Thickness};
pub use sprite::Sprite;

/// Grid metrics describing the target cell, in pixels.
///
/// This is the sole geometric input to the rasterizer. It is a plain data
/// struct with no dependency on any terminal type, so external consumers can
/// populate it however they compute their grid. Every field is in device
/// pixels.
///
/// Use [`Metrics::simple`] for a reasonable default derived from just a cell
/// width and height; construct the struct directly when you have real font
/// metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    /// Width of a single cell.
    pub cell_width: u32,
    /// Height of a single cell.
    pub cell_height: u32,

    /// Baseline position measured from the top of the cell. Not used by most
    /// sprites (they fill the cell), but kept for parity and future use.
    pub cell_baseline: u32,

    /// Distance from the top of the cell to the top of the underline stroke.
    pub underline_position: u32,
    /// Thickness of the underline stroke.
    pub underline_thickness: u32,

    /// Distance from the top of the cell to the top of the strikethrough.
    pub strikethrough_position: u32,
    /// Thickness of the strikethrough stroke.
    pub strikethrough_thickness: u32,

    /// Distance from the top of the cell to the top of the overline. May be
    /// negative (above the cell) — hence signed.
    pub overline_position: i32,
    /// Thickness of the overline stroke.
    pub overline_thickness: u32,

    /// Base thickness for box-drawing lines. Light lines use this, heavy lines
    /// use double, super-light uses half. Drives seam alignment, so all
    /// box/branch/powerline glyphs read from it.
    pub box_thickness: u32,

    /// Thickness of cursor outlines / bars.
    pub cursor_thickness: u32,
    /// Height of full-height cursor sprites (rect, hollow rect, bar). Lets a
    /// caller shrink/grow the cursor independent of the cell.
    pub cursor_height: u32,
}

impl Metrics {
    /// Build a sensible [`Metrics`] from just a cell width and height.
    ///
    /// Thicknesses and positions are derived with the same heuristics Ghostty
    /// uses when it lacks real font metrics: a 1px-minimum line thickness that
    /// scales gently with cell height, an underline near the bottom, a
    /// strikethrough near the middle. This is enough to render every sprite
    /// correctly; supply real metrics for pixel-accurate decoration placement.
    #[must_use]
    pub fn simple(cell_width: u32, cell_height: u32) -> Self {
        // Roughly cell_height / 12, min 1 — matches the feel of Ghostty's
        // `@max(1, @ceil(underlineThickness()))` for typical faces.
        let thickness = (cell_height / 12).max(1);
        Self {
            cell_width,
            cell_height,
            cell_baseline: cell_height.saturating_sub(cell_height / 5),
            underline_position: cell_height.saturating_sub(thickness * 2),
            underline_thickness: thickness,
            strikethrough_position: cell_height / 2,
            strikethrough_thickness: thickness,
            overline_position: 0,
            overline_thickness: thickness,
            box_thickness: thickness,
            cursor_thickness: thickness.max(1),
            cursor_height: cell_height,
        }
    }
}

/// A rasterized sprite glyph: an 8-bit alpha coverage bitmap plus the offsets
/// needed to place it in the cell.
///
/// `alpha` is row-major, `width * height` bytes, one coverage value per pixel
/// (`0` = transparent, `255` = fully covered). The bitmap is trimmed to its
/// non-transparent bounding box; `offset_x`/`offset_y` tell the caller where
/// that box sits relative to the cell so the glyph draws in the right place.
///
/// The offset convention matches Ghostty's atlas glyph: `offset_x` is the
/// displacement of the bitmap's left edge from the left edge of the cell, and
/// `offset_y` is the distance from the cell's baseline up to the *top* of the
/// bitmap (i.e. it is measured from the bottom).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glyph {
    /// Width of the trimmed bitmap in pixels.
    pub width: u32,
    /// Height of the trimmed bitmap in pixels.
    pub height: u32,
    /// Horizontal placement offset from the left of the cell.
    pub offset_x: i32,
    /// Vertical placement offset (distance from baseline to bitmap top).
    pub offset_y: i32,
    /// Row-major alpha coverage, `width * height` bytes.
    pub alpha: Vec<u8>,
}

impl Glyph {
    /// An empty glyph (used for unallocated codepoints inside a handled range).
    fn empty() -> Self {
        Self {
            width: 0,
            height: 0,
            offset_x: 0,
            offset_y: 0,
            alpha: Vec::new(),
        }
    }
}

/// Returns `true` if `codepoint` is drawn by this crate.
///
/// This covers every Unicode range in the dispatch table plus the
/// [`Sprite`] pseudo-codepoints. Mirrors Ghostty's `Face.hasCodepoint`.
#[must_use]
pub fn has_codepoint(codepoint: u32) -> bool {
    dispatch::draw_fn_for(codepoint).is_some()
}

/// Render the glyph for `codepoint` at the given cell metrics.
///
/// Returns `None` if the codepoint is not a sprite glyph (see
/// [`has_codepoint`]). The returned [`Glyph`] is deterministic: the same
/// codepoint and metrics always produce byte-identical output.
///
/// For wide glyphs, pre-multiply `metrics.cell_width` by the cell width before
/// calling (this crate always draws into a single logical cell of the given
/// width).
#[must_use]
pub fn render(codepoint: u32, metrics: &Metrics) -> Option<Glyph> {
    let draw = dispatch::draw_fn_for(codepoint)?;

    // Full-height cursor sprites use cursor_height; everything else fills the
    // cell. Matches Face.renderGlyph.
    let height = match Sprite::from_codepoint(codepoint) {
        Some(Sprite::CursorRect | Sprite::CursorHollowRect | Sprite::CursorBar) => {
            metrics.cursor_height
        }
        _ => metrics.cell_height,
    };
    let width = metrics.cell_width;

    if width == 0 || height == 0 {
        return Some(Glyph::empty());
    }

    let padding_x = width / 4;
    let padding_y = height / 4;

    let mut canvas = Canvas::new(width, height, padding_x, padding_y);
    draw(codepoint, &mut canvas, width, height, metrics);

    Some(canvas.into_glyph(metrics, height))
}
