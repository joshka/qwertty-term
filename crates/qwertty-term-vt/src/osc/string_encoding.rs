//! Specialized string decoders needed by OSC 133 (`cmdline`/`cmdline_url`).
//!
//! Ported minimally from `src/os/string_encoding.zig` (a general OS-layer
//! helper, not terminal-specific — the `os/` prefix marks it as living
//! outside `src/terminal/` entirely). It has exactly one caller in the
//! whole ghostty codebase: `osc/parsers/semantic_prompt.zig`'s
//! `writeCommandLine`. Per `docs/analysis/osc.md`, only the two decode
//! functions that caller needs are ported (`printfQDecode`,
//! `urlPercentDecode`); the encode direction (`urlPercentEncode`, used by
//! config/CLI code elsewhere) is out of scope for this chunk.

/// Error returned when a buffer is not validly encoded. Port of
/// `string_encoding.zig`'s `error{DecodeError}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError;

/// Decode a buffer encoded the way bash's `printf %q` encodes a string,
/// appending the result to `out`. Port of `string_encoding.zig`
/// `printfQDecode` (`string_encoding.zig:6-66`).
pub fn printf_q_decode(out: &mut String, buf: &str) -> Result<(), DecodeError> {
    // Strip `$'...'` or `'...'` quoting.
    let data = if let Some(rest) = buf.strip_prefix("$'") {
        if buf.len() < 3 || !buf.ends_with('\'') {
            return Err(DecodeError);
        }
        &rest[..rest.len() - 1]
    } else if let Some(rest) = buf.strip_prefix('\'') {
        if buf.len() < 2 || !buf.ends_with('\'') {
            return Err(DecodeError);
        }
        &rest[..rest.len() - 1]
    } else {
        buf
    };

    let bytes = data.as_bytes();
    let mut src = 0;
    while src < bytes.len() {
        match bytes[src] {
            b'\\' => {
                if src + 1 >= bytes.len() {
                    return Err(DecodeError);
                }
                match bytes[src + 1] {
                    c @ (b' ' | b'\\' | b'"' | b'\'' | b'$') => {
                        out.push(c as char);
                        src += 2;
                    }
                    b'e' => {
                        out.push('\x1b');
                        src += 2;
                    }
                    b'n' => {
                        out.push('\n');
                        src += 2;
                    }
                    b'r' => {
                        out.push('\r');
                        src += 2;
                    }
                    b't' => {
                        out.push('\t');
                        src += 2;
                    }
                    b'v' => {
                        out.push('\x0b');
                        src += 2;
                    }
                    _ => return Err(DecodeError),
                }
            }
            c => {
                out.push(c as char);
                src += 1;
            }
        }
    }

    Ok(())
}

/// Decode a URL-percent-encoded buffer, appending the result to `out`.
/// Port of `string_encoding.zig` `urlPercentDecode` (`string_encoding.zig:
/// 166-191`).
pub fn url_percent_decode(out: &mut Vec<u8>, buf: &str) -> Result<(), DecodeError> {
    let bytes = buf.as_bytes();
    let mut src = 0;
    while src < bytes.len() {
        match bytes[src] {
            b'%' => {
                if src + 2 >= bytes.len() {
                    return Err(DecodeError);
                }
                let hi = hex_digit(bytes[src + 1]).ok_or(DecodeError)?;
                let lo = hex_digit(bytes[src + 2]).ok_or(DecodeError)?;
                out.push((hi << 4) | lo);
                src += 3;
            }
            c => {
                out.push(c);
                src += 1;
            }
        }
    }
    Ok(())
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Zig: string_encoding.zig "printf_q 1"..."printf_q 10" (10 tests).

    #[test]
    fn printf_q_1_escaped_space() {
        let mut out = String::new();
        printf_q_decode(&mut out, "bobr\\ kurwa").unwrap();
        assert_eq!(out, "bobr kurwa");
    }

    #[test]
    fn printf_q_2_escaped_newline() {
        let mut out = String::new();
        printf_q_decode(&mut out, "bobr\\nkurwa").unwrap();
        assert_eq!(out, "bobr\nkurwa");
    }

    #[test]
    fn printf_q_3_unknown_escape() {
        let mut out = String::new();
        assert_eq!(printf_q_decode(&mut out, "bobr\\dkurwa"), Err(DecodeError));
    }

    #[test]
    fn printf_q_4_trailing_backslash() {
        let mut out = String::new();
        assert_eq!(printf_q_decode(&mut out, "bobr kurwa\\"), Err(DecodeError));
    }

    #[test]
    fn printf_q_5_dollar_quote() {
        let mut out = String::new();
        printf_q_decode(&mut out, "$'bobr kurwa'").unwrap();
        assert_eq!(out, "bobr kurwa");
    }

    #[test]
    fn printf_q_6_plain_quote() {
        let mut out = String::new();
        printf_q_decode(&mut out, "'bobr kurwa'").unwrap();
        assert_eq!(out, "bobr kurwa");
    }

    #[test]
    fn printf_q_7_unterminated_dollar_quote() {
        let mut out = String::new();
        assert_eq!(printf_q_decode(&mut out, "$'bobr kurwa"), Err(DecodeError));
    }

    #[test]
    fn printf_q_8_bare_dollar_quote() {
        let mut out = String::new();
        assert_eq!(printf_q_decode(&mut out, "$'"), Err(DecodeError));
    }

    #[test]
    fn printf_q_9_unterminated_plain_quote() {
        let mut out = String::new();
        assert_eq!(printf_q_decode(&mut out, "'bobr kurwa"), Err(DecodeError));
    }

    #[test]
    fn printf_q_10_bare_quote() {
        let mut out = String::new();
        assert_eq!(printf_q_decode(&mut out, "'"), Err(DecodeError));
    }

    // Zig: string_encoding.zig "singles percent", "percent 1".."percent 7"
    // (7 tests; "singles percent" folded into one loop test here as in Zig).

    #[test]
    fn url_percent_singles() {
        for c in 0u8..255 {
            let mut out = Vec::new();
            let buf = format!("%{c:02x}");
            url_percent_decode(&mut out, &buf).unwrap();
            assert_eq!(out, vec![c]);

            let mut out = Vec::new();
            let buf = format!("%{c:02X}");
            url_percent_decode(&mut out, &buf).unwrap();
            assert_eq!(out, vec![c]);
        }
    }

    #[test]
    fn url_percent_1_space() {
        let mut out = Vec::new();
        url_percent_decode(&mut out, "bobr%20kurwa").unwrap();
        assert_eq!(out, b"bobr kurwa");
    }

    #[test]
    fn url_percent_2_truncated() {
        let mut out = Vec::new();
        assert_eq!(
            url_percent_decode(&mut out, "bobr%2kurwa"),
            Err(DecodeError)
        );
    }

    #[test]
    fn url_percent_3_bare() {
        let mut out = Vec::new();
        assert_eq!(url_percent_decode(&mut out, "bobr%kurwa"), Err(DecodeError));
    }

    #[test]
    fn url_percent_4_double_percent() {
        let mut out = Vec::new();
        assert_eq!(
            url_percent_decode(&mut out, "bobr%%kurwa"),
            Err(DecodeError)
        );
    }

    #[test]
    fn url_percent_5_trailing_valid() {
        let mut out = Vec::new();
        url_percent_decode(&mut out, "bobr%20kurwa%20").unwrap();
        assert_eq!(out, b"bobr kurwa ");
    }

    #[test]
    fn url_percent_6_trailing_truncated() {
        let mut out = Vec::new();
        assert_eq!(
            url_percent_decode(&mut out, "bobr%20kurwa%2"),
            Err(DecodeError)
        );
    }

    #[test]
    fn url_percent_7_trailing_bare() {
        let mut out = Vec::new();
        assert_eq!(
            url_percent_decode(&mut out, "bobr%20kurwa%"),
            Err(DecodeError)
        );
    }
}
