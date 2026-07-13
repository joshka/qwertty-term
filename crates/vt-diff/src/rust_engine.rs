//! The pure-Rust `qwertty-term-vt` terminal as an [`Oracle`].
//!
//! This is the Phase-1 in-tree oracle: it drives the ported stream dispatch
//! layer ([`qwertty_term_vt::stream::Stream`] over a
//! [`qwertty_term_vt::stream::TerminalHandler`]) and reports observable state
//! (screen text + cursor) the same way [`crate::ReferenceTerminal`] does for
//! the Zig library, so the two can be diffed byte-for-byte.

use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};

use crate::oracle::{CursorPos, Oracle, normalize_screen_text};

/// The Rust `qwertty-term-vt` terminal, used as the in-tree oracle in differential
/// tests (the counterpart to [`crate::ReferenceTerminal`]).
pub struct RustTerminal {
    stream: Stream<TerminalHandler>,
}

impl RustTerminal {
    /// Create a terminal with the given grid size and no scrollback.
    ///
    /// Zero scrollback keeps the plain-text dump identical to the visible
    /// grid — the comparison space of the harness — matching
    /// [`ReferenceTerminal::new`](crate::ReferenceTerminal::new).
    pub fn new(cols: u16, rows: u16) -> Self {
        Self::with_scrollback(cols, rows, 0)
    }

    /// Create a terminal with the given grid size and scrollback capacity
    /// (in bytes, matching the reference's budget semantics closely enough
    /// for the harness's zero-scrollback comparisons).
    pub fn with_scrollback(cols: u16, rows: u16, max_scrollback: usize) -> Self {
        assert!(cols > 0 && rows > 0, "grid dimensions must be non-zero");
        let terminal = Terminal::new(Options {
            cols,
            rows,
            max_scrollback,
            colors: Default::default(),
        });
        Self {
            stream: Stream::new(TerminalHandler::new(terminal)),
        }
    }

    /// Resize the grid.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.stream.handler.terminal.resize(cols, rows);
    }

    /// Access the accumulated reply bytes (DSR/DA/CPR/DECRQSS), in order.
    pub fn output(&self) -> &[u8] {
        &self.stream.handler.output
    }

    fn terminal(&self) -> &Terminal {
        &self.stream.handler.terminal
    }

    /// Plain-text dump produced by the **ported formatter**
    /// ([`qwertty_term_vt::formatter`]) with `trim = true`, whole active screen —
    /// the Rust mirror of [`ReferenceTerminal::raw_text`](crate::ReferenceTerminal::raw_text)
    /// (`ghostty_formatter_terminal_*`, PLAIN). Used by the formatter
    /// differential test.
    pub fn formatter_raw_text(&self) -> String {
        use qwertty_term_vt::formatter::{Options, TerminalExtra};
        self.terminal()
            .format(&Options::plain(), &TerminalExtra::none())
    }

    /// Styled dump via the ported **VT** formatter (`Options::vt`) — re-emits the
    /// screen as VT sequences INCLUDING SGR attributes, the Rust mirror of
    /// [`ReferenceTerminal::raw_text_vt`](crate::ReferenceTerminal::raw_text_vt).
    /// Used by the differential oracle to compare cell attributes, which the
    /// plain-text dump discards.
    pub fn formatter_vt_text(&self) -> String {
        use qwertty_term_vt::formatter::{Options, TerminalExtra};
        self.terminal()
            .format(&Options::vt(), &TerminalExtra::none())
    }
}

impl Oracle for RustTerminal {
    fn feed(&mut self, bytes: &[u8]) {
        self.stream.feed(bytes);
    }

    fn text(&self) -> String {
        // Mirror `ReferenceTerminal::text` exactly: dump through the (ported)
        // plain-text formatter over the whole screen, INCLUDING scrollback —
        // not the viewport-only `plain_string`. The trait contract says `text`
        // includes retained scrollback, and the reference side does; using the
        // viewport here made the oracle blind to scrolled-off content (it could
        // not observe divergences above the active area).
        normalize_screen_text(&self.formatter_raw_text())
    }

    fn styled_text(&self) -> String {
        self.formatter_vt_text()
    }

    fn cursor(&self) -> CursorPos {
        let cursor = &self.terminal().screen().cursor;
        CursorPos {
            row: cursor.y,
            col: cursor.x,
        }
    }

    fn term_state(&self) -> crate::TermState {
        use qwertty_term_vt::modes::Mode;
        use qwertty_term_vt::terminal::ScreenKey;
        let t = self.terminal();
        let pages = &t.screen().pages;
        crate::TermState {
            pending_wrap: t.screen().cursor.pending_wrap,
            alt_screen: t.screens.active_key() == ScreenKey::Alternate,
            cursor_visible: t.modes.get(Mode::CursorVisible),
            // "total rows minus the viewport" — mirror of ghostty SCROLLBACK_ROWS.
            scrollback_rows: pages.total_rows().saturating_sub(pages.rows() as usize),
        }
    }
}
