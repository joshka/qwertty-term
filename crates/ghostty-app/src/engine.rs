//! Terminal engine wrapper over `ghostty-vt`.
//!
//! A thin adapter around `ghostty_vt`'s [`Stream`] + [`Terminal`], exposing only
//! what the AppKit host needs: feed PTY bytes in, drain reply bytes out, take a
//! windowed render snapshot, read the input-affecting modes / kitty flags for
//! the key encoder, read the OSC 7 working directory (for new-tab inheritance),
//! and resize.
//!
//! This mirrors the reference `crates/spike/src/engine.rs` (read-only spike
//! material) — same call sites into `ghostty-vt` — but is an independent copy so
//! the app doesn't path-depend on `ghostty-spike` (which pulls in eframe). The
//! subset here is what R5 actually exercises.

use ghostty_input::key_encode::{KittyFlags, Options as EncodeOptions};
use ghostty_input::mouse_encode::{MouseEvent, MouseFormat};
use ghostty_vt::modes::Mode;
use ghostty_vt::snapshot::SnapshotWindow;
use ghostty_vt::stream::{Stream, TerminalHandler};
use ghostty_vt::terminal::{Options, Terminal};

/// The terminal engine, backed by `ghostty-vt`.
pub struct Engine {
    stream: Stream<TerminalHandler>,
}

impl Engine {
    /// Create a new engine with the given grid size.
    pub fn new(cols: usize, rows: usize) -> Self {
        let terminal = Terminal::new(Options {
            cols: clamp_dim(cols),
            rows: clamp_dim(rows),
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

    /// Feed PTY output bytes into the parser/terminal.
    pub fn write(&mut self, bytes: &[u8]) {
        self.stream.feed(bytes);
    }

    /// Drain any reply bytes (DSR/DA/CPR/DECRQSS/…) queued in response to fed
    /// bytes, destined for the PTY.
    pub fn take_output(&mut self) -> Vec<u8> {
        self.stream.handler.take_output()
    }

    /// Drain the most recent OSC 52 clipboard write request, if any:
    /// `(kind, raw_base64_data)`. Handed up raw (still base64-encoded, per
    /// upstream's apprt-decodes-it policy).
    pub fn take_clipboard(&mut self) -> Option<(u8, String)> {
        self.stream.handler.take_clipboard()
    }

    /// Resize the grid.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.terminal_mut().resize(clamp_dim(cols), clamp_dim(rows));
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

    /// A cheap, windowed snapshot containing only the rows needed to render the
    /// viewport `scrollback_offset` rows up from the bottom (0 = live active
    /// area). Use this on the per-frame render path.
    pub fn snapshot_window(&self, scrollback_offset: usize) -> SnapshotWindow {
        self.terminal().snapshot_window(scrollback_offset)
    }

    /// A plain-text dump of the visible screen (used by smoke modes).
    pub fn screen_dump(&self) -> String {
        self.terminal().plain_string()
    }

    /// The OSC 7 working directory as a filesystem path, if the running shell
    /// has reported one. The stored value is a `file://host/path` URL (or a bare
    /// path); [`pwd_path_from_osc7`] extracts the local path. Used to spawn a new
    /// tab's shell in the current tab's directory.
    pub fn pwd(&self) -> Option<String> {
        let raw = self.terminal().get_pwd()?;
        let s = std::str::from_utf8(raw).ok()?;
        pwd_path_from_osc7(s)
    }

    // -- input-affecting modes ------------------------------------------------

    pub fn bracketed_paste(&self) -> bool {
        self.mode(Mode::BracketedPaste)
    }

    pub fn focus_reporting(&self) -> bool {
        self.mode(Mode::FocusEvent)
    }

    /// The kitty keyboard protocol flags currently active on the active screen.
    pub fn kitty_flags(&self) -> KittyFlags {
        let flags = self.terminal().screen().kitty_keyboard.current();
        KittyFlags::from_bits(flags.int())
    }

    /// Key-encoding options derived from current terminal mode state, for
    /// `ghostty_input::key_encode::encode`. `macos_option_as_alt` is left at its
    /// default here; the input path overlays the user's config value.
    pub fn key_encode_options(&self) -> EncodeOptions {
        EncodeOptions {
            cursor_key_application: self.mode(Mode::CursorKeys),
            keypad_key_application: self.mode(Mode::KeypadKeys),
            backarrow_key_mode: self.mode(Mode::BackarrowKeyMode),
            ignore_keypad_with_numlock: self.mode(Mode::IgnoreKeypadWithNumlock),
            alt_esc_prefix: self.mode(Mode::AltEscPrefix),
            modify_other_keys_state_2: self.terminal().flags.modify_other_keys_2,
            kitty_flags: self.kitty_flags(),
            ..Default::default()
        }
    }

    /// The terminal's requested mouse reporting mode (`None` if off).
    pub fn mouse_event(&self) -> MouseEvent {
        if self.mode(Mode::MouseEventAny) {
            MouseEvent::Any
        } else if self.mode(Mode::MouseEventButton) {
            MouseEvent::Button
        } else if self.mode(Mode::MouseEventNormal) {
            MouseEvent::Normal
        } else if self.mode(Mode::MouseEventX10) {
            MouseEvent::X10
        } else {
            MouseEvent::None
        }
    }

    /// The terminal's requested mouse report format. Precedence matches upstream:
    /// SGR-pixels, SGR, urxvt, UTF-8, else X10.
    pub fn mouse_format(&self) -> MouseFormat {
        if self.mode(Mode::MouseFormatSgrPixels) {
            MouseFormat::SgrPixels
        } else if self.mode(Mode::MouseFormatSgr) {
            MouseFormat::Sgr
        } else if self.mode(Mode::MouseFormatUrxvt) {
            MouseFormat::Urxvt
        } else if self.mode(Mode::MouseFormatUtf8) {
            MouseFormat::Utf8
        } else {
            MouseFormat::X10
        }
    }

    fn mode(&self, mode: Mode) -> bool {
        self.terminal().modes.get(mode)
    }
}

/// Extract the local filesystem path from an OSC 7 value. OSC 7 carries a
/// `file://<host>/<path>` URL; we take the path component (everything from the
/// first `/` after the authority). A bare path (no scheme) is returned as-is.
/// Returns `None` for an empty result. Percent-decoding is handled minimally
/// (only `%20` → space, the common case) — full RFC 3986 decoding is deferred.
pub fn pwd_path_from_osc7(value: &str) -> Option<String> {
    let path = if let Some(rest) = value.strip_prefix("file://") {
        // rest = "<host>/<path>"; the path starts at the first '/'.
        match rest.find('/') {
            Some(i) => &rest[i..],
            None => rest,
        }
    } else {
        value
    };
    let decoded = path.replace("%20", " ");
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

/// Clamp a requested dimension into the engine's supported `u16` range (at least
/// one cell; the engine panics on a zero dimension).
fn clamp_dim(value: usize) -> u16 {
    value.clamp(1, u16::MAX as usize) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_dumps_text() {
        let mut engine = Engine::new(20, 3);
        engine.write(b"hello");
        assert!(engine.screen_dump().contains("hello"));
    }

    #[test]
    fn drains_dsr_reply() {
        let mut engine = Engine::new(80, 24);
        engine.write(b"\x1b[6n");
        assert_eq!(engine.take_output(), b"\x1b[1;1R");
    }

    #[test]
    fn resize_changes_dims() {
        let mut engine = Engine::new(80, 24);
        engine.resize(100, 30);
        assert_eq!((engine.cols(), engine.rows()), (100, 30));
    }

    #[test]
    fn reports_title() {
        let mut engine = Engine::new(80, 24);
        engine.write(b"\x1b]0;hi\x07");
        assert_eq!(engine.title().as_deref(), Some("hi"));
    }

    #[test]
    fn tracks_pwd_via_osc7() {
        let mut engine = Engine::new(80, 24);
        engine.write(b"\x1b]7;file://localhost/Users/me/proj\x1b\\");
        assert_eq!(engine.pwd().as_deref(), Some("/Users/me/proj"));
    }

    #[test]
    fn osc7_path_extraction() {
        assert_eq!(
            pwd_path_from_osc7("file://host/Users/me").as_deref(),
            Some("/Users/me")
        );
        assert_eq!(
            pwd_path_from_osc7("file:///Users/me").as_deref(),
            Some("/Users/me")
        );
        assert_eq!(
            pwd_path_from_osc7("file://host/a/b%20c").as_deref(),
            Some("/a/b c")
        );
        assert_eq!(
            pwd_path_from_osc7("/bare/path").as_deref(),
            Some("/bare/path")
        );
        assert_eq!(pwd_path_from_osc7(""), None);
    }
}
