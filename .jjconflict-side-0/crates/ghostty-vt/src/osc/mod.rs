//! OSC (Operating System Command) command parsing.
//!
//! Ported from ghostty `src/terminal/osc.zig` + `src/terminal/osc/parsers/`
//! (commit `2da015cd6`); see `docs/analysis/osc.md` for the full survey.
//!
//! This module is the structured consumer of the VT parser's raw OSC byte
//! seam (`docs/analysis/vt-parser.md`, "OSC boundary" section):
//! [`parser::Action::OscStart`]/[`OscPut`](parser::Action::OscPut)/
//! [`OscEnd`](parser::Action::OscEnd). A caller (eventually the `stream`
//! layer; today, tests in this module and downstream consumers) drives
//! [`Parser`] directly from those events:
//!
//! ```
//! use ghostty_vt::parser::{self, Action};
//! use ghostty_vt::osc;
//!
//! let mut vt = parser::Parser::new();
//! let mut osc_parser = osc::Parser::new();
//!
//! for b in [0x1B, b']'] {
//!     vt.next(b);
//! }
//! osc_parser.reset();
//! for c in "0;my title".bytes() {
//!     let a = vt.next(c);
//!     if let Some(Action::OscPut(b)) = a[1] {
//!         osc_parser.next(b);
//!     }
//! }
//! let a = vt.next(0x07); // BEL
//! let Some(Action::OscEnd(term)) = a[0] else { unreachable!() };
//! let cmd = osc_parser.end(Some(term)).unwrap();
//! assert_eq!(cmd, osc::Command::ChangeWindowTitle("my title".to_string()));
//! ```
//!
//! Unlike ghostty's `osc.Parser` (a hand-rolled byte-driven prefix trie
//! over `State`, `osc.zig:318-371`), this port buffers raw bytes into a
//! capture [`Vec<u8>`] via [`Parser::next`] and performs the OSC-number
//! prefix dispatch once, in [`Parser::end`], by matching on the buffered
//! prefix. This is behaviorally equivalent (same valid/invalid partition,
//! same capture-mode-per-command-family — see `docs/analysis/osc.md`
//! divergence #2) and considerably simpler in Rust.

mod parsers;
pub mod rgb;
mod string_encoding;
mod support;

pub use parsers::change_window_title::TitleMode;
pub use parsers::color::{ColorList, ColorRequest, ColorTarget};
pub use parsers::context_signal::{ContextAction, ContextSignal, ContextType, ExitStatus};
pub use parsers::kitty_clipboard_protocol::{
    ClipboardLocation, ClipboardOperation, ClipboardStatus, KittyClipboardProtocol,
};
pub use parsers::kitty_color::{KittyColorProtocol, KittyColorRequest};
pub use parsers::kitty_dnd_protocol::{DndEventType, KittyDndProtocol};
pub use parsers::kitty_text_sizing::{HAlign, KittyTextSizing, VAlign};
pub use parsers::semantic_prompt::{
    Click, ClickEvents, PromptKind, Redraw, SemanticPrompt, SemanticPromptAction,
};
pub use rgb::{Dynamic, Special};

/// Maximum size of a "normal" OSC capture buffer, before an allocator
/// (i.e. unbounded growth) is required. Port of `osc.zig` `Parser.MAX_BUF`.
pub const MAX_BUF: usize = 2048;

/// The terminator used to end an OSC command. Port of `osc.zig`
/// `Terminator`.
///
/// For OSC commands that demand a response, ghostty tries to match the
/// terminator used in the request, since that is most likely to be
/// accepted by the calling program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Terminator {
    /// The preferred string terminator: ESC followed by `\`.
    #[default]
    St,
    /// Some applications and terminals use BEL (0x07) as the terminator.
    Bel,
}

impl Terminator {
    /// Initialize the terminator based on the last byte seen. Port of
    /// `osc.zig` `Terminator.init`.
    pub fn init(ch: Option<u8>) -> Terminator {
        match ch {
            Some(0x07) => Terminator::Bel,
            _ => Terminator::St,
        }
    }

    /// The terminator as a byte string.
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            Terminator::St => b"\x1b\\",
            Terminator::Bel => b"\x07",
        }
    }
}

/// A parsed OSC command. Port of `osc.zig` `Command` (the `Key` union).
///
/// `Command::Invalid` from the Zig source has no variant here: Rust's
/// `Option<Command>` (the return type of [`Parser::end`]) plays that role,
/// so a failed/invalid parse is `None`, not a sentinel variant.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// OSC 0/2: set the window title.
    ChangeWindowTitle(String),
    /// OSC 1: set the window icon name. Parsed but not acted on (ghostty
    /// doesn't define icon-name semantics either).
    ChangeWindowIcon(String),
    /// OSC 133: semantic prompt marker.
    SemanticPrompt(SemanticPrompt),
    /// OSC 52: get/set clipboard contents.
    ClipboardContents { kind: u8, data: String },
    /// OSC 7: report the current working directory.
    ReportPwd { value: String },
    /// OSC 22: set the mouse cursor shape.
    MouseShape { value: String },
    /// OSC 4,5,10-19,104,105,110-119: color get/set/reset operations.
    ColorOperation {
        requests: ColorList,
        terminator: Terminator,
    },
    /// OSC 21: kitty color protocol.
    KittyColorProtocol(KittyColorProtocol),
    /// OSC 9 / OSC 777: desktop notification.
    ShowDesktopNotification { title: String, body: String },
    /// OSC 8: start a hyperlink.
    HyperlinkStart { id: Option<String>, uri: String },
    /// OSC 8: end a hyperlink.
    HyperlinkEnd,
    /// ConEmu OSC 9;1: sleep.
    ConemuSleep { duration_ms: u16 },
    /// ConEmu OSC 9;2: show a GUI message box.
    ConemuShowMessageBox(String),
    /// ConEmu OSC 9;3: change tab title.
    ConemuChangeTabTitle(ConemuChangeTabTitle),
    /// ConEmu OSC 9;4: progress report.
    ConemuProgressReport(ProgressReport),
    /// ConEmu OSC 9;5: wait for input.
    ConemuWaitInput,
    /// ConEmu OSC 9;6: GUI macro.
    ConemuGuimacro(String),
    /// ConEmu OSC 9;7: run a process.
    ConemuRunProcess(String),
    /// ConEmu OSC 9;8: output an environment variable.
    ConemuOutputEnvironmentVariable(String),
    /// ConEmu OSC 9;10: xterm keyboard/output emulation.
    ConemuXtermEmulation {
        keyboard: Option<bool>,
        output: Option<bool>,
    },
    /// ConEmu OSC 9;11: comment.
    ConemuComment(String),
    /// OSC 66: kitty text sizing protocol.
    KittyTextSizing(KittyTextSizing),
    /// OSC 5522: kitty clipboard protocol.
    KittyClipboardProtocol(KittyClipboardProtocol),
    /// OSC 72: kitty drag-and-drop protocol.
    KittyDndProtocol(KittyDndProtocol),
    /// OSC 3008: hierarchical context signalling (UAPI spec).
    ContextSignal(ContextSignal),
}

/// The `9;3` payload: either a reset back to the default tab title, or a
/// new value. Port of `osc9.zig`'s anonymous
/// `union(enum) { reset, value: [:0]const u8 }`.
///
/// Kept as a distinct variant (not `Value(String::new())`) because `9;3;`
/// (trailing `;`, nothing after) is a *reset*, not an empty-string value
/// (`docs/analysis/osc.md` divergence #5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConemuChangeTabTitle {
    Reset,
    Value(String),
}

/// ConEmu OSC 9;4 progress state. Port of `osc.zig` `Command.ProgressReport`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressReport {
    pub state: ProgressState,
    pub progress: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressState {
    Remove,
    Set,
    Error,
    Indeterminate,
    Pause,
}

/// The incremental OSC parser. Port of `osc.zig` `Parser`.
///
/// Drives from the VT parser's raw OSC byte seam: call [`Parser::reset`] on
/// `OscStart`, [`Parser::next`] per `OscPut` byte, and [`Parser::end`] on
/// `OscEnd` (passing the terminating byte from that event) to get a
/// [`Command`].
#[derive(Debug, Default)]
pub struct Parser {
    /// Whether unbounded (allocating) capture is permitted. Mirrors Zig's
    /// `alloc: ?Allocator` — the Rust port's `Vec<u8>`-backed capture
    /// doesn't need a real allocator handle to grow, so this degenerates to
    /// a permission flag (`docs/analysis/osc.md` divergence #3).
    allow_unbounded: bool,
    /// The accumulated OSC body bytes (everything after `ESC ]`).
    buf: Vec<u8>,
    /// Set once `buf` has overflowed a fixed-size (non-unbounded) capture,
    /// or the parser has otherwise determined the sequence is invalid.
    invalid: bool,
}

impl Parser {
    /// Create a parser that only permits fixed-size (2048-byte) capture.
    /// Commands that require unbounded capture (4/5/10-19/21/52/66/72/5522)
    /// will fail to parse. Port of `osc.zig` `Parser.init(null)`.
    pub fn new() -> Parser {
        Parser::default()
    }

    /// Create a parser that permits unbounded capture. Port of `osc.zig`
    /// `Parser.init(alloc)` with a real allocator.
    pub fn with_allocator() -> Parser {
        Parser {
            allow_unbounded: true,
            ..Parser::default()
        }
    }

    /// Reset the parser state for a new OSC sequence. Port of `osc.zig`
    /// `Parser.reset`.
    pub fn reset(&mut self) {
        self.buf.clear();
        self.invalid = false;
    }

    /// Feed the next byte of the OSC body. Port of `osc.zig` `Parser.next`.
    ///
    /// Unlike Zig's trie (which only starts capturing, and only enforces
    /// `MAX_BUF`, after the numeric OSC prefix has been consumed), this
    /// port buffers the whole body unconditionally and defers the
    /// prefix-aware `MAX_BUF` check to [`Parser::end`] — seeing the
    /// dispatch prefix requires having the bytes first. A generous safety
    /// cap (prefix length can't exceed a handful of bytes) still applies
    /// here so pathological input can't grow this buffer unboundedly when
    /// `allow_unbounded` is false.
    pub fn next(&mut self, c: u8) {
        if self.invalid {
            return;
        }
        if !self.allow_unbounded && self.buf.len() >= MAX_BUF + support::MAX_PREFIX_LEN {
            self.invalid = true;
            return;
        }
        self.buf.push(c);
    }

    /// End the sequence and return the parsed command, if any. Port of
    /// `osc.zig` `Parser.end`.
    ///
    /// `terminator_ch` is the final byte seen (the exact byte the VT
    /// parser's `OscEnd` event carries): `0x07` for BEL, anything else for
    /// ST.
    pub fn end(&mut self, terminator_ch: Option<u8>) -> Option<Command> {
        if self.invalid {
            return None;
        }
        parsers::dispatch(&self.buf, terminator_ch, self.allow_unbounded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{self, Action};

    fn feed(
        vt: &mut parser::Parser,
        osc: &mut Parser,
        body: &str,
        terminator: u8,
    ) -> Option<Command> {
        vt.next(0x1B);
        {
            let a = vt.next(b']');
            assert!(matches!(a[2], Some(Action::OscStart)));
        }
        osc.reset();
        for c in body.bytes() {
            let a = vt.next(c);
            let Some(Action::OscPut(b)) = a[1] else {
                panic!("expected OscPut, got {a:?}");
            };
            osc.next(b);
        }
        let a = vt.next(terminator);
        let Some(Action::OscEnd(term)) = a[0] else {
            panic!("expected OscEnd, got {a:?}");
        };
        osc.end(Some(term))
    }

    #[test]
    fn seam_integration_change_window_title() {
        let mut vt = parser::Parser::new();
        let mut osc = Parser::new();
        let cmd = feed(&mut vt, &mut osc, "0;abc", 0x07);
        assert_eq!(cmd, Some(Command::ChangeWindowTitle("abc".to_string())));
    }

    #[test]
    fn terminator_init() {
        assert_eq!(Terminator::init(Some(0x07)), Terminator::Bel);
        assert_eq!(Terminator::init(Some(0x1B)), Terminator::St);
        assert_eq!(Terminator::init(None), Terminator::St);
    }

    #[test]
    fn buffer_overflow_without_allocator_invalidates() {
        let mut p = Parser::new();
        for c in b"0;" {
            p.next(*c);
        }
        for _ in 0..(MAX_BUF + 10) {
            p.next(b'a');
        }
        assert_eq!(p.end(None), None);
    }
}
