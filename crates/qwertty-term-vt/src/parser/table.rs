//! State transition table for VT emulation, ported from
//! `src/terminal/parse_table.zig` (ghostty `2da015cd6`).
//!
//! This is based on the vt100.net state machine
//! (<https://vt100.net/emu/dec_ansi_parser>) but has some modifications:
//!
//!   * `csi_param` accepts the colon character (`:`) since the SGR command
//!     accepts colon as a valid parameter value.
//!
//! Construction order is load-bearing and mirrors the Zig source exactly:
//! the "anywhere" transitions are written first, then per-state blocks;
//! later writes overwrite earlier ones (notably `osc_string` claims
//! `0x20..=0xFF` and `dcs_passthrough` claims `0x80..=0xFF`, overriding the
//! anywhere C1 transitions so OSC/DCS strings can carry raw UTF-8). Unwritten
//! cells default to "stay in state, no action".
//!
//! The `dcs_passthrough` high-byte override is a deliberate deviation from the
//! upstream `parse_table.zig`, which only added the `0x20..=0xFF` override to
//! `osc_string` (`5e800df27`) and left `dcs_passthrough` terminating on C1
//! bytes -- a latent bug that truncates tmux control-mode (`ESC P 1000 p`)
//! `%output` payloads containing UTF-8. See the `dcs_passthrough` block.

use super::State;

/// Internal transition action taken while moving between states. This is
/// distinct from [`super::Action`], which is the caller-visible result.
/// Mirrors `Parser.TransitionAction` (Parser.zig:35-47).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TransitionAction {
    None,
    Ignore,
    Print,
    Execute,
    Collect,
    Param,
    EscDispatch,
    CsiDispatch,
    Put,
    OscPut,
    ApcPut,
}

/// The transition to take within the table.
#[derive(Clone, Copy, Debug)]
pub(super) struct Transition {
    pub(super) state: State,
    pub(super) action: TransitionAction,
}

pub(super) const STATE_COUNT: usize = 14;

const ALL_STATES: [State; STATE_COUNT] = [
    State::Ground,
    State::Escape,
    State::EscapeIntermediate,
    State::CsiEntry,
    State::CsiIntermediate,
    State::CsiParam,
    State::CsiIgnore,
    State::DcsEntry,
    State::DcsParam,
    State::DcsIntermediate,
    State::DcsPassthrough,
    State::DcsIgnore,
    State::OscString,
    State::SosPmApcString,
];

/// The state transition table, indexed as `TABLE[byte][state]`.
pub(super) static TABLE: [[Transition; STATE_COUNT]; 256] = gen_table();

type OptTable = [[Option<Transition>; STATE_COUNT]; 256];

const fn single(t: &mut OptTable, c: u8, s0: State, s1: State, a: TransitionAction) {
    t[c as usize][s0 as usize] = Some(Transition {
        state: s1,
        action: a,
    });
}

const fn range(t: &mut OptTable, from: u8, to: u8, s0: State, s1: State, a: TransitionAction) {
    let mut i = from;
    loop {
        single(t, i, s0, s1, a);
        // If `to` is 0xFF, `i + 1` would overflow; break before incrementing.
        if i == to {
            break;
        }
        i += 1;
    }
}

/// Generate the full state transition table. Mirrors `genTable`
/// (parse_table.zig:45-353) block by block, in the same order.
const fn gen_table() -> [[Transition; STATE_COUNT]; 256] {
    use TransitionAction as A;

    let mut result: OptTable = [[None; STATE_COUNT]; 256];
    let t = &mut result;

    // anywhere transitions
    let mut si = 0;
    while si < STATE_COUNT {
        let source = ALL_STATES[si];

        // anywhere => ground
        single(t, 0x18, source, State::Ground, A::Execute);
        single(t, 0x1A, source, State::Ground, A::Execute);
        range(t, 0x80, 0x8F, source, State::Ground, A::Execute);
        range(t, 0x91, 0x97, source, State::Ground, A::Execute);
        single(t, 0x99, source, State::Ground, A::Execute);
        single(t, 0x9A, source, State::Ground, A::Execute);
        single(t, 0x9C, source, State::Ground, A::None);

        // anywhere => escape
        single(t, 0x1B, source, State::Escape, A::None);

        // anywhere => sos_pm_apc_string
        single(t, 0x98, source, State::SosPmApcString, A::None);
        single(t, 0x9E, source, State::SosPmApcString, A::None);
        single(t, 0x9F, source, State::SosPmApcString, A::None);

        // anywhere => csi_entry
        single(t, 0x9B, source, State::CsiEntry, A::None);

        // anywhere => dcs_entry
        single(t, 0x90, source, State::DcsEntry, A::None);

        // anywhere => osc_string
        single(t, 0x9D, source, State::OscString, A::None);

        si += 1;
    }

    // ground
    {
        // events
        single(t, 0x19, State::Ground, State::Ground, A::Execute);
        range(t, 0, 0x17, State::Ground, State::Ground, A::Execute);
        range(t, 0x1C, 0x1F, State::Ground, State::Ground, A::Execute);
        range(t, 0x20, 0x7F, State::Ground, State::Ground, A::Print);
    }

    // escape_intermediate
    {
        let source = State::EscapeIntermediate;

        single(t, 0x19, source, source, A::Execute);
        range(t, 0, 0x17, source, source, A::Execute);
        range(t, 0x1C, 0x1F, source, source, A::Execute);
        range(t, 0x20, 0x2F, source, source, A::Collect);
        single(t, 0x7F, source, source, A::Ignore);

        // => ground
        range(t, 0x30, 0x7E, source, State::Ground, A::EscDispatch);
    }

    // sos_pm_apc_string
    {
        let source = State::SosPmApcString;

        // events
        single(t, 0x19, source, source, A::ApcPut);
        range(t, 0, 0x17, source, source, A::ApcPut);
        range(t, 0x1C, 0x1F, source, source, A::ApcPut);
        range(t, 0x20, 0x7F, source, source, A::ApcPut);
    }

    // escape
    {
        let source = State::Escape;

        // events
        single(t, 0x19, source, source, A::Execute);
        range(t, 0, 0x17, source, source, A::Execute);
        range(t, 0x1C, 0x1F, source, source, A::Execute);
        single(t, 0x7F, source, source, A::Ignore);

        // => ground
        range(t, 0x30, 0x4F, source, State::Ground, A::EscDispatch);
        range(t, 0x51, 0x57, source, State::Ground, A::EscDispatch);
        range(t, 0x60, 0x7E, source, State::Ground, A::EscDispatch);
        single(t, 0x59, source, State::Ground, A::EscDispatch);
        single(t, 0x5A, source, State::Ground, A::EscDispatch);
        single(t, 0x5C, source, State::Ground, A::EscDispatch);

        // => escape_intermediate
        range(t, 0x20, 0x2F, source, State::EscapeIntermediate, A::Collect);

        // => sos_pm_apc_string
        single(t, 0x58, source, State::SosPmApcString, A::None);
        single(t, 0x5E, source, State::SosPmApcString, A::None);
        single(t, 0x5F, source, State::SosPmApcString, A::None);

        // => dcs_entry
        single(t, 0x50, source, State::DcsEntry, A::None);

        // => csi_entry
        single(t, 0x5B, source, State::CsiEntry, A::None);

        // => osc_string
        single(t, 0x5D, source, State::OscString, A::None);
    }

    // dcs_entry
    {
        let source = State::DcsEntry;

        // events
        single(t, 0x19, source, source, A::Ignore);
        range(t, 0, 0x17, source, source, A::Ignore);
        range(t, 0x1C, 0x1F, source, source, A::Ignore);
        single(t, 0x7F, source, source, A::Ignore);

        // => dcs_intermediate
        range(t, 0x20, 0x2F, source, State::DcsIntermediate, A::Collect);

        // => dcs_ignore
        single(t, 0x3A, source, State::DcsIgnore, A::None);

        // => dcs_param
        range(t, 0x30, 0x39, source, State::DcsParam, A::Param);
        single(t, 0x3B, source, State::DcsParam, A::Param);
        range(t, 0x3C, 0x3F, source, State::DcsParam, A::Collect);

        // => dcs_passthrough
        range(t, 0x40, 0x7E, source, State::DcsPassthrough, A::None);
    }

    // dcs_intermediate
    {
        let source = State::DcsIntermediate;

        // events
        single(t, 0x19, source, source, A::Ignore);
        range(t, 0, 0x17, source, source, A::Ignore);
        range(t, 0x1C, 0x1F, source, source, A::Ignore);
        range(t, 0x20, 0x2F, source, source, A::Collect);
        single(t, 0x7F, source, source, A::Ignore);

        // => dcs_ignore
        range(t, 0x30, 0x3F, source, State::DcsIgnore, A::None);

        // => dcs_passthrough
        range(t, 0x40, 0x7E, source, State::DcsPassthrough, A::None);
    }

    // dcs_ignore
    {
        let source = State::DcsIgnore;

        // events
        single(t, 0x19, source, source, A::Ignore);
        range(t, 0, 0x17, source, source, A::Ignore);
        range(t, 0x1C, 0x1F, source, source, A::Ignore);
    }

    // dcs_param
    {
        let source = State::DcsParam;

        // events
        single(t, 0x19, source, source, A::Ignore);
        range(t, 0, 0x17, source, source, A::Ignore);
        range(t, 0x1C, 0x1F, source, source, A::Ignore);
        range(t, 0x30, 0x39, source, source, A::Param);
        single(t, 0x3B, source, source, A::Param);
        single(t, 0x7F, source, source, A::Ignore);

        // => dcs_ignore
        single(t, 0x3A, source, State::DcsIgnore, A::None);
        range(t, 0x3C, 0x3F, source, State::DcsIgnore, A::None);

        // => dcs_intermediate
        range(t, 0x20, 0x2F, source, State::DcsIntermediate, A::Collect);

        // => dcs_passthrough
        range(t, 0x40, 0x7E, source, State::DcsPassthrough, A::None);
    }

    // dcs_passthrough
    {
        let source = State::DcsPassthrough;

        // events
        single(t, 0x19, source, source, A::Put);
        range(t, 0, 0x17, source, source, A::Put);
        range(t, 0x1C, 0x1F, source, source, A::Put);
        range(t, 0x20, 0x7E, source, source, A::Put);
        single(t, 0x7F, source, source, A::Ignore);

        // High bytes: mirror `osc_string`'s `0x20..=0xFF` override (see the
        // header note and the `osc_string` block below). Without this the
        // anywhere C1 rules (`0x80..=0x8F` -> ground execute, `0x9C` -> ground,
        // plus the `0x90`/`0x9B`/`0x9D`/`0x9E`/`0x9F` introducers) would
        // terminate the DCS the instant a byte >= 0x80 appears, truncating a
        // passthrough body that carries raw UTF-8 / 8-bit data (e.g. tmux
        // control-mode `%output`, where an emoji's trailing `0x80` continuation
        // byte silently ends control mode). Put them through to the DCS handler
        // instead. Real 7-bit ST (`ESC \`) and `0x18`/`0x1A` still terminate,
        // since those bytes are outside `0x80..=0xFF` and keep their rules.
        range(t, 0x80, 0xFF, source, source, A::Put);
    }

    // csi_param
    {
        let source = State::CsiParam;

        // events
        single(t, 0x19, source, source, A::Execute);
        range(t, 0, 0x17, source, source, A::Execute);
        range(t, 0x1C, 0x1F, source, source, A::Execute);
        range(t, 0x30, 0x39, source, source, A::Param);
        single(t, 0x3A, source, source, A::Param);
        single(t, 0x3B, source, source, A::Param);
        single(t, 0x7F, source, source, A::Ignore);

        // => ground
        range(t, 0x40, 0x7E, source, State::Ground, A::CsiDispatch);

        // => csi_ignore
        range(t, 0x3C, 0x3F, source, State::CsiIgnore, A::None);

        // => csi_intermediate
        range(t, 0x20, 0x2F, source, State::CsiIntermediate, A::Collect);
    }

    // csi_ignore
    {
        let source = State::CsiIgnore;

        // events
        single(t, 0x19, source, source, A::Execute);
        range(t, 0, 0x17, source, source, A::Execute);
        range(t, 0x1C, 0x1F, source, source, A::Execute);
        range(t, 0x20, 0x3F, source, source, A::Ignore);
        single(t, 0x7F, source, source, A::Ignore);

        // => ground
        range(t, 0x40, 0x7E, source, State::Ground, A::None);
    }

    // csi_intermediate
    {
        let source = State::CsiIntermediate;

        // events
        single(t, 0x19, source, source, A::Execute);
        range(t, 0, 0x17, source, source, A::Execute);
        range(t, 0x1C, 0x1F, source, source, A::Execute);
        range(t, 0x20, 0x2F, source, source, A::Collect);
        single(t, 0x7F, source, source, A::Ignore);

        // => ground
        range(t, 0x40, 0x7E, source, State::Ground, A::CsiDispatch);

        // => csi_ignore
        range(t, 0x30, 0x3F, source, State::CsiIgnore, A::None);
    }

    // csi_entry
    {
        let source = State::CsiEntry;

        // events
        single(t, 0x19, source, source, A::Execute);
        range(t, 0, 0x17, source, source, A::Execute);
        range(t, 0x1C, 0x1F, source, source, A::Execute);
        single(t, 0x7F, source, source, A::Ignore);

        // => ground
        range(t, 0x40, 0x7E, source, State::Ground, A::CsiDispatch);

        // => csi_ignore
        single(t, 0x3A, source, State::CsiIgnore, A::None);

        // => csi_intermediate
        range(t, 0x20, 0x2F, source, State::CsiIntermediate, A::Collect);

        // => csi_param
        range(t, 0x30, 0x39, source, State::CsiParam, A::Param);
        single(t, 0x3B, source, State::CsiParam, A::Param);
        range(t, 0x3C, 0x3F, source, State::CsiParam, A::Collect);
    }

    // osc_string
    {
        let source = State::OscString;

        // events
        single(t, 0x19, source, source, A::Ignore);
        range(t, 0, 0x06, source, source, A::Ignore);
        range(t, 0x08, 0x17, source, source, A::Ignore);
        range(t, 0x1C, 0x1F, source, source, A::Ignore);
        range(t, 0x20, 0xFF, source, source, A::OscPut);

        // XTerm accepts either BEL or ST for terminating OSC sequences,
        // and when returning information, uses the same terminator used
        // in a query.
        single(t, 0x07, source, State::Ground, A::None);
    }

    // Create our immutable version: unset cells stay in the same state
    // with no action.
    let mut table = [[Transition {
        state: State::Ground,
        action: A::None,
    }; STATE_COUNT]; 256];
    let mut c = 0;
    while c < 256 {
        let mut s = 0;
        while s < STATE_COUNT {
            table[c][s] = match result[c][s] {
                Some(tr) => tr,
                None => Transition {
                    state: ALL_STATES[s],
                    action: A::None,
                },
            };
            s += 1;
        }
        c += 1;
    }

    table
}

#[cfg(test)]
mod tests {
    use super::*;

    // Zig: parse_table.zig test (forces comptime table evaluation). Here we
    // additionally spot-check a few cells against the vt100.net machine and
    // ghostty's documented deviations.
    #[test]
    fn table_builds_with_expected_cells() {
        // ground print range
        let tr = TABLE[b'a' as usize][State::Ground as usize];
        assert_eq!(tr.state as u8, State::Ground as u8);
        assert_eq!(tr.action, TransitionAction::Print);

        // csi_param accepts colon (ghostty deviation)
        let tr = TABLE[b':' as usize][State::CsiParam as usize];
        assert_eq!(tr.state as u8, State::CsiParam as u8);
        assert_eq!(tr.action, TransitionAction::Param);

        // ...but csi_entry does not (matches Williams)
        let tr = TABLE[b':' as usize][State::CsiEntry as usize];
        assert_eq!(tr.state as u8, State::CsiIgnore as u8);

        // osc_string overrides the anywhere C1 rules: 0x9C is osc_put
        let tr = TABLE[0x9C][State::OscString as usize];
        assert_eq!(tr.state as u8, State::OscString as u8);
        assert_eq!(tr.action, TransitionAction::OscPut);

        // dcs_passthrough mirrors osc_string for high bytes: 0x9C is a Put
        // (a UTF-8 continuation byte) that stays in DcsPassthrough rather than
        // terminating the DCS. This is what lets tmux control-mode %output
        // carry raw UTF-8 without truncation.
        let tr = TABLE[0x9C][State::DcsPassthrough as usize];
        assert_eq!(tr.state as u8, State::DcsPassthrough as u8);
        assert_eq!(tr.action, TransitionAction::Put);

        // ...while everywhere else (e.g. escape) 0x9C returns to ground
        let tr = TABLE[0x9C][State::Escape as usize];
        assert_eq!(tr.state as u8, State::Ground as u8);
        assert_eq!(tr.action, TransitionAction::None);

        // The full 0x80..=0xFF range is Put in DcsPassthrough, including the
        // C1 introducers (0x90 DCS, 0x9B CSI, 0x9D OSC) and 0x80 (the emoji
        // continuation byte that used to terminate control mode).
        for c in [0x80u8, 0x90, 0x9B, 0x9D, 0x9F, 0xA6, 0xF0, 0xFF] {
            let tr = TABLE[c as usize][State::DcsPassthrough as usize];
            assert_eq!(
                tr.state as u8,
                State::DcsPassthrough as u8,
                "byte {c:#04x} should stay in DcsPassthrough"
            );
            assert_eq!(tr.action, TransitionAction::Put, "byte {c:#04x} should Put");
        }

        // 7-bit ST (ESC) and CAN/SUB still leave DcsPassthrough as before.
        assert_eq!(
            TABLE[0x1B][State::DcsPassthrough as usize].state as u8,
            State::Escape as u8
        );
        assert_eq!(
            TABLE[0x18][State::DcsPassthrough as usize].state as u8,
            State::Ground as u8
        );
        assert_eq!(
            TABLE[0x1A][State::DcsPassthrough as usize].state as u8,
            State::Ground as u8
        );

        // sos_pm_apc_string high bytes default to no-op self transition
        let tr = TABLE[0xA0][State::SosPmApcString as usize];
        assert_eq!(tr.state as u8, State::SosPmApcString as u8);
        assert_eq!(tr.action, TransitionAction::None);
    }
}
