//! Emoji presentation (text vs color) and the resolver's presentation mode.
//!
//! Port of upstream `font.Presentation` (`src/font/main.zig:62-66`) and
//! `Collection.PresentationMode` (`src/font/Collection.zig:862-873`) at commit
//! `2da015cd6`. See `docs/analysis/font-discovery.md` §5.

use unicode_properties::{EmojiStatus, UnicodeEmoji};

/// The presentation for a codepoint: a text (outline) glyph or an emoji (color)
/// glyph.
///
/// Mirrors upstream `Presentation` (`main.zig:62`): `text` is forced by the
/// variation selector `U+FE0E` (VS15), `emoji` by `U+FE0F` (VS16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Presentation {
    /// A text (outline) glyph. Default for most codepoints; forced by VS15.
    Text,
    /// An emoji (color) glyph. Default for emoji-presentation codepoints;
    /// forced by VS16.
    Emoji,
}

/// The requested presentation for a codepoint during resolution.
///
/// Mirrors upstream `Collection.PresentationMode` (`Collection.zig:862`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationMode {
    /// The codepoint has an explicit presentation that is required (VS15/VS16).
    Explicit(Presentation),
    /// No explicit presentation; use the presentation derived from the UCD.
    Default(Presentation),
    /// Any presentation is acceptable (the last-resort search).
    Any,
}

impl Presentation {
    /// The default presentation for a bare codepoint, from the Unicode
    /// `Emoji_Presentation` property.
    ///
    /// The analog of upstream's
    /// `uucode.get(.is_emoji_presentation, cp) ? .emoji : .text`
    /// (CodepointResolver.zig:152-157). A codepoint whose
    /// [`UnicodeEmoji::emoji_status`] is one of the `EmojiPresentation*`
    /// variants defaults to [`Presentation::Emoji`]; everything else (including
    /// invalid scalars) defaults to [`Presentation::Text`].
    pub fn default_for(cp: u32) -> Presentation {
        let Some(c) = char::from_u32(cp) else {
            return Presentation::Text;
        };
        if is_emoji_presentation(c) {
            Presentation::Emoji
        } else {
            Presentation::Text
        }
    }
}

/// True if `c` has the Unicode `Emoji_Presentation` property (i.e. defaults to
/// an emoji/color rendering absent a variation selector).
pub fn is_emoji_presentation(c: char) -> bool {
    matches!(
        c.emoji_status(),
        EmojiStatus::EmojiPresentation
            | EmojiStatus::EmojiPresentationAndModifierBase
            | EmojiStatus::EmojiPresentationAndEmojiComponent
            | EmojiStatus::EmojiPresentationAndModifierAndEmojiComponent
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_codepoint_defaults_to_emoji() {
        // U+1F600 GRINNING FACE has Emoji_Presentation.
        assert_eq!(Presentation::default_for(0x1F600), Presentation::Emoji);
    }

    #[test]
    fn ascii_defaults_to_text() {
        assert_eq!(Presentation::default_for('A' as u32), Presentation::Text);
    }

    #[test]
    fn text_presentation_symbol_defaults_to_text() {
        // U+270C VICTORY HAND is emoji-capable but *text*-presentation by
        // default (needs VS16 to force color); this is the exact case the
        // upstream resolver test pins.
        assert_eq!(Presentation::default_for(0x270C), Presentation::Text);
    }

    #[test]
    fn cjk_defaults_to_text() {
        // U+6C34 水 is a CJK ideograph, not emoji.
        assert_eq!(Presentation::default_for(0x6C34), Presentation::Text);
    }
}
