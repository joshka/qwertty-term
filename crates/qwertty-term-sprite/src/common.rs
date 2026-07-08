//! Shared helper types and functions for drawing sprite glyphs.
//!
//! Ported from `src/font/sprite/draw/common.zig`. The [`Fraction`] type and its
//! [`Fraction::min`]/[`Fraction::max`] rounding rules are load-bearing for
//! seam-free adjacency and are reproduced here **exactly** — see the module
//! docs on [`crate`].

use crate::{Canvas, Metrics};

/// The thickness of a line, relative to the cell's base box thickness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thickness {
    /// Half the base thickness (min 1px).
    SuperLight,
    /// The base thickness.
    Light,
    /// Double the base thickness.
    Heavy,
}

impl Thickness {
    /// The real height, in pixels, of a line of this thickness given the cell's
    /// base box thickness.
    #[must_use]
    pub fn height(self, base: u32) -> u32 {
        match self {
            Thickness::SuperLight => (base / 2).max(1),
            Thickness::Light => base,
            Thickness::Heavy => base * 2,
        }
    }
}

/// Coverage shades used for the shaded block elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Shade {
    /// Fully transparent.
    Off = 0x00,
    /// 25% coverage.
    Light = 0x40,
    /// 50% coverage.
    Medium = 0x80,
    /// 75% coverage.
    Dark = 0xc0,
    /// Fully opaque.
    On = 0xff,
}

/// Which quadrants of a cell a feature occupies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Quads {
    /// Top-left.
    pub tl: bool,
    /// Top-right.
    pub tr: bool,
    /// Bottom-left.
    pub bl: bool,
    /// Bottom-right.
    pub br: bool,
}

/// A corner of a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    /// Top-left.
    Tl,
    /// Top-right.
    Tr,
    /// Bottom-left.
    Bl,
    /// Bottom-right.
    Br,
}

/// An edge of a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// Top edge.
    Top,
    /// Left edge.
    Left,
    /// Bottom edge.
    Bottom,
    /// Right edge.
    Right,
}

/// Horizontal alignment within a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Horizontal {
    /// Align to the left edge.
    Left,
    /// Align to the right edge.
    Right,
    /// Center horizontally.
    Center,
}

/// Vertical alignment within a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vertical {
    /// Align to the top edge.
    Top,
    /// Align to the bottom edge.
    Bottom,
    /// Center vertically.
    Middle,
}

/// Alignment of a figure within a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alignment {
    /// Horizontal alignment.
    pub horizontal: Horizontal,
    /// Vertical alignment.
    pub vertical: Vertical,
}

impl Alignment {
    /// Centered both ways.
    pub const CENTER: Alignment = Alignment {
        horizontal: Horizontal::Center,
        vertical: Vertical::Middle,
    };
    /// Top-centered.
    pub const UPPER: Alignment = Alignment {
        horizontal: Horizontal::Center,
        vertical: Vertical::Top,
    };
    /// Bottom-centered.
    pub const LOWER: Alignment = Alignment {
        horizontal: Horizontal::Center,
        vertical: Vertical::Bottom,
    };
    /// Left-centered.
    pub const LEFT: Alignment = Alignment {
        horizontal: Horizontal::Left,
        vertical: Vertical::Middle,
    };
    /// Right-centered.
    pub const RIGHT: Alignment = Alignment {
        horizontal: Horizontal::Right,
        vertical: Vertical::Middle,
    };
    /// Top-left.
    pub const UPPER_LEFT: Alignment = Alignment {
        horizontal: Horizontal::Left,
        vertical: Vertical::Top,
    };
    /// Top-right.
    pub const UPPER_RIGHT: Alignment = Alignment {
        horizontal: Horizontal::Right,
        vertical: Vertical::Top,
    };
    /// Bottom-left.
    pub const LOWER_LEFT: Alignment = Alignment {
        horizontal: Horizontal::Left,
        vertical: Vertical::Bottom,
    };
    /// Bottom-right.
    pub const LOWER_RIGHT: Alignment = Alignment {
        horizontal: Horizontal::Right,
        vertical: Vertical::Bottom,
    };
}

/// A fraction of the way across a cell, horizontally or vertically.
///
/// The variants collapse many named aliases (eighths, quarters, thirds,
/// halves, edge names) onto a small set of underlying float values, exactly
/// like the Zig enum. What matters is the *value* returned by
/// [`Fraction::value`] and the asymmetric rounding in [`Fraction::min`] vs
/// [`Fraction::max`] — that asymmetry is what makes adjacent cells seam-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fraction {
    /// 0/1 — the min edge (left/top).
    Zero,
    /// 1/8.
    OneEighth,
    /// 1/4 (= 2/8).
    OneQuarter,
    /// 1/3.
    OneThird,
    /// 3/8.
    ThreeEighths,
    /// 1/2.
    Half,
    /// 5/8.
    FiveEighths,
    /// 2/3.
    TwoThirds,
    /// 3/4 (= 6/8).
    ThreeQuarters,
    /// 7/8.
    SevenEighths,
    /// 1/1 — the max edge (right/bottom).
    One,
}

impl Fraction {
    /// Eighth fractions, indexable `0..=8` (`eighths()[i]` = `i/8`).
    #[must_use]
    pub fn eighths() -> [Fraction; 9] {
        [
            Fraction::Zero,
            Fraction::OneEighth,
            Fraction::OneQuarter,
            Fraction::ThreeEighths,
            Fraction::Half,
            Fraction::FiveEighths,
            Fraction::ThreeQuarters,
            Fraction::SevenEighths,
            Fraction::One,
        ]
    }

    /// Quarter fractions, indexable `0..=4` (`quarters()[i]` = `i/4`).
    #[must_use]
    pub fn quarters() -> [Fraction; 5] {
        [
            Fraction::Zero,
            Fraction::OneQuarter,
            Fraction::Half,
            Fraction::ThreeQuarters,
            Fraction::One,
        ]
    }

    /// The underlying float value in `0.0..=1.0`.
    #[must_use]
    pub fn value(self) -> f64 {
        match self {
            Fraction::Zero => 0.0,
            Fraction::OneEighth => 0.125,
            Fraction::OneQuarter => 0.25,
            Fraction::OneThird => 1.0 / 3.0,
            Fraction::ThreeEighths => 0.375,
            Fraction::Half => 0.5,
            Fraction::FiveEighths => 0.625,
            Fraction::TwoThirds => 2.0 / 3.0,
            Fraction::ThreeQuarters => 0.75,
            Fraction::SevenEighths => 0.875,
            Fraction::One => 1.0,
        }
    }

    /// Pixel position of this fraction when used as a **min** (left/top) edge.
    ///
    /// Rounds via the complementary fraction taken from the far end so that
    /// rounding evens out across the cell. For `size = 7` and `Half`,
    /// `7 - round((1 - 0.5) * 7) = 3`, while `max` gives `round(0.5 * 7) = 4`;
    /// so `Zero..Half` and `Half..One` are 4px and 3px, and *both* edges land
    /// on the same pixel in the next cell. Do not "simplify" this.
    #[must_use]
    pub fn min(self, size: u32) -> i32 {
        let s = f64::from(size);
        (s - ((1.0 - self.value()) * s).round()) as i32
    }

    /// Pixel position of this fraction when used as a **max** (right/bottom)
    /// edge. See [`Fraction::min`] for why this rounds differently.
    #[must_use]
    pub fn max(self, size: u32) -> i32 {
        let s = f64::from(size);
        (self.value() * s).round() as i32
    }

    /// The fraction as a raw float across `size`, without pixel alignment. Use
    /// when drawing paths where sub-pixel positioning is fine.
    #[must_use]
    pub fn float(self, size: u32) -> f64 {
        self.value() * f64::from(size)
    }
}

/// Fill the sub-rectangle of the cell bounded by the given fraction lines.
pub fn fill(
    metrics: &Metrics,
    canvas: &mut Canvas,
    x0: Fraction,
    x1: Fraction,
    y0: Fraction,
    y1: Fraction,
) {
    canvas.box_fill(
        x0.min(metrics.cell_width),
        y0.min(metrics.cell_height),
        x1.max(metrics.cell_width),
        y1.max(metrics.cell_height),
        Shade::On as u8,
    );
}

/// Centered vertical line of the given thickness.
pub fn vline_middle(metrics: &Metrics, canvas: &mut Canvas, thickness: Thickness) {
    let thick_px = thickness.height(metrics.box_thickness);
    vline(
        canvas,
        0,
        metrics.cell_height as i32,
        (metrics.cell_width.saturating_sub(thick_px) / 2) as i32,
        thick_px,
    );
}

/// Centered horizontal line of the given thickness.
pub fn hline_middle(metrics: &Metrics, canvas: &mut Canvas, thickness: Thickness) {
    let thick_px = thickness.height(metrics.box_thickness);
    hline(
        canvas,
        0,
        metrics.cell_width as i32,
        (metrics.cell_height.saturating_sub(thick_px) / 2) as i32,
        thick_px,
    );
}

/// Vertical line with left edge at `x`, from `y1` to `y2`.
pub fn vline(canvas: &mut Canvas, y1: i32, y2: i32, x: i32, thickness_px: u32) {
    canvas.box_fill(x, y1, x + thickness_px as i32, y2, Shade::On as u8);
}

/// Horizontal line with top edge at `y`, from `x1` to `x2`.
pub fn hline(canvas: &mut Canvas, x1: i32, x2: i32, y: i32, thickness_px: u32) {
    canvas.box_fill(x1, y, x2, y + thickness_px as i32, Shade::On as u8);
}
