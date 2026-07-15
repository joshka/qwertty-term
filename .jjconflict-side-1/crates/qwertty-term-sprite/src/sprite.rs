//! Pseudo-codepoints for the terminal's own render-model sprites: text
//! decorations (underlines, strikethrough, overline) and cursors.
//!
//! Ported from the `Sprite` enum in `src/font/sprite.zig`. These live at
//! codepoints above the Unicode range (starting one past `U+10FFFF`) so they
//! never collide with real characters, exactly as upstream.

/// One past the maximum Unicode codepoint (`U+10FFFF`). Sprite pseudo-codepoints
/// begin here.
pub const SPRITE_START: u32 = 0x0011_0000;

/// A terminal render-model pseudo-glyph.
///
/// Each variant maps to a fixed pseudo-codepoint at or above [`SPRITE_START`],
/// in the same order as the Zig enum so the numeric values match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Sprite {
    /// Single underline.
    Underline = SPRITE_START,
    /// Double underline.
    UnderlineDouble,
    /// Dotted underline.
    UnderlineDotted,
    /// Dashed underline.
    UnderlineDashed,
    /// Curly (wavy) underline.
    UnderlineCurly,
    /// Strikethrough.
    Strikethrough,
    /// Overline.
    Overline,
    /// Solid block cursor.
    CursorRect,
    /// Hollow (outlined) block cursor.
    CursorHollowRect,
    /// Vertical bar cursor.
    CursorBar,
    /// Underline cursor.
    CursorUnderline,
}

impl Sprite {
    /// The pseudo-codepoint for this sprite.
    #[must_use]
    pub fn codepoint(self) -> u32 {
        self as u32
    }

    /// The sprite for a codepoint, if it is in the sprite pseudo-range.
    #[must_use]
    pub fn from_codepoint(cp: u32) -> Option<Sprite> {
        if cp < SPRITE_START {
            return None;
        }
        Some(match cp - SPRITE_START {
            0 => Sprite::Underline,
            1 => Sprite::UnderlineDouble,
            2 => Sprite::UnderlineDotted,
            3 => Sprite::UnderlineDashed,
            4 => Sprite::UnderlineCurly,
            5 => Sprite::Strikethrough,
            6 => Sprite::Overline,
            7 => Sprite::CursorRect,
            8 => Sprite::CursorHollowRect,
            9 => Sprite::CursorBar,
            10 => Sprite::CursorUnderline,
            _ => return None,
        })
    }
}
