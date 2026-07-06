//! Terminal color types. Minimal port of `src/terminal/color.zig`.
//!
//! Only the pieces the page memory model needs are ported here: [`Rgb`], the
//! 256-entry [`Palette`], the named-color [`Name`] enum, and the [`DEFAULT`]
//! palette. The parsing/config/x11/OSC machinery in the Zig original is out of
//! scope for the page chunk and will be ported alongside the SGR/OSC work.

/// A 24-bit RGB color. Port of `color.zig` `RGB` (a `packed struct(u24)`).
///
/// Stored `#[repr(C)]` with three `u8`s so it byte-copies into page memory and
/// matches the Zig field order (r, g, b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[repr(C)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// The 256-color palette. Port of `color.zig` `Palette = [256]RGB`.
pub type Palette = [Rgb; 256];

/// Named palette colors (indices 0-15). Port of `color.zig` `Name`.
///
/// Only the offset of [`Name::BrightBlack`] (8) is load-bearing for the page
/// port: `Style::fg` uses it as the bold-is-bright palette offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Name {
    Black = 0,
    Red = 1,
    Green = 2,
    Yellow = 3,
    Blue = 4,
    Magenta = 5,
    Cyan = 6,
    White = 7,
    BrightBlack = 8,
    BrightRed = 9,
    BrightGreen = 10,
    BrightYellow = 11,
    BrightBlue = 12,
    BrightMagenta = 13,
    BrightCyan = 14,
    BrightWhite = 15,
}

impl Name {
    /// Default color for a named value (`color.zig` `Name.default`).
    const fn default_rgb(self) -> Rgb {
        match self {
            Name::Black => Rgb::new(0x1D, 0x1F, 0x21),
            Name::Red => Rgb::new(0xCC, 0x66, 0x66),
            Name::Green => Rgb::new(0xB5, 0xBD, 0x68),
            Name::Yellow => Rgb::new(0xF0, 0xC6, 0x74),
            Name::Blue => Rgb::new(0x81, 0xA2, 0xBE),
            Name::Magenta => Rgb::new(0xB2, 0x94, 0xBB),
            Name::Cyan => Rgb::new(0x8A, 0xBE, 0xB7),
            Name::White => Rgb::new(0xC5, 0xC8, 0xC6),
            Name::BrightBlack => Rgb::new(0x66, 0x66, 0x66),
            Name::BrightRed => Rgb::new(0xD5, 0x4E, 0x53),
            Name::BrightGreen => Rgb::new(0xB9, 0xCA, 0x4A),
            Name::BrightYellow => Rgb::new(0xE7, 0xC5, 0x47),
            Name::BrightBlue => Rgb::new(0x7A, 0xA6, 0xDA),
            Name::BrightMagenta => Rgb::new(0xC3, 0x97, 0xD8),
            Name::BrightCyan => Rgb::new(0x70, 0xC0, 0xB1),
            Name::BrightWhite => Rgb::new(0xEA, 0xEA, 0xEA),
        }
    }
}

/// The bright-black offset used for bold-is-bright palette remapping.
pub const BRIGHT_BLACK_OFFSET: u8 = Name::BrightBlack as u8;

/// The default 256-color palette. Port of `color.zig` `default`.
///
/// - 0-15: named colors.
/// - 16-231: the 6×6×6 color cube.
/// - 232-255: the grayscale ramp.
pub const DEFAULT: Palette = build_default_palette();

const fn build_default_palette() -> Palette {
    let mut result = [Rgb::new(0, 0, 0); 256];

    // Named values (0-15).
    let names = [
        Name::Black,
        Name::Red,
        Name::Green,
        Name::Yellow,
        Name::Blue,
        Name::Magenta,
        Name::Cyan,
        Name::White,
        Name::BrightBlack,
        Name::BrightRed,
        Name::BrightGreen,
        Name::BrightYellow,
        Name::BrightBlue,
        Name::BrightMagenta,
        Name::BrightCyan,
        Name::BrightWhite,
    ];
    let mut i = 0;
    while i < 16 {
        result[i] = names[i].default_rgb();
        i += 1;
    }

    // Color cube (16-231): 6×6×6.
    let mut r = 0u8;
    while r < 6 {
        let mut g = 0u8;
        while g < 6 {
            let mut b = 0u8;
            while b < 6 {
                result[i] = Rgb::new(
                    if r == 0 { 0 } else { r * 40 + 55 },
                    if g == 0 { 0 } else { g * 40 + 55 },
                    if b == 0 { 0 } else { b * 40 + 55 },
                );
                i += 1;
                b += 1;
            }
            g += 1;
        }
        r += 1;
    }

    // Gray ramp (232-255).
    while i < 256 {
        let value = ((i as u8 - 232) * 10) + 8;
        result[i] = Rgb::new(value, value, value);
        i += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // Sanity: the default palette matches the values the Zig style tests rely
    // on (palette 1 = red, 2 = green, 3 = yellow, 7 = white).
    #[test]
    fn default_palette_named() {
        assert_eq!(DEFAULT[1], Rgb::new(0xCC, 0x66, 0x66));
        assert_eq!(DEFAULT[2], Rgb::new(0xB5, 0xBD, 0x68));
        assert_eq!(DEFAULT[3], Rgb::new(0xF0, 0xC6, 0x74));
        assert_eq!(DEFAULT[7], Rgb::new(0xC5, 0xC8, 0xC6));
    }

    #[test]
    fn default_palette_cube_and_ramp() {
        // First cube entry (index 16) is all-zero.
        assert_eq!(DEFAULT[16], Rgb::new(0, 0, 0));
        // Last cube entry (index 231) is all-max component.
        assert_eq!(DEFAULT[231], Rgb::new(255, 255, 255));
        // Gray ramp start (232) = 8, end (255) = 238.
        assert_eq!(DEFAULT[232], Rgb::new(8, 8, 8));
        assert_eq!(DEFAULT[255], Rgb::new(238, 238, 238));
    }

    #[test]
    fn rgb_is_three_bytes() {
        assert_eq!(size_of::<Rgb>(), 3);
    }
}
