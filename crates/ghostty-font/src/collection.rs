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
//! resolver probes candidates with the cheap [`crate::deferred::DeferredFace`]
//! `has_codepoint` and loads only the winner). Upstream keeps entries deferred
//! and loads lazily in `getFaceFromEntry`; the eager-on-add reduction keeps the
//! `get_face(index) -> &Face` contract simple while still exercising the
//! deferred probe path. Size-adjustment of fallback faces (`ic_width` rescale)
//! is a documented deferral.

use crate::coretext::Face;
use crate::presentation::PresentationMode;

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
    ) -> Result<Collection, crate::coretext::Error> {
        let mut collection = Collection::new(primary);

        // Complete the style table from the embedded variable fonts, mirroring
        // upstream's default-config `wght`-variation mechanism.
        let bold = crate::coretext::Face::load_embedded_bold(size_px)?;
        collection.add_fallback(Style::Bold, bold);

        let italic = crate::coretext::Face::load_embedded_italic(size_px)?;
        collection.add_fallback(Style::Italic, italic);

        let bold_italic = crate::coretext::Face::load_embedded_bold_italic(size_px)?;
        collection.add_fallback(Style::BoldItalic, bold_italic);

        let symbols = crate::coretext::Face::load_embedded_symbols_nerd_font(size_px)?;
        collection.add_fallback(Style::Regular, symbols);
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

    /// True if every style list has at least one entry.
    pub fn all_styles_populated(&self) -> bool {
        Style::ALL.iter().all(|&s| !self.list(s).is_empty())
    }
}

#[cfg(all(test, target_os = "macos"))]
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
}
