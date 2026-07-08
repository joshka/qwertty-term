//! Bracketed-paste wrapping and control-char stripping (port of
//! `input/paste.zig`).
//!
//! The Zig original exposes a single `encode` function that is generic over
//! `[]u8` (mutable, infallible) vs `[]const u8` (const, fallible with
//! `Error.MutableRequired` if a copy would be needed). Rust has no equivalent
//! "generic over mutability" trick worth reproducing here, so this port
//! collapses both branches into one simple infallible function that always
//! allocates a fresh `String` for the body. See [`encode`] for details.

/// The set of byte values that are always replaced by a space (per xterm's
/// behavior) for any text insertion method e.g. a paste, drag and drop, etc.
/// These are copied directly from xterm's source. Port of the `strip` local
/// constant in `paste.zig`'s `encode`.
const STRIP: &[u8] = &[
    0x00, // NUL
    0x08, // BS
    0x05, // ENQ
    0x04, // EOT
    0x1B, // ESC
    0x7F, // DEL
    // These can be overridden by the running terminal program via tcsetattr,
    // so they aren't totally safe to hardcode like this. In practice, I
    // haven't seen modern programs change these and its a much bigger
    // architectural change to pass these through so for now they're
    // hardcoded.
    0x03, // VINTR (Ctrl+C)
    0x1C, // VQUIT (Ctrl+\)
    0x15, // VKILL (Ctrl+U)
    0x1A, // VSUSP (Ctrl+Z)
    0x11, // VSTART (Ctrl+Q)
    0x13, // VSTOP (Ctrl+S)
    0x17, // VWERASE (Ctrl+W)
    0x16, // VLNEXT (Ctrl+V)
    0x12, // VREPRINT (Ctrl+R)
    0x0F, // VDISCARD (Ctrl+O)
];

/// Options controlling paste encoding. Port of `paste.Options`. The Zig
/// `fromTerminal` constructor is skipped here since it depends on
/// `qwertty-term-vt`'s `Terminal`; callers should read `t.modes.get(.bracketed_paste)`
/// themselves and construct this directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    /// True if bracketed paste mode is on.
    pub bracketed: bool,
}

/// Encode the given data for pasting. The resulting value can be written to
/// the pty to perform a paste of the input data. Returns `(prefix, body,
/// suffix)`; the caller can concatenate the three to get the full byte
/// sequence to write.
///
/// Unlike the Zig original (which had a separate const/fallible and
/// mutable/infallible path to avoid allocating when no modification was
/// needed), this port always allocates a fresh `String` for the body and
/// always succeeds. This is simpler to use correctly and the allocation cost
/// is negligible relative to a paste operation.
///
/// WARNING: The input data is not checked for safety. See [`is_safe`] to
/// check if the data is safe to paste.
pub fn encode(data: &str, opts: Options) -> (String, String, String) {
    // If we have any of the strip values, then we need to replace them with
    // spaces. This is what xterm does and it does it regardless of
    // bracketed paste mode. This is a security measure to prevent pastes
    // from containing bytes that could be used to inject commands.
    let mut body: Vec<u8> = data.as_bytes().to_vec();
    for b in body.iter_mut() {
        if STRIP.contains(b) {
            *b = b' ';
        }
    }

    // Bracketed paste mode (mode 2004) wraps pasted data in fenceposts so
    // that the terminal can ignore things like newlines.
    if opts.bracketed {
        let body = String::from_utf8(body)
            .expect("stripping STRIP bytes (all ASCII) from valid UTF-8 preserves UTF-8 validity");
        return ("\x1b[200~".to_string(), body, "\x1b[201~".to_string());
    }

    // Non-bracketed. We have to replace newline with `\r`. This matches the
    // behavior of xterm and other terminals. For `\r\n` this will result in
    // `\r\r` which does match xterm.
    for b in body.iter_mut() {
        if *b == b'\n' {
            *b = b'\r';
        }
    }

    let body = String::from_utf8(body)
        .expect("stripping/newline-replacing ASCII bytes in valid UTF-8 preserves UTF-8 validity");
    (String::new(), body, String::new())
}

/// Returns true if the data looks safe to paste. Data is considered unsafe
/// if it contains any of the following:
///
/// - `\n`: Newlines can be used to inject commands.
/// - `\x1b[201~`: This is the end of a bracketed paste. This can be used to
///   exit a bracketed paste and inject commands.
///
/// We consider any scenario unsafe regardless of current terminal state. For
/// example, even if bracketed paste mode is not active, we still consider
/// `\x1b[201~` unsafe. The existence of these types of bytes should raise
/// suspicion that the producer of the paste data is acting strangely.
pub fn is_safe(data: &[u8]) -> bool {
    !data.contains(&b'\n') && !data.windows(b"\x1b[201~".len()).any(|w| w == b"\x1b[201~")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of `test isSafe`.
    #[test]
    fn is_safe_test() {
        assert!(is_safe(b"hello"));
        assert!(!is_safe(b"hello\n"));
        assert!(!is_safe(b"hello\nworld"));
        assert!(!is_safe(b"he\x1b[201~llo"));
    }

    // Port of `test "encode bracketed"`.
    #[test]
    fn encode_bracketed() {
        let (prefix, body, suffix) = encode("hello", Options { bracketed: true });
        assert_eq!(prefix, "\x1b[200~");
        assert_eq!(body, "hello");
        assert_eq!(suffix, "\x1b[201~");
    }

    // Port of `test "encode unbracketed no newlines"`.
    #[test]
    fn encode_unbracketed_no_newlines() {
        let (prefix, body, suffix) = encode("hello", Options { bracketed: false });
        assert_eq!(prefix, "");
        assert_eq!(body, "hello");
        assert_eq!(suffix, "");
    }

    // Port of `test "encode unbracketed newlines const"` and
    // `test "encode unbracketed newlines"`.
    //
    // The "...const" test in Zig exercised the `Error.MutableRequired`
    // error path taken when const data contains a `\n` and needs a mutable
    // copy to rewrite in place. Our API is always infallible (it always
    // allocates a fresh `String`), so there is no error path to replicate.
    // The underlying newline-rewriting behavior that test was protecting is
    // still exercised here, matching the Zig "mutable" variant of the test.
    #[test]
    fn encode_unbracketed_newlines() {
        let (prefix, body, suffix) = encode("hello\nworld", Options { bracketed: false });
        assert_eq!(prefix, "");
        assert_eq!(body, "hello\rworld");
        assert_eq!(suffix, "");
    }

    // Port of `test "encode unbracketed windows-stye newline"`.
    #[test]
    fn encode_unbracketed_windows_style_newline() {
        let (prefix, body, suffix) = encode("hello\r\nworld", Options { bracketed: false });
        assert_eq!(prefix, "");
        assert_eq!(body, "hello\r\rworld");
        assert_eq!(suffix, "");
    }

    // Port of `test "encode strip unsafe bytes const"` and
    // `test "encode strip unsafe bytes mutable bracketed"`.
    //
    // As above, the "...const" error-path test doesn't apply to our
    // infallible API. The strip behavior it was protecting is exercised by
    // this bracketed-mutable-equivalent test.
    #[test]
    fn encode_strip_unsafe_bytes_bracketed() {
        let (prefix, body, suffix) = encode("hel\x1blo\x00world", Options { bracketed: true });
        assert_eq!(prefix, "\x1b[200~");
        assert_eq!(body, "hel lo world");
        assert_eq!(suffix, "\x1b[201~");
    }

    // Port of `test "encode strip unsafe bytes mutable unbracketed"`.
    #[test]
    fn encode_strip_unsafe_bytes_unbracketed() {
        let (prefix, body, suffix) = encode("hel\x03lo", Options { bracketed: false });
        assert_eq!(prefix, "");
        assert_eq!(body, "hel lo");
        assert_eq!(suffix, "");
    }

    // Port of `test "encode strip multiple unsafe bytes"`.
    #[test]
    fn encode_strip_multiple_unsafe_bytes() {
        let (_, body, _) = encode("\x00\x08\x7f", Options { bracketed: true });
        assert_eq!(body, "   ");
    }
}
