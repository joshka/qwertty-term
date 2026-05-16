use unicode_width::UnicodeWidthChar;

use crate::{
    cell::Cell,
    parser::{one_based_to_zero, param_or},
    screen::{Cursor, Region, Screen, ScreenKind},
    terminal::Terminal,
};

impl Terminal {
    pub(super) fn advance_utf8(&mut self, byte: u8) {
        self.utf8.push(byte);

        match str::from_utf8(&self.utf8) {
            Ok(s) => {
                if let Some(ch) = s.chars().next() {
                    self.print_char(ch);
                }
                self.utf8.clear();
            }
            Err(err) if err.error_len().is_some() => {
                self.print_char(char::REPLACEMENT_CHARACTER);
                self.utf8.clear();
            }
            Err(_) if self.utf8.len() >= 4 => {
                self.print_char(char::REPLACEMENT_CHARACTER);
                self.utf8.clear();
            }
            Err(_) => {}
        }
    }

    pub(super) fn print_char(&mut self, ch: char) {
        let width = UnicodeWidthChar::width(ch).unwrap_or(1).clamp(1, 2);

        if self.screen().pending_wrap {
            self.screen_mut().cursor.col = 0;
            self.linefeed();
            self.screen_mut().pending_wrap = false;
        }

        if width == 2 && self.cursor().col + 1 >= self.cols {
            self.screen_mut().cursor.col = 0;
            self.linefeed();
        }

        let cursor = self.cursor();
        let idx = Screen::index(self.cols, cursor.col, cursor.row);
        let style = self.current_style;
        self.clear_wide_fragments_around(cursor.col, cursor.row);
        self.screen_mut().grid[idx] = Cell::printable(ch, style);
        if width == 2 && cursor.col + 1 < self.cols {
            let continuation = Screen::index(self.cols, cursor.col + 1, cursor.row);
            self.screen_mut().grid[continuation] = Cell::wide_continuation(style);
        }

        if cursor.col + width >= self.cols {
            if self.modes.wraparound {
                self.screen_mut().pending_wrap = true;
            }
        } else {
            self.screen_mut().cursor.col += width;
        }
        self.last_printed = Some(ch);
    }

    pub(super) fn backspace(&mut self) {
        let screen = self.screen_mut();
        screen.pending_wrap = false;
        screen.cursor.col = screen.cursor.col.saturating_sub(1);
    }

    pub(super) fn horizontal_tab(&mut self) {
        self.horizontal_tab_n(1);
    }

    pub(super) fn horizontal_tab_n(&mut self, count: usize) {
        let cols = self.cols;
        for _ in 0..count {
            let current = self.cursor().col;
            let next = self
                .tab_stops
                .iter()
                .enumerate()
                .skip(current + 1)
                .find_map(|(col, stop)| stop.then_some(col))
                .unwrap_or(cols - 1);
            let screen = self.screen_mut();
            screen.pending_wrap = false;
            screen.cursor.col = next.min(cols - 1);
        }
    }

    pub(super) fn horizontal_tab_back_n(&mut self, count: usize) {
        for _ in 0..count {
            let current = self.cursor().col;
            let previous = self
                .tab_stops
                .iter()
                .take(current)
                .enumerate()
                .rev()
                .find_map(|(col, stop)| stop.then_some(col))
                .unwrap_or(0);
            let screen = self.screen_mut();
            screen.pending_wrap = false;
            screen.cursor.col = previous;
        }
    }

    pub(super) fn carriage_return(&mut self) {
        let screen = self.screen_mut();
        screen.pending_wrap = false;
        screen.cursor.col = 0;
    }

    pub(super) fn linefeed(&mut self) {
        self.screen_mut().pending_wrap = false;
        let row = self.cursor().row;
        if row == self.scroll_region.bottom {
            self.scroll_up_region(self.scroll_region.top, self.scroll_region.bottom);
        } else if row + 1 < self.rows {
            self.screen_mut().cursor.row += 1;
        }
    }

    pub(super) fn reverse_index(&mut self) {
        self.screen_mut().pending_wrap = false;
        let row = self.cursor().row;
        if row == self.scroll_region.top {
            self.scroll_down_region(self.scroll_region.top, self.scroll_region.bottom);
        } else {
            self.screen_mut().cursor.row = row.saturating_sub(1);
        }
    }

    pub(super) fn cursor_up(&mut self, n: usize) {
        let screen = self.screen_mut();
        screen.pending_wrap = false;
        screen.cursor.row = screen.cursor.row.saturating_sub(n);
    }

    pub(super) fn cursor_down(&mut self, n: usize) {
        let rows = self.rows;
        let screen = self.screen_mut();
        screen.pending_wrap = false;
        screen.cursor.row = (screen.cursor.row + n).min(rows - 1);
    }

    pub(super) fn cursor_right(&mut self, n: usize) {
        let cols = self.cols;
        let screen = self.screen_mut();
        screen.pending_wrap = false;
        screen.cursor.col = (screen.cursor.col + n).min(cols - 1);
    }

    pub(super) fn cursor_left(&mut self, n: usize) {
        let screen = self.screen_mut();
        screen.pending_wrap = false;
        screen.cursor.col = screen.cursor.col.saturating_sub(n);
    }

    pub(super) fn cursor_next_line(&mut self, n: usize) {
        self.cursor_down(n);
        self.carriage_return();
    }

    pub(super) fn cursor_previous_line(&mut self, n: usize) {
        self.cursor_up(n);
        self.carriage_return();
    }

    pub(super) fn set_cursor(&mut self, col: usize, row: usize) {
        let cols = self.cols;
        let rows = self.rows;
        let screen = self.screen_mut();
        screen.pending_wrap = false;
        screen.cursor.col = col.min(cols - 1);
        screen.cursor.row = row.min(rows - 1);
    }

    pub(super) fn save_cursor(&mut self) {
        let screen = self.screen_mut();
        screen.pending_wrap = false;
        screen.saved_cursor = screen.cursor;
    }

    pub(super) fn restore_cursor(&mut self) {
        let cols = self.cols;
        let rows = self.rows;
        let screen = self.screen_mut();
        screen.pending_wrap = false;
        screen.cursor.col = screen.saved_cursor.col.min(cols - 1);
        screen.cursor.row = screen.saved_cursor.row.min(rows - 1);
    }

    pub(super) fn erase_display(&mut self, mode: usize) {
        self.screen_mut().pending_wrap = false;
        match mode {
            0 => {
                let cursor = self.cursor();
                self.clear_range(cursor.col, cursor.row, self.cols - 1, cursor.row);
                for row in cursor.row + 1..self.rows {
                    self.clear_range(0, row, self.cols - 1, row);
                }
            }
            1 => {
                let cursor = self.cursor();
                for row in 0..cursor.row {
                    self.clear_range(0, row, self.cols - 1, row);
                }
                self.clear_range(0, cursor.row, cursor.col, cursor.row);
            }
            2 => {
                for row in 0..self.rows {
                    self.clear_range(0, row, self.cols - 1, row);
                }
            }
            3 => {
                if self.active == ScreenKind::Primary {
                    self.primary.scrollback.clear();
                }
            }
            _ => {}
        }
    }

    pub(super) fn erase_line(&mut self, mode: usize) {
        self.screen_mut().pending_wrap = false;
        let cursor = self.cursor();
        match mode {
            0 => self.clear_range(cursor.col, cursor.row, self.cols - 1, cursor.row),
            1 => self.clear_range(0, cursor.row, cursor.col, cursor.row),
            2 => self.clear_range(0, cursor.row, self.cols - 1, cursor.row),
            _ => {}
        }
    }

    pub(super) fn insert_blank_chars(&mut self, count: usize) {
        let cursor = self.cursor();
        let count = count.min(self.cols - cursor.col);
        if count == 0 {
            return;
        }

        let cols = self.cols;
        let style = self.current_style;
        let screen = self.screen_mut();
        screen.pending_wrap = false;
        let row_start = Screen::index(cols, 0, cursor.row);
        for col in (cursor.col + count..cols).rev() {
            screen.grid[row_start + col] = screen.grid[row_start + col - count];
        }
        for col in cursor.col..cursor.col + count {
            screen.grid[row_start + col] = Cell::blank(style);
        }
    }

    pub(super) fn delete_chars(&mut self, count: usize) {
        let cursor = self.cursor();
        let count = count.min(self.cols - cursor.col);
        if count == 0 {
            return;
        }

        let cols = self.cols;
        let style = self.current_style;
        let screen = self.screen_mut();
        screen.pending_wrap = false;
        let row_start = Screen::index(cols, 0, cursor.row);
        for col in cursor.col..cols - count {
            screen.grid[row_start + col] = screen.grid[row_start + col + count];
        }
        for col in cols - count..cols {
            screen.grid[row_start + col] = Cell::blank(style);
        }
    }

    pub(super) fn erase_chars(&mut self, count: usize) {
        let cursor = self.cursor();
        let end = (cursor.col + count).min(self.cols);
        let cols = self.cols;
        let style = self.current_style;
        let screen = self.screen_mut();
        screen.pending_wrap = false;
        let row_start = Screen::index(cols, 0, cursor.row);
        for col in cursor.col..end {
            screen.grid[row_start + col] = Cell::blank(style);
        }
    }

    pub(super) fn insert_lines(&mut self, count: usize) {
        let cursor = self.cursor();
        if cursor.row < self.scroll_region.top || cursor.row > self.scroll_region.bottom {
            return;
        }

        let count = count.min(self.scroll_region.bottom - cursor.row + 1);
        for row in (cursor.row + count..=self.scroll_region.bottom).rev() {
            self.copy_row(row - count, row);
        }
        for row in cursor.row..cursor.row + count {
            self.clear_range(0, row, self.cols - 1, row);
        }
    }

    pub(super) fn delete_lines(&mut self, count: usize) {
        let cursor = self.cursor();
        if cursor.row < self.scroll_region.top || cursor.row > self.scroll_region.bottom {
            return;
        }

        let count = count.min(self.scroll_region.bottom - cursor.row + 1);
        for row in cursor.row..=self.scroll_region.bottom - count {
            self.copy_row(row + count, row);
        }
        for row in self.scroll_region.bottom + 1 - count..=self.scroll_region.bottom {
            self.clear_range(0, row, self.cols - 1, row);
        }
    }

    pub(super) fn repeat_preceding_char(&mut self, count: usize) {
        let Some(ch) = self.last_printed else {
            return;
        };
        for _ in 0..count {
            self.print_char(ch);
        }
    }

    pub(super) fn scroll_up_n(&mut self, count: usize) {
        for _ in 0..count {
            self.scroll_up_region(self.scroll_region.top, self.scroll_region.bottom);
        }
    }

    pub(super) fn scroll_down_n(&mut self, count: usize) {
        for _ in 0..count {
            self.scroll_down_region(self.scroll_region.top, self.scroll_region.bottom);
        }
    }

    pub(super) fn set_scroll_region(&mut self, params: &[Option<usize>]) {
        let top = one_based_to_zero(param_or(params, 0, 1));
        let bottom = one_based_to_zero(param_or(params, 1, self.rows));
        if top < bottom && bottom < self.rows {
            self.scroll_region = Region { top, bottom };
            self.set_cursor(0, 0);
        }
    }

    pub(super) fn select_graphic_rendition(&mut self, params: &[Option<usize>]) {
        self.current_style.apply_sgr(params);
    }

    pub(super) fn set_tab_stop(&mut self) {
        let col = self.cursor().col;
        if col < self.tab_stops.len() {
            self.tab_stops[col] = true;
        }
    }

    pub(super) fn clear_tabs(&mut self, params: &[Option<usize>]) {
        match param_or(params, 0, 0) {
            0 => {
                let col = self.cursor().col;
                if col < self.tab_stops.len() {
                    self.tab_stops[col] = false;
                }
            }
            3 | 5 => self.tab_stops.fill(false),
            _ => {}
        }
    }

    pub(super) fn switch_alternate_screen(&mut self, enabled: bool) {
        if enabled {
            self.primary.saved_cursor = self.primary.cursor;
            self.alternate
                .reset(self.cols, self.rows, self.current_style);
            self.active = ScreenKind::Alternate;
        } else {
            self.active = ScreenKind::Primary;
            self.primary.cursor = self.primary.saved_cursor;
        }
        self.screen_mut().pending_wrap = false;
        self.scroll_region = Region {
            top: 0,
            bottom: self.rows - 1,
        };
    }

    pub(super) fn screen_alignment_test(&mut self) {
        let cols = self.cols;
        let rows = self.rows;
        let style = self.current_style;
        let screen = self.screen_mut();
        for row in 0..rows {
            for col in 0..cols {
                screen.grid[Screen::index(cols, col, row)] = Cell::printable('E', style);
            }
        }
        screen.cursor = Cursor::default();
        screen.pending_wrap = false;
    }

    fn clear_wide_fragments_around(&mut self, col: usize, row: usize) {
        let style = self.current_style;
        let idx = Screen::index(self.cols, col, row);
        if self.screen().grid[idx].wide_continuation && col > 0 {
            let previous = Screen::index(self.cols, col - 1, row);
            self.screen_mut().grid[previous] = Cell::blank(style);
        }
        if col + 1 < self.cols {
            let next = Screen::index(self.cols, col + 1, row);
            if self.screen().grid[next].wide_continuation {
                self.screen_mut().grid[next] = Cell::blank(style);
            }
        }
    }

    fn clear_range(&mut self, start_col: usize, row: usize, end_col: usize, end_row: usize) {
        let cols = self.cols;
        let style = self.current_style;
        let screen = self.screen_mut();
        for y in row..=end_row {
            let from = if y == row { start_col } else { 0 };
            let to = if y == end_row { end_col } else { cols - 1 };
            for x in from..=to {
                let idx = Screen::index(cols, x, y);
                screen.grid[idx] = Cell::blank(style);
            }
        }
    }

    fn scroll_up_region(&mut self, top: usize, bottom: usize) {
        if top == 0 && bottom == self.rows - 1 && self.active == ScreenKind::Primary {
            let removed = self.primary.grid[..self.cols].to_vec();
            self.primary.scrollback.push(removed);
            if self.primary.scrollback.len() > self.max_scrollback {
                self.primary.scrollback.remove(0);
            }
        }

        let cols = self.cols;
        let style = self.current_style;
        let screen = self.screen_mut();
        for row in top..bottom {
            let dst = Screen::index(cols, 0, row);
            let src = Screen::index(cols, 0, row + 1);
            screen.grid.copy_within(src..src + cols, dst);
        }
        let start = Screen::index(cols, 0, bottom);
        for idx in start..start + cols {
            screen.grid[idx] = Cell::blank(style);
        }
    }

    fn scroll_down_region(&mut self, top: usize, bottom: usize) {
        let cols = self.cols;
        let style = self.current_style;
        let screen = self.screen_mut();
        for row in (top + 1..=bottom).rev() {
            let dst = Screen::index(cols, 0, row);
            let src = Screen::index(cols, 0, row - 1);
            screen.grid.copy_within(src..src + cols, dst);
        }
        let start = Screen::index(cols, 0, top);
        for idx in start..start + cols {
            screen.grid[idx] = Cell::blank(style);
        }
    }

    fn copy_row(&mut self, src_row: usize, dst_row: usize) {
        let cols = self.cols;
        let screen = self.screen_mut();
        let src = Screen::index(cols, 0, src_row);
        let dst = Screen::index(cols, 0, dst_row);
        let row = screen.grid[src..src + cols].to_vec();
        screen.grid[dst..dst + cols].copy_from_slice(&row);
    }
}
