//! OSC 21: kitty color protocol. Port of `osc/parsers/kitty_color.zig`
//! (parser) + `src/terminal/kitty/color.zig` (the `OSC`/`Kind` support
//! type, ported minimally as it's tiny and OSC-owned in spirit).

use crate::color::Rgb;
use crate::osc::{Command, Terminator};

/// A color-protocol key: either a named special slot or a palette index.
/// Port of `kitty/color.zig` `Kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyColorKind {
    Palette(u8),
    Special(KittyColorSpecial),
}

/// Port of `kitty/color.zig` `Special`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyColorSpecial {
    Foreground,
    Background,
    SelectionForeground,
    SelectionBackground,
    Cursor,
    CursorText,
    VisualBell,
    SecondTransparentBackground,
}

impl KittyColorKind {
    /// Port of `kitty/color.zig` `Kind.parse`.
    pub fn parse(key: &str) -> Option<KittyColorKind> {
        let special = match key {
            "foreground" => Some(KittyColorSpecial::Foreground),
            "background" => Some(KittyColorSpecial::Background),
            "selection_foreground" => Some(KittyColorSpecial::SelectionForeground),
            "selection_background" => Some(KittyColorSpecial::SelectionBackground),
            "cursor" => Some(KittyColorSpecial::Cursor),
            "cursor_text" => Some(KittyColorSpecial::CursorText),
            "visual_bell" => Some(KittyColorSpecial::VisualBell),
            "second_transparent_background" => Some(KittyColorSpecial::SecondTransparentBackground),
            _ => None,
        };
        if let Some(s) = special {
            return Some(KittyColorKind::Special(s));
        }
        key.parse::<u8>().ok().map(KittyColorKind::Palette)
    }

    /// True if this key is backed by terminal color state. A query for such a
    /// key with no current value still gets an empty `key=` report (rather than
    /// being skipped entirely). Palette entries plus the foreground/background/
    /// cursor specials qualify. Port of `kitty/color.zig` `Kind.hasTerminalQueryColor`
    /// (14c829883).
    pub fn has_terminal_query_color(self) -> bool {
        matches!(
            self,
            KittyColorKind::Palette(_)
                | KittyColorKind::Special(
                    KittyColorSpecial::Foreground
                        | KittyColorSpecial::Background
                        | KittyColorSpecial::Cursor,
                )
        )
    }
}

impl std::fmt::Display for KittyColorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KittyColorKind::Palette(p) => write!(f, "{p}"),
            KittyColorKind::Special(s) => write!(
                f,
                "{}",
                match s {
                    KittyColorSpecial::Foreground => "foreground",
                    KittyColorSpecial::Background => "background",
                    KittyColorSpecial::SelectionForeground => "selection_foreground",
                    KittyColorSpecial::SelectionBackground => "selection_background",
                    KittyColorSpecial::Cursor => "cursor",
                    KittyColorSpecial::CursorText => "cursor_text",
                    KittyColorSpecial::VisualBell => "visual_bell",
                    KittyColorSpecial::SecondTransparentBackground =>
                        "second_transparent_background",
                }
            ),
        }
    }
}

/// The maximum number of key requests permitted in one OSC 21, as a DoS
/// guard (not a meaningful protocol limit). Port of `kitty/color.zig`
/// `Kind.max`, doubled in `kitty_color.zig`'s check.
const MAX_REQUESTS: usize = (u8::MAX as usize + 1 + 8) * 2;

/// A single kitty-color-protocol request. Port of `kitty/color.zig`
/// `OSC.Request`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KittyColorRequest {
    Query(KittyColorKind),
    Set { key: KittyColorKind, color: Rgb },
    Reset(KittyColorKind),
}

/// OSC 21 command payload. Port of `kitty/color.zig` `OSC`.
#[derive(Debug, Clone, PartialEq)]
pub struct KittyColorProtocol {
    pub list: Vec<KittyColorRequest>,
    pub terminator: Terminator,
}

/// Parse OSC 21. Port of `kitty_color.zig` `parse`. Requires unbounded
/// capture (checked by the caller).
pub fn parse(rest: &str, terminator: Terminator) -> Option<Command> {
    let data = rest.strip_prefix(';').unwrap_or("");
    let mut list = Vec::new();
    for kv in data.split(';') {
        if list.len() >= MAX_REQUESTS {
            return None;
        }
        let mut it = kv.splitn(2, '=');
        let Some(k) = it.next() else { continue };
        if k.is_empty() {
            continue;
        }
        let Some(key) = KittyColorKind::parse(k) else {
            continue;
        };
        let value = it.next().unwrap_or("").trim_matches(' ');
        if value.is_empty() {
            list.push(KittyColorRequest::Reset(key));
        } else if value == "?" {
            list.push(KittyColorRequest::Query(key));
        } else if let Ok(color) = Rgb::parse(value) {
            list.push(KittyColorRequest::Set { key, color });
        }
        // else: invalid color format, logged and skipped in Zig.
    }
    Some(Command::KittyColorProtocol(KittyColorProtocol {
        list,
        terminator,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    fn run_with_alloc(body: &str) -> Option<Command> {
        let mut p = osc::Parser::with_allocator();
        for c in body.bytes() {
            p.next(c);
        }
        p.end(Some(0x1b))
    }

    // Zig: kitty_color.zig "OSC 21: kitty color protocol".
    #[test]
    fn osc_21_kitty_color_protocol() {
        // Zig uses "aliceblue" (X11 name) for `cursor`; substituted with
        // its hex equivalent per docs/analysis/osc.md divergence #1.
        let input = "21;foreground=?;background=rgb:f0/f8/ff;cursor=#f0f8ff;cursor_text;visual_bell=;selection_foreground=#xxxyyzz;selection_background=?;selection_background=#aabbcc;2=?;3=rgbi:1.0/1.0/1.0";
        let cmd = run_with_alloc(input).unwrap();
        let Command::KittyColorProtocol(proto) = cmd else {
            panic!("expected KittyColorProtocol");
        };
        assert_eq!(proto.list.len(), 9);
        assert_eq!(
            proto.list[0],
            KittyColorRequest::Query(KittyColorKind::Special(KittyColorSpecial::Foreground))
        );
        assert_eq!(
            proto.list[1],
            KittyColorRequest::Set {
                key: KittyColorKind::Special(KittyColorSpecial::Background),
                color: Rgb::new(0xf0, 0xf8, 0xff)
            }
        );
        assert_eq!(
            proto.list[2],
            KittyColorRequest::Set {
                key: KittyColorKind::Special(KittyColorSpecial::Cursor),
                color: Rgb::new(0xf0, 0xf8, 0xff)
            }
        );
        assert_eq!(
            proto.list[3],
            KittyColorRequest::Reset(KittyColorKind::Special(KittyColorSpecial::CursorText))
        );
        assert_eq!(
            proto.list[4],
            KittyColorRequest::Reset(KittyColorKind::Special(KittyColorSpecial::VisualBell))
        );
        // selection_foreground=#xxxyyzz is an invalid color -- skipped
        // entirely (not present in the list at all).
        assert_eq!(
            proto.list[5],
            KittyColorRequest::Query(KittyColorKind::Special(
                KittyColorSpecial::SelectionBackground
            ))
        );
        assert_eq!(
            proto.list[6],
            KittyColorRequest::Set {
                key: KittyColorKind::Special(KittyColorSpecial::SelectionBackground),
                color: Rgb::new(0xaa, 0xbb, 0xcc)
            }
        );
        assert_eq!(
            proto.list[7],
            KittyColorRequest::Query(KittyColorKind::Palette(2))
        );
        assert_eq!(
            proto.list[8],
            KittyColorRequest::Set {
                key: KittyColorKind::Palette(3),
                color: Rgb::new(0xff, 0xff, 0xff)
            }
        );
    }

    // Zig: kitty_color.zig "OSC 21: kitty color protocol without allocator".
    #[test]
    fn osc_21_without_allocator() {
        let mut p = osc::Parser::new();
        for c in "21;foreground=?".bytes() {
            p.next(c);
        }
        assert_eq!(p.end(Some(0x1b)), None);
    }

    // Zig: kitty_color.zig "OSC 21: kitty color protocol double reset" --
    // in the Rust port, `Parser::reset` just replaces fields with fresh
    // defaults, so double-reset safety is trivial by construction; ported
    // anyway for parity (see docs/analysis/osc.md divergence #4).
    #[test]
    fn osc_21_kitty_color_protocol_double_reset() {
        let mut p = osc::Parser::with_allocator();
        for c in "21;foreground=?".bytes() {
            p.next(c);
        }
        assert!(p.end(Some(0x1b)).is_some());

        p.reset();
        p.reset();
    }

    // Zig: kitty_color.zig "OSC 21: kitty color protocol reset after invalid".
    #[test]
    fn osc_21_reset_after_invalid() {
        let mut p = osc::Parser::with_allocator();
        for c in "21;foreground=?".bytes() {
            p.next(c);
        }
        assert!(p.end(Some(0x1b)).is_some());

        p.reset();
        p.next(b'X');
        // 'X' alone doesn't match any known OSC prefix.
        assert_eq!(p.end(None), None);
        p.reset();
    }

    // Zig: kitty_color.zig "OSC 21: kitty color protocol no key".
    #[test]
    fn osc_21_no_key() {
        let cmd = run_with_alloc("21;").unwrap();
        let Command::KittyColorProtocol(proto) = cmd else {
            panic!("expected KittyColorProtocol");
        };
        assert_eq!(proto.list.len(), 0);
    }

    // Zig: kitty/color.zig "OSC: kitty color protocol kind string".
    #[test]
    fn kind_display() {
        assert_eq!(
            KittyColorKind::Special(KittyColorSpecial::Foreground).to_string(),
            "foreground"
        );
        assert_eq!(KittyColorKind::Palette(42).to_string(), "42");
    }
}
