//! Page-independent hyperlink value cached on the cursor.
//!
//! Port of `src/terminal/hyperlink.zig`'s heap-allocated `Hyperlink` (the
//! page-independent form), which the Page chunk deliberately did NOT port
//! (page memory only holds the offset-based `PageEntry`). The cursor keeps one
//! of these so an active OSC 8 link can be re-inserted into a new page whenever
//! the cursor's page pin changes (scroll/resize/capacity increase).

/// The link id: an explicit client-provided string, or an implicit monotonic
/// counter. Port of `hyperlink.Hyperlink.id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HyperlinkId {
    Explicit(Vec<u8>),
    Implicit(u32),
}

/// A page-independent hyperlink (URI + id), owned on the heap. Port of
/// `hyperlink.Hyperlink`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hyperlink {
    pub uri: Vec<u8>,
    pub id: HyperlinkId,
}

impl Hyperlink {
    /// The explicit id bytes, if any (for re-inserting into a page).
    pub fn explicit_id(&self) -> Option<&[u8]> {
        match &self.id {
            HyperlinkId::Explicit(v) => Some(v),
            HyperlinkId::Implicit(_) => None,
        }
    }
}
