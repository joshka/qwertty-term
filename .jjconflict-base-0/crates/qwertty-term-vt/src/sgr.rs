//! SGR (Select Graphic Rendition) attribute parsing and types. Port of
//! `src/terminal/sgr.zig` (1103 lines, 31 inline tests).
//!
//! This is the largest and most correctness-critical of this chunk's
//! modules: every branch of the Zig `Parser.next()` state machine is ported
//! faithfully, including the rare-but-load-bearing colon-subparameter edge
//! cases (mixed colon/semicolon separators, malformed direct-color
//! sequences, trailing colons with no following subparam) that upstream
//! pinned via fuzzing (kakoune inputs, a fuzzer-found crash on `ESC[58:4:m`).
//!
//! # Divergences from the Zig source
//!
//! - **Attribute variant naming**: Zig's `Attribute` union uses Zig
//!   identifier syntax for numeric-ish tags (`@"8_bg"`, `@"256_fg"`,
//!   `@"256_underline_color"`). These are renamed to valid, idiomatic Rust
//!   identifiers: `Bg8`, `Fg8`, `Bg8Bright`, `Fg8Bright`, `Bg256`, `Fg256`,
//!   `UnderlineColor256`. The numeric meaning (8-color vs 256-color) is
//!   unchanged; only the spelling differs.
//! - **Named colors reuse `crate::color::Name`** instead of a
//!   `sgr`-local copy — `color.zig`'s `Name` enum (used by `csi.zig` sgr
//!   attributes `@"8_bg"`/`@"8_fg"`/etc.) is already ported as
//!   [`crate::color::Name`] with matching variant semantics
//!   (`Name::Black = 0` .. `Name::BrightWhite = 15`), so this module reuses
//!   it rather than duplicating a second color-name enum.
//! - **RGB reuses `crate::color::Rgb`** for `color.RGB` (already ported).
//! - **The C ABI union machinery** (`Attribute.Value`/`.C`/`.CValue`/`.cval`,
//!   the `lib.TaggedUnion` padding, `Underline.C`) has no port here: this
//!   chunk is Rust-only per the embeddability rules (Rust API primary, FFI
//!   is `qwertty-term-ffi`'s job later) — `test "sgr: Attribute C compat"` (which
//!   only asserts `Attribute.C` exists/compiles) is therefore N/A and not
//!   ported; see the accounting note at the bottom of this file's test
//!   module.
//! - **`Unknown.full`/`Unknown.partial`** are `&'a [u16]` slices borrowing
//!   from the parser's `params`, exactly like the Zig slices — this requires
//!   [`Attribute::Unknown`] and thus [`Attribute<'a>`] itself to carry a
//!   lifetime, unlike the rest of the crate's parser types which already do
//!   this for the same reason (see `parser::Csi<'a>`).

use crate::color::{Name, Rgb};
use crate::parser::SepList;

/// The underline style. Port of `sgr.zig` `Attribute.Underline` (`enum(u3)`).
///
/// Note: this is intentionally a separate type from
/// [`crate::page::style::Underline`] even though the two currently have
/// identical variants/discriminants — `sgr::Underline` is the wire-level SGR
/// parse result, `page::style::Underline` is the cell-style storage
/// representation. Keeping them distinct (rather than reusing one type for
/// both roles) matches how `sgr.zig`'s `Attribute.Underline` and
/// `style.zig`'s color/flag fields are separate concerns in the Zig source
/// even though the terminal/stream layer (a later chunk) will map one to the
/// other 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Underline {
    #[default]
    None = 0,
    Single = 1,
    Double = 2,
    Curly = 3,
    Dotted = 4,
    Dashed = 5,
}

/// An unknown/unsupported SGR attribute. Port of `sgr.zig` `Attribute.Unknown`.
///
/// Holds the full parameter list and the sub-slice where parsing got hung
/// up, matching the Zig fields verbatim (minus the C ABI `cval`/`.C`, see
/// module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unknown<'a> {
    /// The full SGR input.
    pub full: &'a [u16],
    /// The remaining params, where parsing got hung up.
    pub partial: &'a [u16],
}

/// Attribute type for SGR. Port of `sgr.zig` `Attribute`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribute<'a> {
    /// Unset all attributes.
    Unset,

    /// Unknown attribute; the raw CSI command parameters are here.
    Unknown(Unknown<'a>),

    /// Bold the text.
    Bold,
    ResetBold,

    /// Italic text.
    Italic,
    ResetItalic,

    /// Faint/dim text. Note: reset faint is the same SGR code as reset bold.
    Faint,

    /// Underline the text.
    Underline(Underline),
    UnderlineColor(Rgb),
    UnderlineColor256(u8),
    ResetUnderlineColor,

    /// Overline the text.
    Overline,
    ResetOverline,

    /// Blink the text.
    Blink,
    ResetBlink,

    /// Invert fg/bg colors.
    Inverse,
    ResetInverse,

    /// Invisible.
    Invisible,
    ResetInvisible,

    /// Strikethrough the text.
    Strikethrough,
    ResetStrikethrough,

    /// Set foreground color as RGB values.
    DirectColorFg(Rgb),

    /// Set background color as RGB values.
    DirectColorBg(Rgb),

    /// Set the background/foreground as a named color attribute.
    Bg8(Name),
    Fg8(Name),

    /// Reset the fg/bg to their default values.
    ResetFg,
    ResetBg,

    /// Set the background/foreground as a named bright color attribute.
    Bg8Bright(Name),
    Fg8Bright(Name),

    /// Set background color as 256-color palette.
    Bg256(u8),

    /// Set foreground color as 256-color palette.
    Fg256(u8),
}

/// Parser parses the attributes from a list of SGR parameters. Port of
/// `sgr.zig` `Parser`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Parser<'a> {
    pub params: &'a [u16],
    pub params_sep: SepList,
    pub idx: usize,
}

impl<'a> Parser<'a> {
    /// Empty state parser. Port of `sgr.zig` `Parser.empty`.
    pub const fn empty() -> Self {
        Self {
            params: &[],
            params_sep: SepList::EMPTY,
            idx: 0,
        }
    }

    /// Construct a parser over `params` with all-semicolon separators.
    pub const fn new(params: &'a [u16]) -> Self {
        Self {
            params,
            params_sep: SepList::EMPTY,
            idx: 0,
        }
    }

    /// Returns true if the present position has a colon separator. This
    /// always returns false for the last value since it has no separator.
    /// Port of `sgr.zig` `Parser.isColon`.
    fn is_colon(&self) -> bool {
        self.params_sep.is_set(self.idx)
    }

    /// Port of `sgr.zig` `Parser.countColon`.
    fn count_colon(&self) -> usize {
        let mut count = 0usize;
        let mut idx = self.idx;
        while idx < self.params.len() - 1 && self.params_sep.is_set(idx) {
            count += 1;
            idx += 1;
        }
        count
    }

    /// Consumes all the remaining parameters separated by a colon and
    /// returns an unknown attribute. Port of `sgr.zig`
    /// `Parser.consumeUnknownColon`.
    fn consume_unknown_colon(&mut self) {
        let count = self.count_colon();
        self.idx += count + 1;
    }

    /// Next returns the next attribute or `None` if there are no more
    /// attributes. Port of `sgr.zig` `Parser.next`.
    ///
    /// Named `next` (not `advance`/`parse_next`) to mirror the Zig method
    /// name 1:1; this isn't `Iterator::next` because `Attribute<'a>`
    /// borrows from `self.params` with the parser's own lifetime `'a`
    /// rather than a reborrow of `&mut self`, which `Iterator` can't express.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<Attribute<'a>> {
        if self.idx >= self.params.len() {
            // Add one to ensure we don't loop on unset.
            self.idx += 1;

            // If we're at index zero it means we must have an empty list
            // and an empty list implicitly means unset, otherwise we're
            // done and return None.
            return if self.idx - 1 == 0 {
                Some(Attribute::Unset)
            } else {
                None
            };
        }

        let slice = &self.params[self.idx..self.params.len()];
        let colon = self.params_sep.is_set(self.idx);
        self.idx += 1;

        // Our last one will have an idx be the last value.
        if slice.is_empty() {
            return None;
        }

        // If we have a colon separator then we need to ensure we're parsing
        // a value that allows it.
        if colon && !matches!(slice[0], 4 | 38 | 48 | 58) {
            // In real world use it's very rare that we receive an invalid
            // sequence. Consume all the colon separated values and return
            // them as unknown.
            let start = self.idx;
            while self.params_sep.is_set(self.idx) {
                self.idx += 1;
            }
            self.idx += 1;
            let partial_len = (self.idx - start + 1).min(slice.len());
            return Some(Attribute::Unknown(Unknown {
                full: self.params,
                partial: &slice[0..partial_len],
            }));
        }

        match slice[0] {
            0 => return Some(Attribute::Unset),

            1 => return Some(Attribute::Bold),

            2 => return Some(Attribute::Faint),

            3 => return Some(Attribute::Italic),

            4 => {
                if colon {
                    // A trailing colon with no following sub-param (e.g.
                    // "ESC[58:4:m") leaves the colon separator bit set on
                    // the last param without adding another entry, so we
                    // can see param 4 with a colon but nothing after it.
                    if slice.len() < 2 {
                        return Some(Attribute::Unknown(Unknown {
                            full: self.params,
                            partial: slice,
                        }));
                    }

                    if self.is_colon() {
                        self.consume_unknown_colon();
                        return Some(Attribute::Unknown(Unknown {
                            full: self.params,
                            partial: slice,
                        }));
                    }

                    self.idx += 1;
                    return Some(Attribute::Underline(match slice[1] {
                        0 => Underline::None,
                        1 => Underline::Single,
                        2 => Underline::Double,
                        3 => Underline::Curly,
                        4 => Underline::Dotted,
                        5 => Underline::Dashed,
                        // For unknown underline styles, just render a
                        // single underline.
                        _ => Underline::Single,
                    }));
                }

                return Some(Attribute::Underline(Underline::Single));
            }

            5 => return Some(Attribute::Blink),

            6 => return Some(Attribute::Blink),

            7 => return Some(Attribute::Inverse),

            8 => return Some(Attribute::Invisible),

            9 => return Some(Attribute::Strikethrough),

            21 => return Some(Attribute::Underline(Underline::Double)),

            22 => return Some(Attribute::ResetBold),

            23 => return Some(Attribute::ResetItalic),

            24 => return Some(Attribute::Underline(Underline::None)),

            25 => return Some(Attribute::ResetBlink),

            27 => return Some(Attribute::ResetInverse),

            28 => return Some(Attribute::ResetInvisible),

            29 => return Some(Attribute::ResetStrikethrough),

            30..=37 => {
                return Some(Attribute::Fg8(name_from_offset(slice[0] - 30)));
            }

            38 => {
                if slice.len() >= 2 {
                    match slice[1] {
                        // `2` indicates direct-color (r, g, b). We need at
                        // least 3 more params for this to make sense.
                        2 => {
                            if let Some(v) =
                                self.parse_direct_color(DirectColorTarget::Fg, slice, colon)
                            {
                                return Some(v);
                            }
                        }
                        // `5` indicates indexed color.
                        5 if slice.len() >= 3 => {
                            self.idx += 2;
                            return Some(Attribute::Fg256(slice[2] as u8));
                        }
                        _ => {}
                    }
                }
            }

            39 => return Some(Attribute::ResetFg),

            40..=47 => {
                return Some(Attribute::Bg8(name_from_offset(slice[0] - 40)));
            }

            48 => {
                if slice.len() >= 2 {
                    match slice[1] {
                        2 => {
                            if let Some(v) =
                                self.parse_direct_color(DirectColorTarget::Bg, slice, colon)
                            {
                                return Some(v);
                            }
                        }
                        5 if slice.len() >= 3 => {
                            self.idx += 2;
                            return Some(Attribute::Bg256(slice[2] as u8));
                        }
                        _ => {}
                    }
                }
            }

            49 => return Some(Attribute::ResetBg),

            53 => return Some(Attribute::Overline),
            55 => return Some(Attribute::ResetOverline),

            58 => {
                if slice.len() >= 2 {
                    match slice[1] {
                        2 => {
                            if let Some(v) =
                                self.parse_direct_color(DirectColorTarget::Underline, slice, colon)
                            {
                                return Some(v);
                            }
                        }
                        5 if slice.len() >= 3 => {
                            self.idx += 2;
                            return Some(Attribute::UnderlineColor256(slice[2] as u8));
                        }
                        _ => {}
                    }
                }
            }

            59 => return Some(Attribute::ResetUnderlineColor),

            // 82 instead of 90 to offset to "bright" colors.
            90..=97 => {
                return Some(Attribute::Fg8Bright(name_from_offset(slice[0] - 82)));
            }

            100..=107 => {
                return Some(Attribute::Bg8Bright(name_from_offset(slice[0] - 92)));
            }

            _ => {}
        }

        Some(Attribute::Unknown(Unknown {
            full: self.params,
            partial: slice,
        }))
    }

    /// Port of `sgr.zig` `Parser.parseDirectColor`.
    fn parse_direct_color(
        &mut self,
        target: DirectColorTarget,
        slice: &'a [u16],
        colon: bool,
    ) -> Option<Attribute<'a>> {
        // Any direct color style must have at least 5 values.
        if slice.len() < 5 {
            return None;
        }

        // Only used for direct color sets (38, 48, 58) and subparam 2.
        debug_assert_eq!(slice[1], 2);

        let make = |target: DirectColorTarget, rgb: Rgb| match target {
            DirectColorTarget::Fg => Attribute::DirectColorFg(rgb),
            DirectColorTarget::Bg => Attribute::DirectColorBg(rgb),
            DirectColorTarget::Underline => Attribute::UnderlineColor(rgb),
        };

        // Note: we truncate to u8 because the value should be 0 to 255. If
        // it isn't, the behavior is undefined so we just... truncate it.
        if !colon {
            // If we don't have a colon, then we expect exactly 3 semicolon
            // separated values.
            self.idx += 4;
            return Some(make(
                target,
                Rgb::new(slice[2] as u8, slice[3] as u8, slice[4] as u8),
            ));
        }

        // We have a colon, we might have either 5 or 6 values depending on
        // if the colorspace is present.
        let count = self.count_colon();
        match count {
            3 => {
                // This is the much more common case in the wild.
                self.idx += 4;
                Some(make(
                    target,
                    Rgb::new(slice[2] as u8, slice[3] as u8, slice[4] as u8),
                ))
            }
            4 => {
                self.idx += 5;
                Some(make(
                    target,
                    Rgb::new(slice[3] as u8, slice[4] as u8, slice[5] as u8),
                ))
            }
            _ => {
                self.consume_unknown_colon();
                None
            }
        }
    }
}

/// Which color slot a direct-color (38/48/58) sequence targets. Internal
/// helper standing in for Zig's `comptime tag: Attribute.Tag` parameter to
/// `parseDirectColor` (Rust has no comptime-enum-tag equivalent, so the
/// three call sites are parameterized over this small enum instead and
/// switched over explicitly in `make` above).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectColorTarget {
    Fg,
    Bg,
    Underline,
}

/// Map a 0-7 offset to the corresponding [`Name`]. Port of the repeated
/// `@enumFromInt(slice[0] - N)` pattern in `sgr.zig`'s `Parser.next` (30-37,
/// 40-47, 90-97 after `-82`, 100-107 after `-92` all resolve to a `Name` in
/// 0-15 this way).
fn name_from_offset(offset: u16) -> Name {
    match offset {
        0 => Name::Black,
        1 => Name::Red,
        2 => Name::Green,
        3 => Name::Yellow,
        4 => Name::Blue,
        5 => Name::Magenta,
        6 => Name::Cyan,
        7 => Name::White,
        8 => Name::BrightBlack,
        9 => Name::BrightRed,
        10 => Name::BrightGreen,
        11 => Name::BrightYellow,
        12 => Name::BrightBlue,
        13 => Name::BrightMagenta,
        14 => Name::BrightCyan,
        15 => Name::BrightWhite,
        _ => unreachable!("name_from_offset called with out-of-range offset {offset}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_parse(params: &[u16]) -> Attribute<'_> {
        let mut p = Parser::new(params);
        p.next().unwrap()
    }

    fn test_parse_colon(params: &[u16]) -> Attribute<'_> {
        let mut p = Parser::new(params);
        // Mark all parameters except the last as having a colon after.
        for i in 0..params.len().saturating_sub(1) {
            p.params_sep.set(i);
        }
        p.next().unwrap()
    }

    // Note: `test "sgr: Attribute C compat"` has no port — see module docs
    // ("The C ABI union machinery... has no port here").

    // Port of "sgr: Parser".
    #[test]
    fn parser() {
        assert_eq!(test_parse(&[]), Attribute::Unset);
        assert_eq!(test_parse(&[0]), Attribute::Unset);

        {
            let v = test_parse(&[38, 2, 40, 44, 52]);
            let Attribute::DirectColorFg(rgb) = v else {
                panic!("expected DirectColorFg, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(40, 44, 52));
        }

        assert!(matches!(
            test_parse(&[38, 2, 44, 52]),
            Attribute::Unknown(_)
        ));

        {
            let v = test_parse(&[48, 2, 40, 44, 52]);
            let Attribute::DirectColorBg(rgb) = v else {
                panic!("expected DirectColorBg, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(40, 44, 52));
        }

        assert!(matches!(
            test_parse(&[48, 2, 44, 52]),
            Attribute::Unknown(_)
        ));
    }

    // Port of "sgr: Parser multiple".
    #[test]
    fn parser_multiple() {
        let mut p = Parser::new(&[0, 38, 2, 40, 44, 52]);
        assert_eq!(p.next().unwrap(), Attribute::Unset);
        assert!(matches!(p.next().unwrap(), Attribute::DirectColorFg(_)));
        assert_eq!(p.next(), None);
        assert_eq!(p.next(), None);
    }

    // Port of "sgr: unsupported with colon".
    #[test]
    fn unsupported_with_colon() {
        let mut sep = SepList::EMPTY;
        sep.set(0);
        let mut p = Parser {
            params: &[0, 4, 1],
            params_sep: sep,
            idx: 0,
        };
        assert!(matches!(p.next().unwrap(), Attribute::Unknown(_)));
        assert_eq!(p.next().unwrap(), Attribute::Bold);
        assert_eq!(p.next(), None);
    }

    // Port of "sgr: unsupported with multiple colon".
    #[test]
    fn unsupported_with_multiple_colon() {
        let mut sep = SepList::EMPTY;
        sep.set(0);
        sep.set(1);
        let mut p = Parser {
            params: &[0, 4, 2, 1],
            params_sep: sep,
            idx: 0,
        };
        assert!(matches!(p.next().unwrap(), Attribute::Unknown(_)));
        assert_eq!(p.next().unwrap(), Attribute::Bold);
        assert_eq!(p.next(), None);
    }

    // Port of "sgr: bold".
    #[test]
    fn bold() {
        assert_eq!(test_parse(&[1]), Attribute::Bold);
        assert_eq!(test_parse(&[22]), Attribute::ResetBold);
    }

    // Port of "sgr: italic".
    #[test]
    fn italic() {
        assert_eq!(test_parse(&[3]), Attribute::Italic);
        assert_eq!(test_parse(&[23]), Attribute::ResetItalic);
    }

    // Port of "sgr: underline".
    #[test]
    fn underline() {
        assert_eq!(test_parse(&[4]), Attribute::Underline(Underline::Single));
        assert_eq!(test_parse(&[24]), Attribute::Underline(Underline::None));
    }

    // Port of "sgr: underline styles".
    #[test]
    fn underline_styles() {
        assert_eq!(
            test_parse_colon(&[4, 2]),
            Attribute::Underline(Underline::Double)
        );
        assert_eq!(
            test_parse_colon(&[4, 0]),
            Attribute::Underline(Underline::None)
        );
        assert_eq!(
            test_parse_colon(&[4, 1]),
            Attribute::Underline(Underline::Single)
        );
        assert_eq!(
            test_parse_colon(&[4, 3]),
            Attribute::Underline(Underline::Curly)
        );
        assert_eq!(
            test_parse_colon(&[4, 4]),
            Attribute::Underline(Underline::Dotted)
        );
        assert_eq!(
            test_parse_colon(&[4, 5]),
            Attribute::Underline(Underline::Dashed)
        );
    }

    // Port of "sgr: underline style with more".
    #[test]
    fn underline_style_with_more() {
        let mut sep = SepList::EMPTY;
        sep.set(0);
        let mut p = Parser {
            params: &[4, 2, 1],
            params_sep: sep,
            idx: 0,
        };
        assert_eq!(p.next().unwrap(), Attribute::Underline(Underline::Double));
        assert_eq!(p.next().unwrap(), Attribute::Bold);
        assert_eq!(p.next(), None);
    }

    // Port of "sgr: underline style with too many colons".
    #[test]
    fn underline_style_with_too_many_colons() {
        let mut sep = SepList::EMPTY;
        sep.set(0);
        sep.set(1);
        let mut p = Parser {
            params: &[4, 2, 3, 1],
            params_sep: sep,
            idx: 0,
        };
        assert!(matches!(p.next().unwrap(), Attribute::Unknown(_)));
        assert_eq!(p.next().unwrap(), Attribute::Bold);
        assert_eq!(p.next(), None);
    }

    // Port of "sgr: blink".
    #[test]
    fn blink() {
        assert_eq!(test_parse(&[5]), Attribute::Blink);
        assert_eq!(test_parse(&[6]), Attribute::Blink);
        assert_eq!(test_parse(&[25]), Attribute::ResetBlink);
    }

    // Port of "sgr: inverse".
    #[test]
    fn inverse() {
        assert_eq!(test_parse(&[7]), Attribute::Inverse);
        assert_eq!(test_parse(&[27]), Attribute::ResetInverse);
    }

    // Port of "sgr: strikethrough".
    #[test]
    fn strikethrough() {
        assert_eq!(test_parse(&[9]), Attribute::Strikethrough);
        assert_eq!(test_parse(&[29]), Attribute::ResetStrikethrough);
    }

    // Port of "sgr: 8 color".
    #[test]
    fn eight_color() {
        let mut p = Parser::new(&[31, 43, 90, 103]);

        let v = p.next().unwrap();
        assert_eq!(v, Attribute::Fg8(Name::Red));

        let v = p.next().unwrap();
        assert_eq!(v, Attribute::Bg8(Name::Yellow));

        let v = p.next().unwrap();
        assert_eq!(v, Attribute::Fg8Bright(Name::BrightBlack));

        let v = p.next().unwrap();
        assert_eq!(v, Attribute::Bg8Bright(Name::BrightYellow));
    }

    // Port of "sgr: 256 color".
    #[test]
    fn two_five_six_color() {
        let mut p = Parser::new(&[38, 5, 161, 48, 5, 236]);
        assert!(matches!(p.next().unwrap(), Attribute::Fg256(_)));
        assert!(matches!(p.next().unwrap(), Attribute::Bg256(_)));
        assert_eq!(p.next(), None);
    }

    // Port of "sgr: 256 color underline".
    #[test]
    fn two_five_six_color_underline() {
        let mut p = Parser::new(&[58, 5, 9]);
        assert!(matches!(p.next().unwrap(), Attribute::UnderlineColor256(_)));
        assert_eq!(p.next(), None);
    }

    // Port of "sgr: 24-bit bg color".
    #[test]
    fn twenty_four_bit_bg_color() {
        let v = test_parse_colon(&[48, 2, 1, 2, 3]);
        let Attribute::DirectColorBg(rgb) = v else {
            panic!("expected DirectColorBg, got {v:?}");
        };
        assert_eq!(rgb, Rgb::new(1, 2, 3));
    }

    // Port of "sgr: underline color".
    #[test]
    fn underline_color() {
        {
            let v = test_parse_colon(&[58, 2, 1, 2, 3]);
            let Attribute::UnderlineColor(rgb) = v else {
                panic!("expected UnderlineColor, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(1, 2, 3));
        }
        {
            let v = test_parse_colon(&[58, 2, 0, 1, 2, 3]);
            let Attribute::UnderlineColor(rgb) = v else {
                panic!("expected UnderlineColor, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(1, 2, 3));
        }
    }

    // Port of "sgr: reset underline color".
    #[test]
    fn reset_underline_color() {
        let mut p = Parser::new(&[59]);
        assert_eq!(p.next().unwrap(), Attribute::ResetUnderlineColor);
    }

    // Port of "sgr: invisible".
    #[test]
    fn invisible() {
        let mut p = Parser::new(&[8, 28]);
        assert_eq!(p.next().unwrap(), Attribute::Invisible);
        assert_eq!(p.next().unwrap(), Attribute::ResetInvisible);
    }

    // Port of "sgr: underline, bg, and fg".
    #[test]
    fn underline_bg_and_fg() {
        let mut p = Parser::new(&[4, 38, 2, 255, 247, 219, 48, 2, 242, 93, 147, 4]);
        {
            let v = p.next().unwrap();
            assert_eq!(v, Attribute::Underline(Underline::Single));
        }
        {
            let v = p.next().unwrap();
            let Attribute::DirectColorFg(rgb) = v else {
                panic!("expected DirectColorFg, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(255, 247, 219));
        }
        {
            let v = p.next().unwrap();
            let Attribute::DirectColorBg(rgb) = v else {
                panic!("expected DirectColorBg, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(242, 93, 147));
        }
        {
            let v = p.next().unwrap();
            assert_eq!(v, Attribute::Underline(Underline::Single));
        }
    }

    // Port of "sgr: direct color fg missing color". This used to crash.
    #[test]
    fn direct_color_fg_missing_color() {
        let mut p = Parser::new(&[38, 5]);
        while p.next().is_some() {}
    }

    // Port of "sgr: direct color bg missing color". This used to crash.
    #[test]
    fn direct_color_bg_missing_color() {
        let mut p = Parser::new(&[48, 5]);
        while p.next().is_some() {}
    }

    // Port of "sgr: direct fg/bg/underline ignore optional color space".
    // These behaviors have been verified against xterm.
    #[test]
    fn direct_fg_bg_underline_ignore_optional_color_space() {
        // Colon version should skip the optional color space identifier.
        {
            // 3 8 : 2 : Pi : Pr : Pg : Pb
            let v = test_parse_colon(&[38, 2, 0, 1, 2, 3]);
            let Attribute::DirectColorFg(rgb) = v else {
                panic!("expected DirectColorFg, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(1, 2, 3));
        }
        {
            // 4 8 : 2 : Pi : Pr : Pg : Pb
            let v = test_parse_colon(&[48, 2, 0, 1, 2, 3]);
            let Attribute::DirectColorBg(rgb) = v else {
                panic!("expected DirectColorBg, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(1, 2, 3));
        }
        {
            // 5 8 : 2 : Pi : Pr : Pg : Pb
            let v = test_parse_colon(&[58, 2, 0, 1, 2, 3]);
            let Attribute::UnderlineColor(rgb) = v else {
                panic!("expected UnderlineColor, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(1, 2, 3));
        }

        // Semicolon version should not parse optional color space identifier.
        {
            // 3 8 ; 2 ; Pr ; Pg ; Pb
            let v = test_parse(&[38, 2, 0, 1, 2, 3]);
            let Attribute::DirectColorFg(rgb) = v else {
                panic!("expected DirectColorFg, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(0, 1, 2));
        }
        {
            // 4 8 ; 2 ; Pr ; Pg ; Pb
            let v = test_parse(&[48, 2, 0, 1, 2, 3]);
            let Attribute::DirectColorBg(rgb) = v else {
                panic!("expected DirectColorBg, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(0, 1, 2));
        }
        {
            // 5 8 ; 2 ; Pr ; Pg ; Pb
            let v = test_parse(&[58, 2, 0, 1, 2, 3]);
            let Attribute::UnderlineColor(rgb) = v else {
                panic!("expected UnderlineColor, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(0, 1, 2));
        }
    }

    // Port of "sgr: direct fg colon with too many colons".
    #[test]
    fn direct_fg_colon_with_too_many_colons() {
        let mut sep = SepList::EMPTY;
        for idx in 0..6 {
            sep.set(idx);
        }
        let mut p = Parser {
            params: &[38, 2, 0, 1, 2, 3, 4, 1],
            params_sep: sep,
            idx: 0,
        };
        assert!(matches!(p.next().unwrap(), Attribute::Unknown(_)));
        assert_eq!(p.next().unwrap(), Attribute::Bold);
        assert_eq!(p.next(), None);
    }

    // Port of "sgr: direct fg colon with colorspace and extra param".
    #[test]
    fn direct_fg_colon_with_colorspace_and_extra_param() {
        let mut sep = SepList::EMPTY;
        for idx in 0..5 {
            sep.set(idx);
        }
        let mut p = Parser {
            params: &[38, 2, 0, 1, 2, 3, 1],
            params_sep: sep,
            idx: 0,
        };

        {
            let v = p.next().unwrap();
            let Attribute::DirectColorFg(rgb) = v else {
                panic!("expected DirectColorFg, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(1, 2, 3));
        }

        assert_eq!(p.next().unwrap(), Attribute::Bold);
        assert_eq!(p.next(), None);
    }

    // Port of "sgr: direct fg colon no colorspace and extra param".
    #[test]
    fn direct_fg_colon_no_colorspace_and_extra_param() {
        let mut sep = SepList::EMPTY;
        for idx in 0..4 {
            sep.set(idx);
        }
        let mut p = Parser {
            params: &[38, 2, 1, 2, 3, 1],
            params_sep: sep,
            idx: 0,
        };

        {
            let v = p.next().unwrap();
            let Attribute::DirectColorFg(rgb) = v else {
                panic!("expected DirectColorFg, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(1, 2, 3));
        }

        assert_eq!(p.next().unwrap(), Attribute::Bold);
        assert_eq!(p.next(), None);
    }

    // Kakoune sent this complex SGR sequence that caused invalid behavior.
    // Port of "sgr: kakoune input". This used to crash.
    #[test]
    fn kakoune_input() {
        let mut sep = SepList::EMPTY;
        sep.set(1);
        sep.set(8);
        sep.set(9);
        sep.set(10);
        sep.set(11);
        sep.set(12);
        let mut p = Parser {
            params: &[0, 4, 3, 38, 2, 175, 175, 215, 58, 2, 0, 190, 80, 70],
            params_sep: sep,
            idx: 0,
        };

        {
            let v = p.next().unwrap();
            assert_eq!(v, Attribute::Unset);
        }
        {
            let v = p.next().unwrap();
            assert_eq!(v, Attribute::Underline(Underline::Curly));
        }
        {
            let v = p.next().unwrap();
            let Attribute::DirectColorFg(rgb) = v else {
                panic!("expected DirectColorFg, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(175, 175, 215));
        }
        {
            let v = p.next().unwrap();
            let Attribute::UnderlineColor(rgb) = v else {
                panic!("expected UnderlineColor, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(190, 80, 70));
        }
    }

    // Discussion #5930, another input sent by kakoune. Port of "sgr:
    // kakoune input issue underline, fg, and bg".
    //
    // echo -e "\033[4:3;38;2;51;51;51;48;2;170;170;170;58;2;255;97;136mset
    // everything in one sequence, broken\033[m"
    #[test]
    fn kakoune_input_issue_underline_fg_and_bg() {
        let mut sep = SepList::EMPTY;
        sep.set(0);
        let mut p = Parser {
            params: &[
                4, 3, 38, 2, 51, 51, 51, 48, 2, 170, 170, 170, 58, 2, 255, 97, 136,
            ],
            params_sep: sep,
            idx: 0,
        };

        {
            let v = p.next().unwrap();
            assert_eq!(v, Attribute::Underline(Underline::Curly));
        }
        {
            let v = p.next().unwrap();
            let Attribute::DirectColorFg(rgb) = v else {
                panic!("expected DirectColorFg, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(51, 51, 51));
        }
        {
            let v = p.next().unwrap();
            let Attribute::DirectColorBg(rgb) = v else {
                panic!("expected DirectColorBg, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(170, 170, 170));
        }
        {
            let v = p.next().unwrap();
            let Attribute::UnderlineColor(rgb) = v else {
                panic!("expected UnderlineColor, got {v:?}");
            };
            assert_eq!(rgb, Rgb::new(255, 97, 136));
        }

        assert_eq!(p.next(), None);
    }

    // Fuzz crash: afl-out/stream/default/crashes/id:000021. Input
    // "ESC [ 5 8 : 4 : m" produces params [58, 4] with colon separator bits
    // set at indices 0 and 1. The trailing colon causes the second
    // iteration to see param 4 (underline) with a colon, triggering
    // `assert(slice.len >= 2)` with `slice.len == 1` in the original Zig.
    // Port of "sgr: underline colon with trailing separator and short
    // slice".
    #[test]
    fn underline_colon_with_trailing_separator_and_short_slice() {
        let mut sep = SepList::EMPTY;
        sep.set(0);
        sep.set(1);
        let mut p = Parser {
            params: &[58, 4],
            params_sep: sep,
            idx: 0,
        };

        // 58:4 is not a valid underline color (sub-param 4 is not 2 or 5),
        // so it falls through as unknown.
        assert!(matches!(p.next().unwrap(), Attribute::Unknown(_)));

        // Param 4 with a trailing colon but no sub-param is malformed, so it
        // also falls through as unknown rather than panicking.
        assert!(matches!(p.next().unwrap(), Attribute::Unknown(_)));

        assert_eq!(p.next(), None);
    }
}
