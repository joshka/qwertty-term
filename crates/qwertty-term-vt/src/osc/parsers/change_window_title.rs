//! OSC 0/2: change window title. Port of
//! `osc/parsers/change_window_title.zig`.

use crate::osc::{Command, MAX_BUF};

/// Title mode is not modeled by the parser itself in ghostty (it's the
/// `stream.zig` layer's job to interpret hex-vs-UTF8-vs-Latin1 based on
/// terminal mode); this marker exists purely so the module doc references
/// something concrete. Not part of the Zig `Command` payload.
pub type TitleMode = ();

/// Parse OSC 0 and OSC 2. Port of `change_window_title.zig` `parse`.
///
/// `rest` is the OSC body after the numeric prefix, e.g. for `0;abc` this
/// is called with `";abc"` (matching Zig's `parser.capture` being active
/// only once the `;` is seen — a body of just `";"` yields the empty
/// title, and there is no valid "0" with no `;` at all, since the trie
/// itself requires it to reach a leaf state).
///
/// Zig's parser writes a trailing NUL into the (2048-byte) capture buffer
/// to null-terminate the `[:0]const u8` title, so a title of exactly
/// `MAX_BUF` bytes has nowhere for that NUL to go and the whole command
/// fails (`osc.zig`'s "exactly at buffer length" test) — `MAX_BUF - 1` is
/// the longest title that still fits. The Rust port doesn't need a NUL
/// terminator for a `String`, but reproduces this exact boundary for
/// fidelity.
pub fn parse(rest: &str) -> Option<Command> {
    let title = rest.strip_prefix(';')?;
    if title.len() >= MAX_BUF {
        return None;
    }
    Some(Command::ChangeWindowTitle(title.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    // Zig: change_window_title.zig "OSC 0: change_window_title".
    #[test]
    fn osc_0_change_window_title() {
        let mut p = osc::Parser::new();
        for c in "0;ab".bytes() {
            p.next(c);
        }
        assert_eq!(
            p.end(None),
            Some(Command::ChangeWindowTitle("ab".to_string()))
        );
    }

    // Zig: change_window_title.zig "OSC 0: longer than buffer".
    #[test]
    fn osc_0_longer_than_buffer() {
        let mut p = osc::Parser::new();
        for c in "0;".bytes() {
            p.next(c);
        }
        for _ in 0..(osc::MAX_BUF + 2) {
            p.next(b'a');
        }
        assert_eq!(p.end(None), None);
    }

    // Zig: change_window_title.zig "OSC 0: one shorter than buffer length".
    #[test]
    fn osc_0_one_shorter_than_buffer_length() {
        let mut p = osc::Parser::new();
        for c in "0;".bytes() {
            p.next(c);
        }
        let title = "a".repeat(osc::MAX_BUF - 1);
        for c in title.bytes() {
            p.next(c);
        }
        assert_eq!(p.end(None), Some(Command::ChangeWindowTitle(title)));
    }

    // Zig: change_window_title.zig "OSC 0: exactly at buffer length". Null
    // because Zig's buffer always reserves space for a NUL terminator; see
    // the `parse` doc comment above for how this port reproduces the
    // boundary without needing a real NUL reservation.
    #[test]
    fn osc_0_exactly_at_buffer_length() {
        let mut p = osc::Parser::new();
        for c in "0;".bytes() {
            p.next(c);
        }
        for _ in 0..osc::MAX_BUF {
            p.next(b'a');
        }
        assert_eq!(p.end(None), None);
    }

    // Zig: change_window_title.zig "OSC 2: change_window_title with 2".
    #[test]
    fn osc_2_change_window_title() {
        let mut p = osc::Parser::new();
        for c in "2;ab".bytes() {
            p.next(c);
        }
        assert_eq!(
            p.end(None),
            Some(Command::ChangeWindowTitle("ab".to_string()))
        );
    }

    // Zig: change_window_title.zig "OSC 2: change_window_title with utf8".
    #[test]
    fn osc_2_change_window_title_utf8() {
        let mut p = osc::Parser::new();
        // '—' EM DASH U+2014, ' ', '‐' HYPHEN U+2010 (chosen to conflict
        // with the 0x90 C1 control when misinterpreted byte-wise).
        for c in "2;— ‐".bytes() {
            p.next(c);
        }
        assert_eq!(
            p.end(None),
            Some(Command::ChangeWindowTitle("— ‐".to_string()))
        );
    }

    // Zig: change_window_title.zig "OSC 2: change_window_title empty".
    #[test]
    fn osc_2_change_window_title_empty() {
        let mut p = osc::Parser::new();
        for c in "2;".bytes() {
            p.next(c);
        }
        assert_eq!(p.end(None), Some(Command::ChangeWindowTitle(String::new())));
    }
}
