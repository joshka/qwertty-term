//! The common terminal-oracle interface used for differential testing.

/// Cursor position in the active area, 0-indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPos {
    /// Row within the active area (0 = top).
    pub row: u16,
    /// Column (0 = leftmost).
    pub col: u16,
}

/// Scalar terminal state beyond the grid contents — cheap flags the plain-text
/// dump can't express, compared alongside the screen in a differential test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermState {
    /// The cursor has a pending soft-wrap (next print wraps first).
    pub pending_wrap: bool,
    /// The alternate screen is active (vs. the primary screen).
    pub alt_screen: bool,
    /// The cursor is visible (DECTCEM / mode 25).
    pub cursor_visible: bool,
    /// Any mouse-tracking mode is active (X10 / normal / button / any-event).
    pub mouse_tracking: bool,
    /// Number of scrollback rows (total rows minus the viewport).
    pub scrollback_rows: usize,
}

/// Observable terminal state captured from an oracle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenDump {
    /// Plain-text screen contents, normalized via [`normalize_screen_text`]:
    /// no trailing whitespace on any line, no trailing blank lines, no
    /// trailing newline.
    pub text: String,
    /// Styled screen contents: the screen re-emitted as VT sequences including
    /// SGR attributes (the VT formatter). Compared raw — both engines' VT
    /// formatters agree byte-for-byte — so it catches color/bold/underline/
    /// hyperlink divergences the plain `text` discards.
    pub styled: String,
    /// Cursor position in the active area.
    pub cursor: CursorPos,
    /// Scalar flags (wrap/screen/cursor) compared alongside the grid.
    pub state: TermState,
}

/// A terminal implementation that can serve as one side of a differential
/// test: feed bytes in, dump observable state out.
///
/// Implemented by `ReferenceTerminal` (libghostty-vt behind FFI, `reference`
/// feature) and, in Phase 1, by the pure-Rust `qwertty-term-vt` terminal.
pub trait Oracle {
    /// Feed raw VT bytes to the terminal. Must never fail on malformed
    /// input; garbage bytes only affect state.
    fn feed(&mut self, bytes: &[u8]);

    /// The plain-text contents of the active screen (including any
    /// scrollback the implementation retains), normalized via
    /// [`normalize_screen_text`].
    fn text(&self) -> String;

    /// The styled contents of the active screen (VT formatter output, including
    /// scrollback), compared raw.
    fn styled_text(&self) -> String;

    /// Cursor position in the active area, 0-indexed.
    fn cursor(&self) -> CursorPos;

    /// Scalar terminal flags (wrap/screen/cursor).
    fn term_state(&self) -> TermState;

    /// Capture all observable state together.
    fn dump(&self) -> ScreenDump {
        ScreenDump {
            text: self.text(),
            styled: self.styled_text(),
            cursor: self.cursor(),
            state: self.term_state(),
        }
    }
}

/// Normalize a plain-text screen dump for comparison: strip trailing
/// whitespace from every line, then drop trailing blank lines (and the
/// trailing newline). This matches the convention used by the replay
/// fixtures' `expected.txt` files.
pub fn normalize_screen_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for line in raw.split('\n') {
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.truncate(out.trim_end_matches('\n').len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_trailing_line_whitespace() {
        assert_eq!(normalize_screen_text("ab  \ncd\t\n"), "ab\ncd");
    }

    #[test]
    fn normalize_strips_trailing_blank_lines() {
        assert_eq!(normalize_screen_text("ab\n\n   \n\n"), "ab");
    }

    #[test]
    fn normalize_keeps_interior_blank_lines_and_indentation() {
        assert_eq!(normalize_screen_text("  ab\n\ncd\n"), "  ab\n\ncd");
    }
}
