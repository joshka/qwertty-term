//! A lazily-loaded font face (`DeferredFace`).
//!
//! Port of the CoreText arm of Ghostty's `src/font/DeferredFace.zig` (commit
//! `2da015cd6`), reduced to what the resolver's fallback chain needs: hold a
//! discovered `CTFont`, answer a cheap `has_codepoint` probe without paying a
//! full [`Face`] init, and materialize a [`Face`] on demand at a given pixel
//! size. See `docs/analysis/font-discovery.md` §4.
//!
//! Deferred (documented): variation re-application on load (the reduced search
//! descriptor pushes no variation targets), and the non-CoreText backends.

#![cfg(target_os = "macos")]

use std::ptr::NonNull;

use objc2_core_foundation::{CFIndex, CFRetained};
use objc2_core_text::{CTFont, CTFontSymbolicTraits};

use crate::coretext::{Error, Face};
use crate::presentation::{Presentation, PresentationMode};

/// A font face discovered but not yet loaded at a render size.
///
/// Wraps a retained `CTFont` (materialized during discovery, typically at a
/// nominal size 12). `has_codepoint` probes it directly; `load` re-copies it at
/// the caller's pixel size into a full [`Face`].
pub struct DeferredFace {
    font: CFRetained<CTFont>,
}

impl DeferredFace {
    /// Wrap a discovered `CTFont`. Internal to the discovery module.
    pub(crate) fn from_ct_font(font: CFRetained<CTFont>) -> DeferredFace {
        DeferredFace { font }
    }

    /// The family name of the deferred face (DeferredFace.zig:143-169).
    pub fn family_name(&self) -> String {
        // SAFETY: font is a valid CTFont; family_name is non-null.
        let cf = unsafe { self.font.family_name() };
        cf.to_string()
    }

    /// The display name of the deferred face (DeferredFace.zig:173-202). Used
    /// for fallback logging and the discovery tests.
    pub fn name(&self) -> String {
        // SAFETY: font is a valid CTFont; display_name is non-null.
        let cf = unsafe { self.font.display_name() };
        cf.to_string()
    }

    /// The presentation this face advertises (symbolic color-glyph trait).
    pub fn presentation(&self) -> Presentation {
        // SAFETY: font is a valid CTFont.
        let traits = unsafe { self.font.symbolic_traits() };
        if traits.contains(CTFontSymbolicTraits::TraitColorGlyphs) {
            Presentation::Emoji
        } else {
            Presentation::Text
        }
    }

    /// True if this deferred face satisfies `cp` under `p_mode`, WITHOUT loading
    /// a full [`Face`] (DeferredFace.zig:357-385, CoreText arm).
    ///
    /// A presentation constraint is checked against the face's symbolic-trait
    /// color-ness (the cheap gate upstream uses for deferred faces); then a
    /// `CTFontGetGlyphsForCharacters` CMap probe confirms the glyph exists.
    ///
    /// `fallback` selects the default-mode asymmetry: a fallback face is held to
    /// the explicit presentation for a `Default(p)` request, a non-fallback one
    /// is not (Collection.zig:808-814). Discovered faces are always fallbacks,
    /// but the flag is threaded for parity with the loaded-face path.
    pub fn has_codepoint(&self, cp: u32, p_mode: PresentationMode, fallback: bool) -> bool {
        let want: Option<Presentation> = match p_mode {
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

        if let Some(desired) = want
            && self.presentation() != desired
        {
            return false;
        }

        self.has_glyph(cp)
    }

    /// Low-level CMap probe: true if the `CTFont` maps `cp` to a non-zero glyph.
    fn has_glyph(&self, cp: u32) -> bool {
        let Some(ch) = char::from_u32(cp) else {
            return false;
        };
        let mut utf16 = [0u16; 2];
        let encoded = ch.encode_utf16(&mut utf16);
        let len = encoded.len();

        let mut glyphs = [0u16; 2];
        // SAFETY: both buffers hold >= len (1 or 2) elements.
        let ok = unsafe {
            self.font.glyphs_for_characters(
                NonNull::new(utf16.as_mut_ptr()).unwrap(),
                NonNull::new(glyphs.as_mut_ptr()).unwrap(),
                len as CFIndex,
            )
        };
        ok && glyphs[0] != 0
    }

    /// Materialize a full [`Face`] at `size_px` pixels (DeferredFace.zig:253-264,
    /// `loadCoreText` → `Face.initFontCopy`).
    ///
    /// Re-copies the retained `CTFont` at the requested pixel size via the same
    /// descriptor path the rest of the crate uses. The resulting face has no
    /// `source_bytes` (it is a system face), so its metrics come from CoreText
    /// accessors (handled by `Face::face_metrics`'s no-source-bytes arm).
    pub fn load(&self, size_px: f64) -> Result<Face, Error> {
        Face::from_ct_font_at_size(&self.font, size_px)
    }
}

impl std::fmt::Debug for DeferredFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeferredFace")
            .field("family", &self.family_name())
            .finish()
    }
}
