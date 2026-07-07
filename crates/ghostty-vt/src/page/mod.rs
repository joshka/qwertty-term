//! Page-based scrollback memory model (port of `src/terminal/page.zig`).
//!
//! Everything in this crate sits on this module: offset-addressed pages laid out as
//! `[Rows][Cells][Styles][Graphemes][Strings][Hyperlinks]`, bitmap allocators for
//! grapheme/string data, ref-counted style deduplication, and (later, in the PageList
//! chunk) pin-tracked persistent references. Ported first, per the Phase 1 plan.
//!
//! `PageList.zig` is a follow-on chunk; the API here is shaped for it (capacity
//! adjustment, `clone_from`, integrity checks).
//!
//! See `docs/analysis/page-memory.md` for the maintainer-grade survey of the Zig
//! implementation this ports (ghostty commit `2da015cd6`).

pub mod bitmap;
pub mod hash;
pub mod hyperlink;
pub mod offset_map;
pub mod ref_set;
pub mod size;
pub mod style;

mod page_impl;

pub use page_impl::{
    Capacity, Cell, CloneFromError, ContentTag, GraphemeError, HyperlinkInsertId,
    InsertHyperlinkError, IntegrityError, OutOfMemory, Page, ReflowManagedError, Row,
    SemanticContent, SemanticPrompt, Size, Wide,
};

/// The byte size of a standard-capacity page's layout. Used by PageList to size its
/// pool item and decide pooled-vs-non-standard pages. Port of `PageList.std_size`.
pub fn size_of_std_page() -> usize {
    page_impl::Layout::compute(Capacity::std()).total_size
}

/// The byte length of a page's backing memory. Used by PageList byte accounting.
pub fn page_byte_len(page: &Page) -> usize {
    page.byte_len()
}

/// The total layout byte size for a capacity. Used by PageList's `increaseCapacity`
/// to detect capacities that overflow the max page size.
pub fn layout_total_size(cap: Capacity) -> usize {
    page_impl::Layout::compute(cap).total_size
}

/// The default style ID (0). Re-exported for PageList reflow.
pub fn style_default_id() -> style::Id {
    style::DEFAULT_ID
}
