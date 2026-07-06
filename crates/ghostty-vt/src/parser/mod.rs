//! VT-series parser for escape and control sequences.
//!
//! Ported from ghostty `src/terminal/Parser.zig` (commit `2da015cd6`); see
//! `docs/analysis/vt-parser.md` for the survey. This is implemented directly
//! as the state machine described on vt100.net:
//! <https://vt100.net/emu/dec_ansi_parser>
//!
//! One deliberate seam difference from the Zig source: ghostty's `Parser`
//! embeds an `osc.Parser` and emits a structured `osc_dispatch` action. The
//! OSC command parser (osc.zig + osc/parsers/) is a separate upcoming chunk,
//! so this port emits raw OSC byte events instead — [`Action::OscStart`] on
//! entry, [`Action::OscPut`] per byte, and [`Action::OscEnd`] carrying the
//! terminating byte on exit — mirroring ghostty's own APC (start/put/end) and
//! DCS (hook/put/unhook) surfaces. The structured port slots in later by
//! replacing exactly these three emission sites; no state transition changes.

mod table;

use table::{TABLE, TransitionAction};

/// Maximum number of intermediate characters during parsing. This is 4
/// because ghostty also uses the intermediates array for UTF8 decoding
/// which can be at most 4 bytes (Parser.zig:187-190).
pub const MAX_INTERMEDIATE: usize = 4;

/// Maximum number of CSI parameters. This is arbitrary. Practically, the
/// only CSI command that uses more than 3 parameters is the SGR command
/// which can be infinitely long. 24 is a reasonable limit based on
/// empirical data (Parser.zig:192-203; raised from 16 for a 17-param
/// Kakoune SGR).
pub const MAX_PARAMS: usize = 24;

/// States for the state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Ground = 0,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiIntermediate,
    CsiParam,
    CsiIgnore,
    DcsEntry,
    DcsParam,
    DcsIntermediate,
    DcsPassthrough,
    DcsIgnore,
    OscString,
    SosPmApcString,
}

/// The separator used for a CSI param.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sep {
    Semicolon,
    Colon,
}

/// The list of separators used for CSI params. The bit at index `i`
/// specifies the separator AFTER param `i`. For example `0;4:3` has bit 1
/// set. Mirrors `Action.CSI.SepList` (`StaticBitSet(MAX_PARAMS)`,
/// Parser.zig:85-93).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SepList(u32);

impl SepList {
    /// The empty separator list (all semicolons).
    pub const EMPTY: SepList = SepList(0);

    fn set(&mut self, index: usize) {
        debug_assert!(index < MAX_PARAMS);
        self.0 |= 1 << index;
    }

    /// Whether the separator after param `index` was a colon.
    pub const fn is_set(self, index: usize) -> bool {
        self.0 & (1 << index) != 0
    }

    /// The separator after param `index`.
    pub const fn sep(self, index: usize) -> Sep {
        if self.is_set(index) {
            Sep::Colon
        } else {
            Sep::Semicolon
        }
    }

    /// Number of colon separators.
    pub const fn count(self) -> u32 {
        self.0.count_ones()
    }
}

/// CSI dispatch payload. The slices point into the parser's internal
/// storage and are only valid until the next call to
/// [`Parser::next`] (enforced by the borrow).
#[derive(Debug, PartialEq, Eq)]
pub struct Csi<'a> {
    pub intermediates: &'a [u8],
    pub params: &'a [u16],
    pub params_sep: SepList,
    pub final_byte: u8,
}

/// ESC dispatch payload.
#[derive(Debug, PartialEq, Eq)]
pub struct Esc<'a> {
    pub intermediates: &'a [u8],
    pub final_byte: u8,
}

/// DCS hook payload.
#[derive(Debug, PartialEq, Eq)]
pub struct Dcs<'a> {
    pub intermediates: &'a [u8],
    pub params: &'a [u16],
    pub final_byte: u8,
}

/// Action is the action that a caller of the parser is expected to take
/// as a result of some input byte. Mirrors `Parser.Action`
/// (Parser.zig:49-185), except for the raw OSC events (see module docs).
#[derive(Debug, PartialEq, Eq)]
pub enum Action<'a> {
    /// Draw character to the screen. The parser itself only ever emits
    /// ASCII here (`0x20..=0x7F`); the stream layer synthesizes prints
    /// for decoded UTF-8 codepoints.
    Print(char),

    /// Execute the C0 or C1 function.
    Execute(u8),

    /// Execute the CSI command.
    CsiDispatch(Csi<'a>),

    /// Execute the ESC command.
    EscDispatch(Esc<'a>),

    /// OSC string started (`ESC ]` / 0x9D). Downstream should reset its
    /// OSC accumulator. (Seam for the structured osc.Command port.)
    OscStart,

    /// One byte of OSC string data.
    OscPut(u8),

    /// OSC string ended; carries the terminating byte exactly as ghostty
    /// passes it to `osc.Parser.end`: 0x07 means BEL-terminated, anything
    /// else (0x1B, 0x18, 0x1A) is treated as ST (osc.zig:263-268).
    OscEnd(u8),

    /// DCS-related events.
    DcsHook(Dcs<'a>),
    DcsPut(u8),
    DcsUnhook,

    /// APC data. Note SOS (`ESC X`) and PM (`ESC ^`) strings also flow
    /// through these events; discrimination is downstream's job.
    ApcStart,
    ApcPut(u8),
    ApcEnd,
}

/// Deferred action tag: `next` performs all mutations first, then
/// materializes borrowed [`Action`] values from these tags.
#[derive(Clone, Copy)]
enum Emit {
    None,
    Print,
    Execute,
    CsiDispatch,
    EscDispatch,
    OscStart,
    OscPut,
    OscEnd,
    DcsHook,
    DcsPut,
    DcsUnhook,
    ApcStart,
    ApcPut,
    ApcEnd,
}

/// The VT parser state machine.
#[derive(Debug)]
pub struct Parser {
    /// Current state of the state machine.
    state: State,

    /// Intermediate tracking.
    intermediates: [u8; MAX_INTERMEDIATE],
    intermediates_idx: u8,

    /// Param tracking, building.
    params: [u16; MAX_PARAMS],
    params_sep: SepList,
    params_idx: u8,
    param_acc: u16,
    param_acc_idx: u8,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub const fn new() -> Self {
        Self {
            state: State::Ground,
            intermediates: [0; MAX_INTERMEDIATE],
            intermediates_idx: 0,
            params: [0; MAX_PARAMS],
            params_sep: SepList::EMPTY,
            params_idx: 0,
            param_acc: 0,
            param_acc_idx: 0,
        }
    }

    /// Current state. Public because the stream layer manipulates the
    /// state directly on its fast paths (stream.zig `handleCodepoint`).
    pub const fn state(&self) -> State {
        self.state
    }

    /// Force the state. See [`Parser::state`].
    pub fn set_state(&mut self, state: State) {
        self.state = state;
    }

    /// Consume the next byte `c` and return the actions to execute. Up to
    /// 3 actions may need to be executed -- in order -- representing the
    /// state exit, transition, and entry actions (Parser.zig:248-311).
    pub fn next(&mut self, c: u8) -> [Option<Action<'_>>; 3] {
        let effect = TABLE[c as usize][self.state as usize];
        let next_state = effect.state;
        let changed = self.state as u8 != next_state as u8;

        // When going from one state to another, the actions take place in
        // this order: (1) exit action from old state, (2) transition
        // action, (3) entry action to new state. All state mutations run
        // in that same order here, *before* the borrowed actions are
        // materialized (same effective order as the Zig source).

        // Exit depends on current state.
        let exit = if !changed {
            Emit::None
        } else {
            match self.state {
                State::OscString => Emit::OscEnd,
                State::DcsPassthrough => Emit::DcsUnhook,
                State::SosPmApcString => Emit::ApcEnd,
                _ => Emit::None,
            }
        };

        let transition = self.do_action(effect.action, c);

        // Entry depends on new state.
        let entry = if !changed {
            Emit::None
        } else {
            match next_state {
                State::Escape | State::DcsEntry | State::CsiEntry => {
                    self.clear();
                    Emit::None
                }
                // Ghostty resets its embedded osc.Parser here; the raw
                // seam surfaces that as an explicit OscStart event.
                State::OscString => Emit::OscStart,
                State::DcsPassthrough => {
                    // Ignore too many parameters. Note the state change
                    // still happens, so puts/unhook follow without a hook
                    // (Parser.zig:291-306).
                    if self.params_idx as usize >= MAX_PARAMS {
                        Emit::None
                    } else {
                        // Finalize parameters
                        if self.param_acc_idx > 0 {
                            self.params[self.params_idx as usize] = self.param_acc;
                            self.params_idx += 1;
                        }
                        Emit::DcsHook
                    }
                }
                State::SosPmApcString => Emit::ApcStart,
                _ => Emit::None,
            }
        };

        self.state = next_state;

        [
            self.build(exit, c),
            self.build(transition, c),
            self.build(entry, c),
        ]
    }

    /// Collect an intermediate byte. Excess intermediates are silently
    /// dropped (no Williams-style ignore flag; the dispatch still fires
    /// with the first `MAX_INTERMEDIATE` bytes) -- Parser.zig:313-322.
    pub fn collect(&mut self, c: u8) {
        if self.intermediates_idx as usize >= MAX_INTERMEDIATE {
            // cold: invalid intermediates count
            return;
        }

        self.intermediates[self.intermediates_idx as usize] = c;
        self.intermediates_idx += 1;
    }

    fn do_action(&mut self, action: TransitionAction, c: u8) -> Emit {
        match action {
            TransitionAction::None | TransitionAction::Ignore => Emit::None,
            TransitionAction::Print => Emit::Print,
            TransitionAction::Execute => Emit::Execute,
            TransitionAction::Collect => {
                self.collect(c);
                Emit::None
            }
            TransitionAction::Param => {
                // Semicolon and colon separate parameters. The table only
                // routes '0'..='9', ';', and ':' here.
                if c == b';' || c == b':' {
                    // Ignore too many parameters
                    if self.params_idx as usize >= MAX_PARAMS {
                        return Emit::None;
                    }

                    // Set param final value
                    self.params[self.params_idx as usize] = self.param_acc;
                    if c == b':' {
                        self.params_sep.set(self.params_idx as usize);
                    }
                    self.params_idx += 1;

                    // Reset current param value to 0
                    self.param_acc = 0;
                    self.param_acc_idx = 0;
                    return Emit::None;
                }

                // A numeric value. Add it to our accumulator, saturating
                // like Zig's `*|=` / `+|=`.
                self.param_acc = self
                    .param_acc
                    .saturating_mul(10)
                    .saturating_add((c - b'0') as u16);

                // Increment our accumulator index, wrapping on overflow
                // like Zig's `@addWithOverflow` (Parser.zig:355-358): 256
                // digits wrap the index to 0, which makes the trailing
                // param look unfinalized at dispatch time.
                self.param_acc_idx = self.param_acc_idx.wrapping_add(1);

                Emit::None
            }
            TransitionAction::OscPut => Emit::OscPut,
            TransitionAction::CsiDispatch => {
                // Ignore too many parameters: the *entire* dispatch is
                // dropped (Parser.zig:367-370).
                if self.params_idx as usize >= MAX_PARAMS {
                    return Emit::None;
                }

                // Finalize parameters if we have one
                if self.param_acc_idx > 0 {
                    self.params[self.params_idx as usize] = self.param_acc;
                    self.params_idx += 1;
                }

                // We only allow colon or mixed separators for the 'm'
                // command (Parser.zig:386-394).
                if c != b'm' && self.params_sep.count() > 0 {
                    return Emit::None;
                }

                Emit::CsiDispatch
            }
            TransitionAction::EscDispatch => Emit::EscDispatch,
            TransitionAction::Put => Emit::DcsPut,
            TransitionAction::ApcPut => Emit::ApcPut,
        }
    }

    fn build(&self, emit: Emit, c: u8) -> Option<Action<'_>> {
        Some(match emit {
            Emit::None => return None,
            // The table only routes 0x20..=0x7F here, so `as char` is
            // exact (ASCII).
            Emit::Print => Action::Print(c as char),
            Emit::Execute => Action::Execute(c),
            Emit::CsiDispatch => Action::CsiDispatch(Csi {
                intermediates: &self.intermediates[..self.intermediates_idx as usize],
                params: &self.params[..self.params_idx as usize],
                params_sep: self.params_sep,
                final_byte: c,
            }),
            Emit::EscDispatch => Action::EscDispatch(Esc {
                intermediates: &self.intermediates[..self.intermediates_idx as usize],
                final_byte: c,
            }),
            Emit::OscStart => Action::OscStart,
            Emit::OscPut => Action::OscPut(c),
            Emit::OscEnd => Action::OscEnd(c),
            Emit::DcsHook => Action::DcsHook(Dcs {
                intermediates: &self.intermediates[..self.intermediates_idx as usize],
                params: &self.params[..self.params_idx as usize],
                final_byte: c,
            }),
            Emit::DcsPut => Action::DcsPut(c),
            Emit::DcsUnhook => Action::DcsUnhook,
            Emit::ApcStart => Action::ApcStart,
            Emit::ApcPut => Action::ApcPut(c),
            Emit::ApcEnd => Action::ApcEnd,
        })
    }

    /// Reset intermediate/param collection state (Parser.zig:409-415).
    pub fn clear(&mut self) {
        self.intermediates_idx = 0;
        self.params_idx = 0;
        self.params_sep = SepList::EMPTY;
        self.param_acc = 0;
        self.param_acc_idx = 0;
    }

    /// Assert internal accumulator indices are within bounds. Used by the
    /// fuzz target to check "bounded state" on arbitrary input.
    #[doc(hidden)]
    pub fn assert_bounded(&self) {
        assert!(self.intermediates_idx as usize <= MAX_INTERMEDIATE);
        assert!(self.params_idx as usize <= MAX_PARAMS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csi<'a>(action: &'a Option<Action<'a>>) -> &'a Csi<'a> {
        match action {
            Some(Action::CsiDispatch(csi)) => csi,
            other => panic!("expected csi_dispatch, got {other:?}"),
        }
    }

    // Zig: Parser.zig:417-439 (unnamed test)
    #[test]
    fn anywhere_apc_print_execute() {
        let mut p = Parser::new();
        _ = p.next(0x9E);
        assert_eq!(p.state(), State::SosPmApcString);
        _ = p.next(0x9C);
        assert_eq!(p.state(), State::Ground);

        {
            let a = p.next(b'a');
            assert!(a[0].is_none());
            assert!(matches!(a[1], Some(Action::Print(_))));
            assert!(a[2].is_none());
        }
        assert_eq!(p.state(), State::Ground);

        {
            let a = p.next(0x19);
            assert!(a[0].is_none());
            assert!(matches!(a[1], Some(Action::Execute(_))));
            assert!(a[2].is_none());
        }
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "esc: ESC ( B"
    #[test]
    fn esc_esc_paren_b() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        _ = p.next(b'(');

        let a = p.next(b'B');
        assert!(a[0].is_none());
        assert!(a[2].is_none());
        let Some(Action::EscDispatch(d)) = &a[1] else {
            panic!("expected esc_dispatch, got {a:?}");
        };
        assert_eq!(d.final_byte, b'B');
        assert_eq!(d.intermediates, b"(");
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: ESC [ H"
    #[test]
    fn csi_esc_bracket_h() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        _ = p.next(0x5B);

        let a = p.next(0x48);
        assert!(a[0].is_none());
        assert!(a[2].is_none());
        let d = csi(&a[1]);
        assert_eq!(d.final_byte, 0x48);
        assert_eq!(d.params.len(), 0);
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: ESC [ 1 ; 4 H"
    #[test]
    fn csi_two_params() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        _ = p.next(0x5B);
        _ = p.next(0x31); // 1
        _ = p.next(0x3B); // ;
        _ = p.next(0x34); // 4

        let a = p.next(0x48); // H
        assert!(a[0].is_none());
        assert!(a[2].is_none());
        let d = csi(&a[1]);
        assert_eq!(d.final_byte, b'H');
        assert_eq!(d.params, &[1, 4]);
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: SGR ESC [ 38 : 2 m"
    #[test]
    fn csi_sgr_38_colon_2() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        _ = p.next(b'[');
        _ = p.next(b'3');
        _ = p.next(b'8');
        _ = p.next(b':');
        _ = p.next(b'2');

        let a = p.next(b'm');
        assert!(a[0].is_none());
        assert!(a[2].is_none());
        let d = csi(&a[1]);
        assert_eq!(d.final_byte, b'm');
        assert_eq!(d.params, &[38, 2]);
        assert!(d.params_sep.is_set(0));
        assert!(!d.params_sep.is_set(1));
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: SGR colon followed by semicolon"
    #[test]
    fn csi_sgr_colon_followed_by_semicolon() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        for c in "[48:2".bytes() {
            let a = p.next(c);
            assert!(a.iter().all(Option::is_none));
        }

        {
            let a = p.next(b'm');
            assert!(a[0].is_none());
            assert!(matches!(a[1], Some(Action::CsiDispatch(_))));
            assert!(a[2].is_none());
        }
        assert_eq!(p.state(), State::Ground);

        _ = p.next(0x1B);
        _ = p.next(b'[');
        {
            let a = p.next(b'H');
            assert!(a[0].is_none());
            assert!(matches!(a[1], Some(Action::CsiDispatch(_))));
            assert!(a[2].is_none());
        }
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: SGR mixed colon and semicolon"
    #[test]
    fn csi_sgr_mixed_colon_and_semicolon() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        for c in "[38:5:1;48:5:0".bytes() {
            let a = p.next(c);
            assert!(a.iter().all(Option::is_none));
        }

        let a = p.next(b'm');
        assert!(a[0].is_none());
        assert!(matches!(a[1], Some(Action::CsiDispatch(_))));
        assert!(a[2].is_none());
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: SGR ESC [ 48 : 2 m"
    #[test]
    fn csi_sgr_48_colon_2_rgb() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        for c in "[48:2:240:143:104".bytes() {
            let a = p.next(c);
            assert!(a.iter().all(Option::is_none));
        }

        let a = p.next(b'm');
        assert!(a[0].is_none());
        assert!(a[2].is_none());
        let d = csi(&a[1]);
        assert_eq!(d.final_byte, b'm');
        assert_eq!(d.params, &[48, 2, 240, 143, 104]);
        for i in 0..4 {
            assert!(d.params_sep.is_set(i));
        }
        assert!(!d.params_sep.is_set(4));
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: SGR ESC [4:3m colon"
    #[test]
    fn csi_sgr_4_colon_3() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        _ = p.next(b'[');
        _ = p.next(b'4');
        _ = p.next(b':');
        _ = p.next(b'3');

        let a = p.next(b'm');
        assert!(a[0].is_none());
        assert!(a[2].is_none());
        let d = csi(&a[1]);
        assert_eq!(d.final_byte, b'm');
        assert_eq!(d.params, &[4, 3]);
        assert!(d.params_sep.is_set(0));
        assert!(!d.params_sep.is_set(1));
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: SGR with many blank and colon"
    #[test]
    fn csi_sgr_many_blank_and_colon() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        for c in "[58:2::240:143:104".bytes() {
            let a = p.next(c);
            assert!(a.iter().all(Option::is_none));
        }

        let a = p.next(b'm');
        assert!(a[0].is_none());
        assert!(a[2].is_none());
        let d = csi(&a[1]);
        assert_eq!(d.final_byte, b'm');
        assert_eq!(d.params, &[58, 2, 0, 240, 143, 104]);
        for i in 0..5 {
            assert!(d.params_sep.is_set(i));
        }
        assert!(!d.params_sep.is_set(5));
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: SGR mixed colon and semicolon with blank" (from a Kakoune
    // actual SGR sequence).
    #[test]
    fn csi_sgr_kakoune_mixed_with_blank() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        for c in "[;4:3;38;2;175;175;215;58:2::190:80:70".bytes() {
            let a = p.next(c);
            assert!(a.iter().all(Option::is_none));
        }

        let a = p.next(b'm');
        assert!(a[0].is_none());
        assert!(a[2].is_none());
        let d = csi(&a[1]);
        assert_eq!(d.final_byte, b'm');
        assert_eq!(
            d.params,
            &[0, 4, 3, 38, 2, 175, 175, 215, 58, 2, 0, 190, 80, 70]
        );
        let colons = [1, 8, 9, 10, 11, 12];
        for i in 0..14 {
            assert_eq!(d.params_sep.is_set(i), colons.contains(&i), "sep {i}");
        }
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: SGR mixed colon and semicolon setting underline, bg, fg"
    // (from a Kakoune actual SGR sequence also).
    #[test]
    fn csi_sgr_kakoune_underline_bg_fg() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        for c in "[4:3;38;2;51;51;51;48;2;170;170;170;58;2;255;97;136".bytes() {
            let a = p.next(c);
            assert!(a.iter().all(Option::is_none));
        }

        let a = p.next(b'm');
        assert!(a[0].is_none());
        assert!(a[2].is_none());
        let d = csi(&a[1]);
        assert_eq!(d.final_byte, b'm');
        assert_eq!(
            d.params,
            &[
                4, 3, 38, 2, 51, 51, 51, 48, 2, 170, 170, 170, 58, 2, 255, 97, 136
            ]
        );
        for i in 0..17 {
            assert_eq!(d.params_sep.is_set(i), i == 0, "sep {i}");
        }
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: colon for non-m final"
    #[test]
    fn csi_colon_for_non_m_final() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        for c in "[38:2h".bytes() {
            let a = p.next(c);
            assert!(a.iter().all(Option::is_none));
        }

        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: request mode decrqm"
    #[test]
    fn csi_request_mode_decrqm() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        for c in "[?2026$".bytes() {
            let a = p.next(c);
            assert!(a.iter().all(Option::is_none));
        }

        let a = p.next(b'p');
        assert!(a[0].is_none());
        assert!(a[2].is_none());
        let d = csi(&a[1]);
        assert_eq!(d.final_byte, b'p');
        assert_eq!(d.intermediates, b"?$");
        assert_eq!(d.params, &[2026]);
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: change cursor"
    #[test]
    fn csi_change_cursor() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        for c in "[3 ".bytes() {
            let a = p.next(c);
            assert!(a.iter().all(Option::is_none));
        }

        let a = p.next(b'q');
        assert!(a[0].is_none());
        assert!(a[2].is_none());
        let d = csi(&a[1]);
        assert_eq!(d.final_byte, b'q');
        assert_eq!(d.intermediates, b" ");
        assert_eq!(d.params, &[3]);
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "osc: change window title". The Zig test asserts a structured
    // `osc.Command.change_window_title`; the raw-event seam asserts the
    // accumulated bytes and BEL terminator instead (see module docs).
    #[test]
    fn osc_change_window_title() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        {
            let a = p.next(b']');
            assert!(matches!(a[2], Some(Action::OscStart)));
        }

        let mut buf = Vec::new();
        for c in "0;abc".bytes() {
            let a = p.next(c);
            let Some(Action::OscPut(b)) = a[1] else {
                panic!("expected osc_put, got {a:?}");
            };
            buf.push(b);
        }

        let a = p.next(0x07); // BEL
        assert_eq!(a[0], Some(Action::OscEnd(0x07)));
        assert!(a[1].is_none());
        assert!(a[2].is_none());
        assert_eq!(p.state(), State::Ground);
        assert_eq!(buf, b"0;abc");
    }

    // Zig: "osc: change window title (end in esc)". Same seam note as
    // above; the ESC terminator byte (0x1B) means ST.
    #[test]
    fn osc_change_window_title_end_in_esc() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        _ = p.next(b']');
        let mut buf = Vec::new();
        for c in "0;abc".bytes() {
            let a = p.next(c);
            let Some(Action::OscPut(b)) = a[1] else {
                panic!("expected osc_put, got {a:?}");
            };
            buf.push(b);
        }

        {
            let a = p.next(0x1B);
            assert_eq!(a[0], Some(Action::OscEnd(0x1B)));
            assert!(a[1].is_none());
            assert!(a[2].is_none());
        }
        _ = p.next(b'\\');
        assert_eq!(p.state(), State::Ground);
        assert_eq!(buf, b"0;abc");
    }

    // Zig: "osc: 112 incomplete sequence"
    // (https://github.com/darrenstarr/VtNetCore/pull/14). The Zig test
    // asserts the structured color_operation command; the raw seam pins
    // that the bytes reach downstream unchanged with a BEL terminator.
    #[test]
    fn osc_112_incomplete_sequence() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        _ = p.next(b']');
        let mut buf = Vec::new();
        for c in "112".bytes() {
            let a = p.next(c);
            let Some(Action::OscPut(b)) = a[1] else {
                panic!("expected osc_put, got {a:?}");
            };
            buf.push(b);
        }

        let a = p.next(0x07);
        assert_eq!(a[0], Some(Action::OscEnd(0x07)));
        assert!(a[1].is_none());
        assert!(a[2].is_none());
        assert_eq!(p.state(), State::Ground);
        assert_eq!(buf, b"112");
    }

    // Zig: "osc: 104 empty". Same seam note as above.
    #[test]
    fn osc_104_empty() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        _ = p.next(b']');
        let mut buf = Vec::new();
        for c in "104".bytes() {
            let a = p.next(c);
            let Some(Action::OscPut(b)) = a[1] else {
                panic!("expected osc_put, got {a:?}");
            };
            buf.push(b);
        }

        let a = p.next(0x07);
        assert_eq!(a[0], Some(Action::OscEnd(0x07)));
        assert!(a[1].is_none());
        assert!(a[2].is_none());
        assert_eq!(p.state(), State::Ground);
        assert_eq!(buf, b"104");
    }

    // Zig: "csi: too many params"
    #[test]
    fn csi_too_many_params() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        _ = p.next(b'[');
        for _ in 0..100 {
            _ = p.next(b'1');
            _ = p.next(b';');
        }
        _ = p.next(b'1');

        let a = p.next(b'C');
        assert!(a.iter().all(Option::is_none));
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "csi: sgr with up to our max parameters"
    #[test]
    fn csi_sgr_up_to_max_params() {
        for max in 1..=MAX_PARAMS {
            let mut p = Parser::new();
            _ = p.next(0x1B);
            _ = p.next(b'[');

            for _ in 0..max - 1 {
                _ = p.next(b'1');
                _ = p.next(b';');
            }
            _ = p.next(b'2');

            let a = p.next(b'H');
            assert!(a[0].is_none());
            assert!(a[2].is_none());
            let d = csi(&a[1]);
            assert_eq!(d.params.len(), max);
            assert_eq!(d.params[max - 1], 2);
            assert_eq!(p.state(), State::Ground);
        }
    }

    // Zig: "csi: sgr beyond our max drops it"
    #[test]
    fn csi_sgr_beyond_max_drops_it() {
        // Has to be +2 for the loops below
        let max = MAX_PARAMS + 2;

        let mut p = Parser::new();
        _ = p.next(0x1B);
        _ = p.next(b'[');

        for _ in 0..max - 1 {
            _ = p.next(b'1');
            _ = p.next(b';');
        }
        _ = p.next(b'2');

        let a = p.next(b'H');
        assert!(a.iter().all(Option::is_none));
        assert_eq!(p.state(), State::Ground);
    }

    // Zig: "dcs: XTGETTCAP"
    #[test]
    fn dcs_xtgettcap() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        for c in "P+".bytes() {
            let a = p.next(c);
            assert!(a.iter().all(Option::is_none));
        }

        let a = p.next(b'q');
        assert!(a[0].is_none());
        assert!(a[1].is_none());
        let Some(Action::DcsHook(hook)) = &a[2] else {
            panic!("expected dcs_hook, got {a:?}");
        };
        assert_eq!(hook.intermediates, b"+");
        assert_eq!(hook.params, &[] as &[u16]);
        assert_eq!(hook.final_byte, b'q');
        assert_eq!(p.state(), State::DcsPassthrough);
    }

    // Zig: "dcs: params"
    #[test]
    fn dcs_params() {
        let mut p = Parser::new();
        _ = p.next(0x1B);
        for c in "P1000".bytes() {
            let a = p.next(c);
            assert!(a.iter().all(Option::is_none));
        }

        let a = p.next(b'p');
        assert!(a[0].is_none());
        assert!(a[1].is_none());
        let Some(Action::DcsHook(hook)) = &a[2] else {
            panic!("expected dcs_hook, got {a:?}");
        };
        assert_eq!(hook.params, &[1000]);
        assert_eq!(hook.final_byte, b'p');
        assert_eq!(p.state(), State::DcsPassthrough);
    }

    // Zig: "dcs: too many params". Regression test for a crash found by
    // fuzzing (afl): entering dcs_passthrough with params_idx == MAX_PARAMS
    // and param_acc_idx > 0 wrote out of bounds. The DCS hook is dropped
    // entirely, consistent with how CSI handles overflow.
    #[test]
    fn dcs_too_many_params() {
        let mut p = Parser::new();
        _ = p.next(0x1B); // ESC
        _ = p.next(b'P'); // DCS entry

        // Feed a digit then MAX_PARAMS semicolons to fill all param slots.
        _ = p.next(b'6');
        for _ in 0..MAX_PARAMS {
            _ = p.next(b';');
        }
        // Feed another digit so param_acc_idx > 0 while params_idx == MAX_PARAMS.
        _ = p.next(b'7');

        // A final byte triggers entry to dcs_passthrough. The DCS should
        // be dropped entirely.
        let a = p.next(b'p');
        assert!(a.iter().all(Option::is_none));
        assert_eq!(p.state(), State::DcsPassthrough);
    }
}
