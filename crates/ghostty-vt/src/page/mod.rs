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
    Capacity, Cell, CloneFromError, ContentTag, GraphemeError, InsertHyperlinkError, OutOfMemory,
    Page, Row, SemanticContent, SemanticPrompt, Size, Wide,
};
