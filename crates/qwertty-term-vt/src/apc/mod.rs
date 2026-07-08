//! APC (Application Program Command) handler.
//!
//! Port of `src/terminal/apc.zig` (402 lines, ghostty commit `2da015cd6`); see
//! `docs/analysis/dcs-apc.md` for the survey this was ported from.
//!
//! [`Handler`] is meant to be driven by the three APC events the parser emits for
//! APC/SOS/PM strings alike (`docs/analysis/vt-parser.md`, deviation #3):
//! [`Handler::start`] on [`crate::parser::Action::ApcStart`], [`Handler::feed`] per
//! [`crate::parser::Action::ApcPut`] byte, and [`Handler::end`] on
//! [`crate::parser::Action::ApcEnd`].
//!
//! # CRITICAL SEAM
//!
//! Ghostty identifies two APC sub-protocols by their leading bytes (`apc.zig:54-98`):
//!
//! | Trigger | Protocol | Owner |
//! |---|---|---|
//! | `G` as the very first byte | Kitty graphics (`kitty/graphics_*.zig`, ~6.3k lines) | sibling chunk, `crates/qwertty-term-vt/src/kitty/` — `TODO(chunk:kitty-gfx)` |
//! | `25a1;` prefix | Glyph protocol (`apc/glyph/*.zig`, ~2.18k lines, depends on the unported font subsystem) | unassigned — `TODO(chunk:font-glyph-protocol)` |
//!
//! Neither sub-protocol's real command types are ported here. This module ports the
//! **identify state machine faithfully** (the `G` fast path, the 4-byte identify
//! buffer, the `;`-terminated prefix match, per-protocol enable/disable, and
//! `max_bytes` enforcement/error-to-ignore policy) but stands in a **raw byte buffer**
//! for each recognized sub-protocol's payload instead of invoking a real parser. See
//! [`Command::KittyRaw`] and [`Command::GlyphRaw`]. A sibling/future chunk swaps the
//! buffer-push in [`Handler::feed`] for a call into its real incremental parser, and
//! the raw-bytes-return in [`Handler::end`] for a call to its `complete`; no other
//! part of this module changes.

/// A completed APC command (mirrors ghostty's `Command` union, `apc.zig:213-231`).
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Kitty graphics protocol. `TODO(chunk:kitty-gfx)`: replace with the real
    /// parsed `kitty_gfx.Command`; today this is the raw payload bytes fed after
    /// the identifying `G`.
    KittyRaw(Vec<u8>),

    /// Glyph protocol. `TODO(chunk:font-glyph-protocol)`: replace with the real
    /// parsed `glyph.Request`; today this is the raw payload bytes fed after the
    /// identifying `25a1;` prefix (verb + options + payload, semicolon-joined,
    /// exactly as received).
    GlyphRaw(Vec<u8>),
}

/// Protocols recognized by the APC handler (mirrors `Protocol`, `apc.zig:194-210`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    Kitty,
    Glyph,
}

impl Protocol {
    /// Default maximum bytes for the given protocol (`Protocol.defaultMaxBytes`,
    /// `apc.zig:199-209`).
    pub const fn default_max_bytes(self) -> usize {
        match self {
            // Kitty graphics payloads can be very large (e.g. full images encoded
            // as base64), so the default is set to 65 MiB.
            Protocol::Kitty => 65 * 1024 * 1024,
            // Glyph protocol messages carry single glyf outlines which are small,
            // but base64 encoding inflates them. 1 MiB is generous for any single
            // simple-glyph record.
            Protocol::Glyph => 1024 * 1024,
        }
    }
}

/// Per-protocol max-bytes overrides (mirrors `Handler.max_bytes:
/// std.EnumMap(Protocol, usize)`, `apc.zig:20-23`).
#[derive(Debug, Clone, Copy)]
struct MaxBytes {
    kitty: usize,
    glyph: usize,
}

impl Default for MaxBytes {
    fn default() -> Self {
        Self {
            kitty: Protocol::Kitty.default_max_bytes(),
            glyph: Protocol::Glyph.default_max_bytes(),
        }
    }
}

impl MaxBytes {
    fn get(&self, protocol: Protocol) -> usize {
        match protocol {
            Protocol::Kitty => self.kitty,
            Protocol::Glyph => self.glyph,
        }
    }
}

/// Per-protocol enabled flags (mirrors `Handler.enabled: std.EnumSet(Protocol)`,
/// default all-enabled, `apc.zig:28`).
#[derive(Debug, Clone, Copy)]
struct Enabled {
    kitty: bool,
    glyph: bool,
}

impl Default for Enabled {
    fn default() -> Self {
        Self {
            kitty: true,
            glyph: true,
        }
    }
}

impl Enabled {
    fn get(&self, protocol: Protocol) -> bool {
        match protocol {
            Protocol::Kitty => self.kitty,
            Protocol::Glyph => self.glyph,
        }
    }

    fn set(&mut self, protocol: Protocol, value: bool) {
        match protocol {
            Protocol::Kitty => self.kitty = value,
            Protocol::Glyph => self.glyph = value,
        }
    }
}

/// Length of the identify buffer (mirrors `identify.buf: [4]u8`, `apc.zig:169`).
const IDENTIFY_BUF_LEN: usize = 4;

/// Internal APC handler state (mirrors ghostty's `State` union, `apc.zig:150-191`).
#[derive(Debug)]
enum State {
    /// Not in the middle of an APC command yet. Feeding a byte in this state is a
    /// caller bug (ghostty marks it `unreachable`, `apc.zig:47`); we mirror that
    /// with a debug assertion in `feed` rather than modeling it in the enum.
    Inactive,

    /// Unrecognized (or since-invalidated) APC sequence -- dropping bytes.
    Ignore,

    /// Waiting to identify the APC sequence (`apc.zig:158-170`).
    Identify {
        len: u8,
        buf: [u8; IDENTIFY_BUF_LEN],
    },

    /// Kitty graphics protocol. Seam: raw byte buffer standing in for
    /// `kitty_gfx.CommandParser`. `TODO(chunk:kitty-gfx)`. `in_data` tracks
    /// whether we've crossed into the payload-data section (after the control
    /// key=value list's terminating `;`) -- the real parser's `max_bytes` only
    /// bounds that section (`graphics_command.zig:103-145`: `control_key`/
    /// `control_value` states are unbounded, only the `.data` state checks
    /// `self.data.items.len >= self.max_bytes`), which the "kitty max bytes
    /// exceeded" test relies on.
    Kitty {
        data: Vec<u8>,
        max_bytes: usize,
        in_data: bool,
        data_len: usize,
    },

    /// Glyph protocol. Seam: raw byte buffer standing in for `glyph.CommandParser`.
    /// `TODO(chunk:font-glyph-protocol)`.
    Glyph { data: Vec<u8>, max_bytes: usize },
}

/// APC command handler. This should be hooked into a terminal stream handler; the
/// start/feed/end methods are meant to be called from the APC parser events
/// (`apc.zig:10-12`).
#[derive(Debug)]
pub struct Handler {
    state: State,
    max_bytes: MaxBytes,
    enabled: Enabled,
}

impl Default for Handler {
    fn default() -> Self {
        Self::new()
    }
}

impl Handler {
    /// Construct a new, inactive handler with default max-bytes and all protocols
    /// enabled (`apc.zig:14-28`).
    pub const fn new() -> Self {
        Self {
            state: State::Inactive,
            max_bytes: MaxBytes {
                kitty: Protocol::Kitty.default_max_bytes(),
                glyph: Protocol::Glyph.default_max_bytes(),
            },
            enabled: Enabled {
                kitty: true,
                glyph: true,
            },
        }
    }

    /// Override the max-bytes limit for one protocol. Mirrors constructing
    /// ghostty's `Handler` with a custom `max_bytes` map (e.g. the "kitty max bytes
    /// exceeded" test, `apc.zig:301`).
    pub fn set_max_bytes(&mut self, protocol: Protocol, bytes: usize) {
        match protocol {
            Protocol::Kitty => self.max_bytes.kitty = bytes,
            Protocol::Glyph => self.max_bytes.glyph = bytes,
        }
    }

    /// Enable or disable APC protocol recognition for future APC sequences. Does
    /// not affect any APC command already being parsed (`Handler.enable`,
    /// `apc.zig:41-43`).
    pub fn enable(&mut self, protocol: Protocol, enabled: bool) {
        self.enabled.set(protocol, enabled);
    }

    /// Called on APC start (`Handler.start`, `apc.zig:34-37`).
    pub fn start(&mut self) {
        self.state = State::Identify {
            len: 0,
            buf: [0; IDENTIFY_BUF_LEN],
        };
    }

    /// Feed one byte into the APC handler (`Handler.feed`, `apc.zig:45-114`).
    pub fn feed(&mut self, byte: u8) {
        match &mut self.state {
            State::Inactive => debug_assert!(false, "feed called before start"),

            // Ignoring this APC command -- no need to store the data.
            State::Ignore => {}

            State::Identify { len, buf } => {
                // Kitty graphics is detected immediately on the `G` byte, since
                // commands begin immediately after with no termination character
                // (apc.zig:58-70).
                if *len == 0 && byte == b'G' && self.enabled.get(Protocol::Kitty) {
                    self.state = State::Kitty {
                        data: Vec::new(),
                        max_bytes: self.max_bytes.get(Protocol::Kitty),
                        in_data: false,
                        data_len: 0,
                    };
                    return;
                }

                // On `;`, the accumulated prefix identifies the protocol
                // (apc.zig:72-88).
                if byte == b';' {
                    let prefix = &buf[..*len as usize];
                    if prefix == b"25a1" && self.enabled.get(Protocol::Glyph) {
                        self.state = State::Glyph {
                            data: Vec::new(),
                            max_bytes: self.max_bytes.get(Protocol::Glyph),
                        };
                    } else {
                        self.state = State::Ignore;
                    }
                    return;
                }

                // Out of space to buffer -- done (apc.zig:90-94).
                if *len as usize >= buf.len() {
                    self.state = State::Ignore;
                    return;
                }

                buf[*len as usize] = byte;
                *len += 1;
            }

            State::Kitty {
                data,
                max_bytes,
                in_data,
                data_len,
            } => {
                // TODO(chunk:kitty-gfx): forward to the real kitty graphics
                // incremental parser instead of buffering raw bytes. Until then,
                // mirror its section boundary: control key=value pairs (before
                // the first top-level `;`) are unbounded; only the payload-data
                // section afterward is subject to `max_bytes`
                // (graphics_command.zig:103-145).
                if !*in_data {
                    data.push(byte);
                    if byte == b';' {
                        *in_data = true;
                    }
                    return;
                }
                if *data_len >= *max_bytes {
                    self.state = State::Ignore;
                    return;
                }
                data.push(byte);
                *data_len += 1;
            }

            State::Glyph { data, max_bytes } => {
                // TODO(chunk:font-glyph-protocol): forward to the real glyph
                // protocol command parser instead of buffering raw bytes.
                if data.len() >= *max_bytes {
                    self.state = State::Ignore;
                    return;
                }
                data.push(byte);
            }
        }
    }

    /// Called on APC end (`Handler.end`, `apc.zig:116-147`). Always resets to
    /// inactive afterward.
    pub fn end(&mut self) -> Option<Command> {
        let state = std::mem::replace(&mut self.state, State::Inactive);
        match state {
            State::Inactive => {
                debug_assert!(false, "end called before start");
                None
            }
            State::Ignore | State::Identify { .. } => None,
            State::Kitty { data, .. } => Some(Command::KittyRaw(data)),
            State::Glyph { data, .. } => Some(Command::GlyphRaw(data)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Port of `apc.zig:233-241`, "unknown APC command".
    #[test]
    fn unknown_apc_command() {
        let mut h = Handler::new();
        h.start();
        for byte in b"Xabcdef1234" {
            h.feed(*byte);
        }
        assert!(h.end().is_none());
    }

    /// Port of `apc.zig:243-253`, "garbage Kitty command". Ghostty skips this test
    /// when `kitty_graphics` is compiled out; that build option is unconditionally
    /// true on real targets, so the seam's raw-buffer stand-in exercises the same
    /// identify behavior.
    #[test]
    fn garbage_kitty_command() {
        let mut h = Handler::new();
        h.start();
        for byte in b"Gabcdef1234" {
            h.feed(*byte);
        }
        // The seam always succeeds in identifying + buffering "G..." (no parse
        // validation happens until the real kitty parser lands), so `end` yields
        // the raw payload rather than `None`. This is a deliberate divergence from
        // ghostty's fully-parsed behavior, documented at the TODO(chunk:kitty-gfx)
        // seam.
        let cmd = h.end();
        assert_eq!(cmd, Some(Command::KittyRaw(b"abcdef1234".to_vec())));
    }

    /// Port of `apc.zig:255-265`, "Kitty command with overflow u32". Seam note as
    /// above: overflow validation belongs to the real kitty parser
    /// (`TODO(chunk:kitty-gfx)`), so this only exercises that the seam buffers the
    /// bytes without erroring.
    #[test]
    fn kitty_command_with_overflow_u32() {
        let mut h = Handler::new();
        h.start();
        for byte in b"Ga=p,i=10000000000" {
            h.feed(*byte);
        }
        let cmd = h.end();
        assert_eq!(cmd, Some(Command::KittyRaw(b"a=p,i=10000000000".to_vec())));
    }

    /// Port of `apc.zig:267-277`, "Kitty command with overflow i32". Seam note as
    /// above.
    #[test]
    fn kitty_command_with_overflow_i32() {
        let mut h = Handler::new();
        h.start();
        for byte in b"Ga=p,i=1,z=-9999999999" {
            h.feed(*byte);
        }
        let cmd = h.end();
        assert_eq!(
            cmd,
            Some(Command::KittyRaw(b"a=p,i=1,z=-9999999999".to_vec()))
        );
    }

    /// Port of `apc.zig:279-293`, "kitty feed error deinits parser". The real test
    /// exercises the kitty parser's own integer-overflow error path
    /// (`TODO(chunk:kitty-gfx)`, not present in the seam). Ported instead against
    /// the identify-time analog available today: overflowing the identify buffer
    /// falls back to `.ignore`, matching ghostty's error->ignore policy.
    #[test]
    fn feed_error_falls_back_to_ignore() {
        let mut h = Handler::new();
        h.start();
        for byte in b"abcde;payload" {
            h.feed(*byte);
        }
        assert!(matches!(h.state, State::Ignore));
    }

    /// Port of `apc.zig:295-312`, "kitty max bytes exceeded".
    #[test]
    fn kitty_max_bytes_exceeded() {
        let mut h = Handler::new();
        h.set_max_bytes(Protocol::Kitty, 4);
        h.start();
        // 'G' identifies kitty, 'a=t;' moves to data state, then feed exceeds
        // max_bytes.
        for byte in b"Ga=t;" {
            h.feed(*byte);
        }
        assert!(!matches!(h.state, State::Ignore));
        for byte in b"abcd" {
            h.feed(*byte);
        }
        assert!(!matches!(h.state, State::Ignore));
        // The 5th data byte exceeds the 4-byte limit.
        h.feed(b'e');
        assert!(matches!(h.state, State::Ignore));
    }

    /// Port of `apc.zig:314-328`, "valid Kitty command". Ported against the seam's
    /// `KittyRaw` payload (`TODO(chunk:kitty-gfx)`) rather than a parsed command.
    #[test]
    fn valid_kitty_command() {
        let mut h = Handler::new();
        h.start();
        let input = b"Gf=24,s=10,v=20,hello=world";
        for byte in input {
            h.feed(*byte);
        }
        let cmd = h.end();
        assert!(matches!(cmd, Some(Command::KittyRaw(_))));
    }

    /// Port of `apc.zig:330-338`, "identify with unrecognized command".
    #[test]
    fn identify_with_unrecognized_command() {
        let mut h = Handler::new();
        h.start();
        for byte in b"abcd;payload" {
            h.feed(*byte);
        }
        assert!(h.end().is_none());
    }

    /// Port of `apc.zig:340-348`, "identify buffer overflow".
    #[test]
    fn identify_buffer_overflow() {
        let mut h = Handler::new();
        h.start();
        for byte in b"abcde;payload" {
            h.feed(*byte);
        }
        assert!(h.end().is_none());
    }

    /// Port of `apc.zig:350-356`, "identify with no input".
    #[test]
    fn identify_with_no_input() {
        let mut h = Handler::new();
        h.start();
        assert!(h.end().is_none());
    }

    /// Port of `apc.zig:358-366`, "identify with unknown partial input".
    #[test]
    fn identify_with_unknown_partial_input() {
        let mut h = Handler::new();
        h.start();
        for byte in b"25a" {
            h.feed(*byte);
        }
        assert!(h.end().is_none());
    }

    /// Port of `apc.zig:368-377`, "garbage glyph command". Ported against the
    /// seam's `GlyphRaw` payload (`TODO(chunk:font-glyph-protocol)`): the real
    /// glyph command parser would reject `"X"` as an unknown verb and yield
    /// `None`, but the seam has no verb validation yet, so it identifies the
    /// `25a1;` prefix and buffers the remaining byte.
    #[test]
    fn garbage_glyph_command() {
        let mut h = Handler::new();
        h.start();
        for byte in b"25a1;X" {
            h.feed(*byte);
        }
        let cmd = h.end();
        assert_eq!(cmd, Some(Command::GlyphRaw(b"X".to_vec())));
    }

    /// Port of `apc.zig:379-391`, "valid glyph command". Ported against the seam's
    /// `GlyphRaw` payload (`TODO(chunk:font-glyph-protocol)`) rather than the real
    /// `Request::query` variant.
    #[test]
    fn valid_glyph_command() {
        let mut h = Handler::new();
        h.start();
        for byte in b"25a1;q;cp=E0A0" {
            h.feed(*byte);
        }
        let cmd = h.end();
        assert_eq!(cmd, Some(Command::GlyphRaw(b"q;cp=E0A0".to_vec())));
    }

    /// Port of `apc.zig:393-402`, "disabled glyph command is ignored".
    #[test]
    fn disabled_glyph_command_is_ignored() {
        let mut h = Handler::new();
        h.enable(Protocol::Glyph, false);
        h.start();
        for byte in b"25a1;q;cp=e0a0" {
            h.feed(*byte);
        }
        assert!(h.end().is_none());
    }
}
