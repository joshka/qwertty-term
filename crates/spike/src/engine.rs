//! Adapter that runs the spike frontends on the `ghostty-vt` engine.
//!
//! This wraps `ghostty_vt`'s [`Stream`] + [`Terminal`] behind the narrow
//! interface the crossterm and egui frontends need: feed pty bytes in, drain
//! reply bytes out, snapshot the visible + scrollback grid for rendering, read
//! cursor / title / input-affecting modes, and resize.
//!
//! The rendering surface is `ghostty_vt`'s owned [`Snapshot`] type (see
//! [`ghostty_vt::snapshot`]) — the frontends consume it directly rather than
//! re-mapping into the spike's legacy cell/style types, which are no longer used
//! on the live path.

use ghostty_vt::modes::Mode;
use ghostty_vt::snapshot::Snapshot;
use ghostty_vt::stream::{Stream, TerminalHandler};
use ghostty_vt::terminal::{Colors, Options, Terminal};

pub use ghostty_vt::screen::cursor::CursorStyle;
pub use ghostty_vt::snapshot::{
    CellStyle, CellWidth, SnapshotCell, SnapshotColor, SnapshotCursor, SnapshotRow,
    SnapshotUnderline, SnapshotWindow,
};

/// Which mouse-tracking mode the running program has requested, if any. Derived
/// from the DEC private modes the engine tracks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseTracking {
    /// Button press/release only (mode 1000).
    Button,
    /// Button + drag motion (mode 1002).
    Drag,
    /// Any motion (mode 1003).
    Any,
}

/// The spike-side terminal engine, backed by `ghostty-vt`.
pub struct Engine {
    stream: Stream<TerminalHandler>,
}

impl Engine {
    /// Create a new engine with the given grid size.
    pub fn new(cols: usize, rows: usize) -> Self {
        Self::with_colors(cols, rows, Colors::default())
    }

    /// Create a new engine with the given grid size and startup dynamic
    /// color state (256-color palette + default fg/bg/cursor). Used to seed
    /// a theme's colors before the first frame; the running program can
    /// still override any of these at runtime via OSC 4/10/11/12, same as
    /// with the default palette.
    pub fn with_colors(cols: usize, rows: usize, colors: Colors) -> Self {
        let terminal = Terminal::new(Options {
            cols: clamp_dim(cols),
            rows: clamp_dim(rows),
            colors,
            ..Default::default()
        });
        Self {
            stream: Stream::new(TerminalHandler::new(terminal)),
        }
    }

    fn terminal(&self) -> &Terminal {
        &self.stream.handler.terminal
    }

    fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.stream.handler.terminal
    }

    /// Feed pty output bytes into the parser/terminal.
    pub fn write(&mut self, bytes: &[u8]) {
        self.stream.feed(bytes);
    }

    /// Drain any reply bytes (DSR/DA/CPR/DECRQSS/…) the engine queued in
    /// response to the fed bytes, destined for the pty.
    pub fn take_output(&mut self) -> Vec<u8> {
        self.stream.handler.take_output()
    }

    /// Drain the most recent OSC 52 clipboard write request, if any.
    /// Returns `(kind, raw_base64_data)` — `ghostty-vt` hands this up raw
    /// (still base64-encoded, per upstream's apprt-decodes-it policy); the
    /// frontend is responsible for base64-decoding and performing the actual
    /// clipboard I/O. An empty `raw_base64_data` means "clear the clipboard".
    pub fn take_clipboard(&mut self) -> Option<(u8, String)> {
        self.stream.handler.take_clipboard()
    }

    /// Resize the grid.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = clamp_dim(cols);
        let rows = clamp_dim(rows);
        self.terminal_mut().resize(cols, rows);
    }

    pub fn cols(&self) -> usize {
        self.terminal().cols as usize
    }

    pub fn rows(&self) -> usize {
        self.terminal().rows as usize
    }

    /// The current window title (OSC 0/2), if set and valid UTF-8.
    pub fn title(&self) -> Option<String> {
        let title = self.terminal().get_title()?;
        std::str::from_utf8(title).ok().map(str::to_owned)
    }

    /// An owned snapshot of the visible grid + scrollback for rendering.
    ///
    /// Prefer [`Engine::snapshot_window`] on a per-rendered-frame call site:
    /// this materializes *every* scrollback row, so its cost grows with
    /// total history length, not with what's actually on screen.
    pub fn snapshot(&self) -> Snapshot {
        self.terminal().snapshot()
    }

    /// A cheap, windowed snapshot containing only the rows needed to render
    /// a viewport `scrollback_offset` rows up from the bottom (0 = the live
    /// active area). Cost is proportional to the visible row count, not to
    /// total scrollback length — use this on the per-frame render path.
    pub fn snapshot_window(&self, scrollback_offset: usize) -> SnapshotWindow {
        self.terminal().snapshot_window(scrollback_offset)
    }

    /// The number of scrollback (history) rows above the active area.
    pub fn scrollback_len(&self) -> usize {
        self.terminal().screen().pages.total_rows() - self.rows()
    }

    /// A plain-text dump of the visible screen (used by replay/smoke modes).
    pub fn screen_dump(&self) -> String {
        self.terminal().plain_string()
    }

    // -- input-affecting modes ------------------------------------------------

    pub fn application_cursor_keys(&self) -> bool {
        self.mode(Mode::CursorKeys)
    }

    pub fn bracketed_paste(&self) -> bool {
        self.mode(Mode::BracketedPaste)
    }

    pub fn focus_reporting(&self) -> bool {
        self.mode(Mode::FocusEvent)
    }

    pub fn cursor_visible(&self) -> bool {
        self.mode(Mode::CursorVisible)
    }

    pub fn sgr_mouse(&self) -> bool {
        self.mode(Mode::MouseFormatSgr) || self.mode(Mode::MouseFormatSgrPixels)
    }

    pub fn mouse_tracking(&self) -> Option<MouseTracking> {
        if self.mode(Mode::MouseEventAny) {
            Some(MouseTracking::Any)
        } else if self.mode(Mode::MouseEventButton) {
            Some(MouseTracking::Drag)
        } else if self.mode(Mode::MouseEventNormal) || self.mode(Mode::MouseEventX10) {
            Some(MouseTracking::Button)
        } else {
            None
        }
    }

    fn mode(&self, mode: Mode) -> bool {
        self.terminal().modes.get(mode)
    }
}

/// Clamp a requested dimension into the engine's supported `u16` range,
/// enforcing at least one cell (the engine panics on a zero dimension).
fn clamp_dim(value: usize) -> u16 {
    value.clamp(1, u16::MAX as usize) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_row_text(snap: &Snapshot, row: usize) -> String {
        let window = snap.visible_window(0);
        let mut s: String = window[row]
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
    fn writes_and_snapshots_text() {
        let mut engine = Engine::new(20, 3);
        engine.write(b"hello world");
        let snap = engine.snapshot();
        assert_eq!(active_row_text(&snap, 0), "hello world");
        assert_eq!(snap.cursor.col, 11);
    }

    #[test]
    fn drains_dsr_reply_to_pty() {
        let mut engine = Engine::new(80, 24);
        engine.write(b"\x1b[6n"); // request cursor position report
        let out = engine.take_output();
        assert_eq!(out, b"\x1b[1;1R");
        assert!(engine.take_output().is_empty());
    }

    #[test]
    fn tracks_input_modes() {
        let mut engine = Engine::new(80, 24);
        assert!(!engine.application_cursor_keys());
        assert!(engine.cursor_visible());
        engine.write(b"\x1b[?1h\x1b[?25l\x1b[?2004h\x1b[?1002h\x1b[?1006h");
        assert!(engine.application_cursor_keys());
        assert!(!engine.cursor_visible());
        assert!(engine.bracketed_paste());
        assert_eq!(engine.mouse_tracking(), Some(MouseTracking::Drag));
        assert!(engine.sgr_mouse());
    }

    #[test]
    fn reports_title() {
        let mut engine = Engine::new(80, 24);
        engine.write(b"\x1b]0;hello\x07");
        assert_eq!(engine.title().as_deref(), Some("hello"));
    }

    #[test]
    fn resize_changes_dims() {
        let mut engine = Engine::new(80, 24);
        engine.resize(100, 30);
        assert_eq!(engine.cols(), 100);
        assert_eq!(engine.rows(), 30);
    }

    #[test]
    fn takes_osc52_clipboard_write_raw() {
        let mut engine = Engine::new(80, 24);
        // "aGVsbG8=" is base64 for "hello"; the engine hands it up
        // undecoded (decoding is the frontend's job, per upstream policy).
        engine.write(b"\x1b]52;c;aGVsbG8=\x1b\\");
        assert_eq!(
            engine.take_clipboard(),
            Some((b'c', "aGVsbG8=".to_string()))
        );
        // Drained; nothing left until another OSC 52 write arrives.
        assert_eq!(engine.take_clipboard(), None);
    }

    #[test]
    fn osc52_query_does_not_produce_a_clipboard_write() {
        let mut engine = Engine::new(80, 24);
        engine.write(b"\x1b]52;c;?\x1b\\");
        assert_eq!(engine.take_clipboard(), None);
    }

    #[test]
    fn snapshot_reflects_dynamic_palette_and_default_colors() {
        let mut engine = Engine::new(10, 2);
        engine.write(b"\x1b]4;1;#112233\x1b\\\x1b]10;#aabbcc\x1b\\\x1b]11;#001122\x1b\\");
        let snap = engine.snapshot();
        assert_eq!(
            snap.palette[1],
            ghostty_vt::color::Rgb::new(0x11, 0x22, 0x33)
        );
        assert_eq!(
            snap.default_fg,
            Some(ghostty_vt::color::Rgb::new(0xaa, 0xbb, 0xcc))
        );
        assert_eq!(
            snap.default_bg,
            Some(ghostty_vt::color::Rgb::new(0x00, 0x11, 0x22))
        );
    }
}
