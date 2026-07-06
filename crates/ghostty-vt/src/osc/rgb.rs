//! Minimal RGB color-spec parser, ported from `src/terminal/color.zig`
//! (`RGB.parse`, `color.zig:642-699`, plus the tiny `Special`/`Dynamic`
//! enums at `color.zig:416-464`).
//!
//! This is a support type from another chunk's file (`src/terminal/
//! color.zig` belongs to the terminal-state/Screen area), ported minimally
//! here because the OSC color parsers (4/5/10-19/21/104/110-119) need it to
//! produce meaningful [`Rgb`] values in their `Command` payloads. Per
//! `docs/analysis/osc.md`, only the hex and `rgb:`/`rgbi:` forms are ported;
//! X11 named colors (`src/terminal/x11_color.zig`, a ~700-entry generated
//! table) are explicitly NOT ported — that table is squarely the color.zig
//! chunk's data file. `Rgb::parse("red")` therefore returns `Err` in this
//! crate today; every OSC test in the Zig source that used a named color
//! has been rewritten here to use the equivalent `#RRGGBB` literal, with a
//! comment noting the substitution.

/// A 24-bit RGB color spec, as produced by OSC 4/5/10-19/21 color requests.
///
/// This is intentionally a separate, minimal type from
/// [`crate::color::Rgb`] (the page-memory color type) to keep this chunk's
/// dependency surface small; the two have identical layout and a future
/// integration pass can unify them (or `From`-convert) once the color.zig
/// chunk lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse a color specification. Port of `color.zig` `RGB.parse`
    /// (`color.zig:642-699`).
    ///
    /// Leading and trailing spaces/tabs are ignored. Accepted forms:
    ///
    /// 1. `rgb:<red>/<green>/<blue>` where each component is 1-4 hex digits.
    /// 2. `rgbi:<red>/<green>/<blue>` where each component is a float in
    ///    `[0.0, 1.0]`.
    /// 3. `#rgb`, `#rrggbb`, `rgb`, `rrggbb`, `#rrrgggbbb`, `#rrrrggggbbbb`.
    /// 4. X11 color names — **not implemented**, see module docs.
    pub fn parse(value: &str) -> Result<Rgb, InvalidFormat> {
        let input = value.trim_matches(|c| c == ' ' || c == '\t');
        if input.is_empty() {
            return Err(InvalidFormat);
        }

        if let Some(hex) = input.strip_prefix('#') {
            return match hex.len() {
                3 => Ok(Rgb::new(
                    from_hex(&hex[0..1])?,
                    from_hex(&hex[1..2])?,
                    from_hex(&hex[2..3])?,
                )),
                6 => Ok(Rgb::new(
                    from_hex(&hex[0..2])?,
                    from_hex(&hex[2..4])?,
                    from_hex(&hex[4..6])?,
                )),
                9 => Ok(Rgb::new(
                    from_hex(&hex[0..3])?,
                    from_hex(&hex[3..6])?,
                    from_hex(&hex[6..9])?,
                )),
                12 => Ok(Rgb::new(
                    from_hex(&hex[0..4])?,
                    from_hex(&hex[4..8])?,
                    from_hex(&hex[8..12])?,
                )),
                _ => Err(InvalidFormat),
            };
        }

        // Bare hex forms (no leading '#'), accepted for compatibility with
        // Ghostty config/theme color values.
        match input.len() {
            3 => {
                return Ok(Rgb::new(
                    from_hex(&input[0..1])?,
                    from_hex(&input[1..2])?,
                    from_hex(&input[2..3])?,
                ));
            }
            6 => {
                return Ok(Rgb::new(
                    from_hex(&input[0..2])?,
                    from_hex(&input[2..4])?,
                    from_hex(&input[4..6])?,
                ));
            }
            _ => {}
        }

        if input.len() < "rgb:a/a/a".len() || &input[0..3] != "rgb" {
            return Err(InvalidFormat);
        }

        let mut i = 3;
        let use_intensity = if input.as_bytes().get(i) == Some(&b'i') {
            i += 1;
            true
        } else {
            false
        };

        if input.as_bytes().get(i) != Some(&b':') {
            return Err(InvalidFormat);
        }
        i += 1;

        let (r, next) = parse_component(input, i, use_intensity)?;
        i = next;
        let (g, next) = parse_component(input, i, use_intensity)?;
        i = next;
        let b = if use_intensity {
            from_intensity(&input[i..])?
        } else {
            from_hex(&input[i..])?
        };

        Ok(Rgb::new(r, g, b))
    }
}

/// Parse one `/`-delimited `rgb:`/`rgbi:` component starting at byte offset
/// `start`, returning the component and the offset just past its trailing
/// `/`.
fn parse_component(
    input: &str,
    start: usize,
    use_intensity: bool,
) -> Result<(u8, usize), InvalidFormat> {
    let rest = &input[start..];
    let end = rest.find('/').ok_or(InvalidFormat)?;
    let slice = &rest[..end];
    let value = if use_intensity {
        from_intensity(slice)?
    } else {
        from_hex(slice)?
    };
    Ok((value, start + end + 1))
}

/// Port of `color.zig` `RGB.fromHex`: parse 1-4 hex digits and scale to a
/// full 8-bit channel value.
fn from_hex(value: &str) -> Result<u8, InvalidFormat> {
    if value.is_empty() || value.len() > 4 {
        return Err(InvalidFormat);
    }
    let color = u16::from_str_radix(value, 16).map_err(|_| InvalidFormat)?;
    let divisor: u32 = match value.len() {
        1 => 0xF,
        2 => 0xFF,
        3 => 0xFFF,
        4 => 0xFFFF,
        _ => unreachable!(),
    };
    Ok(((color as u32) * 255 / divisor) as u8)
}

/// Port of `color.zig` `RGB.fromIntensity`: parse a float in `[0.0, 1.0]`
/// and scale to a full 8-bit channel value.
fn from_intensity(value: &str) -> Result<u8, InvalidFormat> {
    let f: f64 = value.parse().map_err(|_| InvalidFormat)?;
    if !(0.0..=1.0).contains(&f) {
        return Err(InvalidFormat);
    }
    Ok((f * 255.0).round() as u8)
}

/// Error returned by [`Rgb::parse`]. Port of `color.zig`'s
/// `error{InvalidFormat}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidFormat;

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

    // Zig: color.zig "RGB.parse" (partial; only the forms this chunk
    // ported are re-checked here as a sanity cross-check of `Rgb::parse`
    // itself, ahead of the OSC-level tests in `osc/parsers/color.rs` that
    // exercise it through the parser).
    #[test]
    fn parse_hex_forms() {
        assert_eq!(Rgb::parse("#AABBCC").unwrap(), Rgb::new(170, 187, 204));
        assert_eq!(Rgb::parse("aabbcc").unwrap(), Rgb::new(170, 187, 204));
        assert_eq!(Rgb::parse("#abc").unwrap(), Rgb::new(0xAA, 0xBB, 0xCC));
    }

    #[test]
    fn parse_rgb_colon_forms() {
        assert_eq!(Rgb::parse("rgb:7f/a0a0/0").unwrap(), Rgb::new(127, 160, 0));
        assert_eq!(Rgb::parse("rgb:f/ff/fff").unwrap(), Rgb::new(255, 255, 255));
    }

    #[test]
    fn parse_rgbi_colon_forms() {
        assert_eq!(
            Rgb::parse("rgbi:1.0/1.0/1.0").unwrap(),
            Rgb::new(255, 255, 255)
        );
        assert_eq!(Rgb::parse("rgbi:0.0/0.0/0.0").unwrap(), Rgb::new(0, 0, 0));
    }

    #[test]
    fn parse_invalid() {
        assert!(Rgb::parse("").is_err());
        assert!(Rgb::parse("rgb:").is_err());
        assert!(Rgb::parse("rgb:a/a/a/").is_err());
        assert!(Rgb::parse("rgb:00000///").is_err());
        assert!(Rgb::parse("rgb:000/").is_err());
        assert!(Rgb::parse("not/hex/zz").is_err());
    }

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
