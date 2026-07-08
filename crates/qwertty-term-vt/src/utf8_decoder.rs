//! DFA-based non-allocating error-replacing UTF-8 decoder.
//!
//! Ported from ghostty `src/terminal/UTF8Decoder.zig` (commit `2da015cd6`),
//! which is based largely on the excellent work of Bjoern Hoehrmann, with
//! slight modifications to support error-replacement.
//!
//! For details on Bjoern's DFA-based UTF-8 decoder, see
//! <http://bjoern.hoehrmann.de/utf-8/decoder/dfa> (MIT licensed).

#[rustfmt::skip]
const CHAR_CLASSES: [u8; 256] = [
   0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
   0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
   0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
   0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,  0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
   1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,  9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,
   7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,  7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
   8,8,2,2,2,2,2,2,2,2,2,2,2,2,2,2,  2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,
  10,3,3,3,3,3,3,3,3,3,3,3,3,4,3,3, 11,6,6,6,5,8,8,8,8,8,8,8,8,8,8,8,
];

#[rustfmt::skip]
const TRANSITIONS: [u8; 108] = [
   0,12,24,36,60,96,84,12,12,12,48,72, 12,12,12,12,12,12,12,12,12,12,12,12,
  12, 0,12,12,12,12,12, 0,12, 0,12,12, 12,24,12,12,12,12,12,24,12,24,12,12,
  12,12,12,12,12,12,12,24,12,12,12,12, 12,24,12,12,12,12,12,12,12,24,12,12,
  12,12,12,12,12,12,12,36,12,36,12,12, 12,36,12,12,12,12,12,36,12,36,12,12,
  12,36,12,12,12,12,12,12,12,12,12,12,
];

// DFA states
const ACCEPT_STATE: u8 = 0;
const REJECT_STATE: u8 = 12;

/// Streaming UTF-8 decoder. One instance decodes one byte stream; state is
/// carried between [`Utf8Decoder::next`] calls.
#[derive(Debug, Default)]
pub struct Utf8Decoder {
    /// This is where we accumulate our current codepoint.
    accumulator: u32,
    /// The internal state of the DFA.
    state: u8,
}

impl Utf8Decoder {
    pub const fn new() -> Self {
        Self {
            accumulator: 0,
            state: ACCEPT_STATE,
        }
    }

    /// True if the decoder is in the middle of a multi-byte sequence.
    /// (The stream layer drains the decoder before switching to raw
    /// parser input; stream.zig:531-537.)
    pub const fn is_partial(&self) -> bool {
        self.state != ACCEPT_STATE
    }

    /// Takes the next byte in the utf-8 sequence and returns a tuple of
    /// - The codepoint that was generated, if there is one.
    /// - A boolean that indicates whether the provided byte was consumed.
    ///
    /// The only case where the byte is not consumed is if an ill-formed
    /// sequence is reached, in which case a replacement character will be
    /// emitted and the byte will not be consumed.
    ///
    /// If the byte is not consumed, the caller is responsible for calling
    /// again with the same byte before continuing.
    pub fn next(&mut self, byte: u8) -> (Option<char>, bool) {
        let char_class = CHAR_CLASSES[byte as usize];

        let initial_state = self.state;

        if self.state != ACCEPT_STATE {
            self.accumulator <<= 6;
            self.accumulator |= (byte & 0x3F) as u32;
        } else {
            self.accumulator = (0xFF >> char_class) & byte as u32;
        }

        self.state = TRANSITIONS[(self.state + char_class) as usize];

        if self.state == ACCEPT_STATE {
            let cp = self.accumulator;
            self.accumulator = 0;

            // The DFA only accepts well-formed sequences (no surrogates,
            // no overlongs, nothing above U+10FFFF), so `cp` is always a
            // valid scalar here; the fallback is unreachable.
            debug_assert!(char::from_u32(cp).is_some());
            let ch = char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER);

            // Emit the fully decoded codepoint.
            (Some(ch), true)
        } else if self.state == REJECT_STATE {
            self.accumulator = 0;
            self.state = ACCEPT_STATE;
            // Emit a replacement character. If we rejected the first byte
            // in a sequence, then it was consumed, otherwise it was not.
            (
                Some(char::REPLACEMENT_CHARACTER),
                initial_state == ACCEPT_STATE,
            )
        } else {
            // Emit nothing, we're in the middle of a sequence.
            (None, true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Zig: UTF8Decoder.zig "ASCII"
    #[test]
    fn ascii() {
        let mut d = Utf8Decoder::new();
        let mut out = String::new();
        for byte in "Hello, World!".bytes() {
            let (cp, consumed) = d.next(byte);
            assert!(consumed);
            if let Some(cp) = cp {
                out.push(cp);
            }
        }

        assert_eq!(out, "Hello, World!");
    }

    // Zig: UTF8Decoder.zig "Well formed utf-8"
    #[test]
    fn well_formed_utf8() {
        let mut d = Utf8Decoder::new();
        let mut out = Vec::new();
        // 4 bytes, 3 bytes, 2 bytes, 1 byte
        for byte in "😄✤ÁA".bytes() {
            let mut consumed = false;
            while !consumed {
                let (cp, c) = d.next(byte);
                consumed = c;
                // There are no errors in this sequence, so
                // every byte should be consumed first try.
                assert!(consumed);
                if let Some(cp) = cp {
                    out.push(cp);
                }
            }
        }

        assert_eq!(out, ['\u{1F604}', '\u{2724}', '\u{C1}', '\u{41}']);
    }

    // Zig: UTF8Decoder.zig "Partially invalid utf-8"
    #[test]
    fn partially_invalid_utf8() {
        let mut d = Utf8Decoder::new();
        let mut out = Vec::new();
        // Illegally terminated sequence, valid sequence, illegal
        // surrogate pair.
        for &byte in b"\xF0\x9F\xF0\x9F\x98\x84\xED\xA0\x80" {
            let mut consumed = false;
            while !consumed {
                let (cp, c) = d.next(byte);
                consumed = c;
                if let Some(cp) = cp {
                    out.push(cp);
                }
            }
        }

        assert_eq!(
            out,
            ['\u{FFFD}', '\u{1F604}', '\u{FFFD}', '\u{FFFD}', '\u{FFFD}']
        );
    }
}
