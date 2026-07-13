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
    // Ghostty parses OSC bodies as raw bytes (`[]u8`) with no UTF-8 gate, so a
    // stray non-UTF-8 byte in a trailing/opaque field must NOT discard the whole
    // command (issue #169). The numeric prefix and structural markers are always
    // ASCII, so we dispatch on the raw bytes and hand each sub-parser a *lossy*
    // string (invalid bytes → U+FFFD); the ASCII structure survives intact and
    // the only affected content is opaque value fields (titles/URIs/options),
    // which no terminal behavior depends on. OSC 66 is the exception: it gates
    // on `is_safe_utf8`, which rejects non-UTF-8, so it takes the strict path.
    let terminator = Terminator::init(terminator_ch);

    // Numeric prefix dispatch, mirroring osc.zig's trie leaf states.
    // Ordering follows osc.zig's own state list for easy diffing.
    if let Some(rest) = strip_num_prefix(buf, b"0") {
        return fixed(rest, allow_unbounded, change_window_title::parse);
    }
    if let Some(rest) = strip_num_prefix(buf, b"1") {
        return fixed(rest, allow_unbounded, change_window_icon::parse);
    }
    if let Some(rest) = strip_num_prefix(buf, b"2") {
        return fixed(rest, allow_unbounded, change_window_title::parse);
    }
    if let Some(rest) = strip_num_prefix(buf, b"3008") {
        return fixed(rest, allow_unbounded, context_signal::parse);
    }
    if let Some(rest) = strip_num_prefix(buf, b"4") {
        return unbounded(rest, allow_unbounded, |r| {
            color::parse(color::Op::Osc4, r, terminator)
        });
    }
    if let Some(rest) = strip_num_prefix(buf, b"5") {
        return unbounded(rest, allow_unbounded, |r| {
            color::parse(color::Op::Osc5, r, terminator)
        });
    }
    if let Some(rest) = strip_num_prefix(buf, b"7") {
        return fixed(rest, allow_unbounded, report_pwd::parse);
    }
    if let Some(rest) = strip_num_prefix(buf, b"8") {
        return fixed(rest, allow_unbounded, hyperlink::parse);
    }
    if let Some(rest) = strip_num_prefix(buf, b"9") {
        return fixed(rest, allow_unbounded, osc9::parse);
    }
    if let Some(rest) = strip_num_prefix(buf, b"21") {
        return unbounded(rest, allow_unbounded, |r| kitty_color::parse(r, terminator));
    }
    if let Some(rest) = strip_num_prefix(buf, b"22") {
        return fixed(rest, allow_unbounded, mouse_shape::parse);
    }
    if let Some(rest) = strip_num_prefix(buf, b"52") {
        return unbounded(rest, allow_unbounded, clipboard_operation::parse);
    }
    if let Some(rest) = strip_num_prefix(buf, b"66") {
        // OSC 66 gates on `is_safe_utf8`, which requires valid UTF-8; a non-UTF-8
        // payload is rejected upstream, so take the strict conversion here.
        if !allow_unbounded {
            return None;
        }
        let rest = std::str::from_utf8(rest).ok()?;
        return kitty_text_sizing::parse(rest);
    }
    if let Some(rest) = strip_num_prefix(buf, b"72") {
        return unbounded(rest, allow_unbounded, |r| {
            kitty_dnd_protocol::parse(r, terminator)
        });
    }
    if let Some(rest) = strip_num_prefix(buf, b"104") {
        return fixed(rest, allow_unbounded, |r| {
            color::parse(color::Op::Osc104, r, terminator)
        });
    }
    if let Some(rest) = strip_num_prefix(buf, b"105") {
        return fixed(rest, allow_unbounded, |r| {
            color::parse(color::Op::Osc105, r, terminator)
        });
    }
    for n in 10..=19u32 {
        let tag = n.to_string();
        if let Some(rest) = strip_num_prefix(buf, tag.as_bytes()) {
            let op = color::Op::from_osc_number(n).unwrap();
            return unbounded(rest, allow_unbounded, move |r| {
                color::parse(op, r, terminator)
            });
        }
    }
    for n in 110..=119u32 {
        let tag = n.to_string();
        if let Some(rest) = strip_num_prefix(buf, tag.as_bytes()) {
            let op = color::Op::from_osc_number(n).unwrap();
            return fixed(rest, allow_unbounded, move |r| {
                color::parse(op, r, terminator)
            });
        }
    }
    if let Some(rest) = strip_num_prefix(buf, b"133") {
        return fixed(rest, allow_unbounded, semantic_prompt::parse);
    }
    if let Some(rest) = strip_num_prefix(buf, b"777") {
        return fixed(rest, allow_unbounded, rxvt_extension::parse);
    }
    if let Some(rest) = strip_num_prefix(buf, b"1337") {
        return fixed(rest, allow_unbounded, iterm2::parse);
    }
    if let Some(rest) = strip_num_prefix(buf, b"5522") {
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
fn strip_num_prefix<'a>(body: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    let rest = body.strip_prefix(prefix)?;
    match rest.first() {
        None => Some(rest),
        Some(b';') => Some(rest),
        Some(c) if c.is_ascii_digit() => None,
        Some(_) => None,
    }
}

/// Convert an OSC body remainder (raw bytes) to a string for the string-oriented
/// sub-parsers, replacing invalid UTF-8 with U+FFFD. The numeric prefix and
/// structural markers a parser keys on are ASCII and survive unchanged; only
/// opaque value fields are affected. Mirrors upstream treating OSC bodies as raw
/// bytes rather than dropping the whole command on a stray non-UTF-8 byte.
fn lossy(rest: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(rest)
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
    rest: &[u8],
    _allow_unbounded: bool,
    f: impl FnOnce(&str) -> Option<Command>,
) -> Option<Command> {
    // The cap is a byte length (matches the Zig capture), so check it on the raw
    // bytes before the lossy conversion (which can change the char count).
    let body_only = rest.strip_prefix(b";").unwrap_or(rest);
    if body_only.len() > MAX_BUF {
        return None;
    }
    f(&lossy(rest))
}

/// Like [`fixed`], but the command requires `allow_unbounded` (an
/// allocator, in Zig terms) — without it, `ensureAllocator` invalidates the
/// whole parse (`osc.zig:449-454`).
fn unbounded(
    rest: &[u8],
    allow_unbounded: bool,
    f: impl FnOnce(&str) -> Option<Command>,
) -> Option<Command> {
    if !allow_unbounded {
        return None;
    }
    f(&lossy(rest))
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
