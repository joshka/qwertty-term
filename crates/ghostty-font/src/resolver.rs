//! Codepoint → font resolution.
//!
//! Reduced port of Ghostty's `src/font/CodepointResolver.zig` (commit
//! `2da015cd6`). Upstream's `getIndex` is a 7-step chain (documented in
//! `docs/analysis/font-shaping.md`); the reduced cut implements the two steps
//! the first-pixels scope needs — **sprite dispatch** (step 3) and
//! **primary-face exact match** (step 4) — and returns notdef otherwise.
//!
//! Deferred (see the analysis): codepoint overrides (step 2), disabled-style
//! handling / regular retry (steps 1, 5), discovery fallback (step 6), and the
//! any-presentation last resort (step 7). With a single regular face and no
//! discovery, a miss after the primary check has nothing left to try.

use crate::collection::{Collection, FontIndex, Style};

/// Resolves codepoints to a [`FontIndex`] against a [`Collection`] and the
/// procedural sprite subsystem.
pub struct CodepointResolver {
    collection: Collection,
}

impl CodepointResolver {
    /// Create a resolver over `collection`. Sprite dispatch is always enabled
    /// in the reduced cut (upstream gates it on a nullable `sprite` field;
    /// first pixels always want it, so it is unconditional here).
    pub fn new(collection: Collection) -> CodepointResolver {
        CodepointResolver { collection }
    }

    /// Borrow the underlying collection (for faces / rasterization).
    pub fn collection(&self) -> &Collection {
        &self.collection
    }

    /// Resolve `cp` to a font index, mirroring the reduced resolution chain.
    ///
    /// Order (reduced from upstream's 7 steps):
    ///
    /// 1. **Sprite dispatch** (upstream step 3): if the sprite subsystem draws
    ///    this codepoint, return [`FontIndex::Sprite`]. Checked first so a
    ///    box-drawing codepoint always routes to the procedural rasterizer even
    ///    if the font happens to have a glyph for it — matching upstream, where
    ///    sprite dispatch precedes the loaded-font search.
    /// 2. **Primary-face exact match** (upstream step 4, reduced to one face):
    ///    if the primary face has a glyph for `cp`, return its index.
    /// 3. Otherwise `None` (notdef). Upstream's steps 5-7 (regular retry /
    ///    discovery / any-presentation) have nothing to act on with a single
    ///    face and no discovery, so they collapse to this.
    ///
    /// `style` is accepted for API parity but the reduced cut routes every
    /// style to the regular primary (there are no bold/italic faces loaded);
    /// this is the same observable behavior as upstream steps 1/5 when styled
    /// faces are absent.
    pub fn get_index(&self, cp: u32, _style: Style) -> Option<FontIndex> {
        // Step 3 (upstream): sprite dispatch.
        if ghostty_sprite::has_codepoint(cp) {
            return Some(FontIndex::Sprite);
        }

        // Step 4 (upstream), reduced to the primary face.
        if let Some(ch) = char::from_u32(cp)
            && self.collection.primary().glyph_index(ch).is_some()
        {
            return Some(FontIndex::Face {
                style: Style::Regular,
                slot: 0,
            });
        }

        None
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::coretext::Face;

    fn resolver() -> CodepointResolver {
        let face = Face::load_embedded(16.0).expect("load embedded");
        CodepointResolver::new(Collection::new(face))
    }

    #[test]
    fn ascii_resolves_to_primary() {
        let r = resolver();
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
        let r = resolver();
        // U+2500 BOX DRAWINGS LIGHT HORIZONTAL is a sprite codepoint.
        let idx = r.get_index(0x2500, Style::Regular).expect("box resolves");
        assert_eq!(idx, FontIndex::Sprite);
        assert!(idx.is_sprite());
    }

    #[test]
    fn unsupported_codepoint_is_notdef() {
        let r = resolver();
        // A private-use codepoint the embedded font has no glyph for and that
        // is not a sprite range.
        assert!(r.get_index(0x0F0000, Style::Regular).is_none());
    }

    #[test]
    fn styled_request_routes_to_regular() {
        let r = resolver();
        // No bold face loaded: a bold request still resolves 'h' via regular.
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
}
