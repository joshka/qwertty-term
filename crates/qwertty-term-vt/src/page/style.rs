//! Cell style value type and its ref-counted set. Port of `src/terminal/style.zig`.
//!
//! A [`Style`] bundles fg/bg/underline colors and a packed [`Flags`] word. The
//! **default style is ID 0 by convention** ([`DEFAULT_ID`]) — default-styled
//! cells never touch the set. [`StyleSet`] deduplicates and ref-counts styles.
//!
//! Hashing repacks the style into a `u128` ([`PackedStyle`], tags-then-data
//! ordering), XOR-folds the two halves, and finishes with a SplitMix64
//! avalanche (standing in for Zig's `std.hash.int`). Exact hash values are
//! internal.

use std::fmt::{self, Write as _};

use crate::color::{BRIGHT_BLACK_OFFSET, Palette, Rgb};

use super::hash::splitmix64;
use super::ref_set::{RefCountedSet, SetContext};
use super::size::StyleCountInt;

/// The unique identifier for a style. Port of `style.zig` `Id`.
pub type Id = StyleCountInt;

/// The ID used for default styling. Port of `style.zig` `default_id`.
pub const DEFAULT_ID: Id = 0;

/// The underline style. Port of `sgr.zig` `Attribute.Underline` (`enum(u3)`).
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

/// A source-tracked SGR color. Port of `style.zig` `Style.Color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    None,
    Palette(u8),
    Rgb(Rgb),
}

/// The color tag, matching `style.zig` `Color.Tag` values (none=0, palette=1,
/// rgb=2) for the packed-hash layout.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum ColorTag {
    None = 0,
    Palette = 1,
    Rgb = 2,
}

impl Color {
    fn tag(self) -> ColorTag {
        match self {
            Color::None => ColorTag::None,
            Color::Palette(_) => ColorTag::Palette,
            Color::Rgb(_) => ColorTag::Rgb,
        }
    }
}

/// On/off (and underline) style attributes. Port of `style.zig` `Style.Flags`
/// (`packed struct(u16)`), stored LSB-first to match the Zig bit layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: Underline,
}

impl Flags {
    /// Pack into the u16 bit layout (LSB first) used by the hash.
    fn to_u16(self) -> u16 {
        (self.bold as u16)
            | (self.italic as u16) << 1
            | (self.faint as u16) << 2
            | (self.blink as u16) << 3
            | (self.inverse as u16) << 4
            | (self.invisible as u16) << 5
            | (self.strikethrough as u16) << 6
            | (self.overline as u16) << 7
            | (self.underline as u16) << 8
    }
}

/// The style attributes for a cell. Port of `style.zig` `Style`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg_color: Color,
    pub bg_color: Color,
    pub underline_color: Color,
    pub flags: Flags,
}

/// The color to use for bold text. Port of `style.zig` `BoldColor`.
#[derive(Debug, Clone, Copy)]
pub enum BoldColor {
    Color(Rgb),
    Bright,
}

/// Options for [`Style::fg`]. Port of `style.zig` `Style.Fg`.
pub struct Fg<'a> {
    /// Default fg color if the style specifies none.
    pub default: Rgb,
    /// The active palette for mapping palette indices.
    pub palette: &'a Palette,
    /// Optional bold-text color.
    pub bold: Option<BoldColor>,
}

impl Style {
    /// True if this is the default style. Port of `Style.default`.
    pub fn is_default(&self) -> bool {
        *self == Style::default()
    }

    /// The bg-color-only [`Cell`] representing this style's background, or
    /// `None` if the style has no explicit background. Port of `Style.bgCell`.
    ///
    /// Used by the erase/clear paths to preserve the active background color
    /// when blanking cells (`Screen::blank_cell`).
    pub fn bg_cell(&self) -> Option<super::Cell> {
        match self.bg_color {
            Color::None => None,
            Color::Palette(idx) => {
                let mut c = super::Cell::default();
                c.set_color_palette(idx);
                Some(c)
            }
            Color::Rgb(rgb) => {
                let mut c = super::Cell::default();
                c.set_color_rgb(rgb.r, rgb.g, rgb.b);
                Some(c)
            }
        }
    }

    /// Resolve the underline color. Port of `Style.underlineColor`.
    pub fn underline_color(&self, palette: &Palette) -> Option<Rgb> {
        match self.underline_color {
            Color::None => None,
            Color::Palette(idx) => Some(palette[idx as usize]),
            Color::Rgb(rgb) => Some(rgb),
        }
    }

    /// Resolve the fg color given palette + options. Port of `Style.fg`.
    pub fn fg(&self, opts: &Fg) -> Rgb {
        match self.fg_color {
            Color::None => {
                if self.flags.bold
                    && let Some(bold) = opts.bold
                {
                    match bold {
                        BoldColor::Bright => {}
                        BoldColor::Color(v) => return v,
                    }
                }
                opts.default
            }
            Color::Palette(idx) => {
                if self.flags.bold && opts.bold.is_some() {
                    let bright_offset = BRIGHT_BLACK_OFFSET;
                    if idx < bright_offset {
                        return opts.palette[(idx + bright_offset) as usize];
                    }
                }
                opts.palette[idx as usize]
            }
            Color::Rgb(rgb) => {
                if self.flags.bold
                    && rgb == opts.default
                    && let Some(bold) = opts.bold
                {
                    match bold {
                        BoldColor::Color(v) => return v,
                        BoldColor::Bright => {}
                    }
                }
                rgb
            }
        }
    }

    /// Stable 64-bit hash. Port of `Style.hash`.
    ///
    /// Repacks into a `u128` (tags then data), folds the two `u64` halves with
    /// XOR, and finishes with SplitMix64 (Zig uses `std.hash.int`).
    pub fn hash(&self) -> u64 {
        let packed = PackedStyle::from_style(self);
        let wide = packed.to_u64_pair();
        splitmix64(wide[0] ^ wide[1])
    }

    /// Render as VT/SGR sequences (always resets first). Port of `formatterVt`.
    /// `palette`, if set, expands palette colors to RGB.
    pub fn format_vt(&self, palette: Option<&Palette>) -> String {
        let mut out = String::new();
        // Always reset; styles are self-contained.
        out.push_str("\x1b[0m");
        if self.flags.bold {
            out.push_str("\x1b[1m");
        }
        if self.flags.faint {
            out.push_str("\x1b[2m");
        }
        if self.flags.italic {
            out.push_str("\x1b[3m");
        }
        if self.flags.blink {
            out.push_str("\x1b[5m");
        }
        if self.flags.inverse {
            out.push_str("\x1b[7m");
        }
        if self.flags.invisible {
            out.push_str("\x1b[8m");
        }
        if self.flags.strikethrough {
            out.push_str("\x1b[9m");
        }
        if self.flags.overline {
            out.push_str("\x1b[53m");
        }
        match self.flags.underline {
            Underline::None => {}
            Underline::Single => out.push_str("\x1b[4m"),
            Underline::Double => out.push_str("\x1b[4:2m"),
            Underline::Curly => out.push_str("\x1b[4:3m"),
            Underline::Dotted => out.push_str("\x1b[4:4m"),
            Underline::Dashed => out.push_str("\x1b[4:5m"),
        }
        Self::format_vt_color(&mut out, 38, self.fg_color, palette);
        Self::format_vt_color(&mut out, 48, self.bg_color, palette);
        Self::format_vt_color(&mut out, 58, self.underline_color, palette);
        out
    }

    fn format_vt_color(out: &mut String, prefix: u8, value: Color, palette: Option<&Palette>) {
        match value {
            Color::None => {}
            Color::Palette(idx) => {
                if let Some(p) = palette {
                    let rgb = p[idx as usize];
                    let _ = write!(out, "\x1b[{prefix};2;{};{};{}m", rgb.r, rgb.g, rgb.b);
                } else {
                    let _ = write!(out, "\x1b[{prefix};5;{idx}m");
                }
            }
            Color::Rgb(rgb) => {
                let _ = write!(out, "\x1b[{prefix};2;{};{};{}m", rgb.r, rgb.g, rgb.b);
            }
        }
    }

    /// Render as inline CSS properties. Port of `formatterHtml`. `palette`, if
    /// set, expands palette colors to RGB instead of CSS variables.
    pub fn format_html(&self, palette: Option<&Palette>) -> String {
        let mut out = String::new();
        Self::format_html_color(&mut out, "color", self.fg_color, palette);
        Self::format_html_color(&mut out, "background-color", self.bg_color, palette);
        Self::format_html_color(
            &mut out,
            "text-decoration-color",
            self.underline_color,
            palette,
        );

        let has_line = self.flags.underline != Underline::None
            || self.flags.strikethrough
            || self.flags.overline
            || self.flags.blink;
        if has_line {
            out.push_str("text-decoration-line:");
            if self.flags.underline != Underline::None {
                out.push_str(" underline");
            }
            if self.flags.strikethrough {
                out.push_str(" line-through");
            }
            if self.flags.overline {
                out.push_str(" overline");
            }
            if self.flags.blink {
                out.push_str(" blink");
            }
            out.push(';');
        }

        match self.flags.underline {
            Underline::None => {}
            Underline::Single => out.push_str("text-decoration-style: solid;"),
            Underline::Double => out.push_str("text-decoration-style: double;"),
            Underline::Curly => out.push_str("text-decoration-style: wavy;"),
            Underline::Dotted => out.push_str("text-decoration-style: dotted;"),
            Underline::Dashed => out.push_str("text-decoration-style: dashed;"),
        }

        if self.flags.bold {
            out.push_str("font-weight: bold;");
        }
        if self.flags.italic {
            out.push_str("font-style: italic;");
        }
        if self.flags.faint {
            out.push_str("opacity: 0.5;");
        }
        if self.flags.invisible {
            out.push_str("visibility: hidden;");
        }
        if self.flags.inverse {
            out.push_str("filter: invert(100%);");
        }
        out
    }

    fn format_html_color(out: &mut String, property: &str, c: Color, palette: Option<&Palette>) {
        match c {
            Color::None => {}
            Color::Palette(idx) => {
                if let Some(p) = palette {
                    let rgb = p[idx as usize];
                    let _ = write!(out, "{property}: rgb({}, {}, {});", rgb.r, rgb.g, rgb.b);
                } else {
                    let _ = write!(out, "{property}: var(--vt-palette-{idx});");
                }
            }
            Color::Rgb(rgb) => {
                let _ = write!(out, "{property}: rgb({}, {}, {});", rgb.r, rgb.g, rgb.b);
            }
        }
    }
}

impl fmt::Display for Style {
    /// Debug-friendly formatting showing only non-default attributes. Port of
    /// `Style.format`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Style {{ {:?}, {:?}, {:?}, {:?} }}",
            self.fg_color, self.bg_color, self.underline_color, self.flags
        )
    }
}

/// The packed 128-bit form used for hashing. Port of `style.zig` `PackedStyle`.
/// Tags first, then 24-bit data per color, then the u16 flags, then padding —
/// matching the Zig field order so the fold produces a stable hash.
struct PackedStyle {
    tag_fg: ColorTag,
    tag_bg: ColorTag,
    tag_underline: ColorTag,
    data_fg: u32, // 24-bit data
    data_bg: u32,
    data_underline: u32,
    flags: u16,
}

impl PackedStyle {
    fn color_data(c: Color) -> u32 {
        match c {
            Color::None => 0,
            Color::Palette(idx) => idx as u32, // idx in low 8 bits, rest zero
            Color::Rgb(rgb) => {
                // RGB packed as r | g<<8 | b<<16 (Zig packed struct(u24) order).
                (rgb.r as u32) | (rgb.g as u32) << 8 | (rgb.b as u32) << 16
            }
        }
    }

    fn from_style(s: &Style) -> Self {
        Self {
            tag_fg: s.fg_color.tag(),
            tag_bg: s.bg_color.tag(),
            tag_underline: s.underline_color.tag(),
            data_fg: Self::color_data(s.fg_color),
            data_bg: Self::color_data(s.bg_color),
            data_underline: Self::color_data(s.underline_color),
            flags: s.flags.to_u16(),
        }
    }

    /// Serialize into the two u64 halves of the u128, bit-for-bit matching the
    /// Zig `packed struct(u128)` field order:
    /// `[tags: 3×u8][data: 3×u24][flags: u16][pad: u16]`.
    fn to_u64_pair(&self) -> [u64; 2] {
        let mut bits: u128 = 0;
        let mut shift = 0u32;
        let mut put = |val: u128, width: u32| {
            bits |= (val & ((1u128 << width) - 1)) << shift;
            shift += width;
        };
        put(self.tag_fg as u128, 8);
        put(self.tag_bg as u128, 8);
        put(self.tag_underline as u128, 8);
        put(self.data_fg as u128, 24);
        put(self.data_bg as u128, 24);
        put(self.data_underline as u128, 24);
        put(self.flags as u128, 16);
        // remaining 16 bits padding = 0
        [bits as u64, (bits >> 64) as u64]
    }
}

/// The [`SetContext`] for styles: stateless, hashing via [`Style::hash`].
#[derive(Default)]
pub struct StyleContext;

impl SetContext<Style> for StyleContext {
    fn hash(&self, _base: *const u8, value: &Style) -> u64 {
        value.hash()
    }
    fn eql(&self, _base: *const u8, a: &Style, b: &Style) -> bool {
        a == b
    }
}

/// The deduplicating, ref-counted set of styles. Port of `style.zig` `Set`.
pub type StyleSet = RefCountedSet<Style, Id, StyleContext>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::size::OffsetBuf;

    fn set_with<F: FnOnce(&mut StyleSet, *mut u8)>(cap: usize, f: F) {
        let layout = StyleSet::layout(cap);
        let mut backing = vec![0u8; layout.total_size + StyleSet::base_align()];
        let off = backing.as_ptr().align_offset(StyleSet::base_align());
        let base = unsafe { backing.as_mut_ptr().add(off) };
        // SAFETY: base aligned and layout.total_size bytes available.
        let mut set = unsafe { StyleSet::init(OffsetBuf::new(base), layout, StyleContext) };
        f(&mut set, base);
    }

    // Port of style.zig "Set basic usage".
    #[test]
    fn set_basic_usage() {
        set_with(16, |set, base| unsafe {
            let style = Style {
                flags: Flags {
                    bold: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            let style2 = Style {
                flags: Flags {
                    italic: true,
                    ..Default::default()
                },
                ..Default::default()
            };

            let id = set.add(base, style).unwrap();
            assert!(id > 0);

            let id2 = set.add(base, style).unwrap();
            assert_eq!(id, id2);

            {
                let v = &*set.get(base, id);
                assert!(v.flags.bold);
            }

            let id_b = set.add(base, style2).unwrap();
            {
                let v = &*set.get(base, id_b);
                assert!(v.flags.italic);
            }

            assert_eq!(set.ref_count(base, id), 2);
            assert_eq!(set.ref_count(base, id_b), 1);

            set.release(base, id);
            assert_eq!(set.ref_count(base, id), 1);
            set.release(base, id_b);
            assert_eq!(set.ref_count(base, id_b), 0);

            set.release(base, id);
            assert_eq!(set.ref_count(base, id), 0);
        });
    }

    // Port of style.zig "Set capacities".
    #[test]
    fn set_capacities() {
        let _ = StyleSet::layout(16384);
    }

    // Port of style.zig "Style VT formatting *" tests (consolidated).
    #[test]
    fn vt_formatting() {
        let s = |style: Style| style.format_vt(None);
        assert_eq!(s(Style::default()), "\x1b[0m");
        assert_eq!(
            s(Style {
                flags: Flags {
                    bold: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
            "\x1b[0m\x1b[1m"
        );
        assert_eq!(
            s(Style {
                flags: Flags {
                    faint: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
            "\x1b[0m\x1b[2m"
        );
        assert_eq!(
            s(Style {
                flags: Flags {
                    italic: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
            "\x1b[0m\x1b[3m"
        );
        assert_eq!(
            s(Style {
                flags: Flags {
                    blink: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
            "\x1b[0m\x1b[5m"
        );
        assert_eq!(
            s(Style {
                flags: Flags {
                    inverse: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
            "\x1b[0m\x1b[7m"
        );
        assert_eq!(
            s(Style {
                flags: Flags {
                    invisible: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
            "\x1b[0m\x1b[8m"
        );
        assert_eq!(
            s(Style {
                flags: Flags {
                    strikethrough: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
            "\x1b[0m\x1b[9m"
        );
        assert_eq!(
            s(Style {
                flags: Flags {
                    overline: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
            "\x1b[0m\x1b[53m"
        );
        assert_eq!(
            s(Style {
                flags: Flags {
                    underline: Underline::Single,
                    ..Default::default()
                },
                ..Default::default()
            }),
            "\x1b[0m\x1b[4m"
        );
        assert_eq!(
            s(Style {
                flags: Flags {
                    underline: Underline::Double,
                    ..Default::default()
                },
                ..Default::default()
            }),
            "\x1b[0m\x1b[4:2m"
        );
        assert_eq!(
            s(Style {
                flags: Flags {
                    underline: Underline::Curly,
                    ..Default::default()
                },
                ..Default::default()
            }),
            "\x1b[0m\x1b[4:3m"
        );
        assert_eq!(
            s(Style {
                flags: Flags {
                    underline: Underline::Dotted,
                    ..Default::default()
                },
                ..Default::default()
            }),
            "\x1b[0m\x1b[4:4m"
        );
        assert_eq!(
            s(Style {
                flags: Flags {
                    underline: Underline::Dashed,
                    ..Default::default()
                },
                ..Default::default()
            }),
            "\x1b[0m\x1b[4:5m"
        );
    }

    // Port of style.zig fg/bg/underline VT color tests.
    #[test]
    fn vt_formatting_colors() {
        assert_eq!(
            Style {
                fg_color: Color::Palette(42),
                ..Default::default()
            }
            .format_vt(None),
            "\x1b[0m\x1b[38;5;42m"
        );
        assert_eq!(
            Style {
                fg_color: Color::Rgb(Rgb::new(255, 128, 64)),
                ..Default::default()
            }
            .format_vt(None),
            "\x1b[0m\x1b[38;2;255;128;64m"
        );
        assert_eq!(
            Style {
                bg_color: Color::Palette(7),
                ..Default::default()
            }
            .format_vt(None),
            "\x1b[0m\x1b[48;5;7m"
        );
        assert_eq!(
            Style {
                bg_color: Color::Rgb(Rgb::new(32, 64, 96)),
                ..Default::default()
            }
            .format_vt(None),
            "\x1b[0m\x1b[48;2;32;64;96m"
        );
        assert_eq!(
            Style {
                underline_color: Color::Palette(15),
                ..Default::default()
            }
            .format_vt(None),
            "\x1b[0m\x1b[58;5;15m"
        );
        assert_eq!(
            Style {
                underline_color: Color::Rgb(Rgb::new(200, 100, 50)),
                ..Default::default()
            }
            .format_vt(None),
            "\x1b[0m\x1b[58;2;200;100;50m"
        );
    }

    // Port of style.zig "Style VT formatting multiple flags" / "all flags" /
    // "combined colors and flags" / "all colors rgb" / "all colors palette".
    #[test]
    fn vt_formatting_combinations() {
        assert_eq!(
            Style {
                flags: Flags {
                    bold: true,
                    italic: true,
                    underline: Underline::Single,
                    ..Default::default()
                },
                ..Default::default()
            }
            .format_vt(None),
            "\x1b[0m\x1b[1m\x1b[3m\x1b[4m"
        );
        assert_eq!(
            Style {
                flags: Flags {
                    bold: true,
                    faint: true,
                    italic: true,
                    blink: true,
                    inverse: true,
                    invisible: true,
                    strikethrough: true,
                    overline: true,
                    underline: Underline::Curly,
                },
                ..Default::default()
            }
            .format_vt(None),
            "\x1b[0m\x1b[1m\x1b[2m\x1b[3m\x1b[5m\x1b[7m\x1b[8m\x1b[9m\x1b[53m\x1b[4:3m"
        );
        assert_eq!(
            Style {
                fg_color: Color::Rgb(Rgb::new(255, 0, 0)),
                bg_color: Color::Palette(8),
                underline_color: Color::Rgb(Rgb::new(0, 255, 0)),
                flags: Flags {
                    bold: true,
                    italic: true,
                    underline: Underline::Double,
                    ..Default::default()
                },
            }
            .format_vt(None),
            "\x1b[0m\x1b[1m\x1b[3m\x1b[4:2m\x1b[38;2;255;0;0m\x1b[48;5;8m\x1b[58;2;0;255;0m"
        );
        assert_eq!(
            Style {
                fg_color: Color::Rgb(Rgb::new(10, 20, 30)),
                bg_color: Color::Rgb(Rgb::new(40, 50, 60)),
                underline_color: Color::Rgb(Rgb::new(70, 80, 90)),
                ..Default::default()
            }
            .format_vt(None),
            "\x1b[0m\x1b[38;2;10;20;30m\x1b[48;2;40;50;60m\x1b[58;2;70;80;90m"
        );
        assert_eq!(
            Style {
                fg_color: Color::Palette(1),
                bg_color: Color::Palette(2),
                underline_color: Color::Palette(3),
                ..Default::default()
            }
            .format_vt(None),
            "\x1b[0m\x1b[38;5;1m\x1b[48;5;2m\x1b[58;5;3m"
        );
    }

    // Port of style.zig "Style VT formatting palette with palette set emits rgb"
    // and "all palette colors with palette set".
    #[test]
    fn vt_formatting_palette_expansion() {
        assert_eq!(
            Style {
                fg_color: Color::Palette(1),
                ..Default::default()
            }
            .format_vt(Some(&crate::color::DEFAULT)),
            "\x1b[0m\x1b[38;2;204;102;102m"
        );
        assert_eq!(
            Style {
                fg_color: Color::Palette(1),
                bg_color: Color::Palette(2),
                underline_color: Color::Palette(3),
                ..Default::default()
            }
            .format_vt(Some(&crate::color::DEFAULT)),
            "\x1b[0m\x1b[38;2;204;102;102m\x1b[48;2;181;189;104m\x1b[58;2;240;198;116m"
        );
    }

    // Port of style.zig "Style HTML formatting *" tests.
    #[test]
    fn html_formatting() {
        assert_eq!(
            Style {
                flags: Flags {
                    bold: true,
                    ..Default::default()
                },
                ..Default::default()
            }
            .format_html(None),
            "font-weight: bold;"
        );
        assert_eq!(
            Style {
                fg_color: Color::Rgb(Rgb::new(255, 128, 64)),
                ..Default::default()
            }
            .format_html(None),
            "color: rgb(255, 128, 64);"
        );
        assert_eq!(
            Style {
                bg_color: Color::Palette(7),
                ..Default::default()
            }
            .format_html(None),
            "background-color: var(--vt-palette-7);"
        );

        let combined = Style {
            fg_color: Color::Rgb(Rgb::new(255, 0, 0)),
            bg_color: Color::Rgb(Rgb::new(0, 0, 255)),
            flags: Flags {
                bold: true,
                italic: true,
                ..Default::default()
            },
            ..Default::default()
        }
        .format_html(None);
        assert!(combined.contains("color: rgb(255, 0, 0);"));
        assert!(combined.contains("background-color: rgb(0, 0, 255);"));
        assert!(combined.contains("font-weight: bold;"));
        assert!(combined.contains("font-style: italic;"));

        let single = Style {
            flags: Flags {
                underline: Underline::Single,
                ..Default::default()
            },
            ..Default::default()
        }
        .format_html(None);
        assert!(single.contains("text-decoration-line: underline;"));
        assert!(single.contains("text-decoration-style: solid;"));

        let multi = Style {
            flags: Flags {
                underline: Underline::Curly,
                strikethrough: true,
                overline: true,
                ..Default::default()
            },
            ..Default::default()
        }
        .format_html(None);
        assert!(multi.contains("text-decoration-line: underline line-through overline;"));
        assert!(multi.contains("text-decoration-style: wavy;"));

        assert_eq!(
            Style {
                bg_color: Color::Palette(7),
                ..Default::default()
            }
            .format_html(Some(&crate::color::DEFAULT)),
            "background-color: rgb(197, 200, 198);"
        );
        assert_eq!(
            Style {
                fg_color: Color::Palette(1),
                bg_color: Color::Palette(2),
                underline_color: Color::Palette(3),
                ..Default::default()
            }
            .format_html(Some(&crate::color::DEFAULT)),
            "color: rgb(204, 102, 102);background-color: rgb(181, 189, 104);text-decoration-color: rgb(240, 198, 116);"
        );
    }

    // Distinct styles must produce distinct hashes (dedup correctness).
    #[test]
    fn hash_distinguishes_styles() {
        let a = Style::default();
        let b = Style {
            flags: Flags {
                bold: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let c = Style {
            fg_color: Color::Palette(1),
            ..Default::default()
        };
        let d = Style {
            fg_color: Color::Rgb(Rgb::new(1, 0, 0)),
            ..Default::default()
        };
        assert_ne!(a.hash(), b.hash());
        assert_ne!(a.hash(), c.hash());
        assert_ne!(c.hash(), d.hash());
        // Same style, same hash.
        assert_eq!(
            b.hash(),
            Style {
                flags: Flags {
                    bold: true,
                    ..Default::default()
                },
                ..Default::default()
            }
            .hash()
        );
    }
}
