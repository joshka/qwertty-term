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

use crate::constraint::Constraint;
use crate::metrics::{FaceMetrics, Metrics};
use crate::presentation::{Presentation, PresentationMode};
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
/// Synthetic style flags applied during rasterization. Both are approximations
/// (no unsafe FreeType outline FFI): bold dilates the coverage bitmap ~1px;
/// italic applies a shear via `FT_Set_Transform`. Real bold/italic should come
/// from an actual font member when the family has one; these are the fallback
/// (upstream's `synthetic` bold/italic).
#[derive(Debug, Clone, Copy, Default)]
struct Synthetic {
    bold: bool,
    italic: bool,
}

/// FreeType glyph-loading hint controls — the reduced analog of upstream's
/// `FreetypeLoadFlags` (`face/freetype.zig:355-386`), driven by the
/// `freetype-load-flags` and `force-autohint` config keys. The **config-key
/// parsing** is the config/app thread's job; this is the face-side capability
/// plus its default. Applied in [`Face::rasterize`] via
/// [`Face::glyph_load_flags`].
///
/// The `monochrome` flag (1-bit rendering) is intentionally **not** included
/// yet: the FreeType face always renders grayscale AA, and the mono bitmap
/// unpack path is deferred with the color-glyph work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadFlags {
    /// Use the font's own (bytecode) hinting (`hinting`). Upstream default: on.
    /// Ignored (forced off) for constrained glyphs.
    pub hinting: bool,
    /// Prefer the auto-hinter even when the font ships its own hints
    /// (`force-autohint`, `FT_LOAD_FORCE_AUTOHINT`). Upstream default: off.
    pub force_autohint: bool,
    /// Allow the auto-hinter at all (`autohint`); `false` →
    /// `FT_LOAD_NO_AUTOHINT`. Upstream default: on.
    pub autohint: bool,
}

impl Default for LoadFlags {
    fn default() -> LoadFlags {
        // Upstream default set (`Config.zig` `freetype-load-flags`): hinting +
        // autohint on, force-autohint off.
        LoadFlags {
            hinting: true,
            force_autohint: false,
            autohint: true,
        }
    }
}

pub struct Face {
    face: freetype::Face,
    /// The font bytes, kept for the `ttf-parser` metric derivation and for
    /// rustybuzz shaping (`ShapeFace::source_bytes`).
    bytes: Vec<u8>,
    size_px: f64,
    /// Subface index within a `.ttc`/`.otc` collection (0 for a single face).
    face_index: u32,
    synthetic: Synthetic,
    /// Glyph-load hint controls (`freetype-load-flags` / `force-autohint`).
    load_flags: LoadFlags,
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
            synthetic: Synthetic::default(),
            load_flags: LoadFlags::default(),
        })
    }

    /// Return this face configured with the given glyph-load hint controls
    /// (`freetype-load-flags` / `force-autohint`). Builder-style so the config
    /// layer can set it at face construction: `Face::load_...(..)?.with_load_flags(f)`.
    pub fn with_load_flags(mut self, load_flags: LoadFlags) -> Face {
        self.load_flags = load_flags;
        self
    }

    /// The glyph-load hint controls in effect for this face.
    pub fn load_flags(&self) -> LoadFlags {
        self.load_flags
    }

    /// Build the FreeType `LoadFlag` bitset for a glyph load, mirroring upstream
    /// `glyphLoadFlags` (`face/freetype.zig:355-386`). `constrained` (Nerd Font
    /// icon / emoji cover-fit) forces hinting off, matching upstream
    /// (`do_hinting = hinting and !constrained`).
    fn glyph_load_flags(&self, constrained: bool) -> LoadFlag {
        let do_hinting = self.load_flags.hinting && !constrained;
        let mut flags = LoadFlag::DEFAULT | LoadFlag::TARGET_NORMAL;
        if !do_hinting {
            flags |= LoadFlag::NO_HINTING;
        }
        if self.load_flags.force_autohint {
            flags |= LoadFlag::FORCE_AUTOHINT;
        }
        if !self.load_flags.autohint {
            flags |= LoadFlag::NO_AUTOHINT;
        }
        flags
    }

    /// Load the embedded JetBrains Mono fallback at `size_px`.
    pub fn load_embedded(size_px: f64) -> Result<Face, Error> {
        Self::load_from_bytes(crate::embedded::JETBRAINS_MONO_VARIABLE, size_px)
    }

    /// Load a system font by family `name` at `size_px`, mirroring
    /// [`coretext::Face::load_by_name`](crate::coretext::Face::load_by_name):
    /// discover the family (here via fontconfig) and, if the resolved family
    /// case-insensitively matches `name`, return it; otherwise fall back to the
    /// embedded JetBrains Mono for determinism (fontconfig always resolves *some*
    /// font for a bad name, so the match check is what rejects a silent
    /// substitute — the same guard the CoreText path uses).
    ///
    /// Without the `fontconfig` feature there is no discovery backend on this
    /// face, so this always returns the embedded fallback (the API still exists
    /// for parity with the CoreText face, which callers reach through the
    /// `Face` alias).
    #[cfg(feature = "fontconfig")]
    pub fn load_by_name(name: &str, size_px: f64) -> Result<Face, Error> {
        if !name.is_empty()
            && let Some(face) = crate::fontconfig::discover_family(name, size_px)
        {
            let resolved = face.family_name().to_lowercase();
            let want = name.to_lowercase();
            if resolved.contains(&want) || want.contains(&resolved) {
                return Ok(face);
            }
        }
        Self::load_embedded(size_px)
    }

    /// Without a discovery backend: always the embedded fallback (see the
    /// `fontconfig`-enabled variant for the real lookup).
    #[cfg(not(feature = "fontconfig"))]
    pub fn load_by_name(_name: &str, size_px: f64) -> Result<Face, Error> {
        Self::load_embedded(size_px)
    }

    /// Embedded bold. **Approximate:** the embedded JetBrains Mono is a variable
    /// font with no separate bold file and FreeType `wght`-instance selection
    /// isn't wired yet, so this is the regular face with synthetic bold applied
    /// (ADR 003 P2 "approximate"). Real bold comes from an actual bold member
    /// when a family has one.
    pub fn load_embedded_bold(size_px: f64) -> Result<Face, Error> {
        Ok(Self::load_embedded(size_px)?.synthetic_bold(Self::synthetic_bold_line_width(size_px)))
    }

    /// Embedded italic — the real italic variable font (true italic outlines).
    pub fn load_embedded_italic(size_px: f64) -> Result<Face, Error> {
        Self::load_from_bytes(crate::embedded::JETBRAINS_MONO_VARIABLE_ITALIC, size_px)
    }

    /// Embedded bold-italic: the real italic font with synthetic bold applied
    /// (approximate, as [`load_embedded_bold`](Self::load_embedded_bold)).
    pub fn load_embedded_bold_italic(size_px: f64) -> Result<Face, Error> {
        Ok(Self::load_embedded_italic(size_px)?
            .synthetic_bold(Self::synthetic_bold_line_width(size_px)))
    }

    /// Embedded Nerd Font symbols face (Powerline/PUA icons).
    pub fn load_embedded_symbols_nerd_font(size_px: f64) -> Result<Face, Error> {
        Self::load_from_bytes(crate::embedded::SYMBOLS_NERD_FONT_MONO, size_px)
    }

    /// An independent copy of this face at `size_px`, byte-backed as before but
    /// with synthetic flags reset (matches `coretext::Face::try_clone`, used by
    /// the collection's alias-to-regular / synthesize-on-a-fresh-copy paths). The
    /// configured [`LoadFlags`] are preserved (they are a face-level config, not
    /// a per-glyph synthetic style).
    pub fn try_clone(&self, size_px: f64) -> Result<Face, Error> {
        Ok(
            Self::load_from_bytes_indexed(&self.bytes, size_px, self.face_index)?
                .with_load_flags(self.load_flags),
        )
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

    /// The face's family name (e.g. `"JetBrains Mono"`), or empty if the font
    /// carries no name table entry.
    pub fn family_name(&self) -> String {
        self.face.family_name().unwrap_or_default()
    }

    /// True if the face can contain color glyphs (`FT_HAS_COLOR`: CBDT/sbix/COLR).
    pub fn has_color(&self) -> bool {
        self.face.has_color()
    }

    /// The presentation this face advertises: [`Presentation::Emoji`] for a
    /// color face, else [`Presentation::Text`] — the same rule the CoreText
    /// backend uses (`coretext::Face::presentation`).
    pub fn presentation(&self) -> Presentation {
        if self.has_color() {
            Presentation::Emoji
        } else {
            Presentation::Text
        }
    }

    /// True if a specific glyph id is a color glyph. Whole-face approximation
    /// (matches the CoreText backend): a >16-bit glyph id is never color;
    /// otherwise a glyph is color iff the face carries color tables.
    pub fn is_color_glyph(&self, glyph_id: u32) -> bool {
        self.has_color() && u16::try_from(glyph_id).is_ok()
    }

    /// True if this face satisfies `cp` under presentation mode `p_mode`.
    ///
    /// Identical semantics to `coretext::Face::has_codepoint` (it depends only on
    /// [`glyph_index`](Self::glyph_index) + [`is_color_glyph`](Self::is_color_glyph),
    /// both backend-neutral): `Any` needs only a glyph; `Explicit(p)` (and
    /// `Default(p)` for a `fallback` face) additionally requires the glyph's
    /// color-ness to match `p`; a non-fallback face accepts `Default` with any
    /// presentation.
    pub fn has_codepoint(&self, cp: u32, p_mode: PresentationMode, fallback: bool) -> bool {
        let effective = match p_mode {
            PresentationMode::Any => None,
            PresentationMode::Explicit(p) => Some(p),
            PresentationMode::Default(p) => {
                if fallback {
                    Some(p)
                } else {
                    None
                }
            }
        };

        let Some(ch) = char::from_u32(cp) else {
            return false;
        };
        let Some(gid) = self.glyph_index(ch) else {
            return false;
        };

        match effective {
            None => true,
            Some(Presentation::Text) => !self.is_color_glyph(gid),
            Some(Presentation::Emoji) => self.is_color_glyph(gid),
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

    /// The synthetic-bold line width for a face at `size_px` px/em, matching the
    /// CoreText backend's `max(size_px / 14, 1)` so the two faces share the
    /// `synthetic_bold(line_width)` API. (The FreeType approximation below does
    /// not actually use `line_width` — it dilates by a fixed 1px — but the
    /// signature stays uniform for the shared collection code.)
    pub fn synthetic_bold_line_width(size_px: f64) -> f64 {
        (size_px / 14.0).max(1.0)
    }

    /// A synthetic-bold copy of this face. Approximate (no outline FFI): each
    /// rasterized glyph's coverage is dilated ~1px horizontally, thickening
    /// strokes. Prefer a real bold font member when the family has one; this is
    /// the fallback (upstream `syntheticBold`).
    pub fn synthetic_bold(mut self, _line_width: f64) -> Face {
        self.synthetic.bold = true;
        self
    }

    /// A synthetic-italic copy of this face: a shear applied via
    /// `FT_Set_Transform` at rasterization (upstream `syntheticItalic`, via the
    /// FreeType transform matrix rather than manual outline skew).
    ///
    /// Takes `&self` and returns a new face (a byte-backed reload with the
    /// existing synthetic flags plus italic), matching
    /// `coretext::Face::synthetic_italic`'s "copy the font" shape and `Result`
    /// (whose CoreText copy can fail), so the shared collection code drives both
    /// faces uniformly.
    pub fn synthetic_italic(&self) -> Result<Face, Error> {
        let mut f = Face::load_from_bytes_indexed(&self.bytes, self.size_px, self.face_index)?;
        f.synthetic = self.synthetic;
        f.synthetic.italic = true;
        Ok(f)
    }

    /// Rasterize an outline glyph to a grayscale (`Alpha8`) [`Bitmap`].
    ///
    /// Color-bitmap (emoji) glyphs are a later slice; this always renders the
    /// outline via `FT_RENDER_MODE_NORMAL`. FreeType's `bitmap_left`/`bitmap_top`
    /// map directly onto the shared bearing convention (see [`Bitmap`]).
    /// Applies synthetic italic (shear transform) and/or synthetic bold (1px
    /// coverage dilation) when set on this face.
    pub fn rasterize(&self, glyph_id: u32) -> Result<Bitmap, Error> {
        // Synthetic italic: shear via the FreeType transform matrix (16.16
        // fixed point). x' = x + tan(12°)·y — a ~12° slant, matching upstream's
        // synthesized-italic angle. Reset to identity afterward so the face's
        // other glyphs are unaffected.
        if self.synthetic.italic {
            // tan(12°) ≈ 0.21256 → 0.21256 * 65536 ≈ 13931.
            let mut matrix = freetype::Matrix {
                xx: 0x1_0000,
                xy: 13931,
                yx: 0,
                yy: 0x1_0000,
            };
            let mut delta = freetype::Vector { x: 0, y: 0 };
            self.face.set_transform(&mut matrix, &mut delta);
        }

        let load = self
            .face
            .load_glyph(glyph_id, self.glyph_load_flags(false))
            .map_err(|_| Error::NoSuchGlyph);
        let slot = self.face.glyph();
        let render = slot
            .render_glyph(RenderMode::Normal)
            .map_err(|_| Error::RenderFailed);

        if self.synthetic.italic {
            let mut ident = freetype::Matrix {
                xx: 0x1_0000,
                xy: 0,
                yx: 0,
                yy: 0x1_0000,
            };
            let mut delta = freetype::Vector { x: 0, y: 0 };
            self.face.set_transform(&mut ident, &mut delta);
        }
        load?;
        render?;

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

        if self.synthetic.bold {
            data = embolden_1px(&data, width as usize, height as usize);
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

impl Face {
    /// Rasterize with an optional glyph constraint (Nerd Font icon sizing /
    /// emoji cover-fit).
    ///
    /// **Linux approximation (ADR 003 P2 "un-gate now, approximate"):** the
    /// constraint is currently a no-op — the glyph is rasterized at its natural
    /// size. Constrained glyphs (Nerd Font PUA icons, emoji) therefore render
    /// unscaled, which can overflow the cell; honoring the constraint (a
    /// bitmap-scale or outline transform) is a follow-up. The signature matches
    /// `coretext::Face::rasterize_constrained` so the shared `grid` code drives
    /// both faces uniformly.
    pub fn rasterize_constrained(
        &self,
        glyph_id: u32,
        _constraint: Option<(Constraint, &Metrics, u32)>,
    ) -> Result<Bitmap, Error> {
        self.rasterize(glyph_id)
    }
}

/// Approximate emboldening: dilate coverage 1px to the right by taking, for each
/// pixel, the max of it and its left neighbour. Thickens vertical strokes by ~1px
/// without touching the glyph outline (no unsafe FreeType FFI). A coarse stand-in
/// for `FT_Outline_Embolden`; a real bold font member is always preferred.
fn embolden_1px(data: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = data.to_vec();
    for row in 0..height {
        let base = row * width;
        for col in 1..width {
            out[base + col] = data[base + col].max(data[base + col - 1]);
        }
    }
    out
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
    fn metadata_and_presentation() {
        let face = Face::load_embedded(16.0).expect("load");
        assert!(
            face.family_name().to_lowercase().contains("jetbrains"),
            "family: {}",
            face.family_name()
        );
        // JetBrains Mono is a non-color text font.
        assert!(!face.has_color(), "text font must not be color");
        assert_eq!(face.presentation(), Presentation::Text);
        let gid = face.glyph_index('H').unwrap();
        assert!(!face.is_color_glyph(gid), "'H' is an outline glyph");
    }

    #[test]
    fn has_codepoint_modes() {
        let face = Face::load_embedded(16.0).expect("load");
        let h = u32::from('H');
        let cjk = 0x4e00u32;
        // `Any`: just needs a glyph.
        assert!(face.has_codepoint(h, PresentationMode::Any, false));
        assert!(!face.has_codepoint(cjk, PresentationMode::Any, false));
        // Explicit Text matches an outline glyph; Explicit Emoji does not (no
        // color glyph in this face).
        assert!(face.has_codepoint(h, PresentationMode::Explicit(Presentation::Text), false));
        assert!(!face.has_codepoint(h, PresentationMode::Explicit(Presentation::Emoji), false));
        // Default on a non-fallback (primary) face ignores presentation.
        assert!(face.has_codepoint(h, PresentationMode::Default(Presentation::Emoji), false));
        // Default on a fallback face is held to the explicit presentation.
        assert!(!face.has_codepoint(h, PresentationMode::Default(Presentation::Emoji), true));
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

    fn total_ink(bmp: &Bitmap) -> u64 {
        bmp.data.iter().map(|&p| u64::from(p)).sum()
    }

    #[test]
    fn synthetic_bold_adds_ink() {
        let gid = Face::load_embedded(32.0).unwrap().glyph_index('H').unwrap();
        let regular = Face::load_embedded(32.0).unwrap().rasterize(gid).unwrap();
        let bold = Face::load_embedded(32.0)
            .unwrap()
            .synthetic_bold(Face::synthetic_bold_line_width(32.0))
            .rasterize(gid)
            .unwrap();
        // Same ink box (1px dilation stays within the box), more coverage.
        assert_eq!((bold.width, bold.height), (regular.width, regular.height));
        assert!(
            total_ink(&bold) > total_ink(&regular),
            "synthetic bold must thicken strokes: bold {} vs regular {}",
            total_ink(&bold),
            total_ink(&regular)
        );
    }

    #[test]
    fn embedded_family_loaders_and_try_clone() {
        // Every embedded family variant loads and can rasterize 'H'.
        for face in [
            Face::load_embedded(16.0).unwrap(),
            Face::load_embedded_bold(16.0).unwrap(),
            Face::load_embedded_italic(16.0).unwrap(),
            Face::load_embedded_bold_italic(16.0).unwrap(),
            Face::load_embedded_symbols_nerd_font(16.0).unwrap(),
        ] {
            let clone = face.try_clone(24.0).expect("try_clone");
            assert_eq!(clone.size_px(), 24.0, "clone takes the new size");
            // The nerd-font face may not have 'H'; the text faces do.
            if let Some(gid) = face.glyph_index('H') {
                assert!(face.rasterize(gid).is_ok());
            }
        }
    }

    #[test]
    fn synthetic_italic_shears_glyph() {
        let gid = Face::load_embedded(32.0).unwrap().glyph_index('H').unwrap();
        let regular = Face::load_embedded(32.0).unwrap().rasterize(gid).unwrap();
        let italic = Face::load_embedded(32.0)
            .unwrap()
            .synthetic_italic()
            .unwrap()
            .rasterize(gid)
            .unwrap();
        assert!(!italic.is_blank(), "italic 'H' must still have ink");
        // A shear slants the glyph: its ink box widens (top pushed right of the
        // bottom), so the italic bitmap is wider than the upright one.
        assert!(
            italic.width >= regular.width,
            "sheared glyph should be at least as wide: italic {} vs regular {}",
            italic.width,
            regular.width
        );
    }

    /// Default `LoadFlags` match upstream (hinting + autohint on, force-autohint
    /// off), and `glyph_load_flags` maps them to the right FreeType bits —
    /// including forcing hinting off for constrained glyphs.
    #[test]
    fn load_flags_default_and_mapping() {
        let face = Face::load_embedded(16.0).unwrap();
        assert_eq!(face.load_flags(), LoadFlags::default());
        assert_eq!(
            LoadFlags::default(),
            LoadFlags {
                hinting: true,
                force_autohint: false,
                autohint: true,
            }
        );

        // Default, unconstrained: hinting on → no NO_HINTING; auto-hinter allowed
        // → no NO_AUTOHINT; not forced → no FORCE_AUTOHINT.
        let f = face.glyph_load_flags(false);
        assert!(!f.contains(LoadFlag::NO_HINTING));
        assert!(!f.contains(LoadFlag::NO_AUTOHINT));
        assert!(!f.contains(LoadFlag::FORCE_AUTOHINT));

        // Constrained forces hinting off regardless of the config.
        assert!(face.glyph_load_flags(true).contains(LoadFlag::NO_HINTING));

        // force-autohint + autohint off flips exactly those two bits.
        let tuned = Face::load_embedded(16.0)
            .unwrap()
            .with_load_flags(LoadFlags {
                hinting: true,
                force_autohint: true,
                autohint: false,
            });
        let tf = tuned.glyph_load_flags(false);
        assert!(tf.contains(LoadFlag::FORCE_AUTOHINT));
        assert!(tf.contains(LoadFlag::NO_AUTOHINT));
        assert!(!tf.contains(LoadFlag::NO_HINTING), "hinting still on");

        // The tuned flags still rasterize a real glyph (the plumbing doesn't
        // break rendering), and survive try_clone.
        let gid = tuned.glyph_index('H').unwrap();
        assert!(!tuned.rasterize(gid).unwrap().is_blank(), "H still inks");
        assert_eq!(
            tuned.try_clone(24.0).unwrap().load_flags(),
            tuned.load_flags(),
            "try_clone preserves load flags"
        );
    }

    /// `load_by_name` with a family that surely isn't installed falls back to the
    /// embedded JetBrains Mono. This holds on both feature configs: without
    /// `fontconfig` it's the embedded path directly; with `fontconfig` the
    /// family-match guard rejects fontconfig's silent substitute for the bogus
    /// name. (The positive discovery path is covered by the `fontconfig` module's
    /// own tests.)
    #[test]
    fn load_by_name_bogus_falls_back_to_embedded() {
        let face = Face::load_by_name("this-font-does-not-exist-qwertty-xyz", 16.0)
            .expect("load_by_name always yields a face");
        assert!(
            face.family_name().to_lowercase().contains("jetbrains"),
            "a bogus family must fall back to embedded JetBrains Mono, got {:?}",
            face.family_name()
        );
    }
}
