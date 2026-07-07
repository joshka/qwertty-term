//! The cursor and its cached page pointers (port of `Screen.Cursor`,
//! `Screen.SavedCursor`, and `cursor.zig`'s `Style`, commit `2da015cd6`).

use crate::page::hyperlink;
use crate::page::size::{CellCountInt, OffsetInt};
use crate::page::style::{self, Style};
use crate::page::{Cell, Row, SemanticContent};
use crate::pagelist::Pin;

use super::hyperlink::Hyperlink;
use crate::charsets::CharsetState;

/// The visual style of the cursor. Port of `cursor.zig`'s `Style`.
///
/// Whether it blinks is determined by mode 12 (a Terminal concern). This is
/// synchronized with CSI q / DECSCUSR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorStyle {
    /// DECSCUSR 5, 6.
    Bar,
    /// DECSCUSR 1, 2. The default (callers are encouraged to set their own).
    #[default]
    Block,
    /// DECSCUSR 3, 4.
    Underline,
    /// Hollow block. Custom to Ghostty; reported as block.
    BlockHollow,
}

/// The cursor position and style. Port of `Screen.Cursor`.
///
/// `page_pin` is the source of truth (a tracked pin that PageList keeps
/// accurate across all mutations). `x`/`y` (active-area coordinates) and
/// `page_row`/`page_cell` are a cache that every mutating op must keep coherent
/// with the pin, or recover via `cursor_reload`.
pub struct Cursor {
    /// The x/y position within the active area.
    pub x: CellCountInt,
    pub y: CellCountInt,

    /// The visual style of the cursor.
    pub cursor_style: CursorStyle,

    /// "Last column flag (LCF)": if set, the next print forces a soft-wrap.
    pub pending_wrap: bool,

    /// If true, newly printed characters get the protected attribute.
    pub protected: bool,

    /// The active style *value* (source of truth for the style).
    pub style: Style,

    /// The active style id, interned in the cursor's *current page*. Equals
    /// `style::DEFAULT_ID` (0) when the style is default.
    pub style_id: style::Id,

    /// The active OSC 8 hyperlink id in the cursor's page (0 = none).
    pub hyperlink_id: hyperlink::Id,

    /// Monotonic counter for hyperlinks without an explicit id (overflowing).
    pub hyperlink_implicit_id: OffsetInt,

    /// Heap copy of the active hyperlink so it can be re-inserted when the
    /// cursor page pin changes (the page may be cleared). Usually `None`.
    pub hyperlink: Option<Box<Hyperlink>>,

    /// Semantic content applied to newly written cells.
    pub semantic_content: SemanticContent,
    pub semantic_content_clear_eol: bool,

    /// The tracked pin locating the cursor. Raw pointer to a PageList-owned pin.
    pub page_pin: *mut Pin,
    /// Cached row pointer derived from `page_pin`.
    pub page_row: *mut Row,
    /// Cached cell pointer derived from `page_pin`.
    pub page_cell: *mut Cell,
}

impl Cursor {
    /// Construct a cursor at the given pin with fresh (default) state.
    pub(super) fn new(page_pin: *mut Pin, page_row: *mut Row, page_cell: *mut Cell) -> Cursor {
        Cursor {
            x: 0,
            y: 0,
            cursor_style: CursorStyle::Block,
            pending_wrap: false,
            protected: false,
            style: Style::default(),
            style_id: style::DEFAULT_ID,
            hyperlink_id: 0,
            hyperlink_implicit_id: 0,
            hyperlink: None,
            semantic_content: SemanticContent::Output,
            semantic_content_clear_eol: false,
            page_pin,
            page_row,
            page_cell,
        }
    }
}

impl Cursor {
    /// Snapshot the value-state of this cursor for copying onto another screen.
    /// Port of the read side of `cursorCopy`'s `other: Cursor` argument (the
    /// fields it actually reads for the `hyperlink = false` path).
    pub(crate) fn to_copy(&self) -> CursorCopy {
        CursorCopy {
            x: self.x,
            y: self.y,
            style: self.style,
            protected: self.protected,
            pending_wrap: self.pending_wrap,
            cursor_style: self.cursor_style,
            semantic_content: self.semantic_content,
            semantic_content_clear_eol: self.semantic_content_clear_eol,
        }
    }
}

/// The value-state of a cursor to copy onto another screen. Port of the subset
/// of `Screen.Cursor` that `cursorCopy` reads (excluding page pin / hyperlink /
/// style id, which are managed per-screen).
#[derive(Debug, Clone, Copy)]
pub struct CursorCopy {
    pub x: CellCountInt,
    pub y: CellCountInt,
    pub style: Style,
    pub protected: bool,
    pub pending_wrap: bool,
    pub cursor_style: CursorStyle,
    pub semantic_content: SemanticContent,
    pub semantic_content_clear_eol: bool,
}

/// Saved cursor state (DECSC). Port of `Screen.SavedCursor`.
#[derive(Debug, Clone)]
pub struct SavedCursor {
    pub x: CellCountInt,
    pub y: CellCountInt,
    pub style: Style,
    pub protected: bool,
    pub pending_wrap: bool,
    pub origin: bool,
    pub charset: CharsetState,
}
