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
//! Out of scope (deferred, per the plan): sbix pixel quantization, and the full
//! discovery `Score` ranking's variable-axis arm. The nerd-font `constrain(...)`
//! path is now implemented (see `crate::constraint` + `crate::nerd_font_constraints`,
//! applied here via [`Face::rasterize_constrained`]), as is byte-backed shaping
//! for name-loaded faces (see [`Face::load_by_name`] + `crate::shaper`).

#![cfg(target_os = "macos")]

use std::ptr::NonNull;
use std::sync::Arc;

use objc2_core_foundation::{
    CFData, CFDictionary, CFMutableDictionary, CFNumber, CFRetained, CFString, CFType, CFURL,
    CFURLPathStyle, CGFloat, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGColorSpace, CGContext, CGImageAlphaInfo, CGImageByteOrderInfo, CGTextDrawingMode,
};
use objc2_core_text::{
    CTFont, CTFontDescriptor, CTFontManagerCreateFontDescriptorFromData, CTFontOrientation,
    CTFontSymbolicTraits, CTFontTableOptions, kCTFontURLAttribute, kCTFontVariationAttribute,
};

use crate::constraint::{Constraint, GlyphSize};
use crate::embedded;
use crate::metrics::{FaceMetrics, Metrics};

// The glyph constraint types (`GlyphSize`, `Constraint`, and the sizing/
// alignment enums) and the full constrain math now live in `crate::constraint`
// (the complete port of `Glyph.zig`'s `RenderOptions.Constraint`). Nerd Font
// PUA icons additionally use the generated per-codepoint table in
// `crate::nerd_font_constraints`.

// `Bitmap`/`PixelFormat` are the platform-neutral rasterization output; they
// live in `crate::raster` so the FreeType backend produces the same type. Kept
// re-exported here (`coretext::Bitmap`/`coretext::PixelFormat`) for source
// compatibility with existing consumers.
pub use crate::raster::{Bitmap, PixelFormat};

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
    /// for embedded fonts (as a zero-copy `Arc` over the `'static` slice) and,
    /// as of the byte-backed-named-faces work, for **name-loaded** faces whose
    /// backing file we read via the CoreText font URL attribute
    /// (`kCTFontURLAttribute`). Used for the table-derived half of the metrics
    /// reconciliation (see [`Face::face_metrics`]) and — critically — to build a
    /// `rustybuzz::Face` for real shaping (ligatures). `None` only when the
    /// backing bytes can't be obtained (e.g. a discovered system face with no
    /// file URL), where we rely on CoreText's table copies and unshaped
    /// per-codepoint rendering.
    source_bytes: Option<Arc<[u8]>>,
    /// The face index within `source_bytes` for a font *collection* (`.ttc`).
    /// 0 for a single-face file or the embedded fonts. Used to select the right
    /// subface when building the rustybuzz shaper and the ttf-parser metrics
    /// face (a `.ttc` holds several faces in one byte buffer).
    face_index: u32,
    /// Whether the face can contain color glyphs (symbolic-traits gate +
    /// sbix presence). See coretext.zig:103-107, 890-968.
    color: Option<ColorState>,
    /// The `wght` (weight) variation-axis value applied to this face, if any.
    /// Set when the face was materialized from a variable font at an explicit
    /// weight instance (upstream's default-config bold path,
    /// `SharedGridSet.zig:284-287,315-318`: `setVariations(wght=700)`). The
    /// rustybuzz shaper reapplies this so shaping and rasterization agree on
    /// the same instance. `None` for a face at the font's default instance.
    wght: Option<f32>,
    /// A synthetic-bold stroke line width in pixels, if this face is a
    /// synthetic-bold variant (upstream `Face.synthetic_bold`,
    /// coretext.zig:26,183-199). Applied at rasterization time as a
    /// `fill_stroke` with this line width plus an ink-box expansion
    /// (coretext.zig:311-320,511-516). `None` for non-synthetic faces. The
    /// default config never sets this (it uses [`Face::wght`] instead); it is
    /// the fallback when a variable `wght` axis is unavailable.
    synthetic_bold: Option<f64>,
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
            // Give the named face its backing bytes so it can be shaped
            // (rustybuzz needs the raw font data) and so ttf-parser can supply
            // the table-derived metrics. Read the font FILE via CoreText's URL
            // attribute (`kCTFontURLAttribute`) and resolve the face index for a
            // `.ttc` collection. A face we can't back with bytes still works
            // through CoreText's table copies + the unshaped render path.
            let (bytes, face_index) = font_file_bytes(&font).unzip();
            Ok(Face::from_ct_font_indexed(
                font,
                bytes,
                face_index.unwrap_or(0),
            ))
        } else {
            Face::load_embedded(size_px)
        }
    }

    /// Load the embedded JetBrains Mono (variable weight, default `wght=400`
    /// instance) at `size_px` pixels, via
    /// `CTFontManagerCreateFontDescriptorFromData` (coretext.zig:51-70).
    pub fn load_embedded(size_px: f64) -> Result<Face, Error> {
        Face::load_from_bytes(embedded::JETBRAINS_MONO_VARIABLE, size_px)
    }

    /// Load the embedded nerd-symbols fallback font (upstream's
    /// `symbols_nerd_font`) at `size_px` pixels.
    pub fn load_embedded_symbols_nerd_font(size_px: f64) -> Result<Face, Error> {
        Face::load_from_bytes(embedded::SYMBOLS_NERD_FONT_MONO, size_px)
    }

    /// Load the embedded JetBrains Mono **italic** companion (variable weight,
    /// default `wght=400` instance) at `size_px` pixels — upstream's
    /// `embedded.variable_italic`, added as the default italic style face
    /// (`SharedGridSet.zig:293-300`).
    pub fn load_embedded_italic(size_px: f64) -> Result<Face, Error> {
        Face::load_from_bytes(embedded::JETBRAINS_MONO_VARIABLE_ITALIC, size_px)
    }

    /// Load the embedded JetBrains Mono at the **bold** (`wght=700`) variation
    /// instance — upstream's default bold style face
    /// (`SharedGridSet.zig:277-287`: `embedded.variable` +
    /// `setVariations(wght=700)`).
    pub fn load_embedded_bold(size_px: f64) -> Result<Face, Error> {
        Face::load_embedded(size_px)?.with_wght_variation(700.0)
    }

    /// Load the embedded JetBrains Mono **italic** at the **bold** (`wght=700`)
    /// variation instance — upstream's default bold-italic style face
    /// (`SharedGridSet.zig:306-318`: `embedded.variable_italic` +
    /// `setVariations(wght=700)`).
    pub fn load_embedded_bold_italic(size_px: f64) -> Result<Face, Error> {
        Face::load_embedded_italic(size_px)?.with_wght_variation(700.0)
    }

    /// Return a copy of this face with the `wght` (weight) variation axis set to
    /// `value`, keeping the same pixel size.
    ///
    /// Port of upstream `Face.setVariations` reduced to the single `wght` axis
    /// the default-config bold path uses (coretext.zig:225-253,
    /// `SharedGridSet.zig` bold/bold-italic slots). Builds a font descriptor
    /// carrying `kCTFontVariationAttribute = { <wght axis id>: value }` and
    /// copies the CTFont through it, so both CoreText rasterization and (via the
    /// recorded [`Face::wght`]) rustybuzz shaping select the same instance.
    pub fn with_wght_variation(mut self, value: f32) -> Result<Face, Error> {
        // The variation dictionary is keyed by the axis's *identifier*, which
        // for a registered axis is its four-character tag interpreted as a
        // big-endian u32. `wght` = 0x77676874.
        const WGHT_AXIS_ID: i64 = 0x7767_6874;

        let axis_key = CFNumber::new_i64(WGHT_AXIS_ID);
        let axis_val = CFNumber::new_f32(value);
        let var_dict = CFMutableDictionary::<CFNumber, CFNumber>::with_capacity(1);
        var_dict.set(&axis_key, &axis_val);

        let attrs = CFMutableDictionary::<CFString, CFType>::with_capacity(1);
        // SAFETY: kCTFontVariationAttribute is a valid static key; var_dict is a
        // valid CFDictionary value (coerced to &CFType).
        let key = unsafe { kCTFontVariationAttribute };
        attrs.set(key, &var_dict);

        let dict: &CFDictionary = attrs.as_opaque();
        // SAFETY: dict is a valid attribute dictionary carrying the variation.
        let desc = unsafe { CTFontDescriptor::with_attributes(dict) };

        let size = self.size_px();
        // SAFETY: font is valid; null matrix = identity; desc overrides the
        // variation attribute; size preserved.
        let font = unsafe {
            self.font
                .copy_with_attributes(size, std::ptr::null(), Some(&desc))
        };

        self.font = font;
        // Re-derive color state from the new CTFont (traits are unchanged for a
        // weight variation, but keep the invariant that `color` matches `font`).
        // SAFETY: font is a valid CTFont.
        let traits = unsafe { self.font.symbolic_traits() };
        self.color = if traits.contains(CTFontSymbolicTraits::TraitColorGlyphs) {
            let color_table = has_table(&self.font, b"sbix")
                || has_table(&self.font, b"COLR")
                || has_table(&self.font, b"CBDT")
                || has_table(&self.font, b"SVG ");
            Some(ColorState { color_table })
        } else {
            None
        };
        self.wght = Some(value);
        Ok(self)
    }

    /// The `wght` variation-axis value applied to this face, if any.
    ///
    /// The shaper reads this to reapply the same weight instance on its
    /// rustybuzz face, so shaped glyph ids match the rasterized (CoreText)
    /// instance.
    pub fn wght(&self) -> Option<f32> {
        self.wght
    }

    /// Return a copy of this face flagged for a **synthetic bold** stroke of
    /// `line_width` pixels.
    ///
    /// Port of upstream `Face.syntheticBold` (coretext.zig:183-199), the
    /// fallback bold mechanism when a `wght` variation axis is unavailable (the
    /// default config prefers [`Face::with_wght_variation`]). The stroke is
    /// applied at rasterization time (see [`Face::rasterize_constrained`]).
    /// `line_width` upstream is `max(points/14, 1)`.
    pub fn synthetic_bold(mut self, line_width: f64) -> Face {
        self.synthetic_bold = Some(line_width);
        self
    }

    /// The upstream synthetic-bold line width for a face rendered at `size_px`
    /// pixels-per-em: `max(size_px / 14, 1)` (coretext.zig:193-195, where
    /// `points` is the render em size).
    pub fn synthetic_bold_line_width(size_px: f64) -> f64 {
        (size_px / 14.0).max(1.0)
    }

    /// Return an independent copy of this face at `size_px` pixels.
    ///
    /// Re-copies the underlying CTFont (`copyWithAttributes(size, null, null)`,
    /// the same call `Face::from_ct_font_at_size` uses). Preserves the
    /// `source_bytes` (so a byte-backed face stays byte-backed for shaping) but
    /// resets synthetic flags. Used for the alias-to-regular and
    /// synthesize-on-a-fresh-copy paths of family style completion.
    pub fn try_clone(&self, size_px: f64) -> Result<Face, Error> {
        // SAFETY: font is valid; null matrix = identity; no descriptor overrides.
        let font = unsafe {
            self.font
                .copy_with_attributes(size_px as CGFloat, std::ptr::null(), None)
        };
        Ok(Face::from_ct_font_indexed(
            font,
            self.source_bytes.clone(),
            self.face_index,
        ))
    }

    /// Return a **synthetic italic** copy of this face: the same CTFont with the
    /// [`ITALIC_SKEW`] transform matrix baked in, keeping the same pixel size.
    ///
    /// Port of upstream `Face.syntheticItalic` (coretext.zig:174-178:
    /// `copyWithAttributes(0.0, &italic_skew, null)`). The skew is applied at the
    /// CTFont level so both metric queries and rasterization see the slanted
    /// outline; unlike synthetic bold there is no per-rasterize flag. Used as the
    /// fallback italic mechanism when a family has no real italic member (e.g.
    /// FiraCode Nerd Font Mono).
    pub fn synthetic_italic(&self) -> Result<Face, Error> {
        let size = self.size_px();
        // SAFETY: font is a valid CTFont; ITALIC_SKEW is a valid affine
        // transform; no descriptor overrides; size preserved.
        let font = unsafe {
            self.font
                .copy_with_attributes(size as CGFloat, &ITALIC_SKEW, None)
        };
        Ok(Face::from_ct_font_indexed(
            font,
            self.source_bytes.clone(),
            self.face_index,
        ))
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

        Ok(Face::from_ct_font(font, Some(Arc::from(bytes))))
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

    fn from_ct_font(font: CFRetained<CTFont>, source_bytes: Option<Arc<[u8]>>) -> Face {
        Face::from_ct_font_indexed(font, source_bytes, 0)
    }

    fn from_ct_font_indexed(
        font: CFRetained<CTFont>,
        source_bytes: Option<Arc<[u8]>>,
        face_index: u32,
    ) -> Face {
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
            face_index,
            color,
            wght: None,
            synthetic_bold: None,
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
    pub fn source_bytes(&self) -> Option<&[u8]> {
        self.source_bytes.as_deref()
    }

    /// The face index within [`Face::source_bytes`] for a `.ttc` collection
    /// (0 for a single-face file or the embedded fonts). The shaper and metrics
    /// face use this to select the correct subface.
    pub fn face_index(&self) -> u32 {
        self.face_index
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
    /// with the color-glyph trait presents as [`Presentation::Emoji`](crate::Presentation::Emoji),
    /// else [`Presentation::Text`](crate::Presentation::Text)
    /// (DeferredFace.zig:370-373, Collection.zig gate).
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
    /// - [`PresentationMode::Any`](crate::PresentationMode::Any): the face
    ///   merely has a glyph for `cp`.
    /// - [`PresentationMode::Explicit`](crate::PresentationMode::Explicit) /
    ///   [`PresentationMode::Default`](crate::PresentationMode::Default): the
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
    /// ([`Constraint::EMOJI`]) is applied: the authored ink box (which for
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
        constraint: Option<(Constraint, &Metrics, u32)>,
    ) -> Result<Bitmap, Error> {
        let glyph16 = u16::try_from(glyph_id).map_err(|_| Error::NoSuchGlyph)?;
        let mut glyphs = [glyph16; 1];
        let glyphs_ptr = NonNull::new(glyphs.as_mut_ptr()).unwrap();

        // 1. Ink bounding rect: CoreGraphics space, origin bottom-left, +Y up.
        // SAFETY: single-element glyph buffer; null bounding_rects => only the
        // overall rect is computed and returned.
        let mut rect: CGRect = unsafe {
            self.font.bounding_rects_for_glyphs(
                CTFontOrientation::Horizontal,
                glyphs_ptr,
                std::ptr::null_mut(),
                1,
            )
        };

        let is_color = self.is_color_glyph(glyph_id);

        // Synthetic-bold ink expansion (coretext.zig:305-320): a synthetic-bold
        // stroke gains half its line width on every edge, so grow the ink box
        // by the line width and shift the origin down-left by half. Skipped for
        // color (sbix) glyphs, which the stroke doesn't affect.
        if !is_color && let Some(line_width) = self.synthetic_bold {
            rect.size.width += line_width;
            rect.size.height += line_width;
            rect.origin.x -= line_width / 2.0;
            rect.origin.y -= line_width / 2.0;
        }

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

        // 5. Apply the glyph constraint (coretext.zig:336-380, upstream's
        // `RenderOptions.Constraint.constrain`). The caller chooses the
        // constraint: emoji get the fixed `.cover`+center constraint, Nerd Font
        // PUA icons get their per-codepoint table constraint (Item 3), and
        // everything else passes `None` for natural size. We fold the baseline
        // into `y` (the constraint operates on cell-relative, not
        // baseline-relative, positions), constrain, then read back the scaled
        // size and cell-relative origin.
        let (width, height, x, y, scale_w, scale_h) = match constraint {
            Some((c, metrics, constraint_width)) if c.does_anything() => {
                let cell_baseline = f64::from(metrics.cell_baseline);
                let constrained = c.constrain(
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
            // No (or no-op) constraint: natural size, identity scale.
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

        // 10. Synthetic bold: stroke the glyph outline in addition to filling
        // it, thickening it (coretext.zig:511-516). The ink-box was already
        // expanded above to make room for the stroke.
        if !is_color && let Some(line_width) = self.synthetic_bold {
            CGContext::set_text_drawing_mode(Some(&ctx), CGTextDrawingMode::FillStroke);
            CGContext::set_line_width(Some(&ctx), line_width);
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

        // Table half: reuse F1's derivation against the same bytes if we have
        // them AND ttf-parser can parse them (embedded fonts always parse;
        // name-loaded faces usually do too, but a `.ttc` face index or exotic
        // font that ttf-parser rejects falls through to the CoreText-accessor
        // arm rather than panicking).
        let table_parse = self
            .source_bytes
            .as_deref()
            .and_then(|bytes| ttf_parser::Face::parse(bytes, self.face_index).ok());

        if let Some(face) = table_parse {
            // This is byte-for-byte the tables CoreText copies out, so
            // ascent/descent/line_gap/underline/strikethrough/cap/ex are
            // identical to what upstream's getMetrics table arms would produce.
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

/// The CTFont's PostScript name, or `None`.
fn post_script_name(font: &CTFont) -> Option<String> {
    // SAFETY: font is a valid CTFont; the accessor returns a retained CFString.
    let cf = unsafe { font.post_script_name() };
    Some(cf.to_string())
}

/// Read the font FILE backing `font` and the face index within it.
///
/// This is the byte-backed-named-faces mechanism (Item 2): a face obtained by
/// name from CoreText has no in-memory bytes, but its descriptor carries a file
/// URL (`kCTFontURLAttribute`) pointing at the on-disk `.ttf`/`.otf`/`.ttc`. We
/// read that file so the face can be shaped (rustybuzz needs the raw data) and
/// so ttf-parser can supply table-derived metrics.
///
/// For a `.ttc` **collection**, the returned index selects the subface whose
/// PostScript name matches `font` (a collection holds several faces in one
/// file). Returns `None` if the URL attribute is absent (e.g. a purely
/// system-synthesized font) or the file can't be read — the caller then keeps
/// the byte-less (unshaped) path.
fn font_file_bytes(font: &CTFont) -> Option<(Arc<[u8]>, u32)> {
    // The URL attribute lives on the font's descriptor.
    // SAFETY: font is valid; CTFontCopyFontDescriptor returns a retained desc.
    let desc = unsafe { font.font_descriptor() };
    // SAFETY: kCTFontURLAttribute is a valid static key.
    let key = unsafe { kCTFontURLAttribute };
    // SAFETY: desc is valid; attribute() returns a retained CFType or None.
    let attr = unsafe { desc.attribute(key) }?;
    let url = attr.downcast_ref::<CFURL>()?;
    let path = url.file_system_path(CFURLPathStyle::CFURLPOSIXPathStyle)?;
    let bytes = std::fs::read(path.to_string()).ok()?;
    let bytes: Arc<[u8]> = Arc::from(bytes.into_boxed_slice());

    // Resolve the face index within a collection by matching PostScript names.
    let index = collection_face_index(&bytes, font).unwrap_or(0);
    Some((bytes, index))
}

/// For a font collection (`.ttc`), the index of the subface whose PostScript
/// name matches `font`; `Some(0)` for a single-face file. `None` if nothing
/// matches (the caller defaults to index 0).
fn collection_face_index(bytes: &[u8], font: &CTFont) -> Option<u32> {
    let count = ttf_parser::fonts_in_collection(bytes).unwrap_or(1).max(1);
    if count == 1 {
        return Some(0);
    }
    let want = post_script_name(font)?;
    for i in 0..count {
        let Ok(face) = ttf_parser::Face::parse(bytes, i) else {
            continue;
        };
        let ps = face
            .names()
            .into_iter()
            .find(|n| n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME)
            .and_then(|n| n.to_string());
        if ps.as_deref() == Some(want.as_str()) {
            return Some(i);
        }
    }
    None
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
        let out = Constraint::EMOJI.constrain(glyph, &metrics, 2);
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
        let out = Constraint::EMOJI.constrain(glyph, &metrics, 1);
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
