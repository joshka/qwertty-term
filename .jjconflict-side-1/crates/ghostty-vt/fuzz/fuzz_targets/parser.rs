//! Fuzz target: arbitrary bytes through the VT parser and UTF-8 decoder.
//!
//! Invariant: `ghostty-vt` must never panic on arbitrary input, and the
//! parser's internal accumulators must stay within bounds. This drives both
//! the byte-oriented [`Parser`] directly and the [`Utf8Decoder`] the way the
//! stream layer composes them (decode in ground state, feed control bytes to
//! the parser).
//!
//! Run (nightly + cargo-fuzz required):
//!   cargo +nightly fuzz run parser -- -max_total_time=60

#![no_main]

use ghostty_vt::parser::Parser;
use ghostty_vt::utf8_decoder::Utf8Decoder;
use libfuzzer_sys::fuzz_target;

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
});
