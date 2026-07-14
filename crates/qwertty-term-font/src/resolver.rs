//! Codepoint → font resolution (the full fallback chain).
//!
//! Port of Ghostty's `src/font/CodepointResolver.zig` `getIndex`
//! (commit `2da015cd6`), the 7-step chain. F5-reduced implemented steps 3-4
//! (sprite + primary); F5-full adds steps 1 and 4-7: disabled-style fallback,
//! exact match over the priority list, regular retry, **discovery fallback**
//! (on a miss, find a fallback face via CoreText discovery, add it to the
//! collection, return its index), and the any-presentation last resort.
//!
//! See `docs/analysis/font-discovery.md` §6 (the ported chain) and §5
//! (presentation flow).
//!
//! Deferred (documented): step 2 codepoint overrides (no config-map surface),
//! and fallback size-adjustment (`ic_width` rescale).

use crate::collection::{Collection, FontIndex, Style};
#[cfg(any(
    all(target_os = "macos", not(feature = "freetype")),
    feature = "fontconfig"
))]
use crate::descriptor::Descriptor;
#[cfg(all(target_os = "macos", not(feature = "freetype")))]
use crate::discovery;
use crate::presentation::{Presentation, PresentationMode};

/// Resolves codepoints to a [`FontIndex`] against a [`Collection`], the sprite
/// subsystem, and (on a miss) CoreText font discovery.
pub struct CodepointResolver {
    collection: Collection,
    /// Per-style enabled flags (upstream `styles`, StyleStatus). Regular can
    /// never be disabled.
    styles: StyleStatus,
    /// Whether discovery-based fallback is enabled. When false, the resolver
    /// behaves like the F5-reduced cut (sprite + primary, else notdef).
    discover: bool,
    /// The pixel size at which discovered fallback faces are loaded (the render
    /// size the collection was built for). Read by the CoreText and fontconfig
    /// discovery paths; set on every platform for API uniformity (dead only when
    /// neither discovery backend is compiled in).
    #[cfg_attr(
        not(any(
            all(target_os = "macos", not(feature = "freetype")),
            feature = "fontconfig"
        )),
        allow(dead_code)
    )]
    size_px: f64,
}

/// Per-style enable flags (upstream `StyleStatus = EnumArray(Style, bool)`).
#[derive(Debug, Clone, Copy)]
struct StyleStatus {
    bold: bool,
    italic: bool,
    bold_italic: bool,
}

impl StyleStatus {
    fn all_enabled() -> StyleStatus {
        StyleStatus {
            bold: true,
            italic: true,
            bold_italic: true,
        }
    }

    fn get(&self, style: Style) -> bool {
        match style {
            // Regular can never be disabled (upstream invariant).
            Style::Regular => true,
            Style::Bold => self.bold,
            Style::Italic => self.italic,
            Style::BoldItalic => self.bold_italic,
        }
    }

    fn set(&mut self, style: Style, enabled: bool) {
        match style {
            Style::Regular => {}
            Style::Bold => self.bold = enabled,
            Style::Italic => self.italic = enabled,
            Style::BoldItalic => self.bold_italic = enabled,
        }
    }
}

impl CodepointResolver {
    /// Create a resolver over `collection` with discovery fallback enabled.
    ///
    /// Discovered fallback faces are loaded at the primary face's pixel size
    /// (`primary().size_px()`), so a single-arg constructor keeps the
    /// F5-reduced call-site shape while enabling the full chain.
    pub fn new(collection: Collection) -> CodepointResolver {
        let size_px = collection.primary().size_px();
        CodepointResolver {
            collection,
            styles: StyleStatus::all_enabled(),
            discover: true,
            size_px,
        }
    }

    /// Create a resolver over `collection`, loading discovered fallback faces at
    /// an explicit `size_px` (when the primary's reported size differs from the
    /// desired render size).
    pub fn with_size(collection: Collection, size_px: f64) -> CodepointResolver {
        CodepointResolver {
            collection,
            styles: StyleStatus::all_enabled(),
            discover: true,
            size_px,
        }
    }

    /// Create a resolver with discovery fallback **disabled** (the F5-reduced
    /// behavior: sprite + primary, else notdef). Kept for tests / callers that
    /// don't want on-demand system font loading.
    pub fn without_discovery(collection: Collection) -> CodepointResolver {
        CodepointResolver {
            collection,
            styles: StyleStatus::all_enabled(),
            discover: false,
            size_px: 16.0,
        }
    }

    /// Borrow the underlying collection (for faces / rasterization).
    pub fn collection(&self) -> &Collection {
        &self.collection
    }

    /// Disable a font style (upstream `r.styles.set(style, false)`). Regular
    /// cannot be disabled.
    pub fn set_style_enabled(&mut self, style: Style, enabled: bool) {
        self.styles.set(style, enabled);
    }

    /// Resolve `cp` (bare codepoint, default presentation) to a font index.
    ///
    /// Convenience wrapper for `get_index_p(cp, style, None)`.
    pub fn get_index(&mut self, cp: u32, style: Style) -> Option<FontIndex> {
        self.get_index_p(cp, style, None)
    }

    /// Resolve `cp` to a font index under an optional explicit presentation,
    /// mirroring upstream `getIndex(alloc, cp, style, p)`
    /// (CodepointResolver.zig:120-228).
    ///
    /// `p = Some(..)` forces a presentation (VS15/VS16); `p = None` uses the UCD
    /// default. Returns `None` (notdef) if nothing satisfies the codepoint.
    pub fn get_index_p(
        &mut self,
        cp: u32,
        style: Style,
        p: Option<Presentation>,
    ) -> Option<FontIndex> {
        // Step 1: disabled style → restart at regular.
        if style != Style::Regular && !self.styles.get(style) {
            return self.get_index_p(cp, Style::Regular, p);
        }

        // Step 2 (codepoint override): deferred — no config-map surface.

        // Step 3: sprite dispatch.
        if qwertty_term_sprite::has_codepoint(cp) {
            return Some(FontIndex::Sprite);
        }

        // Build the presentation mode: explicit if given, else the UCD default.
        let p_mode = match p {
            Some(v) => PresentationMode::Explicit(v),
            None => PresentationMode::Default(Presentation::default_for(cp)),
        };

        // Step 4: exact style+presentation match over the priority list.
        if let Some(idx) = self.collection.get_index(cp, style, p_mode) {
            return Some(idx);
        }

        // Step 5: non-regular retry at regular.
        if style != Style::Regular
            && let Some(idx) = self.get_index_p(cp, Style::Regular, p)
        {
            return Some(idx);
        }

        // Step 6: discovery fallback (regular only).
        if style == Style::Regular
            && self.discover
            && let Some(idx) = self.discover_fallback(cp, style, p_mode)
        {
            return Some(idx);
        }

        // Step 7: any-presentation last resort.
        if style == Style::Regular {
            if p_mode == PresentationMode::Any {
                return None;
            }
            // A regular request that hasn't already searched `any`: retry any.
            if !matches!(p_mode, PresentationMode::Any) {
                return self
                    .collection
                    .get_index(cp, Style::Regular, PresentationMode::Any);
            }
            return None;
        }

        // Non-regular: fall back to regular with any presentation.
        self.collection
            .get_index(cp, Style::Regular, PresentationMode::Any)
    }

    /// Discovery fallback (upstream step 6, CodepointResolver.zig:169-219):
    /// search for a fallback face that has `cp`, add it to the collection as a
    /// fallback face, and return its index. Infallible (a discovery/load error
    /// is swallowed and yields `None`).
    ///
    /// Backed by CoreText discovery on macOS and fontconfig discovery on Linux
    /// (any `fontconfig`-feature build). Without a discovery backend compiled in,
    /// resolution stops at the collection's own faces (embedded + synthetic) and
    /// the stub below returns `None`.
    ///
    /// The two backends differ only in how the candidate list is obtained (the
    /// CoreText path seeds `CTFontCreateForString` from the primary face;
    /// fontconfig needs no such seed); the presentation-filter + load + add loop
    /// is identical, since [`crate::deferred::DeferredFace`] and
    /// [`crate::fontconfig::FcDeferredFace`] expose the same `has_codepoint` /
    /// `load` surface.
    #[cfg(any(
        all(target_os = "macos", not(feature = "freetype")),
        feature = "fontconfig"
    ))]
    fn discover_fallback(
        &mut self,
        cp: u32,
        style: Style,
        p_mode: PresentationMode,
    ) -> Option<FontIndex> {
        let desc = Descriptor {
            codepoint: cp,
            size: self.size_px as f32,
            bold: matches!(style, Style::Bold | Style::BoldItalic),
            italic: matches!(style, Style::Italic | Style::BoldItalic),
            monospace: false,
            ..Default::default()
        };

        // CoreText discovery uses the primary as the base for
        // `CTFontCreateForString`; fontconfig encodes the codepoint as a charset
        // in the pattern and needs no base face.
        #[cfg(all(target_os = "macos", not(feature = "freetype")))]
        let candidates = discovery::discover_fallback(self.collection.primary(), &desc);
        #[cfg(feature = "fontconfig")]
        let candidates = crate::fontconfig::discover_fallback(&desc);

        for candidate in candidates {
            // Discovery can't filter by presentation, so verify it here (the
            // deferred face is always a fallback → held to the explicit
            // presentation for a Default(p) request).
            if !candidate.has_codepoint(cp, p_mode, true) {
                continue;
            }
            // Load the winner at the render size and add it as a fallback.
            let Ok(face) = candidate.load(self.size_px) else {
                continue;
            };
            return Some(self.collection.add_fallback(style, face));
        }
        None
    }

    /// No discovery backend compiled in: step 6 finds nothing.
    #[cfg(not(any(
        all(target_os = "macos", not(feature = "freetype")),
        feature = "fontconfig"
    )))]
    fn discover_fallback(
        &mut self,
        _cp: u32,
        _style: Style,
        _p_mode: PresentationMode,
    ) -> Option<FontIndex> {
        None
    }
}

#[cfg(all(test, target_os = "macos", not(feature = "freetype")))]
mod tests {
    use super::*;
    use crate::coretext::Face;

    fn resolver_no_discovery() -> CodepointResolver {
        let face = Face::load_embedded(16.0).expect("load embedded");
        CodepointResolver::without_discovery(Collection::new(face))
    }

    fn resolver_with_discovery() -> CodepointResolver {
        let face = Face::load_embedded(16.0).expect("load embedded");
        CodepointResolver::new(Collection::new(face))
    }

    #[test]
    fn ascii_resolves_to_primary() {
        let mut r = resolver_no_discovery();
        let idx = r.get_index('h' as u32, Style::Regular).expect("h resolves");
        assert_eq!(
            idx,
            FontIndex::Face {
                style: Style::Regular,
                slot: 0
            }
        );
    }

    #[test]
    fn box_drawing_resolves_to_sprite() {
        let mut r = resolver_no_discovery();
        let idx = r.get_index(0x2500, Style::Regular).expect("box resolves");
        assert_eq!(idx, FontIndex::Sprite);
        assert!(idx.is_sprite());
    }

    #[test]
    fn unsupported_codepoint_is_notdef_without_discovery() {
        let mut r = resolver_no_discovery();
        // A private-use codepoint the embedded font lacks and that is not a
        // sprite; with discovery off, this is notdef.
        assert!(r.get_index(0x0F0000, Style::Regular).is_none());
    }

    #[test]
    fn styled_request_routes_to_regular_when_absent() {
        let mut r = resolver_no_discovery();
        // No bold face loaded: a bold 'h' resolves via regular (step 5/7).
        let idx = r
            .get_index('h' as u32, Style::Bold)
            .expect("h resolves bold");
        assert_eq!(
            idx,
            FontIndex::Face {
                style: Style::Regular,
                slot: 0
            }
        );
    }

    /// Ported from `test "getIndex disabled font style"`
    /// (CodepointResolver.zig:467-533): a disabled bold style routes to regular.
    #[test]
    fn disabled_style_routes_to_regular() {
        let regular = Face::load_embedded(16.0).expect("regular");
        let mut col = Collection::new(regular);
        // Populate a bold slot so, absent the disable, bold would resolve to it.
        let bold = Face::load_embedded(16.0).expect("bold");
        col.add(Style::Bold, bold);

        let mut r = CodepointResolver::without_discovery(col);
        r.set_style_enabled(Style::Bold, false);

        // Bold now routes to regular (slot 0 of regular).
        let idx = r.get_index('A' as u32, Style::Bold).expect("A resolves");
        assert_eq!(
            idx,
            FontIndex::Face {
                style: Style::Regular,
                slot: 0
            }
        );
    }

    /// Discovery fallback: an emoji codepoint the primary lacks resolves to a
    /// discovered fallback face (Apple Color Emoji via CTFontCreateForString).
    #[test]
    fn emoji_resolves_via_discovery() {
        let mut r = resolver_with_discovery();
        // U+1F600 GRINNING FACE is not in JetBrains Mono; discovery must find a
        // color font for it.
        let idx = r
            .get_index(0x1F600, Style::Regular)
            .expect("emoji resolves via discovery");
        // It should be a *fallback* slot (slot > 0 in the regular list).
        match idx {
            FontIndex::Face { style, slot } => {
                assert_eq!(style, Style::Regular);
                assert!(slot >= 1, "expected a discovered fallback slot, got {slot}");
                // The discovered face should be a color (emoji) face.
                let face = r.collection().get_face(idx).expect("face present");
                assert!(face.has_color(), "emoji fallback face should be color");
            }
            other => panic!("expected a face index, got {other:?}"),
        }
    }

    /// The completed default style table resolves Bold and Italic to their own
    /// (distinct-from-Regular) faces for a plain ASCII codepoint, and
    /// BoldItalic to a fourth face. This is the core "bold/italic actually get
    /// their own face" guarantee behind the field fix.
    #[test]
    fn default_chain_resolves_distinct_styles() {
        let face = Face::load_embedded(16.0).expect("load embedded");
        let col = Collection::new_with_default_fallbacks(face, 16.0).expect("default style table");
        // Discovery off so we isolate the style-table faces (no system fallback).
        let mut r = CodepointResolver::without_discovery(col);

        let reg = r.get_index('a' as u32, Style::Regular).expect("regular a");
        let bold = r.get_index('a' as u32, Style::Bold).expect("bold a");
        let ital = r.get_index('a' as u32, Style::Italic).expect("italic a");
        let bi = r
            .get_index('a' as u32, Style::BoldItalic)
            .expect("bold-italic a");

        // Each resolves to a face in its own style's list (slot 0), so the four
        // FontIndex values are all distinct.
        assert_eq!(
            reg,
            FontIndex::Face {
                style: Style::Regular,
                slot: 0
            }
        );
        assert_eq!(
            bold,
            FontIndex::Face {
                style: Style::Bold,
                slot: 0
            }
        );
        assert_eq!(
            ital,
            FontIndex::Face {
                style: Style::Italic,
                slot: 0
            }
        );
        assert_eq!(
            bi,
            FontIndex::Face {
                style: Style::BoldItalic,
                slot: 0
            }
        );
        assert_ne!(reg, bold);
        assert_ne!(reg, ital);
        assert_ne!(bold, bi);

        // The bold face carries the wght=700 variation instance; regular does
        // not. This is the default-config mechanism (variation, not synthetic).
        let bold_face = r.collection().get_face(bold).expect("bold face");
        let reg_face = r.collection().get_face(reg).expect("reg face");
        assert_eq!(
            bold_face.wght(),
            Some(700.0),
            "bold is the wght=700 instance"
        );
        assert_eq!(reg_face.wght(), None, "regular is the default instance");
    }

    /// Discovery fallback for CJK: 水 (U+6C34) resolves to a system CJK font.
    #[test]
    fn cjk_resolves_via_discovery() {
        let mut r = resolver_with_discovery();
        let idx = r
            .get_index('水' as u32, Style::Regular)
            .expect("CJK resolves via discovery");
        // JetBrains Mono may actually contain 水 (some builds do); either the
        // primary (slot 0) or a discovered fallback is acceptable, as long as
        // the resolved face has the glyph.
        let face = r.collection().get_face(idx).expect("face present");
        assert!(
            face.glyph_index('水').is_some(),
            "resolved face must have 水"
        );
    }
}

/// Fontconfig-backed discovery-fallback tests (the Linux font stack). These run
/// under `--features fontconfig` and exercise the same step-6 wiring the macOS
/// tests above cover, but through [`crate::fontconfig`] instead of CoreText.
/// They skip-with-note when libfontconfig can't initialise at runtime (e.g. a
/// dev mac without it), like the discovery-module tests, so they are meaningful
/// on the ubuntu CI runner (which has libfontconfig) without being flaky
/// elsewhere.
#[cfg(all(test, feature = "fontconfig"))]
mod fontconfig_tests {
    use super::*;
    use crate::Face;

    fn fontconfig_available() -> bool {
        if fontconfig::Fontconfig::new().is_none() {
            eprintln!(
                "note: libfontconfig unavailable at runtime; resolver fontconfig test skipped"
            );
            false
        } else {
            true
        }
    }

    fn resolver_with_discovery() -> CodepointResolver {
        let face = Face::load_embedded(16.0).expect("load embedded");
        CodepointResolver::new(Collection::new(face))
    }

    /// An emoji codepoint the embedded primary lacks resolves to a discovered
    /// fallback face via fontconfig (the Linux analog of
    /// `tests::emoji_resolves_via_discovery`). Requires a color emoji font on the
    /// system (present on the ubuntu runner + typical desktops).
    #[test]
    fn emoji_resolves_via_fontconfig_discovery() {
        if !fontconfig_available() {
            return;
        }
        let mut r = resolver_with_discovery();
        // U+1F600 GRINNING FACE is not in JetBrains Mono; fontconfig must find a
        // font that covers it. If the host has no font for it at all, the
        // resolver returns None — accept that rather than fail (coverage varies
        // by machine), but when it *does* resolve it must be a fallback slot.
        if let Some(idx) = r.get_index(0x1F600, Style::Regular) {
            match idx {
                FontIndex::Face { style, slot } => {
                    assert_eq!(style, Style::Regular);
                    assert!(slot >= 1, "expected a discovered fallback slot, got {slot}");
                    let face = r.collection().get_face(idx).expect("face present");
                    assert!(
                        face.glyph_index('\u{1F600}').is_some(),
                        "discovered emoji fallback must cover U+1F600"
                    );
                }
                other => panic!("expected a face index, got {other:?}"),
            }
        } else {
            eprintln!("note: no system font covers U+1F600 on this host; emoji check skipped");
        }
    }

    /// CJK 水 (U+6C34) resolves to a covering face — the primary if JetBrains
    /// Mono happens to include it, else a fontconfig-discovered CJK font (the
    /// Linux analog of `tests::cjk_resolves_via_discovery`).
    #[test]
    fn cjk_resolves_via_fontconfig_discovery() {
        if !fontconfig_available() {
            return;
        }
        let mut r = resolver_with_discovery();
        if let Some(idx) = r.get_index('水' as u32, Style::Regular) {
            let face = r.collection().get_face(idx).expect("face present");
            assert!(
                face.glyph_index('水').is_some(),
                "resolved face must have 水"
            );
        } else {
            eprintln!("note: no system font covers 水 on this host; CJK check skipped");
        }
    }
}
