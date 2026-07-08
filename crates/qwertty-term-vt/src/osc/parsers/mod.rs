//! Dispatch from a captured OSC body to the per-command parser modules.
//!
//! Port of `osc.zig`'s `State` prefix trie (`osc.zig:318-371`) +
//! `Parser.end`'s dispatch table (`osc.zig:760-836`), flattened into a
//! single string-prefix match performed once the whole body is captured
//! (see `docs/analysis/osc.md` divergence #2 for why this is equivalent).

pub mod change_window_icon;
pub mod change_window_title;
pub mod clipboard_operation;
pub mod color;
pub mod context_signal;
pub mod hyperlink;
pub mod iterm2;
pub mod kitty_clipboard_protocol;
pub mod kitty_color;
pub mod kitty_dnd_protocol;
pub mod kitty_text_sizing;
pub mod mouse_shape;
pub mod osc9;
pub mod report_pwd;
pub mod rxvt_extension;
pub mod semantic_prompt;

use crate::osc::{Command, MAX_BUF, Terminator};

/// Dispatch a fully-captured OSC body (the raw bytes between `ESC ]` and
/// the terminator) to the matching command parser.
///
/// `allow_unbounded` mirrors whether `Parser` was constructed with (Rust)
/// or without (Zig: `alloc: null`) allocator permission — commands that
/// require it (see `docs/analysis/osc.md`'s capture-mode table) fail to
/// parse without it, exactly like `ensureAllocator` in the Zig source.
pub(super) fn dispatch(
    buf: &[u8],
    terminator_ch: Option<u8>,
    allow_unbounded: bool,
) -> Option<Command> {
    // The body must be valid UTF-8 for the string-oriented parsers below.
    // Ghostty's OSC strings are raw bytes (see vt-parser.md: "OSC strings
    // accept raw high bytes"); non-UTF-8 payloads are exceedingly rare in
    // practice (window titles/URIs/etc. are all conventionally UTF-8) and
    // are treated as invalid here rather than adding a byte-oriented
    // parallel path for every sub-parser.
    let body = std::str::from_utf8(buf).ok()?;
    let terminator = Terminator::init(terminator_ch);

    // Numeric prefix dispatch, mirroring osc.zig's trie leaf states.
    // Ordering follows osc.zig's own state list for easy diffing.
    if let Some(rest) = strip_num_prefix(body, "0") {
        return fixed(rest, allow_unbounded, change_window_title::parse);
    }
    if let Some(rest) = strip_num_prefix(body, "1") {
        return fixed(rest, allow_unbounded, change_window_icon::parse);
    }
    if let Some(rest) = strip_num_prefix(body, "2") {
        return fixed(rest, allow_unbounded, change_window_title::parse);
    }
    if let Some(rest) = strip_num_prefix(body, "3008") {
        return fixed(rest, allow_unbounded, context_signal::parse);
    }
    if let Some(rest) = strip_num_prefix(body, "4") {
        return unbounded(rest, allow_unbounded, |r| {
            color::parse(color::Op::Osc4, r, terminator)
        });
    }
    if let Some(rest) = strip_num_prefix(body, "5") {
        return unbounded(rest, allow_unbounded, |r| {
            color::parse(color::Op::Osc5, r, terminator)
        });
    }
    if let Some(rest) = strip_num_prefix(body, "7") {
        return fixed(rest, allow_unbounded, report_pwd::parse);
    }
    if let Some(rest) = strip_num_prefix(body, "8") {
        return fixed(rest, allow_unbounded, hyperlink::parse);
    }
    if let Some(rest) = strip_num_prefix(body, "9") {
        return fixed(rest, allow_unbounded, osc9::parse);
    }
    if let Some(rest) = strip_num_prefix(body, "21") {
        return unbounded(rest, allow_unbounded, |r| kitty_color::parse(r, terminator));
    }
    if let Some(rest) = strip_num_prefix(body, "22") {
        return fixed(rest, allow_unbounded, mouse_shape::parse);
    }
    if let Some(rest) = strip_num_prefix(body, "52") {
        return unbounded(rest, allow_unbounded, clipboard_operation::parse);
    }
    if let Some(rest) = strip_num_prefix(body, "66") {
        return unbounded(rest, allow_unbounded, kitty_text_sizing::parse);
    }
    if let Some(rest) = strip_num_prefix(body, "72") {
        return unbounded(rest, allow_unbounded, |r| {
            kitty_dnd_protocol::parse(r, terminator)
        });
    }
    if let Some(rest) = strip_num_prefix(body, "104") {
        return fixed(rest, allow_unbounded, |r| {
            color::parse(color::Op::Osc104, r, terminator)
        });
    }
    if let Some(rest) = strip_num_prefix(body, "105") {
        return fixed(rest, allow_unbounded, |r| {
            color::parse(color::Op::Osc105, r, terminator)
        });
    }
    for n in 10..=19u32 {
        let tag = n.to_string();
        if let Some(rest) = strip_num_prefix(body, &tag) {
            let op = color::Op::from_osc_number(n).unwrap();
            return unbounded(rest, allow_unbounded, move |r| {
                color::parse(op, r, terminator)
            });
        }
    }
    for n in 110..=119u32 {
        let tag = n.to_string();
        if let Some(rest) = strip_num_prefix(body, &tag) {
            let op = color::Op::from_osc_number(n).unwrap();
            return fixed(rest, allow_unbounded, move |r| {
                color::parse(op, r, terminator)
            });
        }
    }
    if let Some(rest) = strip_num_prefix(body, "133") {
        return fixed(rest, allow_unbounded, semantic_prompt::parse);
    }
    if let Some(rest) = strip_num_prefix(body, "777") {
        return fixed(rest, allow_unbounded, rxvt_extension::parse);
    }
    if let Some(rest) = strip_num_prefix(body, "1337") {
        return fixed(rest, allow_unbounded, iterm2::parse);
    }
    if let Some(rest) = strip_num_prefix(body, "5522") {
        return unbounded(rest, allow_unbounded, |r| {
            kitty_clipboard_protocol::parse(r, terminator)
        });
    }

    // 3, 30, 300, 6, 55, 552, 77: recognized bridge prefixes with no
    // command (osc.zig:809-828).
    None
}

/// Strip a numeric OSC prefix (e.g. `"104"`) followed by either end-of-body
/// or `;`, returning the remainder (including the leading `;` if present,
/// so per-command parsers can distinguish "no body" from "empty body"
/// exactly like the Zig source's captured-data slicing). Returns `None` if
/// `body` doesn't start with exactly this numeric prefix followed by a
/// non-digit (so e.g. matching `"1"` doesn't also match `"10"`'s prefix).
fn strip_num_prefix<'a>(body: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = body.strip_prefix(prefix)?;
    match rest.as_bytes().first() {
        None => Some(rest),
        Some(b';') => Some(rest),
        Some(c) if c.is_ascii_digit() => None,
        Some(_) => None,
    }
}

/// Run `f` over the body remainder (after the prefix), enforcing the
/// fixed-capture `MAX_BUF` limit — `rest` may start with a leading `;`,
/// which does not count against the cap (mirrors Zig: capture starts at
/// the byte *after* that `;`). Commands whose Zig parser additionally
/// reserves a byte for a NUL terminator (change_window_title,
/// change_window_icon, iterm2, kitty_text_sizing) enforce that tighter
/// bound themselves, in their own `parse`, since it's specific to how that
/// parser builds its `[:0]const u8` payload — not a property of capture
/// itself (most fixed-capture commands, e.g. report_pwd/hyperlink, don't
/// need a NUL and so don't reserve the byte).
fn fixed(
    rest: &str,
    _allow_unbounded: bool,
    f: impl FnOnce(&str) -> Option<Command>,
) -> Option<Command> {
    let body_only = rest.strip_prefix(';').unwrap_or(rest);
    if body_only.len() > MAX_BUF {
        return None;
    }
    f(rest)
}

/// Like [`fixed`], but the command requires `allow_unbounded` (an
/// allocator, in Zig terms) — without it, `ensureAllocator` invalidates the
/// whole parse (`osc.zig:449-454`).
fn unbounded(
    rest: &str,
    allow_unbounded: bool,
    f: impl FnOnce(&str) -> Option<Command>,
) -> Option<Command> {
    if !allow_unbounded {
        return None;
    }
    f(rest)
}

/// Kitty's "Escape code safe UTF-8": valid UTF-8 with no C0 escape codes
/// (0x00-0x1F), DEL (0x7F), or C1 escape codes (0x80-0x9F). Port of
/// `osc/encoding.zig` `isSafeUtf8`. Used by OSC 66 (text sizing); ghostty
/// also uses it for OSC 99 (kitty notifications, not yet implemented
/// upstream either, per `osc.zig`'s dispatch table having no OSC 99 entry).
pub(super) fn is_safe_utf8(s: &str) -> bool {
    s.chars().all(|c| {
        let cp = c as u32;
        !(cp <= 0x1f || cp == 0x7f || (0x80..=0x9f).contains(&cp))
    })
}

#[cfg(test)]
mod encoding_tests {
    use super::is_safe_utf8;

    // Zig: osc/encoding.zig "isSafeUtf8".
    #[test]
    fn is_safe_utf8_cases() {
        assert!(is_safe_utf8("Hello world!"));
        assert!(is_safe_utf8("安全的ユニコード☀️"));
        assert!(!is_safe_utf8("No linebreaks\nallowed"));
        assert!(!is_safe_utf8("\x07no bells"));
        assert!(!is_safe_utf8("\x1b]9;no OSCs\x1b\\\x1b[m"));
        assert!(!is_safe_utf8("\u{9f}8-bit escapes are clever, but no"));
    }
}
