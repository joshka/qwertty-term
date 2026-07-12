//! Terminal color types. Port of `src/terminal/color.zig` (1250 lines, 23
//! inline tests) and `src/terminal/x11_color.zig` (see the [`x11_color`]
//! submodule; 107 lines, 2 inline tests).
//!
//! [`Rgb`], the 256-entry [`Palette`], the named-color [`Name`] enum, and
//! the [`DEFAULT`] palette were ported first (alongside the page memory
//! model chunk, which needs them for `Style`). This pass completes the port:
//! [`Rgb::parse`] (hex/`rgb:`/`rgbi:`/X11-name parsing), luminance/contrast
//! helpers, [`Special`]/[`Dynamic`] (xterm's special/dynamic OSC color
//! slots), [`DynamicPalette`]/[`DynamicRgb`] (mutable palette with
//! reset-to-default tracking), [`generate_256_color`] (theme-derived
//! 256-color cube via CIELAB interpolation), and [`parse_palette_entry`]
//! (config `N=COLOR` syntax).
//!
//! # Divergences from the Zig source
//!
//! - **No C ABI.** `RGB.C`/`.cval`/`.fromC`, `PaletteC`/`paletteCval`/
//!   `paletteZval` have no port — this chunk is Rust-only (see the crate's
//!   embeddability rules: FFI lives in `qwertty-term-ffi`, layered over the Rust
//!   API, not duplicated here).
//! - **`PaletteMask`** is a plain `[bool; 256]` instead of Zig's
//!   `std.StaticBitSet(256)`. Same semantics (`is_set`/`set`/`unset`/
//!   `count`), no packed-bit storage — correctness, not the bit-packing, is
//!   what `generate_256_color`'s `skip` mask depends on.
//! - **Error types**: Zig's `error{InvalidFormat}`/`error{Overflow}` become
//!   a small [`ParseColorError`]/[`ParsePaletteEntryError`] enum each,
//!   following normal Rust error-handling conventions rather than Zig error
//!   unions.

pub mod x11_color;

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

    /// Encode as the 16-bit `rgb:RRRR/GGGG/BBBB` form used by xterm color
    /// reports (OSC 4/10/11/12 query replies). Each 8-bit component is widened
    /// to 16 bits by `× 257` (i.e. the byte is duplicated: `0x12` → `0x1212`).
    /// Port of `color.zig` `RGB.encodeRgb16`.
    pub fn encode_rgb16(self) -> String {
        format!(
            "rgb:{:04x}/{:04x}/{:04x}",
            u16::from(self.r) * 257,
            u16::from(self.g) * 257,
            u16::from(self.b) * 257,
        )
    }

    /// Encode as the 8-bit `rgb:RR/GG/BB` form used by the kitty color
    /// protocol (OSC 21 query replies). Port of `color.zig` `RGB.encodeRgb8`.
    pub fn encode_rgb8(self) -> String {
        format!("rgb:{:02x}/{:02x}/{:02x}", self.r, self.g, self.b)
    }

    /// Calculates the contrast ratio between two colors. The contrast ratio
    /// is a value between 1 and 21 where 1 is the lowest contrast and 21 is
    /// the highest contrast. Port of `color.zig` `RGB.contrast`.
    ///
    /// <https://www.w3.org/TR/WCAG20/#contrast-ratiodef>
    pub fn contrast(self, other: Rgb) -> f64 {
        let self_lum = self.luminance();
        let other_lum = other.luminance();
        // pair.0 = lighter, pair.1 = darker
        let (lighter, darker) = if self_lum > other_lum {
            (self_lum, other_lum)
        } else {
            (other_lum, self_lum)
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    /// Calculates luminance based on the W3C formula. Returns a normalized
    /// value between 0 and 1 where 0 is black and 1 is white. Port of
    /// `color.zig` `RGB.luminance`.
    ///
    /// <https://www.w3.org/TR/WCAG20/#relativeluminancedef>
    pub fn luminance(self) -> f64 {
        let r_lum = Self::component_luminance(self.r);
        let g_lum = Self::component_luminance(self.g);
        let b_lum = Self::component_luminance(self.b);
        0.2126 * r_lum + 0.7152 * g_lum + 0.0722 * b_lum
    }

    /// Calculates single-component luminance based on the W3C formula.
    /// Expects sRGB color space. Port of `color.zig`
    /// `RGB.componentLuminance`.
    ///
    /// <https://www.w3.org/TR/WCAG20/#relativeluminancedef>
    fn component_luminance(c: u8) -> f64 {
        let normalized = f64::from(c) / 255.0;
        if normalized <= 0.03928 {
            normalized / 12.92
        } else {
            ((normalized + 0.055) / 1.055).powf(2.4)
        }
    }

    /// Calculates "perceived luminance" which is better for determining
    /// light vs dark. Port of `color.zig` `RGB.perceivedLuminance`.
    ///
    /// Source: <https://www.w3.org/TR/AERT/#color-contrast>
    pub fn perceived_luminance(self) -> f64 {
        let r = f64::from(self.r);
        let g = f64::from(self.g);
        let b = f64::from(self.b);
        0.299 * (r / 255.0) + 0.587 * (g / 255.0) + 0.114 * (b / 255.0)
    }

    /// Parse a color from a floating point intensity value in `[0.0, 1.0]`.
    /// Port of `color.zig` `RGB.fromIntensity`.
    fn from_intensity(value: &str) -> Result<u8, ParseColorError> {
        let i: f64 = value.parse().map_err(|_| ParseColorError::InvalidFormat)?;
        if !(0.0..=1.0).contains(&i) {
            return Err(ParseColorError::InvalidFormat);
        }
        Ok((i * f64::from(u8::MAX)) as u8)
    }

    /// Parse a color from a string of 1, 2, 3, or 4 hexadecimal digits,
    /// representing the color value scaled in 4, 8, 12, or 16 bits
    /// respectively. Port of `color.zig` `RGB.fromHex`.
    fn from_hex(value: &str) -> Result<u8, ParseColorError> {
        if value.is_empty() || value.len() > 4 {
            return Err(ParseColorError::InvalidFormat);
        }
        let color = u32::from_str_radix(value, 16).map_err(|_| ParseColorError::InvalidFormat)?;
        let divisor: u32 = match value.len() {
            1 => 0xF,
            2 => 0xFF,
            3 => 0xFFF,
            4 => 0xFFFF,
            _ => unreachable!(),
        };
        Ok((color * u32::from(u8::MAX) / divisor) as u8)
    }

    /// Parse a color specification. Leading and trailing spaces/tabs are
    /// ignored. Port of `color.zig` `RGB.parse`.
    ///
    /// Accepted forms:
    ///
    /// 1. `rgb:<red>/<green>/<blue>` where each component is 1-4 hex digits.
    /// 2. `rgbi:<red>/<green>/<blue>` where each component is a float in
    ///    `[0.0, 1.0]`.
    /// 3. `#rgb`, `#rrggbb`, `rgb`, `rrggbb`, `#rrrgggbbb`, `#rrrrggggbbbb` —
    ///    hex digit triples; the `#`-prefixed forms specify 4/8/12/16 bits
    ///    of precision per channel, the bare forms (3 or 6 digits only) are
    ///    accepted for Ghostty config/theme compatibility.
    /// 4. X11 color names (see [`x11_color`]).
    pub fn parse(value: &str) -> Result<Rgb, ParseColorError> {
        let input = value.trim_matches(|c| c == ' ' || c == '\t');
        if input.is_empty() {
            return Err(ParseColorError::InvalidFormat);
        }

        if let Some(rest) = input.strip_prefix('#') {
            return match rest.len() {
                3 => Ok(Rgb::new(
                    Self::from_hex(&rest[0..1])?,
                    Self::from_hex(&rest[1..2])?,
                    Self::from_hex(&rest[2..3])?,
                )),
                6 => Ok(Rgb::new(
                    Self::from_hex(&rest[0..2])?,
                    Self::from_hex(&rest[2..4])?,
                    Self::from_hex(&rest[4..6])?,
                )),
                9 => Ok(Rgb::new(
                    Self::from_hex(&rest[0..3])?,
                    Self::from_hex(&rest[3..6])?,
                    Self::from_hex(&rest[6..9])?,
                )),
                12 => Ok(Rgb::new(
                    Self::from_hex(&rest[0..4])?,
                    Self::from_hex(&rest[4..8])?,
                    Self::from_hex(&rest[8..12])?,
                )),
                _ => Err(ParseColorError::InvalidFormat),
            };
        }

        // Check for X11 named colors. We allow whitespace around the edges
        // (already trimmed above).
        if let Some(rgb) = x11_color::get(input) {
            return Ok(rgb);
        }

        match input.len() {
            3 => {
                return Ok(Rgb::new(
                    Self::from_hex(&input[0..1])?,
                    Self::from_hex(&input[1..2])?,
                    Self::from_hex(&input[2..3])?,
                ));
            }
            6 => {
                return Ok(Rgb::new(
                    Self::from_hex(&input[0..2])?,
                    Self::from_hex(&input[2..4])?,
                    Self::from_hex(&input[4..6])?,
                ));
            }
            _ => {}
        }

        if input.len() < "rgb:a/a/a".len() || &input[0..3] != "rgb" {
            return Err(ParseColorError::InvalidFormat);
        }

        let mut i = 3;

        let use_intensity = if input.as_bytes().get(i) == Some(&b'i') {
            i += 1;
            true
        } else {
            false
        };

        if input.as_bytes().get(i) != Some(&b':') {
            return Err(ParseColorError::InvalidFormat);
        }
        i += 1;

        let r = {
            let end = input[i..]
                .find('/')
                .map(|p| i + p)
                .ok_or(ParseColorError::InvalidFormat)?;
            let slice = &input[i..end];
            i = end + 1;
            if use_intensity {
                Self::from_intensity(slice)?
            } else {
                Self::from_hex(slice)?
            }
        };

        let g = {
            let end = input[i..]
                .find('/')
                .map(|p| i + p)
                .ok_or(ParseColorError::InvalidFormat)?;
            let slice = &input[i..end];
            i = end + 1;
            if use_intensity {
                Self::from_intensity(slice)?
            } else {
                Self::from_hex(slice)?
            }
        };

        let b = if use_intensity {
            Self::from_intensity(&input[i..])?
        } else {
            Self::from_hex(&input[i..])?
        };

        Ok(Rgb::new(r, g, b))
    }
}

/// Error returned by [`Rgb::parse`]. Port of `color.zig`
/// `RGB.parse`'s `error{InvalidFormat}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseColorError {
    InvalidFormat,
}

impl std::fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid color format")
    }
}

impl std::error::Error for ParseColorError {}

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

/// A parsed palette entry from Ghostty's config "N=COLOR" syntax. Port of
/// `color.zig` `PaletteEntry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteEntry {
    pub index: u8,
    pub color: Rgb,
}

/// Error returned by [`parse_palette_entry`]. Port of `color.zig`
/// `parsePaletteEntry`'s `error{ InvalidFormat, Overflow }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsePaletteEntryError {
    InvalidFormat,
    Overflow,
}

impl std::fmt::Display for ParsePaletteEntryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFormat => f.write_str("invalid palette entry format"),
            Self::Overflow => f.write_str("palette index out of range"),
        }
    }
}

impl std::error::Error for ParsePaletteEntryError {}

/// Parse a palette entry in Ghostty config syntax: `"N=COLOR"` where `N` is
/// a palette index 0-255 (decimal, or `0x`/`0o`/`0b`-prefixed) and `COLOR`
/// is anything [`Rgb::parse`] accepts. Whitespace (spaces/tabs) around `N`
/// and `COLOR` is ignored. Port of `color.zig` `parsePaletteEntry`.
pub fn parse_palette_entry(value: &str) -> Result<PaletteEntry, ParsePaletteEntryError> {
    let eq_idx = value
        .find('=')
        .ok_or(ParsePaletteEntryError::InvalidFormat)?;
    let index_str = value[..eq_idx].trim_matches(|c| c == ' ' || c == '\t');
    let index = parse_zig_int_u8(index_str)?;
    let rgb =
        Rgb::parse(&value[eq_idx + 1..]).map_err(|_| ParsePaletteEntryError::InvalidFormat)?;
    Ok(PaletteEntry { index, color: rgb })
}

/// Parse a `u8` the way Zig's `std.fmt.parseInt(u8, s, 0)` does: base
/// auto-detected from a `0x`/`0o`/`0b` prefix (case-sensitive prefix,
/// case-insensitive hex digits), otherwise decimal. A leading `+`/`-` sign
/// is accepted only for decimal, matching Zig's base-0 parser. Port of the
/// `std.fmt.parseInt(u8, ..., 0)` call inside `parsePaletteEntry`.
fn parse_zig_int_u8(s: &str) -> Result<u8, ParsePaletteEntryError> {
    let (negative, rest) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (radix, digits) = if let Some(d) = rest.strip_prefix("0x").or(rest.strip_prefix("0X")) {
        (16, d)
    } else if let Some(d) = rest.strip_prefix("0o").or(rest.strip_prefix("0O")) {
        (8, d)
    } else if let Some(d) = rest.strip_prefix("0b").or(rest.strip_prefix("0B")) {
        (2, d)
    } else {
        (10, rest)
    };
    if digits.is_empty() {
        return Err(ParsePaletteEntryError::InvalidFormat);
    }
    let value =
        u32::from_str_radix(digits, radix).map_err(|_| ParsePaletteEntryError::InvalidFormat)?;
    if negative {
        if value == 0 {
            return Ok(0);
        }
        return Err(ParsePaletteEntryError::InvalidFormat);
    }
    u8::try_from(value).map_err(|_| ParsePaletteEntryError::Overflow)
}

/// A bitmask of which palette indexes have been modified from their default
/// value. Port of `color.zig` `PaletteMask` (a `std.StaticBitSet(256)`); see
/// module docs for why this is a plain array instead of a packed bitset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteMask([bool; 256]);

impl Default for PaletteMask {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl PaletteMask {
    /// An empty mask (no indices set). Port of `.initEmpty()`.
    pub const EMPTY: PaletteMask = PaletteMask([false; 256]);

    pub fn is_set(&self, idx: usize) -> bool {
        self.0[idx]
    }

    pub fn set(&mut self, idx: usize) {
        self.0[idx] = true;
    }

    pub fn unset(&mut self, idx: usize) {
        self.0[idx] = false;
    }

    pub fn count(&self) -> usize {
        self.0.iter().filter(|&&b| b).count()
    }

    /// Iterate the set indices, in ascending order. Stands in for Zig's
    /// `mask.iterator(.{})`.
    pub fn iter_set(&self) -> impl Iterator<Item = usize> + '_ {
        self.0
            .iter()
            .enumerate()
            .filter_map(|(i, &set)| set.then_some(i))
    }
}

/// A palette that can have its colors changed and reset. Purposely built for
/// terminal color operations. Port of `color.zig` `DynamicPalette`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DynamicPalette {
    /// The current palette including any user modifications.
    pub current: Palette,
    /// The original/default palette values.
    pub original: Palette,
    /// Which palette indexes have been modified from their default value.
    pub mask: PaletteMask,
}

impl DynamicPalette {
    /// The default dynamic palette, seeded from [`DEFAULT`]. Port of
    /// `color.zig` `DynamicPalette.default`.
    pub const DEFAULT: DynamicPalette = DynamicPalette::new(DEFAULT);

    /// Initialize a dynamic palette with a default palette. Port of
    /// `color.zig` `DynamicPalette.init`.
    pub const fn new(def: Palette) -> Self {
        Self {
            current: def,
            original: def,
            mask: PaletteMask::EMPTY,
        }
    }

    /// Set a custom color at the given palette index. Port of
    /// `DynamicPalette.set`.
    pub fn set(&mut self, idx: u8, color: Rgb) {
        self.current[idx as usize] = color;
        self.mask.set(idx as usize);
    }

    /// Reset the color at the given palette index to its original value.
    /// Port of `DynamicPalette.reset`.
    pub fn reset(&mut self, idx: u8) {
        self.current[idx as usize] = self.original[idx as usize];
        self.mask.unset(idx as usize);
    }

    /// Reset all colors to their original values. Port of
    /// `DynamicPalette.resetAll`.
    pub fn reset_all(&mut self) {
        *self = Self::new(self.original);
    }

    /// Change the default palette, but preserve the changed values. Port of
    /// `DynamicPalette.changeDefault`.
    pub fn change_default(&mut self, def: Palette) {
        self.original = def;

        // Fast path, the palette is usually not changed.
        if self.mask.count() == 0 {
            self.current = self.original;
            return;
        }

        // There are usually less set than unset, so iterate over the
        // changed values and override them.
        let mut current = def;
        for idx in self.mask.iter_set() {
            current[idx] = self.current[idx];
        }
        self.current = current;
    }
}

/// RGB value that can be changed and reset. This can also be totally unset
/// in every way, in which case the caller can determine their own ultimate
/// default. Port of `color.zig` `DynamicRGB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DynamicRgb {
    pub override_: Option<Rgb>,
    pub default: Option<Rgb>,
}

impl DynamicRgb {
    /// Totally unset: no override, no default. Port of
    /// `DynamicRGB.unset`.
    pub const UNSET: DynamicRgb = DynamicRgb {
        override_: None,
        default: None,
    };

    /// Port of `DynamicRGB.init`.
    pub const fn new(def: Rgb) -> Self {
        Self {
            override_: None,
            default: Some(def),
        }
    }

    /// Port of `DynamicRGB.get`.
    pub fn get(&self) -> Option<Rgb> {
        self.override_.or(self.default)
    }

    /// Port of `DynamicRGB.set`.
    pub fn set(&mut self, color: Rgb) {
        self.override_ = Some(color);
    }

    /// Port of `DynamicRGB.reset`.
    pub fn reset(&mut self) {
        self.override_ = self.default;
    }
}

/// The "special colors" as denoted by xterm: these can be set via OSC 5 or
/// via OSC 4 by adding the palette length to the code. Port of `color.zig`
/// `Special`.
///
/// <https://invisible-island.net/xterm/ctlseqs/ctlseqs.html>
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
    /// "The special colors can also be set by adding the maximum number of
    /// colors (e.g., 88 or 256) to these codes in an OSC 4 control" - xterm
    /// ctlseqs. Port of `Special.osc4`.
    pub const fn osc4(self) -> u16 {
        const MAX: u16 = 256; // Palette length.
        self as u16 + MAX
    }
}

/// The "dynamic colors" as denoted by xterm: these can be set via OSC 10
/// through 19. Port of `color.zig` `Dynamic`.
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
    /// The next dynamic color sequentially. This is required because
    /// specifying colors sequentially without their index automatically
    /// uses the next dynamic color. Port of `Dynamic.next`.
    ///
    /// "Each successive parameter changes the next color in the list. The
    /// value of Ps tells the starting point in the list."
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

/// LAB color space. Port of `color.zig`'s file-private `LAB` struct.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Lab {
    l: f32,
    a: f32,
    b: f32,
}

impl Lab {
    /// RGB to LAB. Port of `LAB.fromRgb`.
    fn from_rgb(rgb: Rgb) -> Lab {
        // Step 1: Normalize sRGB channels from [0, 255] to [0.0, 1.0].
        let mut r = f32::from(rgb.r) / 255.0;
        let mut g = f32::from(rgb.g) / 255.0;
        let mut b = f32::from(rgb.b) / 255.0;

        // Step 2: Apply the inverse sRGB companding (gamma correction) to
        // convert from sRGB to linear RGB.
        r = if r > 0.04045 {
            ((r + 0.055) / 1.055).powf(2.4)
        } else {
            r / 12.92
        };
        g = if g > 0.04045 {
            ((g + 0.055) / 1.055).powf(2.4)
        } else {
            g / 12.92
        };
        b = if b > 0.04045 {
            ((b + 0.055) / 1.055).powf(2.4)
        } else {
            b / 12.92
        };

        // Step 3: Convert linear RGB to CIE XYZ (D65 illuminant); X and Z
        // are normalized by the D65 white point (Xn=0.95047, Zn=1.08883;
        // Yn=1.0 is implicit).
        let x = (r * 0.4124564 + g * 0.3575761 + b * 0.1804375) / 0.95047;
        let y = r * 0.2126729 + g * 0.7151522 + b * 0.0721750;
        let z = (r * 0.0193339 + g * 0.119_192 + b * 0.9503041) / 1.08883;

        // Step 4: Apply the CIE f(t) nonlinear transform to each XYZ
        // component.
        let fx = if x > 0.008856 {
            x.cbrt()
        } else {
            7.787 * x + 16.0 / 116.0
        };
        let fy = if y > 0.008856 {
            y.cbrt()
        } else {
            7.787 * y + 16.0 / 116.0
        };
        let fz = if z > 0.008856 {
            z.cbrt()
        } else {
            7.787 * z + 16.0 / 116.0
        };

        // Step 5: Compute the final CIELAB values.
        Lab {
            l: 116.0 * fy - 16.0,
            a: 500.0 * (fx - fy),
            b: 200.0 * (fy - fz),
        }
    }

    /// LAB to RGB. Port of `LAB.toRgb`.
    fn to_rgb(self) -> Rgb {
        // Step 1: Recover f(Y), f(X), f(Z) from L*a*b*.
        let y = (self.l + 16.0) / 116.0;
        let x = self.a / 500.0 + y;
        let z = y - self.b / 200.0;

        // Step 2: Apply the inverse CIE f(t) transform to get back to XYZ,
        // scaled by the D65 white point.
        let x3 = x * x * x;
        let y3 = y * y * y;
        let z3 = z * z * z;
        let xf = (if x3 > 0.008856 {
            x3
        } else {
            (x - 16.0 / 116.0) / 7.787
        }) * 0.95047;
        let yf = if y3 > 0.008856 {
            y3
        } else {
            (y - 16.0 / 116.0) / 7.787
        };
        let zf = (if z3 > 0.008856 {
            z3
        } else {
            (z - 16.0 / 116.0) / 7.787
        }) * 1.08883;

        // Step 3: Convert CIE XYZ back to linear RGB (inverse of the sRGB
        // to XYZ matrix, D65 illuminant).
        let mut r = xf * 3.2404542 - yf * 1.5371385 - zf * 0.4985314;
        let mut g = -xf * 0.969_266 + yf * 1.8760108 + zf * 0.0415560;
        let mut b = xf * 0.0556434 - yf * 0.2040259 + zf * 1.0572252;

        // Step 4: Apply sRGB companding (gamma correction) back to sRGB.
        r = if r > 0.0031308 {
            1.055 * r.powf(1.0 / 2.4) - 0.055
        } else {
            12.92 * r
        };
        g = if g > 0.0031308 {
            1.055 * g.powf(1.0 / 2.4) - 0.055
        } else {
            12.92 * g
        };
        b = if b > 0.0031308 {
            1.055 * b.powf(1.0 / 2.4) - 0.055
        } else {
            12.92 * b
        };

        // Step 5: Clamp to [0.0, 1.0], scale to [0, 255], round to nearest.
        Rgb::new(
            (r.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
            (g.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
            (b.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        )
    }

    /// Linearly interpolate between two LAB colors component-wise. `t=0`
    /// returns `a`, `t=1` returns `b`. Port of `LAB.lerp`.
    fn lerp(t: f32, a: Lab, b: Lab) -> Lab {
        Lab {
            l: a.l + t * (b.l - a.l),
            a: a.a + t * (b.a - a.a),
            b: a.b + t * (b.b - a.b),
        }
    }
}

/// Generate the 256-color palette from the user's base16 theme colors,
/// terminal background, and terminal foreground. Port of `color.zig`
/// `generate256Color`.
///
/// Motivation: the default 256-color palette uses fixed, fully-saturated
/// colors that clash with custom base16 themes, have poor readability in
/// dark shades, and exhibit inconsistent perceived brightness across hues of
/// the same shade. Generating the extended palette from the user's chosen
/// colors lets programs use the richer 256-color range without a separate
/// theme configuration, and light/dark switching works automatically.
///
/// The 216-color cube (indices 16-231) is built via trilinear interpolation
/// in CIELAB space over the 8 base colors (`bg`, `base[1..=6]`, `fg` mapped
/// to the cube's 8 corners). The 24-step grayscale ramp (indices 232-255) is
/// a linear CIELAB interpolation from background to foreground.
///
/// `skip` marks palette indexes to leave untouched (user-defined colors).
///
/// Reference: <https://gist.github.com/jake-stewart/0a8ea46159a7da2c808e5be2177e1783>
pub fn generate_256_color(
    base: Palette,
    skip: PaletteMask,
    bg: Rgb,
    fg: Rgb,
    harmonious: bool,
) -> Palette {
    // Convert bg, fg, and the 8 base theme colors into CIELAB space so all
    // interpolation is perceptually uniform.
    let mut base8_lab = [
        Lab::from_rgb(bg),
        Lab::from_rgb(base[1]),
        Lab::from_rgb(base[2]),
        Lab::from_rgb(base[3]),
        Lab::from_rgb(base[4]),
        Lab::from_rgb(base[5]),
        Lab::from_rgb(base[6]),
        Lab::from_rgb(fg),
    ];

    // For light themes (fg darker than bg), the cube's dark-to-light
    // orientation is inverted relative to the base color mapping. When
    // `harmonious` is false, swap bg and fg so the cube still runs from
    // black (16) to white (231).
    let is_light_theme = base8_lab[7].l < base8_lab[0].l;
    let invert = is_light_theme && !harmonious;
    if invert {
        base8_lab.swap(0, 7);
    }

    // Start from the base palette so indices 0-15 are preserved as-is.
    let mut result = base;

    // Build the 216-color cube (indices 16-231) via trilinear interpolation.
    let mut idx = 16usize;
    for ri in 0..6 {
        let tr = ri as f32 / 5.0;
        let c0 = Lab::lerp(tr, base8_lab[0], base8_lab[1]);
        let c1 = Lab::lerp(tr, base8_lab[2], base8_lab[3]);
        let c2 = Lab::lerp(tr, base8_lab[4], base8_lab[5]);
        let c3 = Lab::lerp(tr, base8_lab[6], base8_lab[7]);
        for gi in 0..6 {
            let tg = gi as f32 / 5.0;
            let c4 = Lab::lerp(tg, c0, c1);
            let c5 = Lab::lerp(tg, c2, c3);
            for bi in 0..6 {
                if !skip.is_set(idx) {
                    let c6 = Lab::lerp(bi as f32 / 5.0, c4, c5);
                    result[idx] = c6.to_rgb();
                }
                idx += 1;
            }
        }
    }

    // Build the 24-step grayscale ramp (indices 232-255).
    for i in 0..24 {
        let t = (i + 1) as f32 / 25.0;
        if !skip.is_set(idx) {
            let c = Lab::lerp(t, base8_lab[0], base8_lab[7]);
            result[idx] = c.to_rgb();
        }
        idx += 1;
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

    // Port of "parsePaletteEntry".
    #[test]
    fn parse_palette_entry_test() {
        let entry = parse_palette_entry("0=#AABBCC").unwrap();
        assert_eq!(entry.index, 0);
        assert_eq!(entry.color, Rgb::new(170, 187, 204));

        let entry = parse_palette_entry("0b1=#014589").unwrap();
        assert_eq!(entry.index, 1);
        assert_eq!(entry.color, Rgb::new(1, 69, 137));

        let entry = parse_palette_entry("0o7=#234567").unwrap();
        assert_eq!(entry.index, 7);
        assert_eq!(entry.color, Rgb::new(35, 69, 103));

        let entry = parse_palette_entry("0xF=#ABCDEF").unwrap();
        assert_eq!(entry.index, 15);
        assert_eq!(entry.color, Rgb::new(171, 205, 239));

        let entry = parse_palette_entry("0 =  #AABBCC").unwrap();
        assert_eq!(entry.index, 0);
        assert_eq!(entry.color, Rgb::new(170, 187, 204));

        let entry = parse_palette_entry(" 1= #DDEEFF    ").unwrap();
        assert_eq!(entry.index, 1);
        assert_eq!(entry.color, Rgb::new(221, 238, 255));

        let entry = parse_palette_entry("  2  =  #123456 ").unwrap();
        assert_eq!(entry.index, 2);
        assert_eq!(entry.color, Rgb::new(18, 52, 86));

        let entry = parse_palette_entry("1=black").unwrap();
        assert_eq!(entry.index, 1);
        assert_eq!(entry.color, Rgb::new(0, 0, 0));

        assert_eq!(
            parse_palette_entry(" "),
            Err(ParsePaletteEntryError::InvalidFormat)
        );
        assert_eq!(
            parse_palette_entry("a"),
            Err(ParsePaletteEntryError::InvalidFormat)
        );
        assert_eq!(
            parse_palette_entry("256=#AABBCC"),
            Err(ParsePaletteEntryError::Overflow)
        );
        assert_eq!(
            parse_palette_entry("1=notacolor"),
            Err(ParsePaletteEntryError::InvalidFormat)
        );
    }

    // Port of "palette: default".
    #[test]
    fn palette_default_matches_name_default_rgb() {
        for i in 0..16u8 {
            let name = match i {
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
                _ => Name::BrightWhite,
            };
            assert_eq!(name.default_rgb(), DEFAULT[i as usize]);
        }
    }

    // Port of "RGB.parse".
    #[test]
    fn rgb_parse() {
        assert_eq!(Rgb::parse("rgbi:1.0/0/0").unwrap(), Rgb::new(255, 0, 0));
        assert_eq!(Rgb::parse("rgb:7f/a0a0/0").unwrap(), Rgb::new(127, 160, 0));
        assert_eq!(Rgb::parse("rgb:f/ff/fff").unwrap(), Rgb::new(255, 255, 255));
        assert_eq!(Rgb::parse("#ffffff").unwrap(), Rgb::new(255, 255, 255));
        assert_eq!(Rgb::parse("#fff").unwrap(), Rgb::new(255, 255, 255));
        assert_eq!(Rgb::parse("#fffffffff").unwrap(), Rgb::new(255, 255, 255));
        assert_eq!(
            Rgb::parse("#ffffffffffff").unwrap(),
            Rgb::new(255, 255, 255)
        );
        assert_eq!(Rgb::parse("#ff0010").unwrap(), Rgb::new(255, 0, 16));
        assert_eq!(Rgb::parse("0A0B0C").unwrap(), Rgb::new(10, 11, 12));
        assert_eq!(Rgb::parse("FFFFFF").unwrap(), Rgb::new(255, 255, 255));
        assert_eq!(Rgb::parse("FFF").unwrap(), Rgb::new(255, 255, 255));
        assert_eq!(Rgb::parse("#345").unwrap(), Rgb::new(51, 68, 85));
        assert_eq!(Rgb::parse(" #AABBCC   ").unwrap(), Rgb::new(170, 187, 204));

        assert_eq!(Rgb::parse("black").unwrap(), Rgb::new(0, 0, 0));
        assert_eq!(Rgb::parse("red").unwrap(), Rgb::new(255, 0, 0));
        assert_eq!(Rgb::parse("green").unwrap(), Rgb::new(0, 255, 0));
        assert_eq!(Rgb::parse("blue").unwrap(), Rgb::new(0, 0, 255));
        assert_eq!(Rgb::parse("white").unwrap(), Rgb::new(255, 255, 255));

        assert_eq!(Rgb::parse("LawnGreen").unwrap(), Rgb::new(124, 252, 0));
        assert_eq!(
            Rgb::parse("medium spring green").unwrap(),
            Rgb::new(0, 250, 154)
        );
        assert_eq!(Rgb::parse(" Forest Green ").unwrap(), Rgb::new(34, 139, 34));
        assert_eq!(
            Rgb::parse("\tForestGreen\t").unwrap(),
            Rgb::new(34, 139, 34)
        );

        // Invalid format
        assert!(Rgb::parse("").is_err());
        assert!(Rgb::parse("  ").is_err());
        assert!(Rgb::parse("rgb;").is_err());
        assert!(Rgb::parse("rgb:").is_err());
        assert!(Rgb::parse(":a/a/a").is_err());
        assert!(Rgb::parse("a/a/a").is_err());
        assert!(Rgb::parse("rgb:a/a/a/").is_err());
        assert!(Rgb::parse("rgb:00000///").is_err());
        assert!(Rgb::parse("rgb:000/").is_err());
        assert!(Rgb::parse("rgbi:a/a/a").is_err());
        assert!(Rgb::parse("rgb:0.5/0.0/1.0").is_err());
        assert!(Rgb::parse("rgb:not/hex/zz").is_err());
        assert!(Rgb::parse("#").is_err());
        assert!(Rgb::parse("#ff").is_err());
        assert!(Rgb::parse("#ffff").is_err());
        assert!(Rgb::parse("#fffff").is_err());
        assert!(Rgb::parse("#gggggg").is_err());
        assert!(Rgb::parse("#12345").is_err());
        assert!(Rgb::parse("12345").is_err());
        assert!(Rgb::parse("nosuchcolor").is_err());
    }

    // Port of "DynamicPalette: init".
    #[test]
    fn dynamic_palette_init() {
        let p = DynamicPalette::new(DEFAULT);
        assert_eq!(p.current, DEFAULT);
        assert_eq!(p.original, DEFAULT);
        assert_eq!(p.mask.count(), 0);
    }

    // Port of "DynamicPalette: set".
    #[test]
    fn dynamic_palette_set() {
        let mut p = DynamicPalette::new(DEFAULT);
        let new_color = Rgb::new(255, 0, 0);

        p.set(0, new_color);
        assert_eq!(p.current[0], new_color);
        assert!(p.mask.is_set(0));
        assert_eq!(p.mask.count(), 1);

        assert_eq!(p.original[0], DEFAULT[0]);
    }

    // Port of "DynamicPalette: reset".
    #[test]
    fn dynamic_palette_reset() {
        let mut p = DynamicPalette::new(DEFAULT);
        let new_color = Rgb::new(255, 0, 0);

        p.set(0, new_color);
        assert!(p.mask.is_set(0));

        p.reset(0);
        assert_eq!(p.current[0], DEFAULT[0]);
        assert!(!p.mask.is_set(0));
        assert_eq!(p.mask.count(), 0);
    }

    // Port of "DynamicPalette: resetAll".
    #[test]
    fn dynamic_palette_reset_all() {
        let mut p = DynamicPalette::new(DEFAULT);
        let new_color = Rgb::new(255, 0, 0);

        p.set(0, new_color);
        p.set(5, new_color);
        p.set(10, new_color);
        assert_eq!(p.mask.count(), 3);

        p.reset_all();
        assert_eq!(p.current, DEFAULT);
        assert_eq!(p.original, DEFAULT);
        assert_eq!(p.mask.count(), 0);
    }

    // Port of "DynamicPalette: changeDefault with no changes".
    #[test]
    fn dynamic_palette_change_default_with_no_changes() {
        let mut p = DynamicPalette::new(DEFAULT);
        let mut new_palette = DEFAULT;
        new_palette[0] = Rgb::new(100, 100, 100);

        p.change_default(new_palette);
        assert_eq!(p.original, new_palette);
        assert_eq!(p.current, new_palette);
        assert_eq!(p.mask.count(), 0);
    }

    // Port of "DynamicPalette: changeDefault preserves changes".
    #[test]
    fn dynamic_palette_change_default_preserves_changes() {
        let mut p = DynamicPalette::new(DEFAULT);
        let custom_color = Rgb::new(255, 0, 0);

        p.set(5, custom_color);
        assert!(p.mask.is_set(5));

        let mut new_palette = DEFAULT;
        new_palette[0] = Rgb::new(100, 100, 100);
        new_palette[5] = Rgb::new(50, 50, 50);

        p.change_default(new_palette);

        assert_eq!(p.original, new_palette);
        assert_eq!(p.current[0], new_palette[0]);
        assert_eq!(p.current[5], custom_color);
        assert!(p.mask.is_set(5));
        assert_eq!(p.mask.count(), 1);
    }

    // Port of "DynamicPalette: changeDefault with multiple changes".
    #[test]
    fn dynamic_palette_change_default_with_multiple_changes() {
        let mut p = DynamicPalette::new(DEFAULT);
        let red = Rgb::new(255, 0, 0);
        let green = Rgb::new(0, 255, 0);
        let blue = Rgb::new(0, 0, 255);

        p.set(1, red);
        p.set(2, green);
        p.set(3, blue);

        let mut new_palette = DEFAULT;
        new_palette[0] = Rgb::new(50, 50, 50);
        new_palette[1] = Rgb::new(60, 60, 60);

        p.change_default(new_palette);

        assert_eq!(p.current[0], new_palette[0]);
        assert_eq!(p.current[1], red);
        assert_eq!(p.current[2], green);
        assert_eq!(p.current[3], blue);
        assert_eq!(p.mask.count(), 3);
    }

    // Port of "LAB.fromRgb".
    #[test]
    fn lab_from_rgb() {
        let epsilon = 0.5;

        // White (255, 255, 255) -> L*=100, a*=0, b*=0
        let white = Lab::from_rgb(Rgb::new(255, 255, 255));
        assert!((white.l - 100.0).abs() < epsilon);
        assert!((white.a - 0.0).abs() < epsilon);
        assert!((white.b - 0.0).abs() < epsilon);

        // Black (0, 0, 0) -> L*=0, a*=0, b*=0
        let black = Lab::from_rgb(Rgb::new(0, 0, 0));
        assert!((black.l - 0.0).abs() < epsilon);
        assert!((black.a - 0.0).abs() < epsilon);
        assert!((black.b - 0.0).abs() < epsilon);

        // Pure red (255, 0, 0) -> L*=53.23, a*=80.11, b*=67.22
        let red = Lab::from_rgb(Rgb::new(255, 0, 0));
        assert!((red.l - 53.23).abs() < epsilon);
        assert!((red.a - 80.11).abs() < epsilon);
        assert!((red.b - 67.22).abs() < epsilon);

        // Pure green (0, 128, 0) -> L*=46.23, a*=-51.70, b*=49.90
        let green = Lab::from_rgb(Rgb::new(0, 128, 0));
        assert!((green.l - 46.23).abs() < epsilon);
        assert!((green.a - (-51.70)).abs() < epsilon);
        assert!((green.b - 49.90).abs() < epsilon);

        // Pure blue (0, 0, 255) -> L*=32.30, a*=79.20, b*=-107.86
        let blue = Lab::from_rgb(Rgb::new(0, 0, 255));
        assert!((blue.l - 32.30).abs() < epsilon);
        assert!((blue.a - 79.20).abs() < epsilon);
        assert!((blue.b - (-107.86)).abs() < epsilon);
    }

    // Port of "generate256Color: base16 preserved".
    #[test]
    fn generate_256_color_base16_preserved() {
        let bg = Rgb::new(0, 0, 0);
        let fg = Rgb::new(255, 255, 255);
        let palette = generate_256_color(DEFAULT, PaletteMask::EMPTY, bg, fg, false);

        for i in 0..16 {
            assert_eq!(DEFAULT[i], palette[i]);
        }
    }

    // Port of "generate256Color: cube corners match base colors".
    #[test]
    fn generate_256_color_cube_corners_match_base_colors() {
        let bg = Rgb::new(0, 0, 0);
        let fg = Rgb::new(255, 255, 255);
        let palette = generate_256_color(DEFAULT, PaletteMask::EMPTY, bg, fg, false);

        assert_eq!(palette[16], bg);
        assert_eq!(palette[231], fg);
    }

    // Port of "generate256Color: cube corners black/white with harmonious=false".
    #[test]
    fn generate_256_color_cube_corners_black_white_with_harmonious_false() {
        let black = Rgb::new(0, 0, 0);
        let white = Rgb::new(255, 255, 255);

        // Dark theme: bg=black, fg=white.
        let dark = generate_256_color(DEFAULT, PaletteMask::EMPTY, black, white, false);
        assert_eq!(dark[16], black);
        assert_eq!(dark[231], white);

        // Light theme: bg=white, fg=black. The bg/fg swap ensures the cube
        // still runs from black (16) to white (231).
        let light = generate_256_color(DEFAULT, PaletteMask::EMPTY, white, black, false);
        assert_eq!(light[16], black);
        assert_eq!(light[231], white);
    }

    // Port of "generate256Color: light theme cube corners with harmonious=true".
    #[test]
    fn generate_256_color_light_theme_cube_corners_with_harmonious_true() {
        let white = Rgb::new(255, 255, 255);
        let black = Rgb::new(0, 0, 0);

        // harmonious=true skips the bg/fg swap, so the cube preserves the
        // original orientation: (0,0,0)=bg=white, (5,5,5)=fg=black.
        let palette = generate_256_color(DEFAULT, PaletteMask::EMPTY, white, black, true);
        assert_eq!(palette[16], white);
        assert_eq!(palette[231], black);
    }

    // Port of "generate256Color: grayscale ramp monotonic luminance".
    #[test]
    fn generate_256_color_grayscale_ramp_monotonic_luminance() {
        let bg = Rgb::new(0, 0, 0);
        let fg = Rgb::new(255, 255, 255);
        let palette = generate_256_color(DEFAULT, PaletteMask::EMPTY, bg, fg, false);

        let mut prev_lum = 0.0;
        for entry in &palette[232..256] {
            let lum = entry.luminance();
            assert!(lum >= prev_lum);
            prev_lum = lum;
        }
    }

    // Port of "generate256Color: skip mask preserves original colors".
    #[test]
    fn generate_256_color_skip_mask_preserves_original_colors() {
        let bg = Rgb::new(0, 0, 0);
        let fg = Rgb::new(255, 255, 255);

        let mut skip = PaletteMask::EMPTY;
        skip.set(20);
        skip.set(100);
        skip.set(240);

        let palette = generate_256_color(DEFAULT, skip, bg, fg, false);
        assert_eq!(DEFAULT[20], palette[20]);
        assert_eq!(DEFAULT[100], palette[100]);
        assert_eq!(DEFAULT[240], palette[240]);

        // A non-skipped index in the cube should differ from the default.
        assert_ne!(palette[21], DEFAULT[21]);
    }

    // Port of "generate256Color: dark theme harmonious has no effect".
    #[test]
    fn generate_256_color_dark_theme_harmonious_has_no_effect() {
        let bg = Rgb::new(0, 0, 0);
        let fg = Rgb::new(255, 255, 255);
        let normal = generate_256_color(DEFAULT, PaletteMask::EMPTY, bg, fg, false);
        let harmonious = generate_256_color(DEFAULT, PaletteMask::EMPTY, bg, fg, true);

        for i in 16..256 {
            assert_eq!(normal[i], harmonious[i]);
        }
    }

    // Port of "generate256Color: light theme harmonious skips inversion".
    #[test]
    fn generate_256_color_light_theme_harmonious_skips_inversion() {
        let bg = Rgb::new(255, 255, 255);
        let fg = Rgb::new(0, 0, 0);
        let inverted = generate_256_color(DEFAULT, PaletteMask::EMPTY, bg, fg, false);
        let harmonious = generate_256_color(DEFAULT, PaletteMask::EMPTY, bg, fg, true);

        // Cube origin (0,0,0) at index 16: without harmonious, bg and fg are
        // swapped so it becomes the fg-anchored corner; with harmonious it
        // stays as bg.
        assert_eq!(harmonious[16], bg);
        assert_ne!(inverted[16], bg);

        let mut differ = 0;
        for i in 16..232 {
            if inverted[i] != harmonious[i] {
                differ += 1;
            }
        }
        assert!(differ > 0);
    }

    // Port of "generate256Color: light theme harmonious grayscale ramp".
    #[test]
    fn generate_256_color_light_theme_harmonious_grayscale_ramp() {
        let bg = Rgb::new(255, 255, 255);
        let fg = Rgb::new(0, 0, 0);

        // harmonious=false swaps bg/fg, so the ramp runs black->white
        // (increasing).
        {
            let palette = generate_256_color(DEFAULT, PaletteMask::EMPTY, bg, fg, false);
            let mut prev_lum = 0.0;
            for entry in &palette[232..256] {
                let lum = entry.luminance();
                assert!(lum >= prev_lum);
                prev_lum = lum;
            }
        }

        // harmonious=true keeps original order, so the ramp runs
        // white->black (decreasing).
        {
            let palette = generate_256_color(DEFAULT, PaletteMask::EMPTY, bg, fg, true);
            let mut prev_lum = 1.0;
            for entry in &palette[232..256] {
                let lum = entry.luminance();
                assert!(lum <= prev_lum);
                prev_lum = lum;
            }
        }
    }

    // Port of "LAB.toRgb".
    #[test]
    fn lab_to_rgb_round_trip() {
        let cases = [
            Rgb::new(255, 255, 255),
            Rgb::new(0, 0, 0),
            Rgb::new(255, 0, 0),
            Rgb::new(0, 128, 0),
            Rgb::new(0, 0, 255),
            Rgb::new(128, 128, 128),
            Rgb::new(64, 224, 208),
        ];

        for expected in cases {
            let lab = Lab::from_rgb(expected);
            let actual = lab.to_rgb();
            assert_eq!(expected, actual);
        }
    }

    // Port of "osc4" (inline test on Special.osc4).
    #[test]
    fn special_osc4() {
        assert_eq!(Special::Bold.osc4(), 256);
        assert_eq!(Special::Underline.osc4(), 257);
        assert_eq!(Special::Blink.osc4(), 258);
        assert_eq!(Special::Reverse.osc4(), 259);
        assert_eq!(Special::Italic.osc4(), 260);
    }

    // Port of "next" (inline test on Dynamic.next).
    #[test]
    fn dynamic_next() {
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
}
