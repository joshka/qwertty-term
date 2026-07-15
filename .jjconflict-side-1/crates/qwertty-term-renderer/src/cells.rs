//! The [`Contents`] cell store + the codepoint-classification helpers the cell
//! engine needs.
//!
//! Port of `src/renderer/cell.zig` (commit `2da015cd6`). `Contents` is the
//! CPU-side data structure the engine ([`crate::engine`]) fills each frame and
//! then uploads into the swap chain's GPU buffers: a flat background-color
//! array plus a per-row collection of foreground [`CellText`] instances
//! (glyphs, underlines, strikethroughs, overlines). Its shape is dictated by
//! upstream's row-wise dirty-clearing goal — each row's foreground list can be
//! cleared independently so a dirty-row rebuild doesn't touch untouched rows.
//!
//! # The cursor-at-`fg[0]` convention (load-bearing)
//!
//! `fg_rows` holds `rows + 2` lists, not `rows`: list `0` and list `rows + 1`
//! are reserved for the cursor glyph, and the real rows live at `fg_rows[y +
//! 1]`. Block cursors go in list `0` (drawn *first*, so text layers on top);
//! all other cursor styles go in list `rows + 1` (drawn *last*, on top of
//! text). This exactly mirrors upstream so that a single flattened
//! concatenation of all lists produces the correct GPU draw order without a
//! separate cursor pass.
//!
//! See `docs/analysis/renderer-r4.md` for the full survey.

use qwertty_term_vt::page::size::CellCountInt;

use crate::wire::{CellBg, CellText};

/// The kind of foreground cell being added, mirroring upstream's `Key` enum
/// (`cell.zig`). All foreground kinds share the [`CellText`] GPU type and the
/// same per-row list; the distinction exists only to document intent at the
/// call site (and, upstream, to select the GPU vertex type — `bg` is a
/// different type and never goes through [`Contents::add`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A glyph.
    Text,
    /// An underline decoration sprite.
    Underline,
    /// A strikethrough decoration sprite.
    Strikethrough,
    /// An overline decoration sprite.
    Overline,
}

/// The contents of all the cells in the terminal.
///
/// The goal of this data structure is to allow for efficient row-wise clearing
/// of data from the GPU buffers, to allow for row-wise dirty tracking to
/// eliminate the overhead of rebuilding the GPU buffers each frame.
///
/// Must be initialized by resizing before calling any operations (the
/// [`Default`] value is a zero-sized grid).
#[derive(Debug, Default)]
pub struct Contents {
    /// Grid dimensions (columns, rows).
    cols: usize,
    rows: usize,

    /// Flat array containing cell background colors for the terminal grid.
    ///
    /// Indexed as `bg_cells[row * cols + col]`. Prefer [`Contents::bg_cell`] /
    /// [`Contents::set_bg_cell`] over direct indexing to avoid integer-size
    /// bugs, matching upstream's `bgCell` accessor.
    bg_cells: Vec<CellBg>,

    /// The per-row foreground [`CellText`] lists. `fg_rows[y + 1]` holds row
    /// `y`; `fg_rows[0]` and `fg_rows[rows + 1]` are the block / non-block
    /// cursor lists (see the module docs). Always `rows + 2` lists once
    /// [`Contents::resize`]d.
    fg_rows: Vec<Vec<CellText>>,
}

impl Contents {
    /// Number of grid columns.
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Number of grid rows.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Resize the cell contents for the given grid size. This always
    /// invalidates the entire cell contents (port of `Contents.resize`).
    ///
    /// The `rows + 2` foreground lists: index 0 and `rows + 1` are the cursor
    /// lists (which must be first and last in the flattened buffer,
    /// respectively); the real rows live at `[1..=rows]`.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;

        let cell_count = cols * rows;
        self.bg_cells = vec![[0, 0, 0, 0]; cell_count];

        // rows + 2 lists: [cursor_first, row_0, .., row_{rows-1}, cursor_last].
        self.fg_rows = (0..rows + 2).map(|_| Vec::new()).collect();
    }

    /// Reset the cell contents to an empty state without resizing (port of
    /// `Contents.reset`). Zeroes every background cell and clears every
    /// foreground list (retaining capacity).
    pub fn reset(&mut self) {
        for bg in &mut self.bg_cells {
            *bg = [0, 0, 0, 0];
        }
        for list in &mut self.fg_rows {
            list.clear();
        }
    }

    /// Read a background cell (port of `Contents.bgCell`, read side).
    pub fn bg_cell(&self, row: usize, col: usize) -> CellBg {
        self.bg_cells[row * self.cols + col]
    }

    /// Write a background cell (port of `Contents.bgCell`, write side).
    pub fn set_bg_cell(&mut self, row: usize, col: usize, value: CellBg) {
        self.bg_cells[row * self.cols + col] = value;
    }

    /// The flat background-cell array, for GPU upload (`frame.cells_bg.sync`).
    pub fn bg_cells(&self) -> &[CellBg] {
        &self.bg_cells
    }

    /// The foreground lists in draw order (cursor-first, rows, cursor-last),
    /// for gathered GPU upload (`frame.cells.syncFromArrayLists`).
    pub fn fg_lists(&self) -> &[Vec<CellText>] {
        &self.fg_rows
    }

    /// Total number of foreground instances across all lists (the instance
    /// count `drawFrame` passes to the cell-text draw).
    pub fn fg_count(&self) -> usize {
        self.fg_rows.iter().map(Vec::len).sum()
    }

    /// Add a foreground cell to the appropriate row list (port of
    /// `Contents.add`). Adding the same cell twice duplicates it in the vertex
    /// buffer; the caller clears the row first with [`Contents::clear`].
    ///
    /// # Panics
    /// If `cell.grid_pos[1]` (the row) is `>= rows` (upstream `assert`).
    pub fn add(&mut self, _key: Key, cell: CellText) {
        let y = cell.grid_pos[1] as usize;
        assert!(
            y < self.rows,
            "fg cell row {y} out of range (rows={})",
            self.rows
        );
        // +1 because list 0 is the cursor list.
        self.fg_rows[y + 1].push(cell);
    }

    /// Clear all cell contents for a given row (port of `Contents.clear`):
    /// zero its background cells and clear its foreground list.
    ///
    /// # Panics
    /// If `y >= rows` (upstream `assert`).
    pub fn clear(&mut self, y: CellCountInt) {
        let y = y as usize;
        assert!(
            y < self.rows,
            "clear row {y} out of range (rows={})",
            self.rows
        );
        let start = y * self.cols;
        for bg in &mut self.bg_cells[start..start + self.cols] {
            *bg = [0, 0, 0, 0];
        }
        // +1 because list 0 is the cursor list.
        self.fg_rows[y + 1].clear();
    }

    /// Set (or clear, with `None`) the cursor glyph (port of
    /// `Contents.setCursor`). Block cursors are drawn first (list 0); every
    /// other style is drawn last (list `rows + 1`). Both cursor lists are
    /// always cleared first so a style change doesn't leave a stale glyph.
    pub fn set_cursor(&mut self, cell: Option<CellText>, style: Option<crate::cursor::Style>) {
        if self.rows == 0 {
            return;
        }
        self.fg_rows[0].clear();
        let last = self.rows + 1;
        self.fg_rows[last].clear();

        let (Some(cell), Some(style)) = (cell, style) else {
            return;
        };

        match style {
            // Block cursors should be drawn first (underneath text).
            crate::cursor::Style::Block => self.fg_rows[0].push(cell),
            // Other cursor styles should be drawn last (over text).
            crate::cursor::Style::BlockHollow
            | crate::cursor::Style::Bar
            | crate::cursor::Style::Underline
            | crate::cursor::Style::Lock => self.fg_rows[last].push(cell),
        }
    }

    /// The current cursor glyph, if present (port of `Contents.getCursorGlyph`).
    /// Checks both cursor lists.
    pub fn cursor_glyph(&self) -> Option<CellText> {
        if self.rows == 0 {
            return None;
        }
        if let Some(&c) = self.fg_rows[0].first() {
            return Some(c);
        }
        if let Some(&c) = self.fg_rows[self.rows + 1].first() {
            return Some(c);
        }
        None
    }
}

/// Returns true if a codepoint for a cell is a *covering* character — one that
/// covers the entire cell. Used to make padding-color=extend work better
/// (upstream #2099). Port of `cell.zig isCovering`.
pub fn is_covering(cp: u32) -> bool {
    // U+2588 FULL BLOCK.
    cp == 0x2588
}

/// Returns true if the codepoint is a "symbol-like" character. Upstream defines
/// this via a generated `symbols_table`; the reduced port covers the private
/// use areas and the specific symbol blocks upstream's table enumerates (see
/// `cell.zig isSymbol`). Symbol-like glyphs are allowed to extend to 2 cells
/// wide when there's room (see [`constraint_width`]).
pub fn is_symbol(cp: u32) -> bool {
    matches!(cp,
        // Private use areas.
        0xE000..=0xF8FF
        | 0xF_0000..=0xF_FFFD
        | 0x10_0000..=0x10_FFFD
        // Arrows.
        | 0x2190..=0x21FF
        // Dingbats.
        | 0x2700..=0x27BF
        // Emoticons.
        | 0x1F600..=0x1F64F
        // Miscellaneous Symbols.
        | 0x2600..=0x26FF
        // Enclosed Alphanumerics.
        | 0x2460..=0x24FF
        // Enclosed Alphanumeric Supplement.
        | 0x1F100..=0x1F1FF
        // Miscellaneous Symbols and Pictographs.
        | 0x1F300..=0x1F5FF
        // Transport and Map Symbols.
        | 0x1F680..=0x1F6FF
    )
}

/// Whether min-contrast should be disabled for a glyph. True for graphics
/// elements (blocks, box drawing, legacy computing, Powerline) — forcing WCAG
/// contrast on them would distort the deliberate seam colors. Port of
/// `cell.zig noMinContrast`.
pub fn no_min_contrast(cp: u32) -> bool {
    is_graphics_element(cp)
}

/// Returns the appropriate `constraint_width` for a cell when rendering its
/// glyph(s). Port of `cell.zig constraintWidth`.
///
/// `grid_width` is the cell's own reported width (1 or 2). `cp` is the cell's
/// codepoint; `prev_cp`/`next_cp` are the neighbours' codepoints (0 for "none",
/// i.e. off-grid or blank). Symbol-like glyphs are allowed to spill into a
/// second (whitespace) cell when the previous glyph isn't itself a
/// (non-graphics) symbol — so multiple PUA icons stay aligned.
pub fn constraint_width(
    grid_width: u8,
    cp: u32,
    prev_cp: u32,
    next_cp: u32,
    at_last_col: bool,
) -> u8 {
    // Grid width 2 is always constrained to 2.
    if grid_width > 1 {
        return grid_width;
    }
    // Non-symbols use their grid width unchanged.
    if !is_symbol(cp) {
        return grid_width;
    }
    // At the end of the screen it must be constrained to one cell.
    if at_last_col {
        return 1;
    }
    // If the previous cell was a (non-graphics) symbol, constrain to 1 so
    // multiple PUA glyphs align. Graphics elements (block/powerline) don't
    // trigger this.
    if is_symbol(prev_cp) && !is_graphics_element(prev_cp) {
        return 1;
    }
    // If the next cell is whitespace, allow up to two cells wide.
    if next_cp == 0 || is_space(next_cp) {
        return 2;
    }
    1
}

/// General spaces treated as whitespace for constraint purposes. Note U+00A0
/// (no-break space) is intentionally excluded, to force fixed width. Port of
/// `cell.zig isSpace`.
fn is_space(cp: u32) -> bool {
    matches!(cp, 0x0020 | 0x2002)
}

/// True if the codepoint is a terminal graphics element: box drawing, block
/// elements, legacy computing, or Powerline. Port of `cell.zig
/// isGraphicsElement`.
fn is_graphics_element(cp: u32) -> bool {
    is_box_drawing(cp) || is_block_element(cp) || is_legacy_computing(cp) || is_powerline(cp)
}

fn is_box_drawing(cp: u32) -> bool {
    matches!(cp, 0x2500..=0x257F)
}

fn is_block_element(cp: u32) -> bool {
    matches!(cp, 0x2580..=0x259F)
}

fn is_legacy_computing(cp: u32) -> bool {
    matches!(cp, 0x1FB00..=0x1FBFF | 0x1CC00..=0x1CEBF)
}

fn is_powerline(cp: u32) -> bool {
    matches!(cp, 0xE0B0..=0xE0D7)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor::Style;
    use crate::wire::Atlas;

    fn fg_cell(x: u16, y: u16) -> CellText {
        CellText::new([x, y], [0, 0, 0, 1], Atlas::Grayscale)
    }

    /// Port of upstream's `test Contents`: start empty, add bg+fg, clear, then
    /// cursor block/hollow behavior.
    #[test]
    fn contents_lifecycle() {
        let (rows, cols) = (10usize, 10usize);
        let mut c = Contents::default();
        c.resize(cols, rows);

        // Start off empty after resizing.
        for y in 0..rows {
            assert_eq!(c.fg_lists()[y + 1].len(), 0);
            for x in 0..cols {
                assert_eq!(c.bg_cell(y, x), [0, 0, 0, 0]);
            }
        }
        // The cursor list starts empty.
        assert_eq!(c.fg_lists()[0].len(), 0);

        // Add some contents.
        let bg_cell: CellBg = [0, 0, 0, 1];
        let cell = fg_cell(4, 1);
        c.set_bg_cell(1, 4, bg_cell);
        c.add(Key::Text, cell);
        assert_eq!(c.bg_cell(1, 4), bg_cell);
        // The fg row index is offset by 1 because of the cursor list.
        assert_eq!(c.fg_lists()[2][0], cell);

        // And we should be able to clear it.
        c.clear(1);
        for y in 0..rows {
            assert_eq!(c.fg_lists()[y + 1].len(), 0);
            for x in 0..cols {
                assert_eq!(c.bg_cell(y, x), [0, 0, 0, 0]);
            }
        }

        // Add a block cursor.
        let mut cursor_cell = fg_cell(2, 3);
        cursor_cell.bools.0 |= crate::wire::CellTextBools::IS_CURSOR_GLYPH;
        c.set_cursor(Some(cursor_cell), Some(Style::Block));
        assert_eq!(c.fg_lists()[0][0], cursor_cell);
        assert_eq!(c.cursor_glyph(), Some(cursor_cell));

        // Remove it.
        c.set_cursor(None, None);
        assert_eq!(c.fg_lists()[0].len(), 0);
        assert_eq!(c.cursor_glyph(), None);

        // Add a hollow cursor (drawn last).
        c.set_cursor(Some(cursor_cell), Some(Style::BlockHollow));
        assert_eq!(c.fg_lists()[rows + 1][0], cursor_cell);
        assert_eq!(c.cursor_glyph(), Some(cursor_cell));
    }

    /// Port of upstream's "Contents clear retains other content".
    #[test]
    fn clear_retains_other_content() {
        let (rows, cols) = (10usize, 10usize);
        let mut c = Contents::default();
        c.resize(cols, rows);

        let bg1: CellBg = [0, 0, 0, 1];
        let fg1 = fg_cell(4, 1);
        c.set_bg_cell(1, 4, bg1);
        c.add(Key::Text, fg1);

        let bg2: CellBg = [0, 0, 0, 1];
        let fg2 = fg_cell(4, 2);
        c.set_bg_cell(2, 4, bg2);
        c.add(Key::Text, fg2);

        // Clear row 1; row 2 untouched.
        c.clear(1);
        assert_eq!(c.bg_cell(2, 4), bg2);
        assert_eq!(c.fg_lists()[3][0], fg2);
    }

    /// Port of upstream's "Contents clear last added content".
    #[test]
    fn clear_last_added_content() {
        let (rows, cols) = (10usize, 10usize);
        let mut c = Contents::default();
        c.resize(cols, rows);

        let bg1: CellBg = [0, 0, 0, 1];
        let fg1 = fg_cell(4, 1);
        c.set_bg_cell(1, 4, bg1);
        c.add(Key::Text, fg1);

        let bg2: CellBg = [0, 0, 0, 1];
        let fg2 = fg_cell(4, 2);
        c.set_bg_cell(2, 4, bg2);
        c.add(Key::Text, fg2);

        // Clear row 2; row 1 untouched.
        c.clear(2);
        assert_eq!(c.bg_cell(1, 4), bg1);
        assert_eq!(c.fg_lists()[2][0], fg1);
    }

    /// Port of upstream's "Contents with zero-sized screen": cursor ops on a
    /// zero-sized grid are safe no-ops.
    #[test]
    fn zero_sized_screen() {
        let mut c = Contents::default();
        c.set_cursor(None, None);
        assert_eq!(c.cursor_glyph(), None);
    }

    /// Constraint-width classification (port of the load-bearing arms of the
    /// upstream "Cell constraint widths" test; the terminal-driven cases are
    /// reproduced as direct classifier calls since the classifier is now a pure
    /// function of the neighbour codepoints).
    #[test]
    fn constraint_widths() {
        // A PUA symbol glyph used as the subject.
        let sym = 0xE000u32;
        // symbol -> nothing (next is blank/0): 2.
        assert_eq!(constraint_width(1, sym, 0, 0, false), 2);
        // symbol -> character: 1.
        assert_eq!(constraint_width(1, sym, 0, 'z' as u32, false), 1);
        // symbol -> space: 2.
        assert_eq!(constraint_width(1, sym, 0, ' ' as u32, false), 2);
        // symbol -> no-break space: 1 (NBSP is not a "space" here).
        assert_eq!(constraint_width(1, sym, 0, 0x00A0, false), 1);
        // symbol -> end of row: 1.
        assert_eq!(constraint_width(1, sym, 0, 0, true), 1);
        // symbol -> symbol: the second is constrained to 1 because prev is a
        // (non-graphics) symbol.
        assert_eq!(constraint_width(1, sym, sym, 0, false), 1);
        // A non-symbol keeps its grid width.
        assert_eq!(constraint_width(1, 'z' as u32, 0, sym, false), 1);
        // Grid width 2 always returns 2.
        assert_eq!(constraint_width(2, '好' as u32, 0, 0, false), 2);
        // powerline -> symbol: powerline is a graphics element, so it does NOT
        // constrain the following symbol (prev-symbol guard excludes graphics).
        assert_eq!(constraint_width(1, sym, 0xE0B0, 0, false), 2);
    }

    #[test]
    fn covering_and_min_contrast_classification() {
        assert!(is_covering(0x2588)); // full block
        assert!(!is_covering('A' as u32));
        // Graphics elements disable min-contrast.
        assert!(no_min_contrast(0x2500)); // box drawing
        assert!(no_min_contrast(0x2588)); // block
        assert!(no_min_contrast(0xE0B0)); // powerline
        assert!(!no_min_contrast('A' as u32));
    }
}
