//! The escaped-byte fixture convention shared with
//! `crates/spike/tests/fixtures/replay`.
//!
//! `.esc` files hold a VT byte stream as printable text: `\e` = ESC, `\n`,
//! `\r`, `\t`, `\\` as usual, `\xHH` for arbitrary bytes; anything else is
//! literal UTF-8. `corpus/` cases and the spike replay fixtures both use it.

/// Decode the `.esc` text form into the raw byte stream.
///
/// Unrecognized escapes (`\z`) are kept literally (backslash + char), and a
/// trailing lone backslash is kept as a backslash, matching the spike
/// decoder byte-for-byte.
pub fn decode_escaped_stream(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            push_utf8(&mut out, ch);
            continue;
        }
        match chars.next() {
            Some('e') => out.push(0x1b),
            Some('n') => out.push(b'\n'),
            Some('r') => out.push(b'\r'),
            Some('t') => out.push(b'\t'),
            Some('\\') => out.push(b'\\'),
            Some('x') => {
                let hi = chars.next().and_then(|c| c.to_digit(16)).expect("hex hi") as u8;
                let lo = chars.next().and_then(|c| c.to_digit(16)).expect("hex lo") as u8;
                out.push((hi << 4) | lo);
            }
            Some(other) => {
                out.push(b'\\');
                push_utf8(&mut out, other);
            }
            None => out.push(b'\\'),
        }
    }
    out
}

fn push_utf8(out: &mut Vec<u8>, ch: char) {
    let mut buf = [0; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_named_escapes() {
        assert_eq!(
            decode_escaped_stream("\\e[2J\\r\\n\\t\\\\"),
            b"\x1b[2J\r\n\t\\"
        );
    }

    #[test]
    fn decodes_hex_bytes() {
        assert_eq!(
            decode_escaped_stream("\\x1b\\x08\\xe4\\xb8\\xad"),
            b"\x1b\x08\xe4\xb8\xad"
        );
    }

    #[test]
    fn keeps_unknown_escapes_literal() {
        assert_eq!(decode_escaped_stream("\\z"), b"\\z");
        assert_eq!(decode_escaped_stream("tail\\"), b"tail\\");
    }

    #[test]
    fn passes_utf8_through() {
        assert_eq!(decode_escaped_stream("中"), "中".as_bytes());
    }
}
