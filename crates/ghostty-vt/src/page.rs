//! Page-based scrollback memory model (port of `src/terminal/page.zig` and
//! `PageList.zig`).
//!
//! Everything in this crate sits on this module: offset-addressed pages laid out as
//! `[Rows][Cells][Styles][Graphemes][Strings][Hyperlinks]`, bitmap allocators for
//! grapheme/string data, ref-counted style deduplication, and pin-tracked persistent
//! references. Ported first, per the Phase 1 plan.

// Phase 1 work lands here; see docs/rewrite-prompt.md ("Signature designs", item 1).
