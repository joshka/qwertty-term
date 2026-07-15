//! Differential tests: `qwertty-term-vt`'s parser vs the `vte` crate.
//!
//! `vte` is a dev-dependency oracle only. Both are DEC-style VT state
//! machines derived from Paul Williams' vt100.net parser, so for the common
//! cases (print, execute, CSI/ESC dispatch, DCS hook/put/unhook) they should
//! agree on the *sequence of dispatched events* for the same byte stream. We
//! normalize each parser's output into a common [`Event`] vocabulary and
//! assert equality.
//!
//! Where the two deliberately diverge, we do NOT force parity — we assert the
//! divergence explicitly and document why, so a future change that
//! accidentally "fixes" it fails loudly. Known divergences:
//!
//! 1. **UTF-8 ownership.** `vte::Parser::advance` decodes UTF-8 internally and
//!    emits `print(char)` for full codepoints. ghostty's parser is
//!    byte-oriented; UTF-8 is the separate `Utf8Decoder`'s job (the stream
//!    layer composes them). So we only feed the differential corpus **ASCII
//!    and control bytes** — multi-byte input is out of scope for a parser-only
//!    comparison and is covered by the decoder's own tests.
//!
//! 2. **OSC surface.** qwertty-term-vt emits raw `OscStart`/`OscPut`/`OscEnd`
//!    byte events (the structured `osc.Command` parser is a later chunk);
//!    `vte` accumulates and emits a structured `osc_dispatch(params,
//!    bell_terminated)`. We normalize both to "an OSC happened with these
//!    concatenated data bytes" and compare that.
//!
//! 3. **Colon subparams on non-`m` finals.** ghostty drops a CSI dispatch
//!    whose final byte is not `m` if any colon separator was seen
//!    (Parser.zig:386-394); `vte` dispatches it. Asserted as a divergence in
//!    [`divergence_colon_non_m_final`].
//!
//! 4. **Param limits.** ghostty caps params at `MAX_PARAMS` (24) and *drops
//!    the whole dispatch* on overflow; `vte` caps at its own `MAX_PARAMS`
//!    (32) and truncates. We keep the shared corpus under 24 params so the
//!    limits don't interfere, and assert the overflow divergence separately.
//!
//! 5. **Empty param list.** A paramless CSI (`ESC [ H`) yields **no** params
//!    in ghostty (empty slice) but a single default `0` param in vte. Both are
//!    valid: ghostty leaves "missing param -> default" to the downstream
//!    command layer, vte materializes the default in the parser. Because this
//!    is systematic, the agreement helper canonicalizes a trailing lone `[0]`
//!    (or empty) CSI param list to empty on both sides; the raw behavior is
//!    pinned in [`divergence_empty_params`].

use qwertty_term_vt::parser::{Action, Parser};
use vte::{Params, Perform};

/// A normalized dispatch event, common to both parsers.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    Print(char),
    Execute(u8),
    /// Flattened params (colon groups flattened to a single sequence, matching
    /// how ghostty stores them), intermediates, final byte.
    Csi(Vec<u16>, Vec<u8>, u8),
    Esc(Vec<u8>, u8),
    /// Concatenated OSC data bytes (excluding the terminator).
    Osc(Vec<u8>),
    DcsHook(Vec<u16>, Vec<u8>, u8),
    DcsPut(u8),
    DcsUnhook,
}

// ---- qwertty-term-vt side ---------------------------------------------------

fn ghostty_events(bytes: &[u8]) -> Vec<Event> {
    let mut parser = Parser::new();
    let mut events = Vec::new();
    let mut osc_buf: Option<Vec<u8>> = None;

    for &b in bytes {
        for action in parser.next(b) {
            let Some(action) = action else { continue };
            match action {
                Action::Print(c) => events.push(Event::Print(c)),
                Action::Execute(byte) => events.push(Event::Execute(byte)),
                Action::CsiDispatch(csi) => events.push(Event::Csi(
                    csi.params.to_vec(),
                    csi.intermediates.to_vec(),
                    csi.final_byte,
                )),
                Action::EscDispatch(esc) => {
                    events.push(Event::Esc(esc.intermediates.to_vec(), esc.final_byte))
                }
                Action::OscStart => osc_buf = Some(Vec::new()),
                Action::OscPut(byte) => {
                    if let Some(buf) = osc_buf.as_mut() {
                        buf.push(byte);
                    }
                }
                Action::OscEnd(_term) => {
                    if let Some(buf) = osc_buf.take() {
                        events.push(Event::Osc(buf));
                    }
                }
                Action::DcsHook(dcs) => events.push(Event::DcsHook(
                    dcs.params.to_vec(),
                    dcs.intermediates.to_vec(),
                    dcs.final_byte,
                )),
                Action::DcsPut(byte) => events.push(Event::DcsPut(byte)),
                Action::DcsUnhook => events.push(Event::DcsUnhook),
                // SOS/PM/APC have no vte equivalent in this corpus.
                Action::ApcStart | Action::ApcPut(_) | Action::ApcEnd => {}
            }
        }
    }
    events
}

// ---- vte side ----------------------------------------------------------

#[derive(Default)]
struct VteSink {
    events: Vec<Event>,
}

fn flatten_params(params: &Params) -> Vec<u16> {
    // vte groups colon-subparams; ghostty flattens them into one array. Match
    // ghostty by flattening. Empty groups become a single 0 (ghostty stores 0
    // for an empty param slot).
    let mut out = Vec::new();
    for group in params.iter() {
        if group.is_empty() {
            out.push(0);
        } else {
            out.extend_from_slice(group);
        }
    }
    out
}

impl Perform for VteSink {
    fn print(&mut self, c: char) {
        self.events.push(Event::Print(c));
    }

    fn execute(&mut self, byte: u8) {
        self.events.push(Event::Execute(byte));
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        self.events.push(Event::Csi(
            flatten_params(params),
            intermediates.to_vec(),
            action as u8,
        ));
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        self.events.push(Event::Esc(intermediates.to_vec(), byte));
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // vte splits OSC on ';'; ghostty keeps raw bytes. Re-join with ';' to
        // recover the concatenated data bytes ghostty would have accumulated.
        let mut buf = Vec::new();
        for (i, part) in params.iter().enumerate() {
            if i > 0 {
                buf.push(b';');
            }
            buf.extend_from_slice(part);
        }
        self.events.push(Event::Osc(buf));
    }

    fn hook(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        self.events.push(Event::DcsHook(
            flatten_params(params),
            intermediates.to_vec(),
            action as u8,
        ));
    }

    fn put(&mut self, byte: u8) {
        self.events.push(Event::DcsPut(byte));
    }

    fn unhook(&mut self) {
        self.events.push(Event::DcsUnhook);
    }
}

fn vte_events(bytes: &[u8]) -> Vec<Event> {
    let mut parser = vte::Parser::new();
    let mut sink = VteSink::default();
    parser.advance(&mut sink, bytes);
    sink.events
}

/// Canonicalize the systematic "empty vs single default-0" param divergence
/// (see divergence note 5): treat a CSI/DCS param list of `[0]` as `[]`.
fn canonicalize(events: Vec<Event>) -> Vec<Event> {
    events
        .into_iter()
        .map(|e| match e {
            Event::Csi(params, inter, f) if params == [0] => Event::Csi(Vec::new(), inter, f),
            Event::DcsHook(params, inter, f) if params == [0] => {
                Event::DcsHook(Vec::new(), inter, f)
            }
            other => other,
        })
        .collect()
}

/// Assert both parsers agree on the given (ASCII / control-byte) stream, after
/// canonicalizing the documented empty-param divergence.
fn assert_agree(bytes: &[u8]) {
    let ours = canonicalize(ghostty_events(bytes));
    let theirs = canonicalize(vte_events(bytes));
    assert_eq!(
        ours, theirs,
        "parser divergence on input {:?}\n ghostty: {:?}\n     vte: {:?}",
        bytes, ours, theirs
    );
}

#[test]
fn agree_plain_text() {
    assert_agree(b"Hello, World! 0123456789");
}

#[test]
fn agree_execute_controls() {
    assert_agree(b"a\r\nb\tc\x07d");
}

#[test]
fn agree_csi_cursor_moves() {
    assert_agree(b"\x1b[H\x1b[2J\x1b[10;20H\x1b[3A\x1b[K");
}

#[test]
fn agree_csi_multi_param() {
    assert_agree(b"\x1b[1;2;3;4;5;6m");
}

#[test]
fn agree_sgr_colon_truecolor() {
    // Final byte is 'm', so ghostty keeps the colon dispatch — should agree.
    assert_agree(b"\x1b[38:2:175:175:215m\x1b[0m");
}

#[test]
fn agree_esc_dispatch() {
    assert_agree(b"\x1b(B\x1b)0\x1b=\x1b>");
}

#[test]
fn agree_osc_title_bel() {
    assert_agree(b"\x1b]0;my title\x07rest");
}

#[test]
fn agree_osc_title_st() {
    assert_agree(b"\x1b]0;my title\x1b\\rest");
}

#[test]
fn agree_dcs_passthrough() {
    assert_agree(b"\x1bP1000pdata\x1b\\");
}

#[test]
fn agree_interleaved() {
    assert_agree(b"pre\x1b[1mbold\x1b[0m mid \x1b]2;t\x07 post\r\n");
}

// ---- documented divergences (asserted, not forced to parity) -----------

/// ghostty drops a CSI whose final byte is not `m` when a colon separator was
/// seen; vte dispatches it. (Parser.zig:386-394.)
#[test]
fn divergence_colon_non_m_final() {
    let input = b"\x1b[38:2h";
    let ours = ghostty_events(input);
    let theirs = vte_events(input);

    assert!(
        ours.is_empty(),
        "ghostty should drop colon-subparam non-`m` CSI, got {ours:?}"
    );
    assert!(
        theirs.iter().any(|e| matches!(e, Event::Csi(_, _, b'h'))),
        "vte should still dispatch the `h`, got {theirs:?}"
    );
}

/// A paramless CSI yields an empty param slice in ghostty but a single `0` in
/// vte. (See divergence note 5.)
#[test]
fn divergence_empty_params() {
    let input = b"\x1b[H";
    let ours = ghostty_events(input);
    let theirs = vte_events(input);
    assert_eq!(ours, vec![Event::Csi(vec![], vec![], b'H')]);
    assert_eq!(theirs, vec![Event::Csi(vec![0], vec![], b'H')]);
}

/// ghostty saturates param accumulation at u16::MAX; vte's param type is also
/// u16 and saturates, so this one actually *agrees* — pin it so we notice if
/// either changes.
#[test]
fn agree_param_saturation() {
    assert_agree(b"\x1b[999999999m");
}

/// Param-count overflow: ghostty (max 24, drops whole dispatch) vs vte
/// (max 32, truncates). Feed 40 params. They must differ; we just require
/// ghostty to have dropped it.
#[test]
fn divergence_param_overflow() {
    let mut input = Vec::from(&b"\x1b["[..]);
    for _ in 0..40 {
        input.extend_from_slice(b"1;");
    }
    input.push(b'm');

    let ours = ghostty_events(&input);
    assert!(
        ours.is_empty(),
        "ghostty drops the dispatch on >24 params, got {ours:?}"
    );
    // vte keeps a (truncated) dispatch; we don't assert its exact shape.
    let theirs = vte_events(&input);
    assert!(
        theirs.iter().any(|e| matches!(e, Event::Csi(..))),
        "vte should still dispatch a truncated CSI, got {theirs:?}"
    );
}
