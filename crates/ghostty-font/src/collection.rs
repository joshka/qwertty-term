//! A font collection: faces grouped by style and priority.
//!
//! Reduced port of Ghostty's `src/font/Collection.zig` (commit `2da015cd6`)
//! per decision 8 of `docs/plans/m3-first-pixels.md`: a slotmap-style arena
//! index instead of the upstream packed `u16` bitfield, while preserving the
//! 4-style grouping and the "sprite is a special non-face index" distinction.
//!
//! See `docs/analysis/font-shaping.md` for the full analysis of upstream's
//! style grouping and index model and the reasoning behind the reduced cut.
//!
//! What is reduced: the reduced Collection stores a single `regular` primary
//! face plus reserved (initially empty) slots for `bold`/`italic`/
//! `bold_italic`. Deferred faces, fallback faces, size adjustment, and the
//! 8192-per-style packed cap are all deferred completeness passes.

use crate::coretext::Face;

/// Font style. Same 4-value grouping as upstream `font.Style` (`main.zig:54`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

/// A handle to a font within a [`Collection`], or the sprite pseudo-font.
///
/// This is the slotmap-style replacement (decision 8) for upstream's packed
/// `Collection.Index` (`Collection.zig:891`). `Face { style, slot }` is the
/// analog of the upstream `{ style, idx }` pair; [`FontIndex::Sprite`] is the
/// analog of `Index.initSpecial(.sprite)` (`Index.special()`), the special
/// non-face index meaning "drawn procedurally by the sprite subsystem."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontIndex {
    /// A real font face at `slot` within the priority list for `style`.
    Face { style: Style, slot: usize },
    /// The sprite pseudo-font (box drawing, block elements, etc.).
    Sprite,
}

impl FontIndex {
    /// True if this is the special sprite index (upstream `Index.special()`
    /// returning non-null).
    pub fn is_sprite(self) -> bool {
        matches!(self, FontIndex::Sprite)
    }
}

/// A collection of faces grouped by style and priority.
///
/// Reduced: one optional face per style, with `regular` always present after
/// [`Collection::new`]. Additional-priority faces and fallback faces are a
/// deferred completeness pass; the public shape (per-style slots, priority
/// order by `add`) is kept so the fuller port slots in without an API change.
pub struct Collection {
    regular: Face,
    bold: Option<Face>,
    italic: Option<Face>,
    bold_italic: Option<Face>,
}

impl Collection {
    /// Create a collection with `primary` as the regular-style face.
    pub fn new(primary: Face) -> Collection {
        Collection {
            regular: primary,
            bold: None,
            italic: None,
            bold_italic: None,
        }
    }

    /// Add a face for `style`, returning its [`FontIndex`].
    ///
    /// Reduced: there is one slot per style, so re-adding a style replaces it.
    /// The returned index always has `slot == 0`. Upstream appends to a
    /// priority list and returns the appended position; that priority list is
    /// a deferred completeness pass.
    pub fn add(&mut self, style: Style, face: Face) -> FontIndex {
        match style {
            Style::Regular => self.regular = face,
            Style::Bold => self.bold = Some(face),
            Style::Italic => self.italic = Some(face),
            Style::BoldItalic => self.bold_italic = Some(face),
        }
        FontIndex::Face { style, slot: 0 }
    }

    /// Get the face for a real (non-sprite) [`FontIndex`], or `None` if the
    /// style slot is empty (or the index is [`FontIndex::Sprite`]).
    pub fn get_face(&self, index: FontIndex) -> Option<&Face> {
        let FontIndex::Face { style, .. } = index else {
            return None;
        };
        self.face_for_style(style)
    }

    /// The face for `style`, or `None` if that style slot is empty.
    pub fn face_for_style(&self, style: Style) -> Option<&Face> {
        match style {
            Style::Regular => Some(&self.regular),
            Style::Bold => self.bold.as_ref(),
            Style::Italic => self.italic.as_ref(),
            Style::BoldItalic => self.bold_italic.as_ref(),
        }
    }

    /// The primary (regular) face. Always present.
    pub fn primary(&self) -> &Face {
        &self.regular
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// Analog of the upstream `Collection.zig` `init` + basic add/get tests
    /// (reduced): a regular face round-trips through the collection and the
    /// primary is always reachable.
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

    /// Style grouping: bold is stored in a separate slot from regular, and an
    /// unpopulated style slot resolves to `None` (the reduced resolver routes
    /// these to regular; here we just assert the slots are distinct).
    #[test]
    fn styles_are_grouped_separately() {
        let regular = Face::load_embedded(16.0).expect("load regular");
        let mut col = Collection::new(regular);

        // Empty style slots.
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
        // Regular is untouched.
        assert!(col.face_for_style(Style::Regular).is_some());
    }

    /// Analog of the upstream `Index.special()` distinction: the sprite index
    /// is a non-face index (`get_face` returns `None`).
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
}
