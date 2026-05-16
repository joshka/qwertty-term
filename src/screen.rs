use crate::{cell::Cell, style::Style};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Cursor {
    pub col: usize,
    pub row: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenKind {
    Primary,
    Alternate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Region {
    pub(crate) top: usize,
    pub(crate) bottom: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Screen {
    pub(crate) grid: Vec<Cell>,
    pub(crate) cursor: Cursor,
    pub(crate) saved_cursor: Cursor,
    pub(crate) pending_wrap: bool,
    pub(crate) scrollback: Vec<Vec<Cell>>,
}

impl Screen {
    pub(crate) fn new(cols: usize, rows: usize, style: Style) -> Self {
        Self {
            grid: vec![Cell::blank(style); cols * rows],
            cursor: Cursor::default(),
            saved_cursor: Cursor::default(),
            pending_wrap: false,
            scrollback: Vec::new(),
        }
    }

    pub(crate) fn reset(&mut self, cols: usize, rows: usize, style: Style) {
        self.grid = vec![Cell::blank(style); cols * rows];
        self.cursor = Cursor::default();
        self.saved_cursor = Cursor::default();
        self.pending_wrap = false;
        self.scrollback.clear();
    }

    pub(crate) fn resize(
        &mut self,
        old_cols: usize,
        old_rows: usize,
        new_cols: usize,
        new_rows: usize,
        style: Style,
    ) {
        let mut resized = vec![Cell::blank(style); new_cols * new_rows];
        let copy_cols = old_cols.min(new_cols);
        let copy_rows = old_rows.min(new_rows);

        for row in 0..copy_rows {
            let old_start = Self::index(old_cols, 0, row);
            let new_start = Self::index(new_cols, 0, row);
            resized[new_start..new_start + copy_cols]
                .copy_from_slice(&self.grid[old_start..old_start + copy_cols]);
        }

        self.grid = resized;
        self.cursor.col = self.cursor.col.min(new_cols - 1);
        self.cursor.row = self.cursor.row.min(new_rows - 1);
        self.saved_cursor.col = self.saved_cursor.col.min(new_cols - 1);
        self.saved_cursor.row = self.saved_cursor.row.min(new_rows - 1);
        self.pending_wrap = false;
    }

    pub(crate) fn index(cols: usize, col: usize, row: usize) -> usize {
        row * cols + col
    }
}

pub(crate) fn plain_text_for_screen(screen: &Screen, cols: usize, rows: usize) -> String {
    let mut last_non_blank_row = None;
    for row in (0..rows).rev() {
        if row_end(screen, cols, row) > 0 {
            last_non_blank_row = Some(row);
            break;
        }
    }

    let Some(last_row) = last_non_blank_row else {
        return String::new();
    };

    let mut out = String::new();
    for row in 0..=last_row {
        if row > 0 {
            out.push('\n');
        }
        let end = row_end(screen, cols, row);
        let start = Screen::index(cols, 0, row);
        push_trimmed_row(&mut out, &screen.grid[start..start + end]);
    }
    out
}

pub(crate) fn push_trimmed_row(out: &mut String, row: &[Cell]) {
    let end = row
        .iter()
        .rposition(|cell| !cell.is_blank())
        .map_or(0, |idx| idx + 1);
    for cell in &row[..end] {
        if !cell.wide_continuation {
            out.push(cell.ch);
        }
    }
}

fn row_end(screen: &Screen, cols: usize, row: usize) -> usize {
    let start = Screen::index(cols, 0, row);
    screen.grid[start..start + cols]
        .iter()
        .rposition(|cell| !cell.is_blank())
        .map_or(0, |col| col + 1)
}

pub(crate) fn default_tab_stops(cols: usize) -> Vec<bool> {
    let mut stops = vec![false; cols];
    for col in (8..cols).step_by(8) {
        stops[col] = true;
    }
    stops
}
