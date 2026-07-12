//! FreeType glyph rasterization — the non-CoreText face backend.
//!
//! Port of the load + rasterize subset of Ghostty's `src/font/face/freetype.zig`
//! (`2da015cd6`), the counterpart to [`crate::coretext`] for the Linux/software
//! render path (ADR 003 P2). Slice 1 covers: load-from-bytes, char→glyph
//! lookup, cell/decoration metrics (via the portable [`crate::tables`] +
//! [`crate::metrics`] derivation, shared with every backend), and grayscale
//! (outline) glyph rasterization to a [`crate::raster::Bitmap`]. Synthetic
//! bold/italic, color-bitmap (emoji) glyphs, `rasterize_constrained`, and
//! `wght` variations are deferred to later slices.
//!
//! FreeType is cross-platform, so this module builds and runs on macOS too
//! (behind the `freetype` Cargo feature) — the tests exercise it there against
//! the same embedded JetBrains Mono the CoreText face uses, and assert metric
//! parity with the portable derivation.

use freetype::face::LoadFlag;
use freetype::{Library, RenderMode};

use crate::metrics::FaceMetrics;
use crate::raster::{Bitmap, PixelFormat};

/// Errors from loading or rasterizing a FreeType face.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The FreeType library failed to initialize.
    LibraryInitFailed,
    /// The font bytes could not be parsed into a face.
    FaceLoadFailed,
    /// Setting the pixel size on the face failed.
    SizeFailed,
    /// The requested glyph id has no glyph in this face.
    NoSuchGlyph,
    /// FreeType failed to load or render the glyph.
    RenderFailed,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Error::LibraryInitFailed => "FreeType library failed to initialize",
            Error::FaceLoadFailed => "failed to load font face from bytes",
            Error::SizeFailed => "failed to set face pixel size",
            Error::NoSuchGlyph => "no such glyph in face",
            Error::RenderFailed => "FreeType failed to load or render the glyph",
        };
        f.write_str(s)
    }
}

impl std::error::Error for Error {}

/// A loaded FreeType font face at a specific pixel size.
///
/// Owns the font bytes (FreeType requires the buffer to outlive the face, and
/// [`Face::face_metrics`] re-parses them with `ttf-parser` for the portable
/// metric derivation).
pub struct Face {
    face: freetype::Face,
    /// The font bytes, kept for the `ttf-parser` metric derivation and for
    /// rustybuzz shaping (`ShapeFace::source_bytes`).
    bytes: Vec<u8>,
    size_px: f64,
    /// Subface index within a `.ttc`/`.otc` collection (0 for a single face).
    face_index: u32,
}

impl Face {
    /// Load a face from in-memory font bytes at `size_px`.
    pub fn load_from_bytes(bytes: &[u8], size_px: f64) -> Result<Face, Error> {
        Self::load_from_bytes_indexed(bytes, size_px, 0)
    }

    /// Load subface `face_index` from a collection (or `0` for a single face).
    pub fn load_from_bytes_indexed(
        bytes: &[u8],
        size_px: f64,
        face_index: u32,
    ) -> Result<Face, Error> {
        let lib = Library::init().map_err(|_| Error::LibraryInitFailed)?;
        let owned = bytes.to_vec();
        let face = lib
            .new_memory_face(owned.clone(), face_index as isize)
            .map_err(|_| Error::FaceLoadFailed)?;
        // Integer pixel sizing for slice 1 (fractional 26.6 sizing is a later
        // refinement); round to the nearest pixel like a display would.
        let px = size_px.round().max(1.0) as u32;
        face.set_pixel_sizes(0, px).map_err(|_| Error::SizeFailed)?;
        Ok(Face {
            face,
            bytes: owned,
            size_px,
            face_index,
        })
    }

    /// Load the embedded JetBrains Mono fallback at `size_px`.
    pub fn load_embedded(size_px: f64) -> Result<Face, Error> {
        Self::load_from_bytes(crate::embedded::JETBRAINS_MONO_VARIABLE, size_px)
    }

    /// The pixel size this face was loaded at.
    pub fn size_px(&self) -> f64 {
        self.size_px
    }

    /// The font bytes this face was loaded from (for rustybuzz shaping). Always
    /// `Some` for a FreeType face — it is always byte-backed (unlike a CoreText
    /// system face, whose bytes may be unavailable).
    pub fn source_bytes(&self) -> Option<&[u8]> {
        Some(&self.bytes)
    }

    /// Subface index within a collection (`.ttc`), `0` for a single face.
    pub fn face_index(&self) -> u32 {
        self.face_index
    }

    /// The applied `wght` variation instance, if any. Slice 1/2 load the font's
    /// default instance and apply no variation, so this is `None` (the shaper
    /// then shapes the default instance, which is correct). Returns `Some` once
    /// a `with_wght_variation` path lands (later P2 slice).
    pub fn wght(&self) -> Option<f32> {
        None
    }

    /// The glyph id for a character, or `None` if the face has no glyph for it.
    pub fn glyph_index(&self, c: char) -> Option<u32> {
        // FreeType returns glyph index 0 (`.notdef`) for missing codepoints.
        match self.face.get_char_index(c as usize) {
            Some(0) | None => None,
            Some(idx) => Some(idx),
        }
    }

    /// Cell/decoration metrics for this face, derived by the portable
    /// [`crate::tables`] + [`crate::metrics`] path (shared with the CoreText
    /// backend) so the derivation is identical regardless of rasterizer.
    pub fn face_metrics(&self) -> FaceMetrics {
        let ttf =
            ttf_parser::Face::parse(&self.bytes, 0).expect("face bytes already parsed by FreeType");
        crate::tables::face_metrics(&ttf, self.size_px)
    }

    /// Rasterize an outline glyph to a grayscale (`Alpha8`) [`Bitmap`].
    ///
    /// Color-bitmap (emoji) glyphs are a later slice; this always renders the
    /// outline via `FT_RENDER_MODE_NORMAL`. FreeType's `bitmap_left`/`bitmap_top`
    /// map directly onto the shared bearing convention (see [`Bitmap`]).
    pub fn rasterize(&self, glyph_id: u32) -> Result<Bitmap, Error> {
        self.face
            .load_glyph(glyph_id, LoadFlag::DEFAULT)
            .map_err(|_| Error::NoSuchGlyph)?;
        let slot = self.face.glyph();
        slot.render_glyph(RenderMode::Normal)
            .map_err(|_| Error::RenderFailed)?;

        let ft_bitmap = slot.bitmap();
        let width = ft_bitmap.width().max(0) as u32;
        let height = ft_bitmap.rows().max(0) as u32;
        let pitch = ft_bitmap.pitch();
        let buffer = ft_bitmap.buffer();

        // Repack from FreeType's (possibly padded / bottom-up) pitch into a
        // tightly packed `width * height` grayscale buffer. A positive pitch is
        // top-down; a negative pitch is bottom-up (rows stored in reverse).
        let mut data = vec![0u8; (width as usize) * (height as usize)];
        let abs_pitch = pitch.unsigned_abs() as usize;
        for row in 0..height as usize {
            let src_row = if pitch >= 0 {
                row
            } else {
                height as usize - 1 - row
            };
            let src_start = src_row * abs_pitch;
            let dst_start = row * width as usize;
            let n = (width as usize).min(abs_pitch);
            if src_start + n <= buffer.len() {
                data[dst_start..dst_start + n].copy_from_slice(&buffer[src_start..src_start + n]);
            }
        }

        Ok(Bitmap {
            width,
            height,
            bearing_x: slot.bitmap_left(),
            bearing_y: slot.bitmap_top(),
            format: PixelFormat::Alpha8,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Metrics;

    #[test]
    fn loads_embedded_and_derives_metrics() {
        let face = Face::load_embedded(16.0).expect("load embedded via FreeType");
        // The portable derivation is shared with the CoreText path, so these
        // must match the pinned values asserted by the crate's smoke test
        // (`lib.rs`), proving the FreeType face feeds the same metrics.
        let metrics = Metrics::calc(face.face_metrics());
        assert_eq!(metrics.cell_width, 10);
        assert_eq!(metrics.cell_height, 21);
        assert_eq!(metrics.cell_baseline, 5);
    }

    #[test]
    fn glyph_index_resolves_and_missing_is_none() {
        let face = Face::load_embedded(16.0).expect("load");
        assert!(face.glyph_index('H').is_some(), "'H' must have a glyph");
        // A codepoint JetBrains Mono has no glyph for (a CJK ideograph).
        assert!(
            face.glyph_index('\u{4e00}').is_none(),
            "unsupported codepoint must resolve to None"
        );
    }

    #[test]
    fn rasterizes_letter_with_ink_and_plausible_box() {
        let face = Face::load_embedded(32.0).expect("load");
        let gid = face.glyph_index('H').expect("'H' glyph");
        let bmp = face.rasterize(gid).expect("rasterize 'H'");

        assert_eq!(bmp.format, PixelFormat::Alpha8);
        assert!(bmp.width > 0 && bmp.height > 0, "H must have an ink box");
        assert_eq!(bmp.data.len(), (bmp.width * bmp.height) as usize);
        assert!(!bmp.is_blank(), "H must have coverage");
        // A capital 'H' at 32px sits above the baseline: its ink top is above
        // the baseline (positive bearing_y) and it has no left overhang.
        assert!(bmp.bearing_y > 0, "H ink should sit above the baseline");
    }

    #[test]
    fn space_is_blank_or_empty() {
        let face = Face::load_embedded(32.0).expect("load");
        if let Some(gid) = face.glyph_index(' ') {
            let bmp = face.rasterize(gid).expect("rasterize space");
            assert!(
                bmp.is_blank() || bmp.width == 0 || bmp.height == 0,
                "space must have no ink"
            );
        }
    }
}
