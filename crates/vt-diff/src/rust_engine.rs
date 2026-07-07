//! The pure-Rust `ghostty-vt` terminal as an [`Oracle`].
//!
//! This is the Phase-1 in-tree oracle: it drives the ported stream dispatch
//! layer ([`ghostty_vt::stream::Stream`] over a
//! [`ghostty_vt::stream::TerminalHandler`]) and reports observable state
//! (screen text + cursor) the same way [`crate::ReferenceTerminal`] does for
//! the Zig library, so the two can be diffed byte-for-byte.

use ghostty_vt::stream::{Stream, TerminalHandler};
use ghostty_vt::terminal::{Options, Terminal};

use crate::oracle::{CursorPos, Oracle, normalize_screen_text};

/// The Rust `ghostty-vt` terminal, used as the in-tree oracle in differential
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
}

impl Oracle for RustTerminal {
    fn feed(&mut self, bytes: &[u8]) {
        self.stream.feed(bytes);
    }

    fn text(&self) -> String {
        normalize_screen_text(&self.terminal().plain_string())
    }

    fn cursor(&self) -> CursorPos {
        let cursor = &self.terminal().screen().cursor;
        CursorPos {
            row: cursor.y,
            col: cursor.x,
        }
    }
}
