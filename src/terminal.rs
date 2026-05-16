//! Rust proof-of-concept for a small slice of Ghostty's VT core.
//!
//! The goal is fidelity for common terminal behavior, not structural parity
//! with Ghostty's Zig implementation. The code intentionally stays compact,
//! safe, and dependency-free while keeping explicit TODOs where a real port
//! would need deeper terminal semantics.

mod edit;
mod effects;
mod modes;
mod report;

use crate::{
    cell::Cell,
    mode::{CursorShape, MouseTracking, TerminalModes},
    parser::{
        CsiSequence, ParserState, is_csi_parameter_or_intermediate, one_based_to_zero, param_or,
    },
    screen::{
        Cursor, Region, Screen, ScreenKind, default_tab_stops, plain_text_for_screen,
        push_trimmed_row,
    },
    style::Style,
};

#[derive(Debug)]
pub struct Terminal {
    cols: usize,
    rows: usize,
    primary: Screen,
    alternate: Screen,
    active: ScreenKind,
    current_style: Style,
    state: ParserState,
    csi: String,
    osc: Vec<u8>,
    utf8: Vec<u8>,
    output: Vec<u8>,
    clipboard: Vec<String>,
    bell_count: usize,
    title: Option<String>,
    modes: TerminalModes,
    tab_stops: Vec<bool>,
    last_printed: Option<char>,
    scroll_region: Region,
    max_scrollback: usize,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        assert!(cols > 0, "terminal must have at least one column");
        assert!(rows > 0, "terminal must have at least one row");

        let current_style = Style::default();
        Self {
            cols,
            rows,
            primary: Screen::new(cols, rows, current_style),
            alternate: Screen::new(cols, rows, current_style),
            active: ScreenKind::Primary,
            current_style,
            state: ParserState::Ground,
            csi: String::new(),
            osc: Vec::new(),
            utf8: Vec::with_capacity(4),
            output: Vec::new(),
            clipboard: Vec::new(),
            bell_count: 0,
            title: None,
            modes: TerminalModes::default(),
            tab_stops: default_tab_stops(cols),
            last_printed: None,
            scroll_region: Region {
                top: 0,
                bottom: rows - 1,
            },
            max_scrollback: 1_000,
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.advance(byte);
        }
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }

    pub fn take_clipboard(&mut self) -> Vec<String> {
        std::mem::take(&mut self.clipboard)
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        assert!(cols > 0, "terminal must have at least one column");
        assert!(rows > 0, "terminal must have at least one row");

        if cols == self.cols && rows == self.rows {
            return;
        }

        self.primary
            .resize(self.cols, self.rows, cols, rows, self.current_style);
        self.alternate
            .resize(self.cols, self.rows, cols, rows, self.current_style);
        self.cols = cols;
        self.rows = rows;
        self.tab_stops = default_tab_stops(cols);
        self.scroll_region = Region {
            top: 0,
            bottom: rows - 1,
        };
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn active_screen(&self) -> ScreenKind {
        self.active
    }

    pub fn cursor(&self) -> Cursor {
        self.screen().cursor
    }

    pub fn current_style(&self) -> Style {
        self.current_style
    }

    pub fn bell_count(&self) -> usize {
        self.bell_count
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn wraparound(&self) -> bool {
        self.modes.wraparound
    }

    pub fn cursor_visible(&self) -> bool {
        self.modes.cursor_visible
    }

    pub fn cursor_shape(&self) -> CursorShape {
        self.modes.cursor_shape
    }

    pub fn application_cursor_keys(&self) -> bool {
        self.modes.application_cursor_keys
    }

    pub fn bracketed_paste(&self) -> bool {
        self.modes.bracketed_paste
    }

    pub fn focus_reporting(&self) -> bool {
        self.modes.focus_reporting
    }

    pub fn mouse_tracking(&self) -> Option<MouseTracking> {
        self.modes.mouse_tracking
    }

    pub fn sgr_mouse(&self) -> bool {
        self.modes.sgr_mouse
    }

    pub fn scroll_region(&self) -> (usize, usize) {
        (self.scroll_region.top, self.scroll_region.bottom)
    }

    pub fn cell(&self, col: usize, row: usize) -> Option<&Cell> {
        if col < self.cols && row < self.rows {
            Some(&self.screen().grid[Screen::index(self.cols, col, row)])
        } else {
            None
        }
    }

    pub fn grid(&self) -> &[Cell] {
        &self.screen().grid
    }

    pub fn scrollback_len(&self) -> usize {
        self.screen().scrollback.len()
    }

    pub fn scrollback_row(&self, row: usize) -> Option<&[Cell]> {
        self.screen().scrollback.get(row).map(Vec::as_slice)
    }

    pub fn scrollback_plain_text(&self) -> String {
        let mut out = String::new();
        for (idx, row) in self.screen().scrollback.iter().enumerate() {
            if idx > 0 {
                out.push('\n');
            }
            push_trimmed_row(&mut out, row);
        }
        out
    }

    pub fn plain_text(&self) -> String {
        plain_text_for_screen(self.screen(), self.cols, self.rows)
    }

    pub fn screen_dump(&self) -> String {
        self.plain_text()
    }

    fn screen(&self) -> &Screen {
        match self.active {
            ScreenKind::Primary => &self.primary,
            ScreenKind::Alternate => &self.alternate,
        }
    }

    fn screen_mut(&mut self) -> &mut Screen {
        match self.active {
            ScreenKind::Primary => &mut self.primary,
            ScreenKind::Alternate => &mut self.alternate,
        }
    }

    fn advance(&mut self, byte: u8) {
        match self.state {
            ParserState::Ground => self.advance_ground(byte),
            ParserState::Escape => self.advance_escape(byte),
            ParserState::EscapeHash => self.advance_escape_hash(byte),
            ParserState::Csi => self.advance_csi(byte),
            ParserState::Osc => self.advance_osc(byte),
            ParserState::OscEscape => self.advance_osc_escape(byte),
        }
    }

    fn advance_ground(&mut self, byte: u8) {
        match byte {
            0x07 => self.bell_count += 1,
            0x08 => self.backspace(),
            b'\t' => self.horizontal_tab(),
            b'\n' | 0x0b | 0x0c => self.linefeed(),
            b'\r' => self.carriage_return(),
            0x1b => {
                self.utf8.clear();
                self.state = ParserState::Escape;
            }
            0x00..=0x1f | 0x7f => {
                // TODO(port): implement remaining C0/C1 controls once the
                // parser grows beyond the common screen-editing path.
            }
            0x20..=0x7e => self.print_char(byte as char),
            _ => self.advance_utf8(byte),
        }
    }

    fn advance_escape(&mut self, byte: u8) {
        match byte {
            b'[' => {
                self.csi.clear();
                self.state = ParserState::Csi;
            }
            b']' => {
                self.osc.clear();
                self.state = ParserState::Osc;
            }
            b'#' => self.state = ParserState::EscapeHash,
            b'H' => {
                self.set_tab_stop();
                self.state = ParserState::Ground;
            }
            b'7' => {
                self.save_cursor();
                self.state = ParserState::Ground;
            }
            b'8' => {
                self.restore_cursor();
                self.state = ParserState::Ground;
            }
            b'D' => {
                self.linefeed();
                self.state = ParserState::Ground;
            }
            b'E' => {
                self.linefeed();
                self.carriage_return();
                self.state = ParserState::Ground;
            }
            b'M' => {
                self.reverse_index();
                self.state = ParserState::Ground;
            }
            b'c' => {
                self.reset();
                self.state = ParserState::Ground;
            }
            _ => {
                // TODO(port): charset designation, locking shifts, keypad
                // modes, and other ESC actions are intentionally omitted.
                self.state = ParserState::Ground;
            }
        }
    }

    fn advance_escape_hash(&mut self, byte: u8) {
        if byte == b'8' {
            self.screen_alignment_test();
        }
        self.state = ParserState::Ground;
    }

    fn advance_csi(&mut self, byte: u8) {
        if is_csi_parameter_or_intermediate(byte) {
            self.csi.push(byte as char);
            return;
        }

        let csi = CsiSequence::parse(std::mem::take(&mut self.csi));
        self.state = ParserState::Ground;

        match byte {
            b'A' if !csi.private => self.cursor_up(param_or(&csi.params, 0, 1)),
            b'B' if !csi.private => self.cursor_down(param_or(&csi.params, 0, 1)),
            b'C' if !csi.private => self.cursor_right(param_or(&csi.params, 0, 1)),
            b'D' if !csi.private => self.cursor_left(param_or(&csi.params, 0, 1)),
            b'E' if !csi.private => self.cursor_next_line(param_or(&csi.params, 0, 1)),
            b'F' if !csi.private => self.cursor_previous_line(param_or(&csi.params, 0, 1)),
            b'G' if !csi.private => {
                let col = one_based_to_zero(param_or(&csi.params, 0, 1));
                self.set_cursor(col, self.cursor().row);
            }
            b'H' | b'f' if !csi.private => {
                let row = one_based_to_zero(param_or(&csi.params, 0, 1));
                let col = one_based_to_zero(param_or(&csi.params, 1, 1));
                self.set_cursor(col, row);
            }
            b'I' if !csi.private => self.horizontal_tab_n(param_or(&csi.params, 0, 1)),
            b'J' if !csi.private => self.erase_display(param_or(&csi.params, 0, 0)),
            b'K' if !csi.private => self.erase_line(param_or(&csi.params, 0, 0)),
            b'L' if !csi.private => self.insert_lines(param_or(&csi.params, 0, 1)),
            b'M' if !csi.private => self.delete_lines(param_or(&csi.params, 0, 1)),
            b'P' if !csi.private => self.delete_chars(param_or(&csi.params, 0, 1)),
            b'S' if !csi.private => self.scroll_up_n(param_or(&csi.params, 0, 1)),
            b'T' if !csi.private => self.scroll_down_n(param_or(&csi.params, 0, 1)),
            b'X' if !csi.private => self.erase_chars(param_or(&csi.params, 0, 1)),
            b'Z' if !csi.private => self.horizontal_tab_back_n(param_or(&csi.params, 0, 1)),
            b'@' if !csi.private => self.insert_blank_chars(param_or(&csi.params, 0, 1)),
            b'b' if !csi.private => self.repeat_preceding_char(param_or(&csi.params, 0, 1)),
            b'd' if !csi.private => {
                let row = one_based_to_zero(param_or(&csi.params, 0, 1));
                self.set_cursor(self.cursor().col, row);
            }
            b'e' if !csi.private => self.cursor_down(param_or(&csi.params, 0, 1)),
            b'g' if !csi.private => self.clear_tabs(&csi.params),
            b'm' if !csi.private => self.select_graphic_rendition(&csi.params),
            b'n' => self.device_status_report(csi.private, &csi.params),
            b'q' if !csi.private && csi.has_intermediate_space() => {
                self.set_cursor_shape(&csi.params)
            }
            b'r' if !csi.private => self.set_scroll_region(&csi.params),
            b's' if !csi.private => self.save_cursor(),
            b'u' if !csi.private => self.restore_cursor(),
            b'c' => self.device_attributes(&csi.raw),
            b'h' if csi.private => self.set_private_modes(&csi.params, true),
            b'l' if csi.private => self.set_private_modes(&csi.params, false),
            _ => {
                // TODO(port): unsupported CSI actions include DSR/DA reports,
                // DECRQM, XTWINOPS, and several DEC private extensions.
            }
        }
    }

    fn advance_osc(&mut self, byte: u8) {
        match byte {
            0x07 => {
                self.finish_osc();
                self.state = ParserState::Ground;
            }
            0x1b => self.state = ParserState::OscEscape,
            _ => self.osc.push(byte),
        }
    }

    fn advance_osc_escape(&mut self, byte: u8) {
        if byte == b'\\' {
            self.finish_osc();
        } else {
            self.osc.push(0x1b);
            self.osc.push(byte);
        }
        self.state = ParserState::Ground;
    }

    fn reset(&mut self) {
        self.current_style = Style::default();
        self.primary.reset(self.cols, self.rows, self.current_style);
        self.alternate
            .reset(self.cols, self.rows, self.current_style);
        self.active = ScreenKind::Primary;
        self.csi.clear();
        self.osc.clear();
        self.utf8.clear();
        self.output.clear();
        self.modes = TerminalModes::default();
        self.tab_stops = default_tab_stops(self.cols);
        self.last_printed = None;
        self.scroll_region = Region {
            top: 0,
            bottom: self.rows - 1,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnsiColor, Color};

    #[test]
    fn basic_print() {
        let mut terminal = Terminal::new(10, 10);

        terminal.write(b"Hello");

        assert_eq!(terminal.cursor(), Cursor { col: 5, row: 0 });
        assert_eq!(terminal.plain_text(), "Hello");
    }

    #[test]
    fn utf8_printable_text() {
        let mut terminal = Terminal::new(10, 2);

        terminal.write("Héλ".as_bytes());

        assert_eq!(terminal.cursor(), Cursor { col: 3, row: 0 });
        assert_eq!(terminal.plain_text(), "Héλ");
    }

    #[test]
    fn utf8_can_span_writes() {
        let mut terminal = Terminal::new(10, 2);
        let bytes = "é".as_bytes();

        terminal.write(&bytes[..1]);
        assert_eq!(terminal.plain_text(), "");
        terminal.write(&bytes[1..]);

        assert_eq!(terminal.plain_text(), "é");
    }

    #[test]
    fn wide_unicode_occupies_two_cells() {
        let mut terminal = Terminal::new(10, 2);

        terminal.write("好x".as_bytes());

        assert_eq!(terminal.cursor(), Cursor { col: 3, row: 0 });
        assert_eq!(terminal.plain_text(), "好x");
        assert_eq!(terminal.cell(0, 0).expect("wide cell").ch(), '好');
        assert!(
            terminal
                .cell(1, 0)
                .expect("wide continuation")
                .is_wide_continuation()
        );
        assert_eq!(terminal.cell(2, 0).expect("x cell").ch(), 'x');
    }

    #[test]
    fn wide_unicode_wraps_when_it_would_cross_edge() {
        let mut terminal = Terminal::new(4, 2);

        terminal.write("abc好".as_bytes());

        assert_eq!(terminal.cursor(), Cursor { col: 2, row: 1 });
        assert_eq!(terminal.plain_text(), "abc\n好");
    }

    #[test]
    fn overwriting_wide_unicode_clears_continuation() {
        let mut terminal = Terminal::new(6, 2);

        terminal.write("a好b".as_bytes());
        terminal.write(b"\x1b[1;2HX");

        assert_eq!(terminal.plain_text(), "aX b");
        assert!(
            !terminal
                .cell(2, 0)
                .expect("old continuation")
                .is_wide_continuation()
        );
    }

    #[test]
    fn cursor_movement() {
        let mut terminal = Terminal::new(10, 10);

        terminal.write(b"Hello\x1b[1;1H");
        assert_eq!(terminal.cursor(), Cursor { col: 0, row: 0 });

        terminal.write(b"\x1b[2;3H");
        assert_eq!(terminal.cursor(), Cursor { col: 2, row: 1 });

        terminal.write(b"\x1b[1B\x1b[1C\x1b[A\x1b[D");
        assert_eq!(terminal.cursor(), Cursor { col: 2, row: 1 });
    }

    #[test]
    fn cursor_movement_clamps_to_screen() {
        let mut terminal = Terminal::new(4, 3);

        terminal.write(b"\x1b[99;99H");
        assert_eq!(terminal.cursor(), Cursor { col: 3, row: 2 });
        terminal.write(b"\x1b[99A\x1b[99D");
        assert_eq!(terminal.cursor(), Cursor { col: 0, row: 0 });
    }

    #[test]
    fn erase_operations() {
        let mut terminal = Terminal::new(20, 10);

        terminal.write(b"Hello World");
        assert_eq!(terminal.cursor(), Cursor { col: 11, row: 0 });

        terminal.write(b"\x1b[1;6H");
        terminal.write(b"\x1b[K");

        assert_eq!(terminal.plain_text(), "Hello");
    }

    #[test]
    fn erase_line_left_and_complete() {
        let mut terminal = Terminal::new(10, 2);

        terminal.write(b"abcdef\x1b[1;4H\x1b[1K");
        assert_eq!(terminal.plain_text(), "    ef");

        terminal.write(b"\x1b[2K");
        assert_eq!(terminal.plain_text(), "");
    }

    #[test]
    fn erase_display_modes() {
        let mut terminal = Terminal::new(5, 3);

        terminal.write(b"abcde12345xy");
        terminal.write(b"\x1b[2;3H\x1b[J");
        assert_eq!(terminal.plain_text(), "abcde\n12");

        terminal.write(b"\x1b[2J");
        assert_eq!(terminal.plain_text(), "");
    }

    #[test]
    fn tabs() {
        let mut terminal = Terminal::new(80, 10);

        terminal.write(b"A\tB");

        assert_eq!(terminal.cursor(), Cursor { col: 9, row: 0 });
        assert_eq!(terminal.plain_text(), "A       B");
    }

    #[test]
    fn c0_controls() {
        let mut terminal = Terminal::new(10, 3);

        terminal.write(b"abc\x08Z\rQ\nR\x07");

        assert_eq!(terminal.bell_count(), 1);
        assert_eq!(terminal.cursor(), Cursor { col: 2, row: 1 });
        assert_eq!(terminal.plain_text(), "QbZ\n R");
    }

    #[test]
    fn sgr_attributes_are_applied_to_cells() {
        let mut terminal = Terminal::new(20, 2);

        terminal.write(b"\x1b[1;2;3;4;5;7;9;31;44mA\x1b[0mB");

        let styled = terminal.cell(0, 0).expect("styled cell").style();
        assert!(styled.bold);
        assert!(styled.faint);
        assert!(styled.italic);
        assert!(styled.underline);
        assert!(styled.blink);
        assert!(styled.inverse);
        assert!(styled.strikethrough);
        assert_eq!(styled.fg, Some(Color::Ansi(AnsiColor::Red)));
        assert_eq!(styled.bg, Some(Color::Ansi(AnsiColor::Blue)));
        assert_eq!(
            terminal.cell(1, 0).expect("reset cell").style(),
            Style::default()
        );
        assert_eq!(terminal.plain_text(), "AB");
    }

    #[test]
    fn sgr_individual_attribute_resets() {
        let mut terminal = Terminal::new(20, 2);

        terminal.write(b"\x1b[1;2;3;4;5;7;9mA\x1b[22;23;24;25;27;29mB");

        let reset = terminal.cell(1, 0).expect("reset cell").style();
        assert_eq!(reset, Style::default());
    }

    #[test]
    fn sgr_bright_colors_and_resets() {
        let mut terminal = Terminal::new(20, 2);

        terminal.write(b"\x1b[91;102mX\x1b[39;49mY");

        let x = terminal.cell(0, 0).expect("x cell").style();
        assert_eq!(x.fg, Some(Color::Ansi(AnsiColor::BrightRed)));
        assert_eq!(x.bg, Some(Color::Ansi(AnsiColor::BrightGreen)));
        assert_eq!(
            terminal.cell(1, 0).expect("y cell").style(),
            Style::default()
        );
    }

    #[test]
    fn sgr_256_and_rgb_colors() {
        let mut terminal = Terminal::new(20, 2);

        terminal.write(b"\x1b[38;5;196;48;2;1;2;3mX");

        let style = terminal.cell(0, 0).expect("x cell").style();
        assert_eq!(style.fg, Some(Color::Indexed(196)));
        assert_eq!(style.bg, Some(Color::Rgb { r: 1, g: 2, b: 3 }));
    }

    #[test]
    fn sgr_colon_rgb_colors() {
        let mut terminal = Terminal::new(20, 2);

        terminal.write(b"\x1b[38:2::10:20:30mX");

        let style = terminal.cell(0, 0).expect("x cell").style();
        assert_eq!(
            style.fg,
            Some(Color::Rgb {
                r: 10,
                g: 20,
                b: 30
            })
        );
    }

    #[test]
    fn wrapping_and_scroll_up_at_bottom() {
        let mut terminal = Terminal::new(4, 2);

        terminal.write(b"abcdE");
        assert_eq!(terminal.cursor(), Cursor { col: 1, row: 1 });
        assert_eq!(terminal.plain_text(), "abcd\nE");

        terminal.write(b"FGH");
        assert_eq!(terminal.cursor(), Cursor { col: 3, row: 1 });
        terminal.write(b"I");
        assert_eq!(terminal.cursor(), Cursor { col: 1, row: 1 });
        assert_eq!(terminal.plain_text(), "EFGH\nI");
        assert_eq!(terminal.scrollback_plain_text(), "abcd");
    }

    #[test]
    fn scrollback_rows_expose_styled_cells() {
        let mut terminal = Terminal::new(3, 2);

        terminal.write(b"\x1b[31mabc\x1b[0mdefg");

        let row = terminal.scrollback_row(0).expect("first scrollback row");
        assert_eq!(row.len(), 3);
        assert_eq!(row[0].ch(), 'a');
        assert_eq!(row[1].ch(), 'b');
        assert_eq!(row[2].ch(), 'c');
        assert_eq!(row[0].style().fg, Some(Color::Ansi(AnsiColor::Red)));
        assert!(terminal.scrollback_row(1).is_none());
    }

    #[test]
    fn wraparound_mode_can_be_disabled() {
        let mut terminal = Terminal::new(4, 2);

        terminal.write(b"\x1b[?7labcdE");

        assert!(!terminal.wraparound());
        assert_eq!(terminal.cursor(), Cursor { col: 3, row: 0 });
        assert_eq!(terminal.plain_text(), "abcE");

        terminal.write(b"\x1b[?7hF");
        assert!(terminal.wraparound());
        assert_eq!(terminal.plain_text(), "abcF");
    }

    #[test]
    fn dec_private_input_and_cursor_modes() {
        let mut terminal = Terminal::new(10, 2);

        assert!(!terminal.application_cursor_keys());
        assert!(terminal.cursor_visible());
        assert!(!terminal.bracketed_paste());
        assert!(!terminal.focus_reporting());
        assert_eq!(terminal.mouse_tracking(), None);
        assert!(!terminal.sgr_mouse());

        terminal.write(b"\x1b[?1h\x1b[?25l\x1b[?1002h\x1b[?1004h\x1b[?1006h\x1b[?2004h");
        assert!(terminal.application_cursor_keys());
        assert!(!terminal.cursor_visible());
        assert!(terminal.bracketed_paste());
        assert!(terminal.focus_reporting());
        assert_eq!(terminal.mouse_tracking(), Some(MouseTracking::Drag));
        assert!(terminal.sgr_mouse());

        terminal.write(b"\x1b[?1l\x1b[?25h\x1b[?1002l\x1b[?1004l\x1b[?1006l\x1b[?2004l");
        assert!(!terminal.application_cursor_keys());
        assert!(terminal.cursor_visible());
        assert!(!terminal.bracketed_paste());
        assert!(!terminal.focus_reporting());
        assert_eq!(terminal.mouse_tracking(), None);
        assert!(!terminal.sgr_mouse());
    }

    #[test]
    fn cursor_shape_control() {
        let mut terminal = Terminal::new(10, 2);

        assert_eq!(terminal.cursor_shape(), CursorShape::Block);

        terminal.write(b"\x1b[4 q");
        assert_eq!(terminal.cursor_shape(), CursorShape::Underline);

        terminal.write(b"\x1b[6 q");
        assert_eq!(terminal.cursor_shape(), CursorShape::Bar);

        terminal.write(b"\x1b[0 q");
        assert_eq!(terminal.cursor_shape(), CursorShape::Block);
    }

    #[test]
    fn scroll_region_limits_linefeed_scrolling() {
        let mut terminal = Terminal::new(4, 4);

        terminal.write(b"1111222233334444");
        terminal.write(b"\x1b[2;3r\x1b[3;1H\n");

        assert_eq!(terminal.scroll_region(), (1, 2));
        assert_eq!(terminal.plain_text(), "1111\n3333\n\n4444");
    }

    #[test]
    fn cursor_row_col_and_next_previous_line() {
        let mut terminal = Terminal::new(10, 5);

        terminal.write(b"\x1b[3d\x1b[4Gx\x1b[2Edown\x1b[1Fup");

        assert_eq!(terminal.plain_text(), "\n\n   x\nup\ndown");
        assert_eq!(terminal.cursor(), Cursor { col: 2, row: 3 });
    }

    #[test]
    fn insert_delete_and_erase_chars() {
        let mut terminal = Terminal::new(10, 2);

        terminal.write(b"abcdef\x1b[1;3H\x1b[2@XY");
        assert_eq!(terminal.plain_text(), "abXYcdef");

        terminal.write(b"\x1b[1;5H\x1b[2P");
        assert_eq!(terminal.plain_text(), "abXYef");

        terminal.write(b"\x1b[1;3H\x1b[3X");
        assert_eq!(terminal.plain_text(), "ab   f");
    }

    #[test]
    fn insert_and_delete_lines_inside_scroll_region() {
        let mut terminal = Terminal::new(4, 4);

        terminal.write(b"aaaabbbbccccdddd");
        terminal.write(b"\x1b[2;4r\x1b[3;1H\x1b[L");
        assert_eq!(terminal.plain_text(), "aaaa\nbbbb\n\ncccc");

        terminal.write(b"\x1b[2;1H\x1b[M");
        assert_eq!(terminal.plain_text(), "aaaa\n\ncccc");
    }

    #[test]
    fn tab_stops_can_be_set_and_cleared() {
        let mut terminal = Terminal::new(12, 2);

        terminal.write(b"\x1b[1;4H\x1bH\x1b[1;1HA\tB");
        assert_eq!(terminal.plain_text(), "A  B");

        terminal.write(b"\r\x1b[2K\x1b[3gC\tD");
        assert_eq!(terminal.plain_text(), "C          D");
    }

    #[test]
    fn horizontal_tab_back_and_repeat_preceding_char() {
        let mut terminal = Terminal::new(12, 2);

        terminal.write(b"A\tB\x1b[1ZC\x1b[3b");

        assert_eq!(terminal.plain_text(), "A       CCCC");
        assert_eq!(terminal.cursor(), Cursor { col: 11, row: 0 });
    }

    #[test]
    fn explicit_scroll_up_and_down() {
        let mut terminal = Terminal::new(4, 3);

        terminal.write(b"aaaabbbbcccc\x1b[1S");
        assert_eq!(terminal.plain_text(), "bbbb\ncccc");

        terminal.write(b"\x1b[1T");
        assert_eq!(terminal.plain_text(), "\nbbbb\ncccc");
    }

    #[test]
    fn save_and_restore_cursor() {
        let mut terminal = Terminal::new(80, 24);

        terminal.write(b"\x1b[10;15H\x1b7\x1b[1;1H");
        assert_eq!(terminal.cursor(), Cursor { col: 0, row: 0 });

        terminal.write(b"\x1b8");
        assert_eq!(terminal.cursor(), Cursor { col: 14, row: 9 });

        terminal.write(b"\x1b[s\x1b[1;1H\x1b[u");
        assert_eq!(terminal.cursor(), Cursor { col: 14, row: 9 });
    }

    #[test]
    fn alt_screen_preserves_primary() {
        let mut terminal = Terminal::new(10, 5);

        terminal.write(b"Primary");
        assert_eq!(terminal.active_screen(), ScreenKind::Primary);

        terminal.write(b"\x1b[?1049hAlt");
        assert_eq!(terminal.active_screen(), ScreenKind::Alternate);
        assert_eq!(terminal.plain_text(), "Alt");

        terminal.write(b"\x1b[?1049l");
        assert_eq!(terminal.active_screen(), ScreenKind::Primary);
        assert_eq!(terminal.plain_text(), "Primary");
    }

    #[test]
    fn osc_title_bel_and_st() {
        let mut terminal = Terminal::new(10, 5);

        terminal.write(b"\x1b]0;hello\x07");
        assert_eq!(terminal.title(), Some("hello"));

        terminal.write(b"\x1b]2;world\x1b\\");
        assert_eq!(terminal.title(), Some("world"));
    }

    #[test]
    fn osc_52_decodes_clipboard_write() {
        let mut terminal = Terminal::new(10, 2);

        terminal.write(b"\x1b]52;c;aGVsbG8=\x07");

        assert_eq!(terminal.take_clipboard(), ["hello".to_string()]);
        assert!(terminal.take_clipboard().is_empty());
    }

    #[test]
    fn device_status_reports_write_pty_responses() {
        let mut terminal = Terminal::new(80, 24);

        terminal.write(b"\x1b[5n");
        assert_eq!(terminal.take_output(), b"\x1b[0n");

        terminal.write(b"\x1b[5;10H\x1b[6n");
        assert_eq!(terminal.take_output(), b"\x1b[5;10R");

        terminal.write(b"\x1b[?996n");
        assert_eq!(terminal.take_output(), b"\x1b[?997;1n");
    }

    #[test]
    fn device_attributes_write_pty_responses() {
        let mut terminal = Terminal::new(80, 24);

        terminal.write(b"\x1b[c");
        assert_eq!(terminal.take_output(), b"\x1b[?62;22c");

        terminal.write(b"\x1b[>c");
        assert_eq!(terminal.take_output(), b"\x1b[>1;0;0c");
    }

    #[test]
    fn dec_screen_alignment() {
        let mut terminal = Terminal::new(3, 2);

        terminal.write(b"\x1b#8");

        assert_eq!(terminal.cursor(), Cursor { col: 0, row: 0 });
        assert_eq!(terminal.plain_text(), "EEE\nEEE");
    }

    #[test]
    fn full_reset() {
        let mut terminal = Terminal::new(5, 2);

        terminal.write(b"\x1b[?7l\x1b[31mText\x1b[?1049hAlt\x1bc");

        assert_eq!(terminal.active_screen(), ScreenKind::Primary);
        assert!(terminal.wraparound());
        assert_eq!(terminal.current_style(), Style::default());
        assert_eq!(terminal.cursor(), Cursor { col: 0, row: 0 });
        assert_eq!(terminal.plain_text(), "");
    }

    #[test]
    fn screen_dump_aliases_plain_text() {
        let mut terminal = Terminal::new(5, 2);

        terminal.write(b"Hi");

        assert_eq!(terminal.screen_dump(), "Hi");
    }

    #[test]
    fn resize_preserves_visible_cells_and_clamps_cursor() {
        let mut terminal = Terminal::new(4, 2);

        terminal.write(b"abcdEF");
        terminal.resize(6, 3);

        assert_eq!(terminal.cols(), 6);
        assert_eq!(terminal.rows(), 3);
        assert_eq!(terminal.plain_text(), "abcd\nEF");

        terminal.write(b"\x1b[99;99H");
        terminal.resize(3, 1);

        assert_eq!(terminal.cursor(), Cursor { col: 2, row: 0 });
        assert_eq!(terminal.plain_text(), "abc");
    }
}
