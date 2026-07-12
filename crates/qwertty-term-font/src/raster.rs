//! Platform-neutral rasterized-glyph types shared by every face backend.
//!
//! [`Bitmap`] and [`PixelFormat`] are the output contract of glyph
//! rasterization, independent of which backend produced them — CoreText
//! (`coretext::Face`, macOS) or FreeType (`freetype::Face`, the Linux/software
//! path, ADR 003 P2). Both backends produce the *same* `Bitmap` so the shared
//! consumers (`grid`, the renderer) don't care which face rasterized a glyph.
//!
//! These were originally defined in `coretext` (macOS only); they were hoisted
//! here when the FreeType backend arrived so a single type crosses both. The
//! `coretext` module re-exports them for source compatibility.

/// Pixel layout of a rasterized glyph [`Bitmap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 1 byte per pixel: coverage/alpha only. Produced for non-color
    /// (outline) glyphs — a `kCGImageAlphaOnly` linear-gray context on
    /// CoreText, `FT_RENDER_MODE_NORMAL` on FreeType.
    Alpha8,
    /// 4 bytes per pixel: premultiplied little-endian BGRA. Produced for color
    /// glyphs (sbix/SVG/CBDT emoji).
    Bgra,
}

/// A rasterized glyph bitmap in CPU memory.
///
/// `bearing_x` is the distance from the left of the cell to the left of the ink
/// box. `bearing_y` is the distance from the glyph's **baseline** to the **top**
/// of the ink box (+Y up) — CoreText returns the ink rect relative to the
/// baseline (the drawing origin); FreeType's `bitmap_top` is the same quantity.
///
/// This is NOT yet cell-relative: to obtain the cell-bottom-relative `offset_y`
/// the shader expects, the caller adds `metrics.cell_baseline` (upstream folds
/// that in inside `renderGlyph`; the reduced `rasterize` has no metrics, so
/// `Grid::render_face_glyph` applies it). Atlas upload is the caller's
/// responsibility (renderer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Distance from the left of the cell to the left of the ink box.
    pub bearing_x: i32,
    /// Distance from the bottom of the cell to the top of the ink box.
    pub bearing_y: i32,
    /// Pixel format of `data`.
    pub format: PixelFormat,
    /// Tightly packed pixel data, `width * height * bytes_per_pixel` bytes.
    pub data: Vec<u8>,
}

impl Bitmap {
    /// Bytes per pixel implied by [`Bitmap::format`].
    pub fn bytes_per_pixel(&self) -> u32 {
        match self.format {
            PixelFormat::Alpha8 => 1,
            PixelFormat::Bgra => 4,
        }
    }

    /// True if every pixel is zero (nothing was drawn).
    pub fn is_blank(&self) -> bool {
        self.data.iter().all(|&b| b == 0)
    }
}
