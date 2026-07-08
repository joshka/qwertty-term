//! OSC-owned color support enums, ported from the tiny `Special`/`Dynamic`
//! enums at `src/terminal/color.zig:416-464`.
//!
//! The RGB color-spec parser itself (`RGB.parse`, `color.zig:642-699`,
//! including X11 named colors) now lives in [`crate::color::Rgb::parse`] —
//! it was ported there in full (hex forms, `rgb:`/`rgbi:`, and the X11
//! name table) once the terminal-state/Screen chunk's `color.zig` port
//! landed. This module previously carried its own minimal, X11-name-less
//! `Rgb`/`Rgb::parse` (a stand-in ported ahead of that chunk); it has been
//! removed in favor of the shared type now that both exist, restoring
//! upstream's single-parser design (`color.zig:642`, used by every OSC
//! color parser). See `docs/analysis/osc.md` divergence #1 (now resolved).

/// The "special" colors addressable via OSC 5/104/105 and the kitty color
/// protocol. Port of `color.zig` `Special` (`color.zig:416-437`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Special {
    Bold = 0,
    Underline = 1,
    Blink = 2,
    Reverse = 3,
    Italic = 4,
}

impl Special {
    pub const COUNT: usize = 5;

    pub const fn from_u8(v: u8) -> Option<Special> {
        match v {
            0 => Some(Special::Bold),
            1 => Some(Special::Underline),
            2 => Some(Special::Blink),
            3 => Some(Special::Reverse),
            4 => Some(Special::Italic),
            _ => None,
        }
    }
}

/// The dynamic colors addressable via OSC 10-19/110-119. Port of
/// `color.zig` `Dynamic` (`color.zig:447-464`). Numeric values match the
/// OSC number they're queried/set through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Dynamic {
    Foreground = 10,
    Background = 11,
    Cursor = 12,
    PointerForeground = 13,
    PointerBackground = 14,
    TektronixForeground = 15,
    TektronixBackground = 16,
    HighlightBackground = 17,
    TektronixCursor = 18,
    HighlightForeground = 19,
}

impl Dynamic {
    /// The next dynamic color sequentially, or `None` past the last one.
    /// Port of `color.zig` `Dynamic.next`.
    pub const fn next(self) -> Option<Dynamic> {
        match self {
            Dynamic::Foreground => Some(Dynamic::Background),
            Dynamic::Background => Some(Dynamic::Cursor),
            Dynamic::Cursor => Some(Dynamic::PointerForeground),
            Dynamic::PointerForeground => Some(Dynamic::PointerBackground),
            Dynamic::PointerBackground => Some(Dynamic::TektronixForeground),
            Dynamic::TektronixForeground => Some(Dynamic::TektronixBackground),
            Dynamic::TektronixBackground => Some(Dynamic::HighlightBackground),
            Dynamic::HighlightBackground => Some(Dynamic::TektronixCursor),
            Dynamic::TektronixCursor => Some(Dynamic::HighlightForeground),
            Dynamic::HighlightForeground => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `Rgb::parse` cross-check tests formerly here (parse_hex_forms,
    // parse_rgb_colon_forms, parse_rgbi_colon_forms, parse_invalid) moved
    // to `crate::color::mod`'s `rgb_parse` test alongside the rest of
    // `Rgb::parse`'s coverage (including X11 names), now that this module
    // no longer defines its own `Rgb` type. See docs/analysis/osc.md
    // divergence #1.

    #[test]
    fn dynamic_next_chain() {
        assert_eq!(Dynamic::Foreground.next(), Some(Dynamic::Background));
        assert_eq!(Dynamic::Background.next(), Some(Dynamic::Cursor));
        assert_eq!(Dynamic::Cursor.next(), Some(Dynamic::PointerForeground));
        assert_eq!(
            Dynamic::PointerForeground.next(),
            Some(Dynamic::PointerBackground)
        );
        assert_eq!(
            Dynamic::PointerBackground.next(),
            Some(Dynamic::TektronixForeground)
        );
        assert_eq!(
            Dynamic::TektronixForeground.next(),
            Some(Dynamic::TektronixBackground)
        );
        assert_eq!(
            Dynamic::TektronixBackground.next(),
            Some(Dynamic::HighlightBackground)
        );
        assert_eq!(
            Dynamic::HighlightBackground.next(),
            Some(Dynamic::TektronixCursor)
        );
        assert_eq!(
            Dynamic::TektronixCursor.next(),
            Some(Dynamic::HighlightForeground)
        );
        assert_eq!(Dynamic::HighlightForeground.next(), None);
    }

    // Zig: color.zig "osc4" test (Special.osc4()).
    #[test]
    fn special_osc4_offset() {
        // Special.osc4() = @intFromEnum(self) + 256 (palette size).
        assert_eq!(Special::Bold as u16 + 256, 256);
        assert_eq!(Special::Underline as u16 + 256, 257);
        assert_eq!(Special::Blink as u16 + 256, 258);
        assert_eq!(Special::Reverse as u16 + 256, 259);
        assert_eq!(Special::Italic as u16 + 256, 260);
    }
}
