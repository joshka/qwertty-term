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

use crate::color::{DEFAULT as DEFAULT_PALETTE, Palette, Rgb};
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

impl SnapshotRow {
    /// An all-blank row of `cols` cells (used to pad a window that's shorter
    /// than the requested row count, e.g. just after startup).
    fn blank(cols: usize) -> Self {
        Self {
            cells: vec![SnapshotCell::blank(); cols],
        }
    }
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

/// A cheap, windowed counterpart to [`Snapshot`]: only the rows needed to
/// render a `rows`-tall viewport are materialized, instead of the whole
/// scrollback history.
///
/// A frontend that snapshots once per rendered frame (rather than once per
/// history row written) should use [`Screen::snapshot_window`] /
/// [`Terminal::snapshot_window`] for that per-frame render path, so cost
/// stays proportional to the *visible* window, not to total scrollback
/// length. The full [`Snapshot`] / `snapshot()` API remains for tests,
/// embedding, and anything that needs random access into scrollback (e.g.
/// extracting selected text that may reach above the current window).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotWindow {
    pub cols: usize,
    /// Exactly `rows` rows (blank-padded if the terminal has fewer rows of
    /// history than requested, e.g. just after startup).
    pub window: Vec<SnapshotRow>,
    /// The absolute logical row index (as in a full [`Snapshot`]'s
    /// `all_rows`) of `window[0]`. Add a visible row index to this to
    /// recover the same absolute row numbering a full snapshot would use —
    /// needed to keep selection coordinates stable across frames regardless
    /// of which snapshot variant rendered them.
    pub window_top: usize,
    /// The number of scrollback (history) rows above the active area, same
    /// meaning as [`Snapshot::scrollback_len`].
    pub scrollback_len: usize,
    pub cursor: SnapshotCursor,
    pub palette: Palette,
    pub default_fg: Option<Rgb>,
    pub default_bg: Option<Rgb>,
}

/// A complete, owned, read-only view of the visible + scrollback grid.
///
/// `all_rows` is ordered oldest-first: scrollback history, then the active
/// area. `active_start` is the index of the first active-area row, so
/// `all_rows[active_start..]` is exactly the live grid and everything before it
/// is scrollback. `cursor.row`/`col` are relative to the active area.
///
/// Cells stay *symbolic*: a [`SnapshotCell`]'s style carries [`SnapshotColor`]
/// values (`Default`/`Palette(u8)`/`Rgb`), not resolved pixels. `palette` /
/// `default_fg` / `default_bg` are the terminal's *current* dynamic color
/// state (as of snapshot time — reflecting any OSC 4/10/11/104/110/111/112
/// mutations) that a renderer resolves `SnapshotColor::Palette`/`Default`
/// through. This keeps a copy of the 256-entry palette per snapshot (cheap:
/// `256 * 3` bytes, `Copy`), rather than resolving colors eagerly per cell, so
/// a renderer that wants to special-case `Default` (e.g. to skip painting a
/// bg-colored rect) still can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub cols: usize,
    pub rows: usize,
    pub all_rows: Vec<SnapshotRow>,
    pub active_start: usize,
    pub cursor: SnapshotCursor,
    /// The current 256-color palette (indices 0-255), including any OSC
    /// 4/104 (+ kitty OSC 21, once wired) modifications. Resolve
    /// [`SnapshotColor::Palette`] through this rather than a fixed table.
    pub palette: Palette,
    /// The dynamic default foreground (OSC 10/110), if the terminal has one
    /// set (either a config default or an OSC override). `None` means the
    /// renderer should fall back to its own default.
    pub default_fg: Option<Rgb>,
    /// The dynamic default background (OSC 11/111), if set. `None` means the
    /// renderer should fall back to its own default.
    pub default_bg: Option<Rgb>,
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

        // Walk every screen row (history + active), oldest first.
        let it = self.pages.row_iterator(
            Direction::RightDown,
            Point::new(Tag::Screen, Default::default()),
            None,
        );
        let all_rows = copy_rows(&self.pages, it, cols);

        // The active area is the last `rows` rows.
        let active_start = all_rows.len().saturating_sub(rows);

        Snapshot {
            cols,
            rows,
            all_rows,
            active_start,
            cursor: self.snapshot_cursor(),
            // A bare `Screen` doesn't own the terminal's dynamic color
            // state (see `Terminal::colors`); default to the built-in
            // palette / no dynamic override here. `Terminal::snapshot`
            // (below) overwrites these three fields with the live state.
            palette: DEFAULT_PALETTE,
            default_fg: None,
            default_bg: None,
        }
    }

    /// Build an owned [`SnapshotWindow`] containing only the `rows` rows
    /// needed to render a viewport `scrollback_offset` rows up from the
    /// bottom (0 = the live active area), instead of the whole screen.
    ///
    /// This is the cheap, additive counterpart to [`Screen::snapshot`] meant
    /// for a per-rendered-frame call site: finding the window's top row walks
    /// *backward* from the bottom of the page list (cost proportional to the
    /// window height, hopping pages as needed), never forward from the top of
    /// scrollback, so this stays cheap regardless of total history length.
    pub fn snapshot_window(&self, scrollback_offset: usize) -> SnapshotWindow {
        let cols = self.cols() as usize;
        let rows = self.rows() as usize;
        let total_rows = self.pages.total_rows();
        let scrollback_len = total_rows.saturating_sub(rows);
        let offset = scrollback_offset.min(scrollback_len);

        // The window is exactly `rows` rows ending `offset` rows up from the
        // bottom of the screen.
        //
        // SAFETY: `get_bottom_right` returns a pin into a live page (or
        // `None` for an empty list); `up` only walks `prev` pointers within
        // the same live page list.
        let bottom_right = self
            .pages
            .get_bottom_right(Tag::Screen)
            .and_then(|p| unsafe { p.up(offset) });

        let window = bottom_right.and_then(|bl_pin| {
            let window_len = rows.min(total_rows.saturating_sub(offset));
            let tl_pin = if window_len == 0 {
                None
            } else {
                // SAFETY: see above; `bl_pin` addresses a live page.
                unsafe { bl_pin.up(window_len - 1) }
            };
            tl_pin.map(|tl_pin| {
                // SAFETY: `tl_pin`/`bl_pin` were both derived above by
                // walking `prev` pointers from the live bottom-right pin, so
                // both address live pages; the iterator is only used for the
                // duration of this call.
                let it = unsafe { tl_pin.row_iterator(Direction::RightDown, Some(bl_pin)) };
                copy_rows(&self.pages, it, cols)
            })
        });
        let mut window = window.unwrap_or_default();

        // Pad at the top if the window is shorter than `rows` (e.g. just
        // after startup, before enough rows have been written) so callers
        // can always assume exactly `rows` entries.
        while window.len() < rows {
            window.insert(0, SnapshotRow::blank(cols));
        }

        SnapshotWindow {
            cols,
            window_top: total_rows.saturating_sub(offset + rows),
            window,
            scrollback_len,
            cursor: self.snapshot_cursor(),
            palette: DEFAULT_PALETTE,
            default_fg: None,
            default_bg: None,
        }
    }

    fn snapshot_cursor(&self) -> SnapshotCursor {
        SnapshotCursor {
            col: self.cursor.x as usize,
            row: self.cursor.y as usize,
            style: self.cursor.cursor_style,
            visible: true,
        }
    }
}

/// Copy every row an iterator yields into owned [`SnapshotRow`]s.
///
/// # Safety
/// `it` must only yield pins into `pages`'s live pages.
fn copy_rows(
    pages: &crate::pagelist::PageList,
    mut it: crate::pagelist::RowIterator,
    cols: usize,
) -> Vec<SnapshotRow> {
    let mut rows = Vec::new();
    // SAFETY: the iterator yields valid pins into live pages for the
    // lifetime of `pages`; we never retain a pin past its use.
    unsafe {
        while let Some(pin) = it.next() {
            let (row, _) = pin.row_and_cell();
            let page = pages.node_data(pin.node);
            let base = page.get_cells(row);
            let base = base as *const crate::page::Cell;

            let mut cells = Vec::with_capacity(cols);
            for x in 0..cols {
                let cell = *base.add(x);
                cells.push(snapshot_cell(page, base.add(x), &cell));
            }
            rows.push(SnapshotRow { cells });
        }
    }
    rows
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
    /// (DEC mode 25) from terminal mode state and the live dynamic color
    /// state (`self.colors`, mutated by OSC 4/5/10-19/104/105/110-119 via
    /// `TerminalHandler::color_operation`) rather than a fixed default.
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
        snap.palette = self.colors.palette.current;
        snap.default_fg = self.colors.foreground.get();
        snap.default_bg = self.colors.background.get();
        snap
    }

    /// Build an owned [`SnapshotWindow`] of just the rows needed to render a
    /// viewport `scrollback_offset` rows up from the bottom. Convenience
    /// wrapper over [`Screen::snapshot_window`] that also reports the real
    /// cursor visibility and live dynamic color state, matching
    /// [`Terminal::snapshot`]'s behavior over [`Screen::snapshot`].
    ///
    /// Prefer this over [`Terminal::snapshot`] on a per-rendered-frame call
    /// site (e.g. a UI redraw loop): cost is proportional to `rows`, not to
    /// total scrollback length.
    pub fn snapshot_window(&self, scrollback_offset: usize) -> SnapshotWindow {
        let mut snap = self.screen().snapshot_window(scrollback_offset);
        snap.cursor.visible = self.modes.get(crate::modes::Mode::CursorVisible);
        snap.palette = self.colors.palette.current;
        snap.default_fg = self.colors.foreground.get();
        snap.default_bg = self.colors.background.get();
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

    // DECSCUSR (`CSI Ps SP q`) should stick on the cursor snapshot, not just
    // flip the blink mode. Regression test for the shell-integration bar
    // cursor (`CSI 5 SP q`) rendering as a block.
    #[test]
    fn decscusr_sets_snapshot_cursor_style() {
        use crate::screen::cursor::CursorStyle;

        let term = feed(10, 2, b"\x1b[5 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Bar);

        let term = feed(10, 2, b"\x1b[6 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Bar);

        let term = feed(10, 2, b"\x1b[3 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Underline);

        let term = feed(10, 2, b"\x1b[4 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Underline);

        let term = feed(10, 2, b"\x1b[1 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Block);

        let term = feed(10, 2, b"\x1b[2 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Block);

        // CSI 0 SP q (default) resets to block.
        let term = feed(10, 2, b"\x1b[5 q\x1b[0 q");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Block);

        // Full reset (RIS) restores the default block style.
        let term = feed(10, 2, b"\x1b[5 q\x1bc");
        assert_eq!(term.snapshot().cursor.style, CursorStyle::Block);
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

    #[test]
    fn default_snapshot_uses_default_palette_and_no_dynamic_overrides() {
        let term = feed(10, 2, b"");
        let snap = term.snapshot();
        assert_eq!(snap.palette, crate::color::DEFAULT);
        assert_eq!(snap.default_fg, None);
        assert_eq!(snap.default_bg, None);
    }

    #[test]
    fn osc_4_set_color_is_reflected_in_snapshot_palette() {
        // OSC 4: set palette index 1 (normally red) to a custom color.
        let term = feed(10, 2, b"\x1b]4;1;#112233\x1b\\");
        let snap = term.snapshot();
        assert_eq!(snap.palette[1], crate::color::Rgb::new(0x11, 0x22, 0x33));
        // Untouched entries keep their default value.
        assert_eq!(snap.palette[2], crate::color::DEFAULT[2]);
    }

    #[test]
    fn osc_104_reset_restores_default_palette() {
        // Set index 1, then reset just that index (OSC 104;1) and confirm it
        // reverts; then set again and reset-all (bare OSC 104).
        let term = feed(10, 2, b"\x1b]4;1;#112233\x1b\\\x1b]104;1\x1b\\");
        let snap = term.snapshot();
        assert_eq!(snap.palette[1], crate::color::DEFAULT[1]);

        let term = feed(10, 2, b"\x1b]4;1;#112233\x1b]4;2;#445566\x1b]104\x1b\\");
        let snap = term.snapshot();
        assert_eq!(snap.palette[1], crate::color::DEFAULT[1]);
        assert_eq!(snap.palette[2], crate::color::DEFAULT[2]);
    }

    #[test]
    fn osc_10_11_set_default_fg_bg_and_osc_110_111_reset() {
        // OSC 10 = fg, OSC 11 = bg.
        let term = feed(10, 2, b"\x1b]10;#aabbcc\x1b\\\x1b]11;#001122\x1b\\");
        let snap = term.snapshot();
        assert_eq!(
            snap.default_fg,
            Some(crate::color::Rgb::new(0xaa, 0xbb, 0xcc))
        );
        assert_eq!(
            snap.default_bg,
            Some(crate::color::Rgb::new(0x00, 0x11, 0x22))
        );

        // OSC 110/111 reset fg/bg back to unset.
        let term = feed(
            10,
            2,
            b"\x1b]10;#aabbcc\x1b\\\x1b]11;#001122\x1b\\\x1b]110\x1b\\\x1b]111\x1b\\",
        );
        let snap = term.snapshot();
        assert_eq!(snap.default_fg, None);
        assert_eq!(snap.default_bg, None);
    }

    #[test]
    fn snapshot_window_matches_full_snapshot_active_area() {
        let term = feed(10, 3, b"Hello");
        let full = term.snapshot();
        let window = term.snapshot_window(0);

        assert_eq!(window.cols, full.cols);
        assert_eq!(window.window.len(), full.rows);
        assert_eq!(window.window, full.all_rows[full.active_start..].to_vec());
        assert_eq!(window.cursor, full.cursor);
        assert_eq!(window.scrollback_len, full.scrollback_len());
    }

    #[test]
    fn snapshot_window_matches_full_snapshot_when_scrolled_into_history() {
        // 4 cols, 2 rows; push several rows into scrollback.
        let term = feed(4, 2, b"aaaabbbbccccddddeeee");
        let full = term.snapshot();
        assert!(full.scrollback_len() >= 2);

        for offset in 0..=full.scrollback_len() {
            let window = term.snapshot_window(offset);
            let expected = full.visible_window(offset);
            assert_eq!(
                window.window, expected,
                "window mismatch at offset {offset}"
            );
        }
    }

    #[test]
    fn snapshot_window_top_is_absolute_logical_row() {
        let term = feed(4, 2, b"aaaabbbbccccddddeeee");
        let full = term.snapshot();
        let window = term.snapshot_window(0);

        // window[0] should be the same row as `all_rows[window_top]` in a
        // full snapshot, preserving absolute row numbering for selection.
        assert_eq!(window.window[0], full.all_rows[window.window_top]);
    }

    #[test]
    fn snapshot_window_always_has_exactly_rows_entries() {
        // A fresh terminal's active area always has exactly `rows` rows (the
        // page list is initialized with `rows` rows up front), so the window
        // is always fully populated, never padded.
        let term = feed(10, 5, b"hi");
        let window = term.snapshot_window(0);
        assert_eq!(window.window.len(), 5);
        assert_eq!(row_text(&window.window[0]), "hi");
    }

    #[test]
    fn snapshot_window_clamps_offset_beyond_scrollback() {
        let term = feed(4, 2, b"aaaabbbbcccc");
        let full = term.snapshot();
        let clamped = term.snapshot_window(full.scrollback_len() + 100);
        let unclamped = term.snapshot_window(full.scrollback_len());
        assert_eq!(clamped.window, unclamped.window);
    }
}
