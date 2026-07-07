//! Shared helpers for the `key=value` lazy-field scanners used by several
//! OSC parsers (kitty color/text-sizing/dnd/clipboard, context_signal,
//! semantic_prompt). Each of those Zig files re-derives the same "scan
//! `;`- or `:`-separated `key=value` pairs, trimming ASCII whitespace
//! around the value" loop (`docs/analysis/osc.md`, "Per-parser structure"
//! intro) — this module factors the common shape out once.

/// The longest numeric OSC prefix (plus its trailing `;`) recognized by
/// [`super::parsers::dispatch`], e.g. `"5522;"`. Used only to bound
/// [`super::Parser::next`]'s safety cap when capture is not unbounded; the
/// real per-command `MAX_BUF` check happens in `dispatch` against the body
/// *after* the prefix, matching Zig's capture-starts-after-prefix timing.
pub const MAX_PREFIX_LEN: usize = "5522;".len();

/// Scan `metadata` for `key=value` pairs separated by `sep`, returning the
/// (whitespace-trimmed) value for the first pair whose key exactly matches
/// `key`, or `None` if not found. Case-sensitive key comparison (kitty's
/// dnd/clipboard protocols rely on this: `x` and `X` are distinct keys).
///
/// Port of the shared shape in `kitty_dnd_protocol.zig` `Option.read`
/// (`kitty_dnd_protocol.zig:115-147`) and `kitty_clipboard_protocol.zig`
/// `Option.read` (`kitty_clipboard_protocol.zig:88-136`).
pub fn read_key_value<'a>(metadata: &'a str, sep: char, key: &str) -> Option<&'a str> {
    let bytes = metadata.as_bytes();
    let mut pos = 0usize;
    while pos < bytes.len() {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= bytes.len() {
            return None;
        }
        if !metadata[pos..].starts_with(key) {
            pos = metadata[pos..].find(sep)? + pos + 1;
            continue;
        }
        pos += key.len();
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= bytes.len() {
            return None;
        }
        if bytes[pos] != b'=' {
            return None;
        }
        let end = metadata[pos..]
            .find(sep)
            .map(|i| pos + i)
            .unwrap_or(metadata.len());
        let start = pos + 1;
        return Some(metadata[start..end].trim_matches(|c: char| c.is_ascii_whitespace()));
    }
    None
}

/// Scan `raw` for `;`-separated `key=value` pairs (context_signal,
/// semantic_prompt shape: only `;` separated, first match wins, continues
/// past non-matching fields). Port of the shared shape in
/// `context_signal.zig` `Field.read` (`context_signal.zig:137-195`) and
/// `semantic_prompt.zig` `Option.read` (`semantic_prompt.zig:147-232`).
pub fn read_semicolon_field<'a>(raw: &'a str, key: &str) -> Option<&'a str> {
    let mut remaining = raw;
    loop {
        if remaining.is_empty() {
            return None;
        }
        let len = remaining.find(';').unwrap_or(remaining.len());
        let full = &remaining[..len];
        if let Some(eq) = full.find('=')
            && &full[..eq] == key
        {
            return Some(&full[eq + 1..]);
        }
        if len < remaining.len() {
            remaining = &remaining[len + 1..];
        } else {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_key_value_basic() {
        assert_eq!(read_key_value("t=a:i=5", ':', "t"), Some("a"));
        assert_eq!(read_key_value("t=a:i=5", ':', "i"), Some("5"));
        assert_eq!(read_key_value("t=a:i=5", ':', "x"), None);
    }

    #[test]
    fn read_key_value_case_sensitive() {
        assert_eq!(read_key_value("x=10:Y=200", ':', "x"), Some("10"));
        assert_eq!(read_key_value("x=10:Y=200", ':', "X"), None);
        assert_eq!(read_key_value("x=10:Y=200", ':', "Y"), Some("200"));
        assert_eq!(read_key_value("x=10:Y=200", ':', "y"), None);
    }

    #[test]
    fn read_semicolon_field_first_match_wins() {
        assert_eq!(read_semicolon_field("k=i;aid=last", "aid"), Some("last"));
        assert_eq!(read_semicolon_field("aid=first;k=i", "aid"), Some("first"));
        assert_eq!(read_semicolon_field(";;aid=value;;", "aid"), Some("value"));
        assert_eq!(read_semicolon_field("", "aid"), None);
        assert_eq!(read_semicolon_field("aid", "aid"), None);
    }
}
