//! Read-only, owned snapshots of terminal state for embedders / frontends.
//!
//! The internal grid is a page-based, pointer-heavy, refcount-interned
//! structure (see [`crate::pagelist`] and [`crate::page`]); none of it is safe
//! or ergonomic to hand to a renderer directly. This module walks the live
//! pages once and copies the *visible, styled* grid into plain `Vec`-backed
//! owned structs a UI layer can render without `unsafe` and without borrowing
//! the terminal.
//!
//! This is additive read-back API: it does not mutate terminal state and it
//! resolves each cell's interned style id into a concrete [`CellStyle`], so
//! callers get colors and attributes, not just text (which is all
//! [`crate::terminal::Terminal::plain_string`] / `dump_string` give).
//!
//! The scrollback model mirrors what a terminal UI wants: [`Snapshot`] holds
//! *every* row (scrollback history + the active area) as a flat list, plus the
//! index where the active area begins. A frontend keeps its own "scrollback
//! offset from the bottom" and slices a window of `rows` out of `all_rows`
//! without ever mutating the engine's viewport.

use crate::page::{ContentTag, Wide};
use crate::pagelist::Direction;
use crate::point::{Point, Tag};
use crate::screen::Screen;
use crate::screen::cursor::CursorStyle;
use crate::terminal::Terminal;

/// A source-tracked color for a snapshot cell. Mirrors the engine's
/// [`crate::page::style::Color`] but is a plain owned value with no lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapshotColor {
    /// No explicit color; the renderer should use its default fg/bg.
    #[default]
    Default,
    /// A palette index: `0..=15` are the ANSI/bright colors, `16..=255` the
    /// 256-color cube + grayscale ramp.
    Palette(u8),
    /// A direct 24-bit RGB color.
    Rgb { r: u8, g: u8, b: u8 },
}

/// The underline style of a snapshot cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapshotUnderline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// The visual style of a single snapshot cell (colors + attributes), resolved
/// from the engine's interned style set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellStyle {
    pub fg: SnapshotColor,
    pub bg: SnapshotColor,
    pub underline_color: SnapshotColor,
    pub bold: bool,
    pub faint: bool,
    pub italic: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: SnapshotUnderline,
}

/// Whether a cell is the lead or the trailing spacer of a wide glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CellWidth {
    /// A normal single-width cell.
    #[default]
    Narrow,
    /// The lead cell of a double-width glyph.
    Wide,
    /// The trailing spacer cell after a wide glyph (renders nothing).
    Spacer,
}

/// One rendered grid cell: its character(s), width, and resolved style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotCell {
    /// The primary character. `' '` for an empty cell. For a spacer this is
    /// `' '` and [`SnapshotCell::is_spacer`] is true.
    pub ch: char,
    /// Any combining/grapheme continuation codepoints after `ch` (usually
    /// empty).
    pub combining: Vec<char>,
    pub width: CellWidth,
    pub style: CellStyle,
}

impl SnapshotCell {
    fn blank() -> Self {
        Self {
            ch: ' ',
            combining: Vec::new(),
            width: CellWidth::Narrow,
            style: CellStyle::default(),
        }
    }

    /// True if this is the trailing spacer of a wide glyph (renderers skip it).
    pub fn is_spacer(&self) -> bool {
        matches!(self.width, CellWidth::Spacer)
    }

    /// True if this cell is the lead of a double-width glyph.
    pub fn is_wide(&self) -> bool {
        matches!(self.width, CellWidth::Wide)
    }
}

/// One row of snapshot cells (always exactly `cols` entries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRow {
    pub cells: Vec<SnapshotCell>,
}

/// The cursor position and appearance at snapshot time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotCursor {
    /// Column within the active area.
    pub col: usize,
    /// Row within the active area (0 = top of active).
    pub row: usize,
    pub style: CursorStyle,
    pub visible: bool,
}

/// A complete, owned, read-only view of the visible + scrollback grid.
///
/// `all_rows` is ordered oldest-first: scrollback history, then the active
/// area. `active_start` is the index of the first active-area row, so
/// `all_rows[active_start..]` is exactly the live grid and everything before it
/// is scrollback. `cursor.row`/`col` are relative to the active area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub cols: usize,
    pub rows: usize,
    pub all_rows: Vec<SnapshotRow>,
    pub active_start: usize,
    pub cursor: SnapshotCursor,
}

impl Snapshot {
    /// The number of scrollback (history) rows above the active area.
    pub fn scrollback_len(&self) -> usize {
        self.active_start
    }

    /// The visible window of `rows` rows, given a scrollback offset measured in
    /// rows *up from the bottom* (0 = the live active area). The returned slice
    /// is always exactly `self.rows` long as long as `all_rows` has at least
    /// that many rows (which it always does — the active area alone is `rows`).
    pub fn visible_window(&self, scrollback_offset: usize) -> &[SnapshotRow] {
        let total = self.all_rows.len();
        let bottom = total.saturating_sub(scrollback_offset.min(self.active_start));
        let top = bottom.saturating_sub(self.rows);
        &self.all_rows[top..bottom]
    }
}

fn to_color(c: crate::page::style::Color) -> SnapshotColor {
    match c {
        crate::page::style::Color::None => SnapshotColor::Default,
        crate::page::style::Color::Palette(i) => SnapshotColor::Palette(i),
        crate::page::style::Color::Rgb(rgb) => SnapshotColor::Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        },
    }
}

fn to_underline(u: crate::page::style::Underline) -> SnapshotUnderline {
    use crate::page::style::Underline as U;
    match u {
        U::None => SnapshotUnderline::None,
        U::Single => SnapshotUnderline::Single,
        U::Double => SnapshotUnderline::Double,
        U::Curly => SnapshotUnderline::Curly,
        U::Dotted => SnapshotUnderline::Dotted,
        U::Dashed => SnapshotUnderline::Dashed,
    }
}

fn to_cell_style(s: &crate::page::style::Style) -> CellStyle {
    CellStyle {
        fg: to_color(s.fg_color),
        bg: to_color(s.bg_color),
        underline_color: to_color(s.underline_color),
        bold: s.flags.bold,
        faint: s.flags.faint,
        italic: s.flags.italic,
        blink: s.flags.blink,
        inverse: s.flags.inverse,
        invisible: s.flags.invisible,
        strikethrough: s.flags.strikethrough,
        overline: s.flags.overline,
        underline: to_underline(s.flags.underline),
    }
}

impl Screen {
    /// Build an owned [`Snapshot`] of the whole screen (scrollback + active),
    /// resolving each cell's style. Does not mutate any terminal state.
    pub fn snapshot(&self) -> Snapshot {
        let cols = self.cols() as usize;
        let rows = self.rows() as usize;

        let mut all_rows: Vec<SnapshotRow> = Vec::new();

        // Walk every screen row (history + active), oldest first.
        let mut it = self.pages.row_iterator(
            Direction::RightDown,
            Point::new(Tag::Screen, Default::default()),
            None,
        );
        // SAFETY: the iterator yields valid pins into live pages for the
        // lifetime of `&self`; we never retain a pin past its use.
        unsafe {
            while let Some(pin) = it.next() {
                let (row, _) = pin.row_and_cell();
                let page = self.pages.node_data(pin.node);
                let base = page.get_cells(row);
                let base = base as *const crate::page::Cell;

                let mut cells = Vec::with_capacity(cols);
                for x in 0..cols {
                    let cell = *base.add(x);
                    cells.push(snapshot_cell(page, base.add(x), &cell));
                }
                all_rows.push(SnapshotRow { cells });
            }
        }

        // The active area is the last `rows` rows.
        let active_start = all_rows.len().saturating_sub(rows);

        let cursor = SnapshotCursor {
            col: self.cursor.x as usize,
            row: self.cursor.y as usize,
            style: self.cursor.cursor_style,
            visible: true,
        };

        Snapshot {
            cols,
            rows,
            all_rows,
            active_start,
            cursor,
        }
    }
}

/// Extract one owned cell from a live page cell.
///
/// # Safety
/// `cell_ptr` must point at `cell` inside `page`'s live memory.
unsafe fn snapshot_cell(
    page: &crate::page::Page,
    cell_ptr: *const crate::page::Cell,
    cell: &crate::page::Cell,
) -> SnapshotCell {
    let width = match cell.wide() {
        Wide::Narrow => CellWidth::Narrow,
        Wide::Wide => CellWidth::Wide,
        Wide::SpacerTail | Wide::SpacerHead => CellWidth::Spacer,
    };

    // Background-color-only cells carry no text but do carry a bg color in the
    // content bits (used by the erase/bg-fill paths). Surface that as a blank
    // cell with the bg set so the renderer paints it.
    let (ch, combining) = match cell.content_tag() {
        ContentTag::Codepoint | ContentTag::CodepointGrapheme => {
            let cp = cell.codepoint();
            let ch = if cp == 0 {
                ' '
            } else {
                char::from_u32(cp).unwrap_or(' ')
            };
            let mut combining = Vec::new();
            if cell.content_tag() == ContentTag::CodepointGrapheme {
                // SAFETY: cell_ptr addresses this cell in this page.
                if let Some(slice) = unsafe { page.lookup_grapheme(cell_ptr as *mut _) } {
                    // SAFETY: slice valid for the page lifetime.
                    for &g in unsafe { &*slice } {
                        if let Some(c) = char::from_u32(g) {
                            combining.push(c);
                        }
                    }
                }
            }
            (ch, combining)
        }
        ContentTag::BgColorPalette | ContentTag::BgColorRgb => (' ', Vec::new()),
    };

    let mut style = if cell.style_id() != crate::page::style::DEFAULT_ID {
        // SAFETY: a non-default style id on a live cell is valid in this page's
        // style set with ref count > 0.
        let s = unsafe { &*page.style_by_id(cell.style_id()) };
        to_cell_style(s)
    } else {
        CellStyle::default()
    };

    // Fold a bg-color-only cell's color into the style bg.
    match cell.content_tag() {
        ContentTag::BgColorPalette => style.bg = SnapshotColor::Palette(cell.color_palette()),
        ContentTag::BgColorRgb => {
            let (r, g, b) = cell.color_rgb();
            style.bg = SnapshotColor::Rgb { r, g, b };
        }
        _ => {}
    }

    SnapshotCell {
        ch,
        combining,
        width,
        style,
    }
}

impl Terminal {
    /// Build an owned [`Snapshot`] of the active screen. Convenience wrapper
    /// over [`Screen::snapshot`] that also reports the real cursor visibility
    /// (DEC mode 25) from terminal mode state.
    pub fn snapshot(&self) -> Snapshot {
        let mut snap = self.screen().snapshot();
        snap.cursor.visible = self.modes.get(crate::modes::Mode::CursorVisible);
        // A blank far-right column can drop out of Tag::Screen's bottom-right if
        // the whole active area is empty; guarantee the row count invariant.
        while snap.all_rows.len() < snap.rows {
            snap.all_rows.push(SnapshotRow {
                cells: vec![SnapshotCell::blank(); snap.cols],
            });
        }
        snap.active_start = snap.all_rows.len().saturating_sub(snap.rows);
        snap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{Stream, TerminalHandler};
    use crate::terminal::{Options, Terminal};

    fn feed(cols: u16, rows: u16, bytes: &[u8]) -> Terminal {
        let term = Terminal::new(Options {
            cols,
            rows,
            ..Default::default()
        });
        let mut stream = Stream::new(TerminalHandler::new(term));
        stream.feed(bytes);
        stream.handler.terminal
    }

    fn row_text(row: &SnapshotRow) -> String {
        let mut s: String = row
            .cells
            .iter()
            .filter(|c| !c.is_spacer())
            .map(|c| c.ch)
            .collect();
        while s.ends_with(' ') {
            s.pop();
        }
        s
    }

    #[test]
    fn plain_text_snapshot() {
        let term = feed(10, 3, b"Hello");
        let snap = term.snapshot();
        assert_eq!(snap.cols, 10);
        assert_eq!(snap.rows, 3);
        assert_eq!(row_text(&snap.all_rows[snap.active_start]), "Hello");
        assert_eq!(snap.cursor.col, 5);
        assert_eq!(snap.cursor.row, 0);
        assert!(snap.cursor.visible);
    }

    #[test]
    fn sgr_colors_are_resolved() {
        // Red fg (31), blue bg (44), bold (1) on 'A', reset then 'B'.
        let term = feed(10, 2, b"\x1b[1;31;44mA\x1b[0mB");
        let snap = term.snapshot();
        let row = &snap.all_rows[snap.active_start];
        assert_eq!(row.cells[0].ch, 'A');
        assert_eq!(row.cells[0].style.fg, SnapshotColor::Palette(1));
        assert_eq!(row.cells[0].style.bg, SnapshotColor::Palette(4));
        assert!(row.cells[0].style.bold);
        assert_eq!(row.cells[1].ch, 'B');
        assert_eq!(row.cells[1].style, CellStyle::default());
    }

    #[test]
    fn rgb_and_256_colors() {
        let term = feed(10, 2, b"\x1b[38;5;196;48;2;1;2;3mX");
        let snap = term.snapshot();
        let cell = &snap.all_rows[snap.active_start].cells[0];
        assert_eq!(cell.style.fg, SnapshotColor::Palette(196));
        assert_eq!(cell.style.bg, SnapshotColor::Rgb { r: 1, g: 2, b: 3 });
    }

    #[test]
    fn wide_char_lead_and_spacer() {
        let term = feed(10, 2, "好x".as_bytes());
        let snap = term.snapshot();
        let row = &snap.all_rows[snap.active_start];
        assert_eq!(row.cells[0].ch, '好');
        assert!(row.cells[0].is_wide());
        assert!(row.cells[1].is_spacer());
        assert_eq!(row.cells[2].ch, 'x');
    }

    #[test]
    fn cursor_hidden_mode() {
        let term = feed(10, 2, b"\x1b[?25l");
        let snap = term.snapshot();
        assert!(!snap.cursor.visible);
    }

    #[test]
    fn scrollback_rows_precede_active() {
        // 4 cols, 2 rows; write enough to push a row into scrollback.
        let term = feed(4, 2, b"aaaabbbbcccc");
        let snap = term.snapshot();
        assert!(snap.scrollback_len() >= 1);
        // The visible window (offset 0) is the active area.
        let window = snap.visible_window(0);
        assert_eq!(window.len(), 2);
        // Scrolling up by 1 reveals a history row at the top.
        let up = snap.visible_window(1);
        assert_eq!(up.len(), 2);
        assert_ne!(row_text(&up[0]), row_text(&window[0]));
    }
}
