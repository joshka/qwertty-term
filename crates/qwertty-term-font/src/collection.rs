//! A font collection: faces grouped by style and priority, with a fallback
//! list per style.
//!
//! Port of the resolution-relevant surface of Ghostty's `src/font/Collection.zig`
//! (commit `2da015cd6`), per decision 8 of `docs/plans/m3-first-pixels.md`: a
//! slotmap-style arena index (`FontIndex`) instead of the packed `u16`
//! bitfield, while preserving the 4-style grouping, the per-style priority list,
//! the fallback/non-fallback distinction, and the "sprite is a special
//! non-face index" semantics.
//!
//! F5-full extends F5-reduced (one face per style) to a **priority-ordered list
//! per style**: `[user faces…, discovered fallback faces…]`. See
//! `docs/analysis/font-discovery.md` §7 (fallback list + style completion) and
//! `docs/analysis/font-shaping.md` (the reduced index model).
//!
//! Loading model: F5-full loads discovered fallback faces **eagerly** when the
//! resolver adds them (it has the render size in hand), storing a loaded
//! [`Face`]. Deferred faces exist transiently during discovery probing (the
//! resolver probes candidates with the cheap `deferred::DeferredFace`
//! `has_codepoint` and loads only the winner, macOS only). Upstream keeps entries deferred
//! and loads lazily in `getFaceFromEntry`; the eager-on-add reduction keeps the
//! `get_face(index) -> &Face` contract simple while still exercising the
//! deferred probe path. Size-adjustment of fallback faces (`ic_width` rescale)
//! is a documented deferral.

use crate::presentation::PresentationMode;
use crate::{Face, FaceError};

/// Discover a styled member of `family` by system font lookup. macOS uses
/// CoreText discovery; on other platforms (no fontconfig yet) it returns `None`,
/// so `new_with_family_styles` falls through to the synthetic ladder.
#[cfg(all(target_os = "macos", not(feature = "freetype")))]
fn discover_family_style(family: &str, bold: bool, italic: bool, size_px: f64) -> Option<Face> {
    crate::discovery::discover_family_style(family, bold, italic, size_px)
}

#[cfg(not(all(target_os = "macos", not(feature = "freetype"))))]
fn discover_family_style(_family: &str, _bold: bool, _italic: bool, _size_px: f64) -> Option<Face> {
    None
}

/// Font style. Same 4-value grouping as upstream `font.Style` (`main.zig:54`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl Style {
    /// All four styles, in declaration order (for iteration).
    pub const ALL: [Style; 4] = [
        Style::Regular,
        Style::Bold,
        Style::Italic,
        Style::BoldItalic,
    ];
}

/// A handle to a font within a [`Collection`], or the sprite pseudo-font.
///
/// The slotmap-style replacement (decision 8) for upstream's packed
/// `Collection.Index`. `Face { style, slot }` is the analog of the upstream
/// `{ style, idx }` pair (`slot` is the position in that style's priority
/// list); [`FontIndex::Sprite`] is the analog of `Index.initSpecial(.sprite)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontIndex {
    /// A real font face at `slot` within the priority list for `style`.
    Face { style: Style, slot: usize },
    /// The sprite pseudo-font (box drawing, block elements, etc.).
    Sprite,
}

impl FontIndex {
    /// True if this is the special sprite index (upstream `Index.special()`).
    pub fn is_sprite(self) -> bool {
        matches!(self, FontIndex::Sprite)
    }
}

/// A single face in a collection's priority list.
///
/// Mirrors upstream `Collection.Entry` (`Collection.zig:751`), reduced: the face
/// is always loaded (see the loading-model note in the module docs), plus the
/// `fallback` flag that drives the presentation asymmetry in `has_codepoint`.
struct Entry {
    face: Face,
    /// True for discovery-added fallback faces; false for user/primary faces.
    /// Controls the default-presentation matching rule (Collection.zig:808-814).
    fallback: bool,
}

/// A collection of faces grouped by style, each an append-ordered priority list.
///
/// The regular list always has at least the primary after [`Collection::new`].
pub struct Collection {
    regular: Vec<Entry>,
    bold: Vec<Entry>,
    italic: Vec<Entry>,
    bold_italic: Vec<Entry>,
}

impl Collection {
    /// Create a collection with `primary` as the first (non-fallback) regular
    /// face.
    pub fn new(primary: Face) -> Collection {
        Collection {
            regular: vec![Entry {
                face: primary,
                fallback: false,
            }],
            bold: Vec::new(),
            italic: Vec::new(),
            bold_italic: Vec::new(),
        }
    }

    /// Create a collection with `primary` as the first (non-fallback) regular
    /// face, plus upstream's full default style table and explicit nerd-symbols
    /// fallback slot.
    ///
    /// This is the parity-default constructor real Ghostty's `SharedGridSet`
    /// uses when building the default (no font-family configured) grid
    /// (`SharedGridSet.zig:260-330`). It completes the four-style table from the
    /// embedded variable fonts, matching upstream's default-config mechanism
    /// exactly (`wght` variation instancing, **not** synthetic stroke):
    ///
    /// - **Regular**: `primary` (the caller's face; embedded `variable` at
    ///   `wght=400` for the no-family default).
    /// - **Bold**: embedded `variable` at the `wght=700` instance
    ///   (`SharedGridSet.zig:277-287`).
    /// - **Italic**: embedded `variable_italic` at `wght=400`
    ///   (`SharedGridSet.zig:293-300`) — a real discovered face, not synthetic.
    /// - **BoldItalic**: embedded `variable_italic` at `wght=700`
    ///   (`SharedGridSet.zig:306-318`).
    ///
    /// The nerd-symbols font (`symbols_nerd_font`) is added as a `.regular`,
    /// `fallback = true` entry ahead of discovery, so a PUA nerd-font codepoint
    /// resolves to it *without* system discovery. A styled (bold/italic) PUA
    /// lookup reaches it through the resolver's step-5 regular retry, matching
    /// upstream (which only registers the symbols font under `.regular`).
    ///
    /// Each built-in style face is a `fallback = true` entry (upstream default
    /// faces are all `.fallback = true`), so the whole default chain is a
    /// fallback chain behind any user-configured face.
    pub fn new_with_default_fallbacks(
        primary: Face,
        size_px: f64,
    ) -> Result<Collection, FaceError> {
        let mut collection = Collection::new(primary);

        // Complete the style table from the embedded variable fonts, mirroring
        // upstream's default-config `wght`-variation mechanism.
        let bold = Face::load_embedded_bold(size_px)?;
        collection.add_fallback(Style::Bold, bold);

        let italic = Face::load_embedded_italic(size_px)?;
        collection.add_fallback(Style::Italic, italic);

        let bold_italic = Face::load_embedded_bold_italic(size_px)?;
        collection.add_fallback(Style::BoldItalic, bold_italic);

        let symbols = Face::load_embedded_symbols_nerd_font(size_px)?;
        collection.add_fallback(Style::Regular, symbols);

        // macOS: pre-seed the system emoji font as a fallback so emoji resolve
        // to Apple Color Emoji at the collection step, ahead of any runtime
        // discovery (which ranks by glyph count and would otherwise prefer a
        // user-installed Noto Color Emoji). Upstream: SharedGridSet.zig:335-354.
        collection.add_apple_emoji_fallback(size_px);
        Ok(collection)
    }

    /// macOS-only: discover the system **Apple Color Emoji** font by family name
    /// and add it as a `.regular`, `fallback = true` entry.
    ///
    /// Direct port of upstream `SharedGridSet.zig:335-354`: "On macOS, always
    /// search for and add the Apple Emoji font as our preferred emoji font for
    /// fallback … in case people add other emoji fonts to their system, we
    /// always want to prefer the official one." This is the load-bearing fix for
    /// the field bug where U+1F980 (🦀) etc. resolved to a user-installed Noto
    /// Color Emoji (more glyphs → wins the discovery Score tiebreak) instead of
    /// Apple Color Emoji. A no-op if the font can't be discovered (never expected
    /// on macOS, where it ships with the OS).
    #[cfg(all(target_os = "macos", not(feature = "freetype")))]
    fn add_apple_emoji_fallback(&mut self, size_px: f64) {
        if let Some(face) = crate::discovery::discover_family("Apple Color Emoji", size_px) {
            self.add_fallback(Style::Regular, face);
        }
    }

    /// Without the CoreText backend: no system Apple emoji font to pre-seed
    /// (upstream adds the embedded Noto emoji fonts there instead). No-op.
    #[cfg(not(all(target_os = "macos", not(feature = "freetype"))))]
    fn add_apple_emoji_fallback(&mut self, _size_px: f64) {}

    /// Create a collection for a **configured `font-family`** (`primary`), whose
    /// bold/italic/bold-italic slots are filled from the family's *own* styled
    /// members when they exist, then completed with upstream's synthetic ladder,
    /// with the embedded default style chain + nerd-symbols behind them.
    ///
    /// This is the named-family analog of [`Collection::new_with_default_fallbacks`].
    /// It mirrors the two-phase upstream construction in
    /// `SharedGridSet.collection` (`SharedGridSet.zig:157-330`):
    ///
    /// 1. **Per-style discovery of the configured family** (`SharedGridSet.zig:191-247`):
    ///    for each styled slot, discover a descriptor carrying `family` + the
    ///    style's bold/italic symbolic traits and take the top-ranked member of
    ///    that same family. For FiraCode Nerd Font Mono this yields a real
    ///    **FiraCode Bold** for the bold slot (family name stays FiraCode, never
    ///    JetBrains Mono). Added as **non-fallback** (upstream
    ///    `addDeferred(..., .fallback = false)`).
    ///
    /// 2. **`completeStyles` synthetic ladder** (`Collection.zig:319-465`) for any
    ///    slot the family didn't provide (FiraCode has no italic / bold-italic):
    ///    - **italic** absent → synthetic italic via the `ITALIC_SKEW` matrix
    ///      (`Collection.zig:373-393` → `face/coretext.zig:174-178`), falling back
    ///      to an **alias to regular** if the skew copy fails.
    ///    - **bold** absent → synthetic bold via the stroke mechanism
    ///      (`Collection.zig:398-421` → `syntheticBold`), else alias to regular.
    ///    - **bold-italic** absent → synthesize italic on top of the bold face we
    ///      have, else synthesize bold on top of italic, else alias
    ///      (`Collection.zig:424-465`). Here: skew the discovered bold.
    ///
    /// Then, **behind** those (as `fallback = true` entries), the embedded default
    /// style chain (`SharedGridSet.zig:262-333`: embedded variable @ wght 700 for
    /// bold, variable-italic for italic/bold-italic) and the nerd-symbols font, so
    /// a codepoint the configured family lacks still resolves. Emoji discovery is
    /// unchanged (handled by the resolver, not here).
    ///
    /// (`ITALIC_SKEW` = `coretext::ITALIC_SKEW` on macOS.)
    pub fn new_with_family_styles(
        primary: Face,
        family: &str,
        size_px: f64,
    ) -> Result<Collection, FaceError> {
        let mut collection = Collection::new(primary);

        // --- Phase 1: discover the configured family's own styled members. ---
        // A real discovered styled member is a non-fallback (user) face: it is
        // the configured font, just a different weight/slant. On Linux (no
        // system discovery yet) these all return None and Phase 2's synthetic
        // ladder fills every styled slot.
        if let Some(face) = discover_family_style(family, true, false, size_px) {
            collection.add(Style::Bold, face);
        }
        if let Some(face) = discover_family_style(family, false, true, size_px) {
            collection.add(Style::Italic, face);
        }
        if let Some(face) = discover_family_style(family, true, true, size_px) {
            collection.add(Style::BoldItalic, face);
        }

        // --- Phase 2: complete missing styles with the synthetic ladder. ---
        // The base for synthesis is the primary (first regular) face.
        let line_width = Face::synthetic_bold_line_width(size_px);

        // Italic: skew the regular; alias-to-regular (a plain regular clone) on
        // failure. We clone by re-copying the primary's CTFont at the same size.
        let have_italic = !collection.italic.is_empty();
        if !have_italic {
            match collection.primary().synthetic_italic() {
                Ok(face) => {
                    collection.add(Style::Italic, face);
                }
                Err(_) => {
                    // alias-to-regular: a straight copy of the primary.
                    let alias = collection.primary_clone(size_px)?;
                    collection.add(Style::Italic, alias);
                }
            }
        }

        // Bold: synthetic-bold stroke of the regular.
        let have_bold = !collection.bold.is_empty();
        if !have_bold {
            let base = collection.primary_clone(size_px)?;
            collection.add(Style::Bold, base.synthetic_bold(line_width));
        }

        // Bold-italic: prefer synthesizing italic on top of the bold we have
        // (real or synthetic); else synthesize bold on top of italic
        // (Collection.zig:424-465). Upstream's final "alias to italic" branch
        // is only reachable when *both* syntheses fail; here synthetic bold is
        // infallible once the italic base clones, so the clone error is the
        // only failure and it propagates rather than aliasing.
        let have_bold_italic = !collection.bold_italic.is_empty();
        if !have_bold_italic {
            // At this point bold is guaranteed populated (real or synthetic).
            let bold_face = collection
                .face_for_style(Style::Bold)
                .expect("bold populated above");
            match bold_face.synthetic_italic() {
                Ok(face) => {
                    collection.add(Style::BoldItalic, face);
                }
                Err(_) => {
                    // Fall back to synthetic bold of the italic face.
                    let italic_face = collection
                        .face_for_style(Style::Italic)
                        .expect("italic populated above")
                        .try_clone(size_px)?;
                    collection.add(Style::BoldItalic, italic_face.synthetic_bold(line_width));
                }
            }
        }

        // --- Behind the configured styles: the embedded default chain + symbols.
        // These are fallback=true so they sit after the configured faces in each
        // style's priority list (SharedGridSet.zig:262-333).
        let embedded_bold = Face::load_embedded_bold(size_px)?;
        collection.add_fallback(Style::Bold, embedded_bold);

        let embedded_italic = Face::load_embedded_italic(size_px)?;
        collection.add_fallback(Style::Italic, embedded_italic);

        let embedded_bold_italic = Face::load_embedded_bold_italic(size_px)?;
        collection.add_fallback(Style::BoldItalic, embedded_bold_italic);

        let symbols = Face::load_embedded_symbols_nerd_font(size_px)?;
        collection.add_fallback(Style::Regular, symbols);

        // macOS system emoji fallback (SharedGridSet.zig:335-354), same as the
        // no-family default chain: emoji resolve to Apple Color Emoji here rather
        // than a user-installed third-party emoji font via discovery.
        collection.add_apple_emoji_fallback(size_px);

        Ok(collection)
    }

    /// Append a user (non-fallback) face for `style`, returning its
    /// [`FontIndex`] (upstream `add`, Collection.zig:112).
    pub fn add(&mut self, style: Style, face: Face) -> FontIndex {
        self.push(
            style,
            Entry {
                face,
                fallback: false,
            },
        )
    }

    /// Append a discovery-found fallback face for `style`, returning its
    /// [`FontIndex`] (upstream `addDeferred` with `fallback = true`,
    /// Collection.zig:164). The face is already loaded (see module docs).
    pub fn add_fallback(&mut self, style: Style, face: Face) -> FontIndex {
        self.push(
            style,
            Entry {
                face,
                fallback: true,
            },
        )
    }

    fn push(&mut self, style: Style, entry: Entry) -> FontIndex {
        let list = self.list_mut(style);
        list.push(entry);
        FontIndex::Face {
            style,
            slot: list.len() - 1,
        }
    }

    fn list(&self, style: Style) -> &[Entry] {
        match style {
            Style::Regular => &self.regular,
            Style::Bold => &self.bold,
            Style::Italic => &self.italic,
            Style::BoldItalic => &self.bold_italic,
        }
    }

    fn list_mut(&mut self, style: Style) -> &mut Vec<Entry> {
        match style {
            Style::Regular => &mut self.regular,
            Style::Bold => &mut self.bold,
            Style::Italic => &mut self.italic,
            Style::BoldItalic => &mut self.bold_italic,
        }
    }

    /// The index of the first face in `style`'s priority list that satisfies
    /// `cp` under `p_mode`, or `None` (upstream `getIndex`, Collection.zig:272).
    ///
    /// Searches in priority (append) order; the per-entry `fallback` flag drives
    /// the default-presentation matching rule.
    pub fn get_index(&self, cp: u32, style: Style, p_mode: PresentationMode) -> Option<FontIndex> {
        let list = self.list(style);
        for (slot, entry) in list.iter().enumerate() {
            if entry.face.has_codepoint(cp, p_mode, entry.fallback) {
                return Some(FontIndex::Face { style, slot });
            }
        }
        None
    }

    /// True if `index` has `cp` under `p_mode` (upstream `hasCodepoint`,
    /// Collection.zig:299).
    pub fn has_codepoint(&self, index: FontIndex, cp: u32, p_mode: PresentationMode) -> bool {
        let FontIndex::Face { style, slot } = index else {
            return false;
        };
        self.list(style)
            .get(slot)
            .is_some_and(|e| e.face.has_codepoint(cp, p_mode, e.fallback))
    }

    /// Get the face for a real (non-sprite) [`FontIndex`], or `None` if the slot
    /// is empty or the index is [`FontIndex::Sprite`].
    pub fn get_face(&self, index: FontIndex) -> Option<&Face> {
        let FontIndex::Face { style, slot } = index else {
            return None;
        };
        self.list(style).get(slot).map(|e| &e.face)
    }

    /// The face for `style`'s first (highest-priority) entry, or `None` if the
    /// style list is empty.
    pub fn face_for_style(&self, style: Style) -> Option<&Face> {
        self.list(style).first().map(|e| &e.face)
    }

    /// The primary (first regular) face. Always present.
    pub fn primary(&self) -> &Face {
        &self.regular[0].face
    }

    /// An independent copy of the primary face at `size_px` (used as the base
    /// for synthetic styled faces, so the stored primary is not consumed).
    fn primary_clone(&self, size_px: f64) -> Result<Face, FaceError> {
        self.primary().try_clone(size_px)
    }

    /// True if every style list has at least one entry.
    pub fn all_styles_populated(&self) -> bool {
        Style::ALL.iter().all(|&s| !self.list(s).is_empty())
    }
}

#[cfg(all(test, target_os = "macos", not(feature = "freetype")))]
mod tests {
    use super::*;
    use crate::presentation::PresentationMode;

    /// Analog of the upstream `Collection.zig` `init` + basic add/get tests: a
    /// regular face round-trips through the collection and the primary is
    /// always reachable.
    #[test]
    fn regular_face_round_trips() {
        let face = Face::load_embedded(16.0).expect("load embedded");
        let col = Collection::new(face);

        let idx = FontIndex::Face {
            style: Style::Regular,
            slot: 0,
        };
        assert!(col.get_face(idx).is_some());
        assert!(col.primary().glyph_index('A').is_some());
    }

    /// Style grouping: bold is stored in a separate list from regular; an
    /// unpopulated style list resolves to `None`.
    #[test]
    fn styles_are_grouped_separately() {
        let regular = Face::load_embedded(16.0).expect("load regular");
        let mut col = Collection::new(regular);

        assert!(col.face_for_style(Style::Bold).is_none());
        assert!(col.face_for_style(Style::Italic).is_none());

        let bold = Face::load_embedded(16.0).expect("load bold");
        let idx = col.add(Style::Bold, bold);
        assert_eq!(
            idx,
            FontIndex::Face {
                style: Style::Bold,
                slot: 0
            }
        );
        assert!(col.face_for_style(Style::Bold).is_some());
        assert!(col.face_for_style(Style::Regular).is_some());
    }

    /// The sprite index is a non-face index (`get_face` returns `None`).
    #[test]
    fn sprite_index_is_special() {
        let face = Face::load_embedded(16.0).expect("load embedded");
        let col = Collection::new(face);

        assert!(FontIndex::Sprite.is_sprite());
        assert!(col.get_face(FontIndex::Sprite).is_none());
        assert!(
            !FontIndex::Face {
                style: Style::Regular,
                slot: 0
            }
            .is_sprite()
        );
    }

    /// A second face appended to the same style takes the next slot (priority
    /// list), and `get_index` searches in append order.
    #[test]
    fn priority_list_appends() {
        let primary = Face::load_embedded(16.0).expect("primary");
        let mut col = Collection::new(primary);
        let second = Face::load_embedded(16.0).expect("second");
        let idx = col.add_fallback(Style::Regular, second);
        assert_eq!(
            idx,
            FontIndex::Face {
                style: Style::Regular,
                slot: 1
            }
        );

        // 'A' is in the primary (slot 0), so get_index returns slot 0 first.
        let found = col
            .get_index('A' as u32, Style::Regular, PresentationMode::Any)
            .expect("A resolves");
        assert_eq!(
            found,
            FontIndex::Face {
                style: Style::Regular,
                slot: 0
            }
        );
    }

    // --- Named-family styled-face completion (family-styles chunk) ---

    const FIRA: &str = "FiraCode Nerd Font Mono";

    /// Total ink coverage of a glyph in a face (sum of Alpha8 bytes). Heavier
    /// (bolder) strokes cover more, so bold > regular for the same glyph.
    fn ink_coverage(face: &Face, c: char) -> u64 {
        let gid = face.glyph_index(c).expect("glyph exists");
        let bmp = face.rasterize(gid).expect("rasterize");
        bmp.data.iter().map(|&b| u64::from(b)).sum()
    }

    /// True if FiraCode Nerd Font Mono is installed (its regular member is
    /// discoverable). Tests that need the family SKIP gracefully otherwise.
    fn fira_installed() -> bool {
        crate::discovery::discover_family_style(FIRA, false, false, 16.0).is_some()
    }

    /// The configured-family chain resolves `Style::Bold` to FiraCode's *own*
    /// bold member — family name FiraCode, never JetBrains Mono. This is the
    /// core regression the family-styles chunk fixes.
    #[test]
    fn fira_bold_is_fira_not_jetbrains() {
        if !fira_installed() {
            eprintln!("SKIP: {FIRA} not installed; family-styled bold test skipped");
            return;
        }
        let primary = Face::load_by_name(FIRA, 16.0).expect("load FiraCode primary");
        assert!(
            primary.family_name().to_lowercase().contains("fira"),
            "primary should be FiraCode, got {:?}",
            primary.family_name()
        );

        let col = Collection::new_with_family_styles(primary, FIRA, 16.0).expect("build chain");

        // Bold slot 0 is FiraCode's own bold, not the embedded JetBrains Mono.
        let bold = col.face_for_style(Style::Bold).expect("bold populated");
        let bold_family = bold.family_name().to_lowercase();
        assert!(
            bold_family.contains("fira"),
            "bold face family should be FiraCode, got {:?}",
            bold.family_name()
        );
        assert!(
            !bold_family.contains("jetbrains"),
            "bold face must NOT be JetBrains Mono, got {:?}",
            bold.family_name()
        );

        // Every style is populated, and the embedded default chain sits behind
        // the configured faces (bold list has both the FiraCode bold and the
        // embedded fallback).
        assert!(col.all_styles_populated());
        assert!(
            col.bold.len() >= 2,
            "bold list should carry the FiraCode bold plus the embedded fallback"
        );
        assert!(
            !col.bold[0].fallback,
            "FiraCode bold is a non-fallback face"
        );
        assert!(
            col.bold.last().unwrap().fallback,
            "embedded bold is a fallback behind it"
        );
    }

    /// The styled *resolver path* (`discover_family_style` directly) returns
    /// FiraCode's bold member, mirroring `SharedGridSet`'s per-style discovery.
    #[test]
    fn fira_styled_resolver_returns_fira_bold() {
        if !fira_installed() {
            eprintln!("SKIP: {FIRA} not installed; styled resolver test skipped");
            return;
        }
        let bold = crate::discovery::discover_family_style(FIRA, true, false, 16.0)
            .expect("FiraCode has a real bold member");
        let name = bold.family_name().to_lowercase();
        assert!(
            name.contains("fira") && !name.contains("jetbrains"),
            "styled bold resolver should return FiraCode bold, got {:?}",
            bold.family_name()
        );
    }

    /// Offscreen ink-coverage: FiraCode's bold 'M' is measurably heavier than
    /// its regular 'M'.
    #[test]
    fn fira_bold_heavier_than_regular() {
        if !fira_installed() {
            eprintln!("SKIP: {FIRA} not installed; ink-coverage test skipped");
            return;
        }
        let primary = Face::load_by_name(FIRA, 16.0).expect("load FiraCode primary");
        let col = Collection::new_with_family_styles(primary, FIRA, 16.0).expect("build chain");

        let regular = ink_coverage(col.face_for_style(Style::Regular).unwrap(), 'M');
        let bold = ink_coverage(col.face_for_style(Style::Bold).unwrap(), 'M');
        eprintln!("FiraCode 'M' ink coverage: regular={regular} bold={bold}");
        assert!(
            bold > regular,
            "bold 'M' ({bold}) should be heavier than regular 'M' ({regular})"
        );
    }

    /// FiraCode has no italic member, so the italic slot is a *synthetic*
    /// (skewed) FiraCode — still FiraCode family, not JetBrains Mono — populated
    /// ahead of the embedded italic fallback.
    #[test]
    fn fira_italic_is_synthetic_fira() {
        if !fira_installed() {
            eprintln!("SKIP: {FIRA} not installed; synthetic-italic test skipped");
            return;
        }
        let primary = Face::load_by_name(FIRA, 16.0).expect("load FiraCode primary");
        let col = Collection::new_with_family_styles(primary, FIRA, 16.0).expect("build chain");

        let italic = col.face_for_style(Style::Italic).expect("italic populated");
        let fam = italic.family_name().to_lowercase();
        assert!(
            fam.contains("fira") && !fam.contains("jetbrains"),
            "synthetic italic should still be FiraCode, got {:?}",
            italic.family_name()
        );
        assert!(
            italic.glyph_index('a').is_some(),
            "synthetic italic renders"
        );
    }

    /// An unknown/uninstalled family name falls back per the ladder without
    /// panicking: discovery finds nothing for the styled slots, so bold is
    /// synthetic-bold of the given primary and every style is populated.
    #[test]
    fn unknown_family_falls_back_without_panic() {
        // `load_by_name` on a nonsense name yields the embedded JetBrains Mono;
        // we feed that as the primary and use a nonsense family for discovery.
        let primary =
            Face::load_by_name("ThisFontDoesNotExist98765", 16.0).expect("embedded fallback");
        let col = Collection::new_with_family_styles(primary, "ThisFontDoesNotExist98765", 16.0)
            .expect("build chain without panic");

        // No discovery match for any style → the synthetic ladder + embedded
        // chain still populate every slot.
        assert!(col.all_styles_populated());
        // Bold 'M' (synthetic-bold of the primary) is heavier than regular 'M'.
        let regular = ink_coverage(col.face_for_style(Style::Regular).unwrap(), 'M');
        let bold = ink_coverage(col.face_for_style(Style::Bold).unwrap(), 'M');
        assert!(
            bold > regular,
            "synthetic bold 'M' ({bold}) should be heavier than regular ({regular})"
        );
    }
}
