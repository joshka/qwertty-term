//! CoreText face loading and CoreGraphics glyph rasterization (macOS).
//!
//! Port of the load + rasterize + metric-extraction paths of Ghostty's
//! `src/font/face/coretext.zig` (commit `2da015cd6`), reduced to the
//! F5-reduced scope: load-by-name discovery (single `CTFontDescriptor` from a
//! family name, falling back to embedded JetBrains Mono on a miss), glyph
//! lookup, single-glyph rasterization through a CoreGraphics bitmap context
//! matching upstream's context settings, and `FaceMetrics` extraction
//! reconciled with the F1 table-derived layer.
//!
//! See `docs/analysis/font-coretext.md` for the commit-stamped, line-referenced
//! analysis this mirrors: the CTFont load path (§1), the CoreText-accessor vs
//! sfnt-table reconciliation rule (§2), the rasterization pipeline and bitmap
//! context config (§3), and color-glyph detection (§4).
//!
//! Out of scope (deferred, per the plan): nerd-font `constrain(...)`, sbix
//! pixel quantization, synthetic italic/bold *wiring* (the transforms are
//! present but not exposed on the reduced API), the full discovery `Score`
//! ranking, and shaping.

#![cfg(target_os = "macos")]

use std::ptr::NonNull;

use objc2_core_foundation::{CFData, CFRetained, CFString, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColorSpace, CGContext, CGImageAlphaInfo, CGImageByteOrderInfo};
use objc2_core_text::{
    CTFont, CTFontDescriptor, CTFontManagerCreateFontDescriptorFromData, CTFontOrientation,
    CTFontSymbolicTraits, CTFontTableOptions,
};

use crate::embedded;
use crate::metrics::{FaceMetrics, Metrics};

/// The size and position of a glyph in cell-relative pixel space (baseline
/// folded into `y`). Port of `Glyph.Size` (`Glyph.zig:24-29`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphSize {
    pub width: f64,
    pub height: f64,
    pub x: f64,
    pub y: f64,
}

/// The emoji cell-fit constraint. A reduced port of upstream's
/// `RenderOptions.Constraint` (`Glyph.zig`) carrying only the branches the
/// color-glyph (emoji) path uses: `size = .cover`, `align_* = .center`, and
/// symmetric horizontal padding. `SharedGrid.renderGlyph` (upstream) applies
/// exactly this constraint to every emoji glyph before rasterizing, which is
/// what scales an Apple-Color-Emoji bitmap (authored far larger than a cell)
/// down to cover the cell box while preserving aspect ratio and centering it.
///
/// The nerd-font-specific sizes (`fit`/`fit_cover1`/`stretch`), the `.icon`
/// height metric, and the relative-scale-group machinery are intentionally
/// omitted (nerd-font constraint tables are a separate deferral); for the
/// emoji case the scale group is the glyph itself (`relative_* = identity`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmojiConstraint {
    pad_left: f64,
    pad_right: f64,
}

impl EmojiConstraint {
    /// The exact constraint upstream hardcodes for emoji in
    /// `SharedGrid.renderGlyph`: `.cover`, centered on both axes, with a small
    /// 2.5% horizontal pad so the glyph doesn't touch the cell edges.
    pub const EMOJI: EmojiConstraint = EmojiConstraint {
        pad_left: 0.025,
        pad_right: 0.025,
    };

    /// Apply the constraint to `glyph` (cell-relative ink box) given `metrics`
    /// and the number of cells `constraint_width` the glyph may occupy. Port of
    /// `Constraint.constrain`/`constrainInner` reduced to the emoji case
    /// (`size = .cover`, `align_* = .center`; the scale group is the glyph
    /// itself). Returns the scaled + centered size/position.
    pub fn constrain(
        self,
        glyph: GlyphSize,
        metrics: &Metrics,
        constraint_width: u32,
    ) -> GlyphSize {
        // The emoji constraint never stretches, so the scale group is the glyph
        // itself (relative_* identity). `max_constraint_width` upstream is 2.
        let min_constraint_width = constraint_width.clamp(1, 2);

        let mut group = glyph;

        // Prescribed scaling (`.cover`), preserving the group center.
        let factor = self.cover_factor(group, metrics, min_constraint_width);
        let center_x = group.x + group.width / 2.0;
        let center_y = group.y + group.height / 2.0;
        group.width *= factor;
        group.height *= factor;
        group.x = center_x - group.width / 2.0;
        group.y = center_y - group.height / 2.0;

        // Prescribed alignment (`center` on both axes).
        group.y = self.aligned_y_center(group, metrics);
        group.x = self.aligned_x_center(group, metrics, min_constraint_width);

        group
    }

    /// The `.cover` scale factor (uniform, aspect-preserving): scale so the
    /// glyph covers the padded target box, taking the smaller of the two axis
    /// factors. Port of `scale_factors` for `size = .cover`.
    fn cover_factor(self, group: GlyphSize, metrics: &Metrics, min_constraint_width: u32) -> f64 {
        let pad_width_factor = min_constraint_width as f64 - (self.pad_left + self.pad_right);
        // `pad_top`/`pad_bottom` are 0 for the emoji constraint.
        let pad_height_factor = 1.0;

        let target_width = pad_width_factor * metrics.face_width;
        let target_height = pad_height_factor * metrics.face_height;

        let width_factor = target_width / group.width;
        let height_factor = target_height / group.height;
        // `.cover`: min of the two, applied uniformly.
        width_factor.min(height_factor)
    }

    /// Vertical center alignment. Port of `aligned_y` for `align_vertical =
    /// .center`.
    fn aligned_y_center(self, group: GlyphSize, metrics: &Metrics) -> f64 {
        // pad_top/pad_bottom are 0 for emoji.
        let start_y = metrics.face_y;
        let end_y = metrics.face_y + (metrics.face_height - group.height);
        (start_y + end_y) / 2.0
    }

    /// Horizontal center alignment. Port of `aligned_x` for `align_horizontal =
    /// .center`.
    fn aligned_x_center(
        self,
        group: GlyphSize,
        metrics: &Metrics,
        min_constraint_width: u32,
    ) -> f64 {
        let full_face_span =
            metrics.face_width + ((min_constraint_width - 1) * metrics.cell_width) as f64;
        let pad_left_dx = self.pad_left * metrics.face_width;
        let pad_right_dx = self.pad_right * metrics.face_width;
        let start_x = pad_left_dx;
        let end_x = full_face_span - group.width - pad_right_dx;
        start_x.max((start_x + end_x) / 2.0)
    }
}

/// The pixel format of a rasterized [`Bitmap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 1 byte per pixel: coverage/alpha only. Produced for non-color
    /// (outline) glyphs via a `kCGImageAlphaOnly` linear-gray context.
    Alpha8,
    /// 4 bytes per pixel: premultiplied little-endian BGRA in Display P3.
    /// Produced for color glyphs (sbix/SVG emoji).
    Bgra,
}

/// A rasterized glyph bitmap in CPU memory.
///
/// Positions mirror upstream `font.Glyph` (coretext.zig:559-566): `bearing_x`
/// is the distance from the left of the cell to the left of the ink box, and
/// `bearing_y` is the distance from the **bottom** of the cell to the **top**
/// of the ink box (baseline-relative, +Y up). Atlas upload is the caller's
/// responsibility (F6/renderer).
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

/// Errors from loading or rasterizing a CoreText face.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The embedded fallback font could not be turned into a CTFont (should
    /// never happen with the bundled bytes; indicates a broken build).
    EmbeddedFontLoadFailed,
    /// The requested glyph id has no glyph in this face.
    NoSuchGlyph,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::EmbeddedFontLoadFailed => write!(f, "failed to load embedded fallback font"),
            Error::NoSuchGlyph => write!(f, "no such glyph in face"),
        }
    }
}

impl std::error::Error for Error {}

/// The matrix applied to a regular font to auto-italicize it
/// (coretext.zig:42-49). Not applied by the reduced API, kept for parity and
/// future synthetic-italic support.
pub const ITALIC_SKEW: objc2_core_foundation::CGAffineTransform =
    objc2_core_foundation::CGAffineTransform {
        a: 1.0,
        b: 0.0,
        c: 0.267949, // approx. tan(15deg)
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

/// A loaded CoreText font face at a specific pixel size.
pub struct Face {
    /// The underlying CTFont (retained).
    font: CFRetained<CTFont>,
    /// The raw font bytes this face was loaded from, if we have them. Present
    /// for embedded fonts and used for the table-derived half of the metrics
    /// reconciliation (see [`Face::face_metrics`]). `None` for system fonts
    /// loaded by name, where we rely on CoreText's table copies instead.
    source_bytes: Option<&'static [u8]>,
    /// Whether the face can contain color glyphs (symbolic-traits gate +
    /// sbix presence). See coretext.zig:103-107, 890-968.
    color: Option<ColorState>,
}

/// Color-glyph detection state (coretext.zig:890-968).
///
/// A face carrying any recognized color-glyph table renders every glyph as
/// color. Upstream's reduced check keys on `sbix` (Apple's bitmap emoji);
/// F5-full extends this to the other common color formats so discovered system
/// emoji fonts — Noto Color Emoji (`COLR`/`CPAL` or `CBDT`/`CBLC`), Apple Color
/// Emoji (`sbix`) — are all recognized as color. Per-glyph SVG-table membership
/// is still deferred (treated as whole-face color when the `SVG ` table is
/// present).
struct ColorState {
    /// True if the face has any non-empty color-glyph table
    /// (`sbix` | `COLR` | `CBDT` | `SVG `). Upstream treats such presence as
    /// "every glyph is color" (coretext.zig:909-959).
    color_table: bool,
}

impl Face {
    /// Load a font by family name at `size_px` pixels.
    ///
    /// Uses `CTFontDescriptorCreateWithNameAndSize` +
    /// `CTFontCreateWithFontDescriptor` (the descriptor-from-name shortcut the
    /// upstream tests use, coretext.zig:1003-1008). Because
    /// `CTFontDescriptorCreateWithNameAndSize` never fails on a bad name (it
    /// resolves to a system default), we read back the resolved family name and
    /// fall back to the embedded JetBrains Mono if it does not
    /// case-insensitively contain `name` (see `docs/analysis/font-coretext.md`
    /// §1.1).
    pub fn load_by_name(name: &str, size_px: f64) -> Result<Face, Error> {
        let cf_name = CFString::from_str(name);
        // SAFETY: cf_name is a valid CFString; size is a finite CGFloat.
        let desc = unsafe { CTFontDescriptor::with_name_and_size(&cf_name, size_px as CGFloat) };
        // SAFETY: desc is a valid descriptor; null matrix = identity.
        let font =
            unsafe { CTFont::with_font_descriptor(&desc, size_px as CGFloat, std::ptr::null()) };

        // Verify the resolved family actually matches the request; otherwise
        // CoreText silently substituted a system default and we prefer the
        // embedded fallback for determinism.
        let resolved = family_name(&font);
        let matched = resolved.to_lowercase().contains(&name.to_lowercase())
            || name.to_lowercase().contains(&resolved.to_lowercase());

        if matched && !name.is_empty() {
            Ok(Face::from_ct_font(font, None))
        } else {
            Face::load_embedded(size_px)
        }
    }

    /// Load the embedded JetBrains Mono at `size_px` pixels, via
    /// `CTFontManagerCreateFontDescriptorFromData` (coretext.zig:51-70).
    pub fn load_embedded(size_px: f64) -> Result<Face, Error> {
        Face::load_from_bytes(embedded::JETBRAINS_MONO, size_px)
    }

    /// Load a face from in-memory font bytes at `size_px` pixels.
    ///
    /// `bytes` must outlive the returned face (`'static` here, satisfied by the
    /// embedded fonts). We keep them for the table-derived metrics half.
    pub fn load_from_bytes(bytes: &'static [u8], size_px: f64) -> Result<Face, Error> {
        // Copy the bytes into a CFData (owns its own storage, so we don't rely
        // on `bytes` staying put for CoreText's sake -- though it is 'static).
        // SAFETY: bytes is a valid slice; null allocator = default.
        let data = unsafe {
            CFData::new(
                None,
                bytes.as_ptr(),
                bytes.len() as objc2_core_foundation::CFIndex,
            )
        }
        .ok_or(Error::EmbeddedFontLoadFailed)?;

        // SAFETY: data is a valid CFData containing font bytes.
        let desc = unsafe { CTFontManagerCreateFontDescriptorFromData(&data) }
            .ok_or(Error::EmbeddedFontLoadFailed)?;

        // SAFETY: desc is a valid descriptor; null matrix = identity.
        let font =
            unsafe { CTFont::with_font_descriptor(&desc, size_px as CGFloat, std::ptr::null()) };

        Ok(Face::from_ct_font(font, Some(bytes)))
    }

    /// Copy an existing `CTFont` at a new pixel size into a [`Face`]
    /// (coretext.zig `initFontCopy`; DeferredFace `loadCoreText`).
    ///
    /// Used by [`crate::deferred::DeferredFace::load`] to materialize a
    /// discovered system font at the render size. The resulting face has no
    /// `source_bytes` (system faces aren't byte-backed), so its metrics come
    /// from CoreText accessors.
    pub(crate) fn from_ct_font_at_size(font: &CTFont, size_px: f64) -> Result<Face, Error> {
        // SAFETY: font is a valid CTFont; null matrix = identity; no descriptor
        // overrides. Analog of upstream `base.copyWithAttributes(size, null,
        // null)`.
        let copy = unsafe { font.copy_with_attributes(size_px as CGFloat, std::ptr::null(), None) };
        Ok(Face::from_ct_font(copy, None))
    }

    fn from_ct_font(font: CFRetained<CTFont>, source_bytes: Option<&'static [u8]>) -> Face {
        // SAFETY: font is a valid CTFont.
        let traits = unsafe { font.symbolic_traits() };
        let color = if traits.contains(CTFontSymbolicTraits::TraitColorGlyphs) {
            let color_table = has_table(&font, b"sbix")
                || has_table(&font, b"COLR")
                || has_table(&font, b"CBDT")
                || has_table(&font, b"SVG ");
            Some(ColorState { color_table })
        } else {
            None
        };

        Face {
            font,
            source_bytes,
            color,
        }
    }

    /// The resolved family name of this face.
    pub fn family_name(&self) -> String {
        family_name(&self.font)
    }

    /// The pixel-per-em size CoreText reports for this face
    /// (`CTFontGetSize`, coretext.zig:639).
    pub fn size_px(&self) -> f64 {
        // SAFETY: font is a valid CTFont. CGFloat is f64 on macOS.
        unsafe { self.font.size() }
    }

    /// The raw font bytes this face was loaded from, if available.
    ///
    /// Present for faces loaded from in-memory bytes (all embedded fonts) and
    /// `None` for system faces loaded by name. The shaper uses these to build a
    /// `rustybuzz::Face` over the same bytes (decision 1); for name-loaded
    /// faces, byte-backed shaping is a deferred completeness pass (a CoreText
    /// shaper, or copying CoreText's table data, would be needed).
    pub fn source_bytes(&self) -> Option<&'static [u8]> {
        self.source_bytes
    }

    /// True if the face can contain color glyphs.
    pub fn has_color(&self) -> bool {
        self.color.is_some()
    }

    /// Borrow the underlying `CTFont` (for discovery: building a per-codepoint
    /// substitute font via `CTFontCreateForString`).
    pub(crate) fn ct_font(&self) -> &CTFont {
        &self.font
    }

    /// The presentation this face advertises via its symbolic traits: a face
    /// with the color-glyph trait presents as [`Presentation::Emoji`], else
    /// [`Presentation::Text`] (DeferredFace.zig:370-373, Collection.zig gate).
    pub fn presentation(&self) -> crate::presentation::Presentation {
        if self.has_color() {
            crate::presentation::Presentation::Emoji
        } else {
            crate::presentation::Presentation::Text
        }
    }

    /// True if this face satisfies `cp` under presentation mode `p_mode`,
    /// mirroring the *loaded-face* arm of upstream `Entry.hasCodepoint`
    /// (Collection.zig:816-831).
    ///
    /// - [`PresentationMode::Any`]: the face merely has a glyph for `cp`.
    /// - [`PresentationMode::Explicit`] / [`PresentationMode::Default`]: the
    ///   face has a glyph AND its glyph's color-ness matches the requested
    ///   presentation (`Text ⇒ !is_color_glyph`, `Emoji ⇒ is_color_glyph`).
    ///
    /// `fallback` selects the default-mode asymmetry (Collection.zig:808-814):
    /// a non-fallback (user/primary) face accepts a `Default(p)` request with
    /// any presentation, whereas a fallback (discovered) face is held to the
    /// explicit `p`.
    pub fn has_codepoint(
        &self,
        cp: u32,
        p_mode: crate::presentation::PresentationMode,
        fallback: bool,
    ) -> bool {
        use crate::presentation::PresentationMode;

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
            Some(crate::presentation::Presentation::Text) => !self.is_color_glyph(gid),
            Some(crate::presentation::Presentation::Emoji) => self.is_color_glyph(gid),
        }
    }

    /// True if a specific glyph id is a color glyph (coretext.zig:264-267,
    /// 952-967).
    ///
    /// A >16-bit glyph id is never color (special ids). Otherwise a glyph is
    /// color iff the face carries a color-glyph table (sbix/COLR/CBDT/SVG) —
    /// the whole-face approximation upstream also uses for sbix.
    pub fn is_color_glyph(&self, glyph_id: u32) -> bool {
        let Some(color) = &self.color else {
            return false;
        };
        if u16::try_from(glyph_id).is_err() {
            return false;
        }
        color.color_table
    }

    /// Look up the glyph id for a Unicode scalar, or `None` if the face has no
    /// glyph for it (coretext.zig:271-287).
    ///
    /// Turns the code point into UTF-16 (a surrogate pair for astral code
    /// points) and calls `CTFontGetGlyphsForCharacters`; a pair is expected to
    /// decode to exactly one glyph (the second slot must be 0).
    pub fn glyph_index(&self, c: char) -> Option<u32> {
        let mut utf16 = [0u16; 2];
        let encoded = c.encode_utf16(&mut utf16);
        let len = encoded.len();

        let mut glyphs = [0u16; 2];
        // SAFETY: both buffers have length >= len (== 1 or 2); pointers valid
        // for `len` elements.
        let ok = unsafe {
            self.font.glyphs_for_characters(
                NonNull::new(utf16.as_mut_ptr()).unwrap(),
                NonNull::new(glyphs.as_mut_ptr()).unwrap(),
                len as objc2_core_foundation::CFIndex,
            )
        };
        if !ok {
            return None;
        }
        if glyphs[0] == 0 {
            return None;
        }
        Some(glyphs[0] as u32)
    }

    /// Rasterize a single glyph into a CPU [`Bitmap`], mirroring upstream's
    /// `renderGlyph` bitmap-context configuration (coretext.zig:289-567).
    ///
    /// Renders at the glyph's natural size (no nerd-font constraint, no
    /// synthetic bold/italic, `thicken=false`). To apply the emoji cell-fit
    /// constraint, use [`Face::rasterize_constrained`]. Returns a zero-sized
    /// blank bitmap for glyphs with no ink (spaces, control chars).
    pub fn rasterize(&self, glyph_id: u32) -> Result<Bitmap, Error> {
        self.rasterize_constrained(glyph_id, None)
    }

    /// Rasterize a glyph, optionally applying a cell-fit constraint.
    ///
    /// When `constraint` is `Some((metrics, constraint_width))` **and** the
    /// glyph is a color (emoji) glyph, upstream's emoji constraint
    /// ([`EmojiConstraint::EMOJI`]) is applied: the authored ink box (which for
    /// Apple Color Emoji is far larger than a cell) is scaled to cover the cell
    /// box preserving aspect ratio and centered, and the CoreGraphics draw is
    /// scaled to match, so the rasterized bitmap is cell-sized. Non-color
    /// glyphs are unaffected (they already fit the cell), matching upstream —
    /// `SharedGrid.renderGlyph` only sets a constraint on the `.emoji`
    /// presentation. Port of the `constrain(...)` block in
    /// `face/coretext.zig:336-390`.
    pub fn rasterize_constrained(
        &self,
        glyph_id: u32,
        constraint: Option<(&Metrics, u32)>,
    ) -> Result<Bitmap, Error> {
        let glyph16 = u16::try_from(glyph_id).map_err(|_| Error::NoSuchGlyph)?;
        let mut glyphs = [glyph16; 1];
        let glyphs_ptr = NonNull::new(glyphs.as_mut_ptr()).unwrap();

        // 1. Ink bounding rect: CoreGraphics space, origin bottom-left, +Y up.
        // SAFETY: single-element glyph buffer; null bounding_rects => only the
        // overall rect is computed and returned.
        let rect: CGRect = unsafe {
            self.font.bounding_rects_for_glyphs(
                CTFontOrientation::Horizontal,
                glyphs_ptr,
                std::ptr::null_mut(),
                1,
            )
        };

        let is_color = self.is_color_glyph(glyph_id);

        // 4. Empty-glyph short circuit (coretext.zig:326-334).
        if rect.size.width < 0.25 || rect.size.height < 0.25 {
            return Ok(Bitmap {
                width: 0,
                height: 0,
                bearing_x: 0,
                bearing_y: 0,
                format: if is_color {
                    PixelFormat::Bgra
                } else {
                    PixelFormat::Alpha8
                },
                data: Vec::new(),
            });
        }

        // 5. Apply the emoji cell-fit constraint (coretext.zig:336-380). We
        // fold the baseline into `y` (the constraint operates on cell-relative,
        // not baseline-relative, positions), constrain, then read back the
        // scaled size and cell-relative origin. When no constraint applies
        // (non-color glyph, or no metrics supplied), this leaves the raw ink
        // rect untouched — matching the reduced natural-size path.
        let (width, height, x, y, scale_w, scale_h) = match constraint {
            Some((metrics, constraint_width)) if is_color => {
                let cell_baseline = f64::from(metrics.cell_baseline);
                let constrained = EmojiConstraint::EMOJI.constrain(
                    GlyphSize {
                        width: rect.size.width,
                        height: rect.size.height,
                        x: rect.origin.x,
                        y: rect.origin.y + cell_baseline,
                    },
                    metrics,
                    constraint_width,
                );
                // Undo the baseline fold to get back a CoreGraphics-space y
                // origin for canvas sizing (the +Y-up space `rect` lives in).
                (
                    constrained.width,
                    constrained.height,
                    constrained.x,
                    constrained.y - cell_baseline,
                    constrained.width / rect.size.width,
                    constrained.height / rect.size.height,
                )
            }
            // No constraint: natural size, identity scale.
            _ => (
                rect.size.width,
                rect.size.height,
                rect.origin.x,
                rect.origin.y,
                1.0,
                1.0,
            ),
        };

        // 6. Sub-pixel canvas sizing (coretext.zig:396-413). The reduced path
        // uses thicken=false so canvas_padding is 0.
        let px_x = x.floor() as i32;
        let px_y = y.floor() as i32;
        let frac_x = x - x.floor();
        let frac_y = y - y.floor();

        let px_width = (width + frac_x).ceil() as u32;
        let px_height = (height + frac_y).ceil() as u32;
        if px_width == 0 || px_height == 0 {
            return Ok(Bitmap {
                width: 0,
                height: 0,
                bearing_x: 0,
                bearing_y: 0,
                format: if is_color {
                    PixelFormat::Bgra
                } else {
                    PixelFormat::Alpha8
                },
                data: Vec::new(),
            });
        }

        // 7. Bitmap context config (coretext.zig:415-461).
        let (depth, format) = if is_color {
            (4u32, PixelFormat::Bgra)
        } else {
            (1u32, PixelFormat::Alpha8)
        };
        let bytes_per_row = (px_width * depth) as usize;
        let mut buf = vec![0u8; bytes_per_row * px_height as usize];

        let color_space = if is_color {
            named_color_space(unsafe { objc2_core_graphics::kCGColorSpaceDisplayP3 })
        } else {
            named_color_space(unsafe { objc2_core_graphics::kCGColorSpaceLinearGray })
        }
        .ok_or(Error::EmbeddedFontLoadFailed)?;

        let bitmap_info: u32 = if is_color {
            CGImageByteOrderInfo::Order32Little.0 | CGImageAlphaInfo::PremultipliedFirst.0
        } else {
            CGImageAlphaInfo::Only.0
        };

        // SAFETY: buf is `bytes_per_row * px_height` bytes; parameters match a
        // valid ARGB/alpha-only bitmap context. The context borrows buf for its
        // lifetime; we drop the context before reading buf back out below.
        let ctx = unsafe {
            cg_bitmap_context_create(
                buf.as_mut_ptr().cast(),
                px_width as usize,
                px_height as usize,
                8,
                bytes_per_row,
                &color_space,
                bitmap_info,
            )
        }
        .ok_or(Error::EmbeddedFontLoadFailed)?;

        // Explicit fill to guarantee no uninitialized pixels
        // (coretext.zig:464-476).
        let full = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: px_width as CGFloat,
                height: px_height as CGFloat,
            },
        };
        if is_color {
            CGContext::set_rgb_fill_color(Some(&ctx), 0.0, 0.0, 0.0, 0.0);
        } else {
            CGContext::set_gray_fill_color(Some(&ctx), 0.0, 0.0);
        }
        CGContext::fill_rect(Some(&ctx), full);

        // 8. Context flags (coretext.zig:482-498). thicken=false in the
        // reduced path.
        CGContext::set_allows_font_smoothing(Some(&ctx), true);
        CGContext::set_should_smooth_fonts(Some(&ctx), false);
        CGContext::set_allows_font_subpixel_positioning(Some(&ctx), true);
        CGContext::set_should_subpixel_position_fonts(Some(&ctx), true);
        CGContext::set_allows_font_subpixel_quantization(Some(&ctx), false);
        CGContext::set_should_subpixel_quantize_fonts(Some(&ctx), false);
        CGContext::set_allows_antialiasing(Some(&ctx), true);
        CGContext::set_should_antialias(Some(&ctx), true);

        // 9. Draw color (coretext.zig:500-508). strength=0 in the reduced path;
        // gray 0 in an alpha-only context yields full coverage.
        if is_color {
            CGContext::set_rgb_fill_color(Some(&ctx), 1.0, 1.0, 1.0, 1.0);
            CGContext::set_rgb_stroke_color(Some(&ctx), 1.0, 1.0, 1.0, 1.0);
        } else {
            CGContext::set_gray_fill_color(Some(&ctx), 0.0, 1.0);
            CGContext::set_gray_stroke_color(Some(&ctx), 0.0, 1.0);
        }

        // 11. CTM translate then scale (coretext.zig:522-534). The scale
        // stretches the ink to the (possibly constrained) size: identity for
        // unconstrained glyphs, and the emoji cover-scale for color glyphs.
        CGContext::translate_ctm(Some(&ctx), frac_x, frac_y);
        CGContext::scale_ctm(Some(&ctx), scale_w, scale_h);

        // 12. Draw the glyph with negated bearings so its ink bottom-left lands
        // at CTM origin [0,0] (coretext.zig:542-545).
        let mut positions = [CGPoint {
            x: -rect.origin.x,
            y: -rect.origin.y,
        }; 1];
        // SAFETY: single-element glyph and position buffers.
        unsafe {
            self.font.draw_glyphs(
                glyphs_ptr,
                NonNull::new(positions.as_mut_ptr()).unwrap(),
                1,
                &ctx,
            );
        }

        // Drop the context so it releases its hold on buf before we read it.
        drop(ctx);

        // 13. Bearings out (coretext.zig:553-566).
        let bearing_x = px_x;
        let bearing_y = px_y + px_height as i32;

        Ok(Bitmap {
            width: px_width,
            height: px_height,
            bearing_x,
            bearing_y,
            format,
            data: buf,
        })
    }

    /// Extract [`FaceMetrics`] for this face, reconciling CoreText glyph
    /// measurements with the F1 table-derived layer.
    ///
    /// See `docs/analysis/font-coretext.md` §2. For faces we have the source
    /// bytes for (all embedded fonts), the table-derived fields (ascent,
    /// descent, line_gap, underline, strikethrough, cap/ex, units_per_em) come
    /// from F1's `tables::face_metrics` reading the same sfnt tables CoreText
    /// would copy; the three glyph-*measured* fields (cell_width, ascii_height,
    /// ic_width) are computed here through CoreText so we can cross-check them.
    /// For name-loaded system faces (no source bytes) all fields come from
    /// CoreText / its table copies.
    pub fn face_metrics(&self) -> FaceMetrics {
        let px_per_em = self.size_px();

        // Glyph-measured fields, via CoreText (coretext.zig:773-846).
        let (ct_cell_width, ct_ascii_height) = self.ascii_measurements();
        let ct_ic_width = self.ic_width();

        if let Some(bytes) = self.source_bytes {
            // Table half: reuse F1's derivation against the same bytes. This is
            // byte-for-byte the tables CoreText copies out, so ascent/descent/
            // line_gap/underline/strikethrough/cap/ex are identical to what
            // upstream's getMetrics table arms would produce.
            let face = ttf_parser::Face::parse(bytes, 0).expect("embedded/known font parses");
            let table_metrics = crate::tables::face_metrics(&face, px_per_em);

            FaceMetrics {
                px_per_em,
                // CoreText-measured glyph fields:
                cell_width: ct_cell_width,
                ascii_height: Some(ct_ascii_height),
                ic_width: ct_ic_width,
                // Table-derived fields (identical between backends):
                ..table_metrics
            }
        } else {
            // No source bytes: fall back to CoreText accessors for the vertical
            // metrics and cap/ex heights. This mirrors the "no table" arms of
            // getMetrics (coretext.zig:643-648, 749-752).
            // SAFETY: font is valid. CGFloat is f64 on macOS.
            let ascent = unsafe { self.font.ascent() };
            let descent = -unsafe { self.font.descent() };
            let line_gap = unsafe { self.font.leading() };
            let cap_height = unsafe { self.font.cap_height() };
            let ex_height = unsafe { self.font.x_height() };

            FaceMetrics {
                px_per_em,
                cell_width: ct_cell_width,
                ascent,
                descent,
                line_gap,
                underline_position: None,
                underline_thickness: None,
                strikethrough_position: None,
                strikethrough_thickness: None,
                cap_height: Some(cap_height),
                ex_height: Some(ex_height),
                ascii_height: Some(ct_ascii_height),
                ic_width: ct_ic_width,
            }
        }
    }

    /// Max printable-ASCII advance (cell width) and the union bounding-box
    /// height of the same glyphs (ascii height), both via CoreText glyph
    /// queries (coretext.zig:773-805).
    fn ascii_measurements(&self) -> (f64, f64) {
        // 0x20..=0x7E printable ASCII (upstream uses 32..127 exclusive of DEL).
        let chars: Vec<u16> = (0x20u16..0x7F).collect();
        let mut glyphs = vec![0u16; chars.len()];

        // SAFETY: chars/glyphs both have `chars.len()` elements.
        let ok = unsafe {
            self.font.glyphs_for_characters(
                NonNull::new(chars.as_ptr() as *mut u16).unwrap(),
                NonNull::new(glyphs.as_mut_ptr()).unwrap(),
                chars.len() as objc2_core_foundation::CFIndex,
            )
        };
        if !ok {
            // Some chars may be missing; CoreText still fills what it can. We
            // proceed with whatever glyph ids we got (0 => .notdef, harmless).
        }

        let glyphs_ptr = NonNull::new(glyphs.as_mut_ptr()).unwrap();

        // Advances.
        let mut advances = vec![
            CGSize {
                width: 0.0,
                height: 0.0
            };
            glyphs.len()
        ];
        // SAFETY: glyphs/advances have glyphs.len() elements.
        unsafe {
            self.font.advances_for_glyphs(
                CTFontOrientation::Horizontal,
                glyphs_ptr,
                advances.as_mut_ptr(),
                glyphs.len() as objc2_core_foundation::CFIndex,
            );
        }
        // CGFloat is f64 on macOS; a.width needs no cast.
        let max_advance = advances.iter().fold(0.0f64, |m, a| m.max(a.width));

        // Overall bounding rect (union) for the ASCII glyphs.
        // SAFETY: glyphs has glyphs.len() elements; null per-glyph out buffer.
        let rect = unsafe {
            self.font.bounding_rects_for_glyphs(
                CTFontOrientation::Horizontal,
                glyphs_ptr,
                std::ptr::null_mut(),
                glyphs.len() as objc2_core_foundation::CFIndex,
            )
        };

        (max_advance, rect.size.height)
    }

    /// Advance of "水" (U+6C34) as the ideograph width, discarded if the ink
    /// width exceeds the advance (patched-font guard, coretext.zig:807-846).
    fn ic_width(&self) -> Option<f64> {
        let glyph = self.glyph_index('水')?;
        let glyph16 = u16::try_from(glyph).ok()?;
        let mut glyphs = [glyph16; 1];
        let glyphs_ptr = NonNull::new(glyphs.as_mut_ptr()).unwrap();

        // SAFETY: single-element glyph buffer; null advances out => return value
        // is the summed advance (one glyph => its advance).
        let advance = unsafe {
            self.font.advances_for_glyphs(
                CTFontOrientation::Horizontal,
                glyphs_ptr,
                std::ptr::null_mut(),
                1,
            )
        };
        // SAFETY: single-element glyph buffer; null out => overall rect only.
        let bounds = unsafe {
            self.font.bounding_rects_for_glyphs(
                CTFontOrientation::Horizontal,
                glyphs_ptr,
                std::ptr::null_mut(),
                1,
            )
        };

        if bounds.size.width > advance {
            return None;
        }
        Some(advance)
    }
}

/// Read a CTFont's family name into a Rust `String`.
fn family_name(font: &CTFont) -> String {
    // SAFETY: font is a valid CTFont; family_name is always non-null.
    let cf = unsafe { font.family_name() };
    cf.to_string()
}

/// True if the CTFont has a non-empty table with the given 4-byte tag.
fn has_table(font: &CTFont, tag: &[u8; 4]) -> bool {
    let tag_u32 = u32::from_be_bytes(*tag);
    // SAFETY: font is valid; NoOptions is a valid options value.
    let data = unsafe { font.table(tag_u32, CTFontTableOptions::empty()) };
    match data {
        Some(d) => d.length() > 0,
        None => false,
    }
}

/// Create a `CGColorSpace` from one of the named-colorspace CFString constants.
fn named_color_space(name: &CFString) -> Option<CFRetained<CGColorSpace>> {
    CGColorSpace::with_name(Some(name))
}

/// Wrapper over the classic C `CGBitmapContextCreate`.
///
/// The `objc2-core-graphics` 0.3 bindings only expose the block-based
/// `CGBitmapContextCreateAdaptive`; the classic entry point is a stable,
/// documented CoreGraphics symbol, so we declare it directly (using the objc2
/// `CGContext`/`CGColorSpace` types for a type-safe boundary). See
/// `docs/analysis/font-coretext.md` §crate-choice.
///
/// # Safety
///
/// `data` must point to at least `bytes_per_row * height` writable bytes and
/// stay valid for the returned context's lifetime; the other parameters must
/// describe a valid bitmap layout for `color_space`.
unsafe fn cg_bitmap_context_create(
    data: *mut std::ffi::c_void,
    width: usize,
    height: usize,
    bits_per_component: usize,
    bytes_per_row: usize,
    color_space: &CGColorSpace,
    bitmap_info: u32,
) -> Option<CFRetained<CGContext>> {
    unsafe extern "C-unwind" {
        fn CGBitmapContextCreate(
            data: *mut std::ffi::c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: &CGColorSpace,
            bitmap_info: u32,
        ) -> Option<NonNull<CGContext>>;
    }
    // SAFETY: forwarded per this function's own safety contract.
    let ret = unsafe {
        CGBitmapContextCreate(
            data,
            width,
            height,
            bits_per_component,
            bytes_per_row,
            color_space,
            bitmap_info,
        )
    };
    // SAFETY: CGBitmapContextCreate returns a +1 retained context.
    ret.map(|p| unsafe { CFRetained::from_raw(p) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Metrics;

    /// The exact grid metrics upstream uses in its `Glyph.zig` "Constraints"
    /// test (JetBrains Mono at size 12, DPI 96). Only the fields the emoji
    /// constraint reads are meaningful; the rest are filled to plausible values.
    fn upstream_test_metrics() -> Metrics {
        Metrics {
            cell_width: 10,
            cell_height: 22,
            cell_baseline: 5,
            underline_position: 19,
            underline_thickness: 1,
            strikethrough_position: 12,
            strikethrough_thickness: 1,
            overline_position: 0,
            overline_thickness: 1,
            box_thickness: 1,
            cursor_thickness: 1,
            cursor_height: 22,
            icon_height: 21.12,
            icon_height_single: 44.48 / 3.0,
            face_width: 9.6,
            face_height: 21.12,
            face_y: 0.2,
        }
    }

    /// Port of upstream's `Glyph.zig` "Constraints" test, emoji case: `🥸`
    /// (U+1F978) from Apple Color Emoji, bbox `{20, 20, 0.46, 1}`, constrained
    /// with the emoji constraint at width 2 → `{18.72, 18.72, 0.44, 1.4}`. This
    /// pins our constraint math to upstream's exact numbers.
    #[test]
    fn emoji_constraint_matches_upstream() {
        let metrics = upstream_test_metrics();
        let glyph = GlyphSize {
            width: 20.0,
            height: 20.0,
            x: 0.46,
            y: 1.0,
        };
        let out = EmojiConstraint::EMOJI.constrain(glyph, &metrics, 2);
        let approx = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(approx(out.width, 18.72), "width {}", out.width);
        assert!(approx(out.height, 18.72), "height {}", out.height);
        assert!(approx(out.x, 0.44), "x {}", out.x);
        assert!(approx(out.y, 1.4), "y {}", out.y);
    }

    /// At constraint width 1 the `.cover` factor is bound by the narrower
    /// (width) target: a square emoji can't cover the full cell height without
    /// overflowing one cell horizontally, so it scales to the padded face
    /// width, aspect preserved. This proves single-cell fit doesn't overflow.
    #[test]
    fn emoji_constraint_single_cell_bound_by_width() {
        let metrics = upstream_test_metrics();
        let glyph = GlyphSize {
            width: 20.0,
            height: 20.0,
            x: 0.46,
            y: 1.0,
        };
        let out = EmojiConstraint::EMOJI.constrain(glyph, &metrics, 1);
        // Padded width target = (1 - 0.05) * face_width; cover uses min factor.
        let target_width = (1.0 - 0.05) * metrics.face_width;
        let approx = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(approx(out.width, target_width), "width {}", out.width);
        assert!(approx(out.width, out.height), "aspect preserved: {out:?}");
        // Fits within the face box on both axes (doesn't overflow the cell).
        assert!(
            out.width <= metrics.face_width + 1e-9,
            "width fits: {out:?}"
        );
        assert!(
            out.height <= metrics.face_height + 1e-9,
            "height fits: {out:?}"
        );
    }

    #[test]
    fn load_embedded_face_has_glyphs() {
        let face = Face::load_embedded(16.0).expect("load embedded");
        assert!(face.family_name().to_lowercase().contains("jetbrains"));
        assert!(!face.has_color());
        // Basic ASCII coverage.
        for c in ['A', 'g', 'M', 'x', ' '] {
            assert!(face.glyph_index(c).is_some(), "missing glyph for {c:?}");
        }
    }

    #[test]
    fn load_by_name_menlo_works() {
        // Menlo ships with macOS; the resolved family should contain "Menlo".
        let face = Face::load_by_name("Menlo", 16.0).expect("load Menlo");
        assert!(
            face.family_name().to_lowercase().contains("menlo"),
            "expected Menlo, got {:?}",
            face.family_name()
        );
        assert!(face.glyph_index('A').is_some());
    }

    #[test]
    fn nonsense_name_falls_back_to_embedded() {
        let face =
            Face::load_by_name("ThisFontDoesNotExist12345", 16.0).expect("fallback to embedded");
        assert!(
            face.family_name().to_lowercase().contains("jetbrains"),
            "expected embedded JetBrains Mono fallback, got {:?}",
            face.family_name()
        );
    }

    /// Rasterize a glyph and assert the bitmap is non-empty, in-bounds, and has
    /// plausible bearings.
    fn assert_plausible_raster(face: &Face, c: char) -> Bitmap {
        let gid = face
            .glyph_index(c)
            .unwrap_or_else(|| panic!("no glyph for {c:?}"));
        let bmp = face.rasterize(gid).expect("rasterize");

        assert!(bmp.width > 0 && bmp.height > 0, "{c:?} rasterized empty");
        assert_eq!(bmp.format, PixelFormat::Alpha8, "{c:?} should be alpha8");
        assert_eq!(
            bmp.data.len(),
            (bmp.width * bmp.height * bmp.bytes_per_pixel()) as usize,
            "{c:?} buffer size mismatch"
        );
        assert!(!bmp.is_blank(), "{c:?} rasterized all-zero");

        // Plausible bearings for a ~16px cell: within a generous box.
        assert!(
            bmp.bearing_x > -8 && bmp.bearing_x < 24,
            "{c:?} implausible bearing_x {}",
            bmp.bearing_x
        );
        assert!(
            bmp.bearing_y > -8 && bmp.bearing_y < 40,
            "{c:?} implausible bearing_y {}",
            bmp.bearing_y
        );
        // Reasonable pixel dimensions for a 16px glyph.
        assert!(bmp.width <= 48 && bmp.height <= 48, "{c:?} too large");
        bmp
    }

    #[test]
    fn rasterize_letters_and_block() {
        let face = Face::load_embedded(16.0).expect("load embedded");
        for c in ['A', 'g', '█'] {
            assert_plausible_raster(&face, c);
        }
    }

    /// Regression pin for the 'A' glyph of the embedded JetBrains Mono @ 16px.
    ///
    /// The exact alpha bytes CoreText produces are sensitive to the macOS
    /// version's rasterizer (hinting, antialiasing LUTs). We therefore pin
    /// only the *shape* invariants that are stable across versions: dimensions,
    /// bearings, a nonzero total coverage, and that the darkest pixels are near
    /// full coverage. If a future macOS changes the exact dimensions this test
    /// will flag it for a human to re-pin.
    #[test]
    fn rasterize_a_regression() {
        let face = Face::load_embedded(16.0).expect("load embedded");
        let gid = face.glyph_index('A').unwrap();
        let bmp = face.rasterize(gid).expect("rasterize A");

        // Dimensions of 'A' in JetBrains Mono @16px. macOS-version-sensitive:
        // if these change, verify the new raster visually before re-pinning.
        assert_eq!(bmp.format, PixelFormat::Alpha8);
        assert!(
            (8..=12).contains(&bmp.width),
            "A width {} outside pinned range (re-pin if macOS changed)",
            bmp.width
        );
        assert!(
            (10..=14).contains(&bmp.height),
            "A height {} outside pinned range (re-pin if macOS changed)",
            bmp.height
        );
        // 'A' has a peak near full coverage somewhere (the stems/apex).
        let peak = *bmp.data.iter().max().unwrap();
        assert!(peak >= 200, "A peak coverage {peak} too low");
        // Baseline-relative top bearing: 'A' rises to about cap height, so the
        // top of the ink box sits well above the baseline (positive bearing_y).
        assert!(
            bmp.bearing_y > 0,
            "A bearing_y {} should be positive",
            bmp.bearing_y
        );
    }

    /// Reconcile CoreText-derived metrics against F1's table-derived pins.
    ///
    /// F1 pinned (embedded JetBrains Mono @16px, flagged unverified-vs-upstream):
    ///   cell_width=10, cell_height=21, cell_baseline=5,
    ///   underline_position=18, underline_thickness=1,
    ///   strikethrough_position=11, strikethrough_thickness=1.
    /// This test is the verification: load the *same bytes* through CoreText and
    /// compare `Metrics::calc`.
    #[test]
    fn metrics_reconciliation_with_f1() {
        let face = Face::load_embedded(16.0).expect("load embedded");
        let ct = Metrics::calc(face.face_metrics());

        assert_eq!(ct.cell_width, 10, "cell_width delta vs F1 pin");
        assert_eq!(ct.cell_height, 21, "cell_height delta vs F1 pin");
        assert_eq!(ct.cell_baseline, 5, "cell_baseline delta vs F1 pin");
        assert_eq!(ct.underline_position, 18);
        assert_eq!(ct.underline_thickness, 1);
        assert_eq!(ct.strikethrough_position, 11);
        assert_eq!(ct.strikethrough_thickness, 1);
    }
}
