//! DCS (Device Control String) command handler.
//!
//! Port of `src/terminal/dcs.zig` (430 lines, ghostty commit `2da015cd6`); see
//! `docs/analysis/dcs-apc.md` for the survey this was ported from.
//!
//! [`Handler`] is meant to be driven by the three DCS events the parser emits
//! (`docs/analysis/vt-parser.md`, "DCS hook surface"): [`Handler::hook`] on
//! [`crate::parser::Action::DcsHook`], [`Handler::put`] per
//! [`crate::parser::Action::DcsPut`] byte, and [`Handler::unhook`] on
//! [`crate::parser::Action::DcsUnhook`].
//!
//! Ghostty implements exactly three DCS commands, discriminated by
//! intermediates/final byte in `tryHook` (`dcs.zig:50-110`):
//!
//! | Intermediates | Final | Command |
//! |---|---|---|
//! | (none), params `== [1000]` | `p` | Tmux control mode enter |
//! | `+` | `q` | XTGETTCAP |
//! | `$` | `q` | DECRQSS |
//!
//! Tmux control mode is now wired (ADR 004 slice 4): the DCS `1000p` seam owns a
//! [`ControlParser`] (`crate::tmux`) that decodes control-mode lines into
//! [`crate::tmux::Notification`]s. `hook` emits [`Notification::Enter`], each body byte
//! is fed to the parser (surfacing an owned notification when a line completes), and
//! `unhook` emits [`Notification::Exit`]. This mirrors upstream's `State.tmux` payload
//! (`dcs.zig`), except that upstream keeps the parser inside the `State` union; our
//! `State` is a unit enum, so the parser lives on the [`Handler`]. The native Viewer
//! that maps notifications to surfaces is ADR 004 slice 5 (app-tails), not here.

/// A hooked DCS command (mirrors ghostty's `DCS` struct, `Parser.zig:124-136`, exposed
/// here as [`crate::parser::Dcs`]).
pub use crate::parser::Dcs;

use crate::tmux::{BufferOverflow, ControlParser, Notification};

/// DCS command handler. This should be hooked into a terminal stream handler; the
/// hook/put/unhook methods are meant to be called from the DCS parser events
/// (`dcs.zig:10-12`).
#[derive(Debug)]
pub struct Handler {
    state: State,

    /// Maximum bytes any DCS command can take, to prevent malicious input from
    /// allocating unbounded memory. Arbitrarily 1 MiB, matching ghostty
    /// (`dcs.zig:16-19`). Applies to XTGETTCAP; DECRQSS has its own fixed 2-byte cap
    /// and tmux control mode manages its own buffering (in `tmux_parser`).
    max_bytes: usize,

    /// The tmux control-mode parser, present only while in [`State::Tmux`] (set on
    /// `hook` of `ESC P 1000 p`, cleared on `unhook`). The Rust analog of upstream's
    /// `State.tmux` payload (`dcs.zig`): our `State` is a unit enum, so the parser
    /// sits on the [`Handler`] instead of inside the state variant.
    tmux_parser: Option<ControlParser>,
}

impl Default for Handler {
    fn default() -> Self {
        Self::new()
    }
}

impl Handler {
    /// Construct a new, inactive handler with the default 1 MiB `max_bytes`
    /// (`dcs.zig:19`).
    pub const fn new() -> Self {
        Self {
            state: State::Inactive,
            max_bytes: 1024 * 1024,
            tmux_parser: None,
        }
    }

    /// Handle a DCS hook (`Handler.hook`, `dcs.zig:25-43`). Ghostty asserts
    /// `state == .inactive` on entry -- the parser guarantees unhook precedes the next
    /// hook -- which we mirror with a debug assertion. Returns a command only for the
    /// tmux-enter case; every other recognized command produces its result at
    /// [`Handler::unhook`] instead.
    pub fn hook(&mut self, dcs: Dcs<'_>) -> Option<Command> {
        debug_assert!(matches!(self.state, State::Inactive));

        match try_hook(dcs) {
            Some(hook) => {
                self.state = hook.state;
                // Entering tmux control mode: spin up the control-mode parser. The
                // Rust analog of upstream initializing the `State.tmux` payload's
                // buffer (`dcs.zig`), kept on the Handler since our `State` is unit.
                if matches!(self.state, State::Tmux) {
                    self.tmux_parser = Some(ControlParser::new());
                }
                hook.command
            }
            None => {
                self.state = State::Ignore;
                None
            }
        }
    }

    /// Put a byte into the DCS handler (`Handler.put`, `dcs.zig:112-155`). Returns a
    /// command if one needs to be executed (only ever the case for tmux control-mode
    /// lines, which are seamed -- see module docs).
    pub fn put(&mut self, byte: u8) -> Option<Command> {
        match self.try_put(byte) {
            Ok(cmd) => cmd,
            Err(()) => {
                // On error we discard state and ignore the rest, matching
                // ghostty's catch-all error policy (dcs.zig:115-121).
                self.state = State::Ignore;
                None
            }
        }
    }

    fn try_put(&mut self, byte: u8) -> Result<Option<Command>, ()> {
        match &mut self.state {
            State::Inactive | State::Ignore => Ok(None),

            State::Tmux => {
                // Forward each body byte to the control-mode parser (upstream
                // `tmux.put(byte)`, `dcs.zig:130-134`), surfacing an owned
                // notification when a line completes.
                //
                // Divergence: upstream propagates the parser's error out of `tryPut`,
                // which `put` catches and turns into a state discard. Our
                // `ControlParser` breaks itself internally on overflow (and returns
                // `Ok(None)` thereafter), so `Err(BufferOverflow)` here just maps to
                // `None` — the parser is already broken and drops the rest. This
                // deliberately avoids a panic/error path for a malicious byte flood
                // while keeping the observable behaviour (silent drop) identical.
                let Some(parser) = self.tmux_parser.as_mut() else {
                    return Ok(None);
                };
                match parser.put(byte) {
                    Ok(Some(n)) => Ok(Some(Command::Tmux(n))),
                    Ok(None) => Ok(None),
                    Err(BufferOverflow) => Ok(None),
                }
            }

            State::XtGetTcap(buf) => {
                if buf.len() >= self.max_bytes {
                    return Err(());
                }
                buf.push(byte);
                Ok(None)
            }

            State::Decrqss { data, len } => {
                if *len as usize >= data.len() {
                    return Err(());
                }
                data[*len as usize] = byte;
                *len += 1;
                Ok(None)
            }
        }
    }

    /// Handle DCS unhook (`Handler.unhook`, `dcs.zig:157-199`). Always resets to
    /// inactive afterward.
    pub fn unhook(&mut self) -> Option<Command> {
        let state = std::mem::replace(&mut self.state, State::Inactive);
        match state {
            State::Inactive | State::Ignore => None,

            State::Tmux => {
                // Tear down the control-mode parser and emit the exit lifecycle
                // event (upstream `dcs.zig:168-170`, `.tmux = .exit`).
                self.tmux_parser = None;
                Some(Command::Tmux(Notification::Exit))
            }

            State::XtGetTcap(mut data) => {
                // Upper-case every buffered byte in place (dcs.zig:177): XTGETTCAP
                // names are always the hex-encoded uppercase form regardless of the
                // case the client sent.
                for b in data.iter_mut() {
                    b.make_ascii_uppercase();
                }
                Some(Command::XtGetTcap(XtGetTcap { data, pos: 0 }))
            }

            State::Decrqss { data, len } => {
                let decrqss = match len {
                    0 => Decrqss::None,
                    1 => match data[0] {
                        b'm' => Decrqss::Sgr,
                        b'r' => Decrqss::Decstbm,
                        b's' => Decrqss::Decslrm,
                        _ => Decrqss::None,
                    },
                    2 => match (data[0], data[1]) {
                        (b' ', b'q') => Decrqss::Decscusr,
                        _ => Decrqss::None,
                    },
                    _ => unreachable!("DECRQSS buffer caps at 2 bytes"),
                };
                Some(Command::Decrqss(decrqss))
            }
        }
    }

    /// Discard any in-progress DCS state (`Handler.discard`, `dcs.zig:201-204`).
    /// Ghostty's `deinit` calls this; in Rust the buffers drop themselves, so this is
    /// just a state reset, kept for API parity and explicit call sites.
    pub fn discard(&mut self) {
        // Free any tmux parser too (upstream `discard` calls `state.deinit()`,
        // which releases the `State.tmux` buffer).
        self.tmux_parser = None;
        self.state = State::Inactive;
    }
}

/// Result of [`try_hook`]: the new state plus an optional immediate command (mirrors
/// ghostty's private `Hook` struct, `dcs.zig:45-48`).
struct Hook {
    state: State,
    command: Option<Command>,
}

/// Classify a DCS hook by intermediates/params/final byte (`tryHook`,
/// `dcs.zig:50-110`). Returns `None` for anything unrecognized.
fn try_hook(dcs: Dcs<'_>) -> Option<Hook> {
    match dcs.intermediates {
        [] => match dcs.final_byte {
            // Tmux control mode: `ESC P 1000 p`, no intermediates, exactly one
            // param equal to 1000 (dcs.zig:53-75).
            b'p' => {
                if dcs.params != [1000] {
                    return None;
                }
                Some(Hook {
                    state: State::Tmux,
                    command: Some(Command::Tmux(Notification::Enter)),
                })
            }
            _ => None,
        },

        [b'+'] => match dcs.final_byte {
            // XTGETTCAP: `ESC P + q <hex-encoded-names> ESC \` (dcs.zig:82-90).
            b'q' => Some(Hook {
                state: State::XtGetTcap(Vec::with_capacity(128)),
                command: None,
            }),
            _ => None,
        },

        [b'$'] => match dcs.final_byte {
            // DECRQSS: `ESC P $ q <setting> ESC \` (dcs.zig:96-103).
            b'q' => Some(Hook {
                state: State::Decrqss {
                    data: [0; 2],
                    len: 0,
                },
                command: None,
            }),
            _ => None,
        },

        _ => None,
    }
}

/// Internal DCS handler state (mirrors ghostty's `State` union, `dcs.zig:260-296`).
#[derive(Debug)]
enum State {
    /// Not in a DCS state at the moment.
    Inactive,

    /// Hooked, but an unknown DCS command or one that went invalid due to bad
    /// input -- ignoring the rest.
    Ignore,

    /// XTGETTCAP: growable byte buffer.
    XtGetTcap(Vec<u8>),

    /// DECRQSS: fixed 2-byte buffer.
    Decrqss { data: [u8; 2], len: u8 },

    /// Tmux control mode. The control-mode parser lives in the Handler's
    /// `tmux_parser` field (our `State` is a unit enum); see module docs.
    Tmux,
}

/// A completed DCS command (mirrors ghostty's `Command` union, `dcs.zig:207-258`).
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// XTGETTCAP
    XtGetTcap(XtGetTcap),

    /// DECRQSS
    Decrqss(Decrqss),

    /// Tmux control mode. Carries a decoded [`crate::tmux::Notification`]: the
    /// enter/exit lifecycle plus every control-mode line (`%output`, `%begin`/`%end`
    /// blocks, `%window-add`, `%layout-change`, …) parsed by the [`ControlParser`].
    /// Mirrors upstream's `Command.tmux` (`dcs.zig`).
    Tmux(Notification),
}

/// XTGETTCAP command payload (mirrors `Command.XTGETTCAP`, `dcs.zig:228-248`).
#[derive(Debug, PartialEq, Eq)]
pub struct XtGetTcap {
    data: Vec<u8>,
    pos: usize,
}

impl XtGetTcap {
    /// Returns the next terminfo key being requested, or `None` when there are no
    /// more keys. The returned value is NOT hex-decoded -- ghostty expects a comptime
    /// lookup table keyed by the raw hex string (`dcs.zig:232-247`).
    ///
    /// Named `next_key` rather than `next` (ghostty's `XTGETTCAP.next`,
    /// `dcs.zig:235`) to avoid colliding with `std::iter::Iterator::next` in Rust.
    pub fn next_key(&mut self) -> Option<&[u8]> {
        if self.pos >= self.data.len() {
            return None;
        }
        let rem = &self.data[self.pos..];
        let idx = rem.iter().position(|&b| b == b';').unwrap_or(rem.len());
        // Note: if we're at the end, idx + 1 is len + 1 so we're over the end, but
        // that's fine because the check above is `>=` so we never read past it
        // (dcs.zig:241-243).
        self.pos += idx + 1;
        Some(&rem[..idx])
    }
}

/// Supported DECRQSS settings (mirrors `Command.DECRQSS`, `dcs.zig:250-257`). Ghostty
/// currently recognizes exactly these four settings; anything else hooks successfully
/// but reports `None` at unhook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decrqss {
    None,
    Sgr,
    Decscusr,
    Decstbm,
    Decslrm,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dcs<'a>(intermediates: &'a [u8], params: &'a [u16], final_byte: u8) -> Dcs<'a> {
        Dcs {
            intermediates,
            params,
            final_byte,
        }
    }

    /// Port of `dcs.zig:298-308`, "unknown DCS command".
    #[test]
    fn unknown_dcs_command() {
        let mut h = Handler::new();
        assert!(h.hook(dcs(&[], &[], b'A')).is_none());
        assert!(matches!(h.state, State::Ignore));
        assert!(h.unhook().is_none());
        assert!(matches!(h.state, State::Inactive));
    }

    /// Port of `dcs.zig:310-323`, "XTGETTCAP command".
    #[test]
    fn xtgettcap_command() {
        let mut h = Handler::new();
        assert!(h.hook(dcs(b"+", &[], b'q')).is_none());
        for byte in b"536D756C78" {
            h.put(*byte);
        }
        let Some(Command::XtGetTcap(mut cmd)) = h.unhook() else {
            panic!("expected xtgettcap command");
        };
        assert_eq!(cmd.next_key(), Some(&b"536D756C78"[..]));
        assert_eq!(cmd.next_key(), None);
    }

    /// Port of `dcs.zig:325-338`, "XTGETTCAP mixed case".
    #[test]
    fn xtgettcap_mixed_case() {
        let mut h = Handler::new();
        assert!(h.hook(dcs(b"+", &[], b'q')).is_none());
        for byte in b"536d756C78" {
            h.put(*byte);
        }
        let Some(Command::XtGetTcap(mut cmd)) = h.unhook() else {
            panic!("expected xtgettcap command");
        };
        assert_eq!(cmd.next_key(), Some(&b"536D756C78"[..]));
        assert_eq!(cmd.next_key(), None);
    }

    /// Port of `dcs.zig:340-354`, "XTGETTCAP command multiple keys".
    #[test]
    fn xtgettcap_command_multiple_keys() {
        let mut h = Handler::new();
        assert!(h.hook(dcs(b"+", &[], b'q')).is_none());
        for byte in b"536D756C78;536D756C78" {
            h.put(*byte);
        }
        let Some(Command::XtGetTcap(mut cmd)) = h.unhook() else {
            panic!("expected xtgettcap command");
        };
        assert_eq!(cmd.next_key(), Some(&b"536D756C78"[..]));
        assert_eq!(cmd.next_key(), Some(&b"536D756C78"[..]));
        assert_eq!(cmd.next_key(), None);
    }

    /// Port of `dcs.zig:356-370`, "XTGETTCAP command invalid data".
    #[test]
    fn xtgettcap_command_invalid_data() {
        let mut h = Handler::new();
        assert!(h.hook(dcs(b"+", &[], b'q')).is_none());
        for byte in b"who;536D756C78" {
            h.put(*byte);
        }
        let Some(Command::XtGetTcap(mut cmd)) = h.unhook() else {
            panic!("expected xtgettcap command");
        };
        assert_eq!(cmd.next_key(), Some(&b"WHO"[..]));
        assert_eq!(cmd.next_key(), Some(&b"536D756C78"[..]));
        assert_eq!(cmd.next_key(), None);
    }

    /// Port of `dcs.zig:372-384`, "DECRQSS command".
    #[test]
    fn decrqss_command() {
        let mut h = Handler::new();
        assert!(h.hook(dcs(b"$", &[], b'q')).is_none());
        h.put(b'm');
        let Some(Command::Decrqss(setting)) = h.unhook() else {
            panic!("expected decrqss command");
        };
        assert_eq!(setting, Decrqss::Sgr);
    }

    /// Port of `dcs.zig:386-406`, "DECRQSS invalid command".
    #[test]
    fn decrqss_invalid_command() {
        let mut h = Handler::new();
        assert!(h.hook(dcs(b"$", &[], b'q')).is_none());
        h.put(b'z');
        let Some(Command::Decrqss(setting)) = h.unhook() else {
            panic!("expected decrqss command");
        };
        assert_eq!(setting, Decrqss::None);

        h.discard();

        assert!(h.hook(dcs(b"$", &[], b'q')).is_none());
        h.put(b'"');
        h.put(b' ');
        // 3rd put overflows the 2-byte buffer -> discard -> ignore.
        h.put(b'q');
        assert!(h.unhook().is_none());
    }

    /// Port of `dcs.zig:408-430`, "tmux enter and implicit exit". Now backed by the
    /// real control-mode parser (ADR 004 slice 4): `hook` emits `Notification::Enter`,
    /// `unhook` emits `Notification::Exit`.
    #[test]
    fn tmux_enter_and_implicit_exit() {
        let mut h = Handler::new();

        let cmd = h.hook(dcs(&[], &[1000], b'p')).unwrap();
        assert_eq!(cmd, Command::Tmux(Notification::Enter));
        assert!(h.tmux_parser.is_some());

        let cmd = h.unhook().unwrap();
        assert_eq!(cmd, Command::Tmux(Notification::Exit));
        assert!(h.tmux_parser.is_none());
    }

    /// A tmux `%output` control line fed through the DCS body surfaces a decoded
    /// [`Notification::Output`]. Confirms the `1000p` seam actually drives the
    /// control-mode parser (ADR 004 slice 4).
    #[test]
    fn tmux_output_line_decoded() {
        let mut h = Handler::new();

        assert_eq!(
            h.hook(dcs(&[], &[1000], b'p')),
            Some(Command::Tmux(Notification::Enter))
        );

        // Feed `%output %1 hi\n`; only the terminating newline completes the line.
        let mut got = None;
        for &byte in b"%output %1 hi\n" {
            if let Some(cmd) = h.put(byte) {
                assert!(got.is_none(), "expected exactly one notification");
                got = Some(cmd);
            }
        }
        assert_eq!(
            got,
            Some(Command::Tmux(Notification::Output {
                pane_id: 1,
                data: b"hi".to_vec(),
            }))
        );

        assert_eq!(h.unhook(), Some(Command::Tmux(Notification::Exit)));
    }

    /// A non-`%` first byte breaks the control-mode parser, which reports an early
    /// exit; the parser then drops the rest until `unhook`.
    #[test]
    fn tmux_broken_line_reports_exit() {
        let mut h = Handler::new();
        h.hook(dcs(&[], &[1000], b'p'));

        // First body byte is not `%` → parser breaks and returns an Exit.
        assert_eq!(h.put(b'x'), Some(Command::Tmux(Notification::Exit)));
        // Broken parser drops subsequent bytes.
        assert_eq!(h.put(b'y'), None);
        assert_eq!(h.put(b'\n'), None);

        // The DCS unhook still emits its own lifecycle Exit.
        assert_eq!(h.unhook(), Some(Command::Tmux(Notification::Exit)));
    }
}
