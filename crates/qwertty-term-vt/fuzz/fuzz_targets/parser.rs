//! Fuzz target: arbitrary bytes through the VT parser and UTF-8 decoder.
//!
//! Invariant: `qwertty-term-vt` must never panic on arbitrary input, and the
//! parser's internal accumulators must stay within bounds. This drives both
//! the byte-oriented [`Parser`] directly and the [`Utf8Decoder`] the way the
//! stream layer composes them (decode in ground state, feed control bytes to
//! the parser).
//!
//! Run (nightly + cargo-fuzz required):
//!   cargo +nightly fuzz run parser -- -dict=parser.dict -max_total_time=60

#![no_main]

use qwertty_term_vt::parser::Parser;
use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};
use qwertty_term_vt::utf8_decoder::Utf8Decoder;
use libfuzzer_sys::fuzz_target;

/// Build a small terminal + stream for the feed-path fuzz below.
fn stream() -> Stream<TerminalHandler> {
    let terminal = Terminal::new(Options {
        cols: 40,
        rows: 12,
        max_scrollback: 0,
        colors: Default::default(),
    });
    Stream::new(TerminalHandler::new(terminal))
}

fuzz_target!(|data: &[u8]| {
    // 1. Raw bytes straight through the parser. Assert internal bounds after
    //    every byte so an overflow shows up here rather than corrupting state.
    let mut parser = Parser::new();
    for &b in data {
        let actions = parser.next(b);
        // Force materialization of any borrowed slices (indexing into the
        // parser's arrays) to catch out-of-bounds slicing.
        for action in &actions {
            std::hint::black_box(action);
        }
        parser.assert_bounded();
    }

    // 2. Bytes through the UTF-8 decoder, forwarding to the parser the way the
    //    stream layer does. Honors the decoder's "re-feed on non-consumed"
    //    contract, with a re-feed cap so a broken contract can't spin forever.
    let mut decoder = Utf8Decoder::new();
    let mut parser = Parser::new();
    for &b in data {
        let mut consumed = false;
        let mut guard = 0u8;
        while !consumed {
            let (cp, c) = decoder.next(b);
            consumed = c;
            if let Some(cp) = cp {
                if (cp as u32) <= 0x1f {
                    let actions = parser.next(cp as u32 as u8);
                    for action in &actions {
                        std::hint::black_box(action);
                    }
                    parser.assert_bounded();
                } else {
                    std::hint::black_box(cp);
                }
            }
            guard += 1;
            assert!(guard < 4, "decoder re-fed the same byte too many times");
        }
    }

    // 3. Bytes through the full `Stream::feed` fast paths (csi_entry /
    //    csi_param bulk consume, ground-run batching), and separately
    //    byte-at-a-time through `Stream::next` (pure state machine). Both must
    //    (a) never panic and (b) reach identical terminal state — a fast-path-
    //    vs-state-machine differential over arbitrary input.
    let mut fast = stream();
    fast.feed(data);
    let mut slow = stream();
    for &b in data {
        slow.next(b);
    }
    let fast_screen = fast.handler.terminal.screen().dump_string(
        qwertty_term_vt::point::Tag::Screen,
        false,
    );
    let slow_screen = slow.handler.terminal.screen().dump_string(
        qwertty_term_vt::point::Tag::Screen,
        false,
    );
    assert_eq!(
        fast_screen, slow_screen,
        "Stream::feed fast path diverged from byte-at-a-time state machine"
    );
});
