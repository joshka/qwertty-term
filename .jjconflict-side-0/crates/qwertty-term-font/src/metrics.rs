//! Cell-metrics derivation from font face metadata.
//!
//! Port of Ghostty's `src/font/Metrics.zig` (commit `2da015cd6`). See
//! `docs/analysis/font-foundations.md` for the full derivation algorithm and
//! rationale (rounding/centering choices, the modifier redistribution logic).

use std::collections::HashMap;

/// Recommended cell dimensions and glyph-decoration placements for a
/// monospace grid using a particular font, in pixels.
///
/// This is a faithful port of Ghostty's `Metrics` struct. All integer fields
/// are pixel quantities; a handful of `f64` fields are kept unrounded because
/// downstream scaling calculations (and the modifier system) need to know how
/// much rounding error there is between the font's design dimensions and our
/// pixel-quantized cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// Recommended cell width and height for a monospace grid using this font.
    pub cell_width: u32,
    pub cell_height: u32,

    /// Distance in pixels from the bottom of the cell to the text baseline.
    pub cell_baseline: u32,

    /// Distance in pixels from the top of the cell to the top of the underline.
    pub underline_position: u32,
    /// Thickness in pixels of the underline.
    pub underline_thickness: u32,

    /// Distance in pixels from the top of the cell to the top of the strikethrough.
    pub strikethrough_position: u32,
    /// Thickness in pixels of the strikethrough.
    pub strikethrough_thickness: u32,

    /// Distance in pixels from the top of the cell to the top of the overline.
    /// Can be negative to adjust the position above the top of the cell.
    pub overline_position: i32,
    /// Thickness in pixels of the overline.
    pub overline_thickness: u32,

    /// Thickness in pixels of box drawing characters.
    pub box_thickness: u32,

    /// The thickness in pixels of the cursor sprite. This has a default value
    /// because it is not determined by fonts but rather by user configuration.
    pub cursor_thickness: u32,

    /// The height in pixels of the cursor sprite.
    pub cursor_height: u32,

    /// The constraint height for nerd fonts icons.
    pub icon_height: f64,

    /// The constraint height for nerd fonts icons limited to a single cell width.
    pub icon_height_single: f64,

    /// The unrounded face width, used in scaling calculations.
    pub face_width: f64,

    /// The unrounded face height, used in scaling calculations.
    pub face_height: f64,

    /// The offset from the bottom of the cell to the bottom of the face's
    /// bounding box, based on the rounded and potentially adjusted cell
    /// height.
    pub face_y: f64,
}

/// Minimum acceptable values for some fields, to prevent modifiers from being
/// able to, for example, cause 0-thickness underlines.
mod minimums {
    pub const CELL_WIDTH: u32 = 1;
    pub const CELL_HEIGHT: u32 = 1;
    pub const UNDERLINE_THICKNESS: u32 = 1;
    pub const STRIKETHROUGH_THICKNESS: u32 = 1;
    pub const OVERLINE_THICKNESS: u32 = 1;
    pub const BOX_THICKNESS: u32 = 1;
    pub const CURSOR_THICKNESS: u32 = 1;
    pub const CURSOR_HEIGHT: u32 = 1;
    pub const ICON_HEIGHT: f64 = 1.0;
    pub const ICON_HEIGHT_SINGLE: f64 = 1.0;
    pub const FACE_HEIGHT: f64 = 1.0;
    pub const FACE_WIDTH: f64 = 1.0;
}

/// Metrics extracted from a font face, based on the metadata tables and
/// glyph measurements.
///
/// Try to pass values with as much precision as possible; do not round them
/// before using them to build this struct. For any `None` fields, estimates
/// will be used (see the doc comment on each getter).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FaceMetrics {
    /// Pixels per em, dividing the other values in this struct by this should
    /// yield sizes in ems, to allow comparing metrics from faces of different
    /// sizes.
    pub px_per_em: f64,

    /// The minimum cell width that can contain any glyph in the ASCII range.
    ///
    /// Determined by measuring all printable glyphs in the ASCII range.
    pub cell_width: f64,

    /// The typographic ascent metric from the font.
    /// This represents the maximum vertical position of the highest ascender.
    ///
    /// Relative to the baseline, in px, +Y=up
    pub ascent: f64,

    /// The typographic descent metric from the font.
    /// This represents the minimum vertical position of the lowest descender.
    ///
    /// Relative to the baseline, in px, +Y=up
    ///
    /// Note: as this value is generally below the baseline, it is typically
    /// negative.
    pub descent: f64,

    /// The typographic line gap (aka "leading") metric from the font.
    /// This represents the additional space to be added between lines in
    /// addition to the space defined by the ascent and descent metrics.
    ///
    /// Positive value in px
    pub line_gap: f64,

    /// The TOP of the underline stroke.
    ///
    /// Relative to the baseline, in px, +Y=up
    pub underline_position: Option<f64>,

    /// The thickness of the underline stroke in px.
    pub underline_thickness: Option<f64>,

    /// The TOP of the strikethrough stroke.
    ///
    /// Relative to the baseline, in px, +Y=up
    pub strikethrough_position: Option<f64>,

    /// The thickness of the strikethrough stroke in px.
    pub strikethrough_thickness: Option<f64>,

    /// The height of capital letters in the font, either derived from a
    /// provided cap height metric or measured from the height of the capital
    /// H glyph.
    pub cap_height: Option<f64>,

    /// The height of lowercase letters in the font, either derived from a
    /// provided ex height metric or measured from the height of the
    /// lowercase x glyph.
    pub ex_height: Option<f64>,

    /// The measured height of the bounding box containing all printable
    /// ASCII characters. This can be different from ascent - descent for two
    /// reasons: non-letter symbols like @ and $ often exceed the ascender and
    /// descender lines; and fonts often bake the line gap into the ascent
    /// and descent metrics (as per, e.g., the Google Fonts guidelines:
    /// <https://simoncozens.github.io/gf-docs/metrics.html>).
    ///
    /// Positive value in px
    pub ascii_height: Option<f64>,

    /// The width of the character "水" (CJK water ideograph, U+6C34), if
    /// present. This is used for font size adjustment, to normalize the
    /// width of CJK fonts mixed with latin fonts.
    ///
    /// NOTE: IC = Ideograph Character
    pub ic_width: Option<f64>,
}

impl FaceMetrics {
    /// Convenience function for getting the line height (ascent - descent +
    /// line_gap).
    pub fn line_height(&self) -> f64 {
        self.ascent - self.descent + self.line_gap
    }

    /// Convenience function for getting the cap height. If this is not
    /// defined in the font, we estimate it as 75% of the ascent.
    pub fn cap_height(&self) -> f64 {
        if let Some(value) = self.cap_height
            && value > 0.0
        {
            return value;
        }
        0.75 * self.ascent
    }

    /// Convenience function for getting the ex height. If this is not
    /// defined in the font, we estimate it as 75% of the cap height.
    pub fn ex_height(&self) -> f64 {
        if let Some(value) = self.ex_height
            && value > 0.0
        {
            return value;
        }
        0.75 * self.cap_height()
    }

    /// Convenience function for getting the ASCII height. If we couldn't
    /// measure this, we use 1.5 * cap_height as our estimator, based on
    /// measurements across programming fonts.
    pub fn ascii_height(&self) -> f64 {
        if let Some(value) = self.ascii_height
            && value > 0.0
        {
            return value;
        }
        1.5 * self.cap_height()
    }

    /// Convenience function for getting the ideograph width. If this is not
    /// defined in the font, we estimate it as the minimum of the ascii
    /// height and two cell widths.
    pub fn ic_width(&self) -> f64 {
        if let Some(value) = self.ic_width
            && value > 0.0
        {
            return value;
        }
        self.ascii_height().min(2.0 * self.cell_width)
    }

    /// Convenience function for getting the underline thickness. If this is
    /// not defined in the font, we estimate it as 15% of the ex height.
    pub fn underline_thickness(&self) -> f64 {
        if let Some(value) = self.underline_thickness
            && value > 0.0
        {
            return value;
        }
        0.15 * self.ex_height()
    }

    /// Convenience function for getting the strikethrough thickness. If this
    /// is not defined in the font, we set it equal to the underline
    /// thickness.
    pub fn strikethrough_thickness(&self) -> f64 {
        if let Some(value) = self.strikethrough_thickness
            && value > 0.0
        {
            return value;
        }
        self.underline_thickness()
    }

    // NOTE: The getters below return positions, not sizes, so both positive
    // and negative values are valid, hence no sign validation.

    /// Convenience function for getting the underline position. If this is
    /// not defined in the font, we place it one underline thickness below
    /// the baseline.
    pub fn underline_position(&self) -> f64 {
        self.underline_position
            .unwrap_or(-self.underline_thickness())
    }

    /// Convenience function for getting the strikethrough position. If this
    /// is not defined in the font, we center it at half the ex height, so
    /// that it's perfectly centered on lower case text.
    pub fn strikethrough_position(&self) -> f64 {
        self.strikethrough_position
            .unwrap_or((self.ex_height() + self.strikethrough_thickness()) * 0.5)
    }
}

impl Metrics {
    /// Purposely not `pub` outside this module: forces callers within the
    /// crate to use struct-update syntax (`Metrics { field: v, ..init() }`)
    /// so unused-field mistakes are caught by the compiler, mirroring the
    /// non-pub `init()` in Metrics.zig. Only used by tests today (production
    /// code always builds a full `Metrics` via `calc`), same as upstream.
    #[cfg(test)]
    fn init() -> Metrics {
        Metrics {
            cell_width: 0,
            cell_height: 0,
            cell_baseline: 0,
            underline_position: 0,
            underline_thickness: 0,
            strikethrough_position: 0,
            strikethrough_thickness: 0,
            overline_position: 0,
            overline_thickness: 0,
            box_thickness: 0,
            cursor_thickness: 1,
            cursor_height: 0,
            icon_height: 0.0,
            icon_height_single: 0.0,
            face_width: 0.0,
            face_height: 0.0,
            face_y: 0.0,
        }
    }

    /// Calculate cell metrics from face-level metrics extracted from a font.
    ///
    /// Try to pass values with as much precision as possible; do not round
    /// them before calling this. For any `None` fields on `face`, estimates
    /// will be used (see `FaceMetrics`'s getters).
    pub fn calc(face: FaceMetrics) -> Metrics {
        // These are the unrounded advance width and line height values,
        // which are retained separately from the rounded cell width and
        // height values (below), for calculations that need to know how
        // much error there is between the design dimensions of the font
        // and the pixel dimensions of our cells.
        let face_width = face.cell_width;
        let face_height = face.line_height();

        // The cell width and height values need to be integers since they
        // represent pixel dimensions of the grid cells in the terminal.
        //
        // We use round-half-away-from-zero for the cell width to limit the
        // difference from the "true" width value to no more than 0.5px. This
        // is a better approximation of the authorial intent of the font than
        // ceiling would be, and makes the apparent spacing match better
        // between low and high DPI displays.
        //
        // This does mean that it's possible for a glyph to overflow the
        // edge of the cell by a pixel if it has no side bearings, but in
        // reality such glyphs are generally meant to connect to adjacent
        // glyphs in some way so it's not really an issue.
        //
        // The same is true for the height. Some fonts are poorly authored
        // and have a descender on a normal glyph that extends right up to
        // the descent value of the face, and this can result in the glyph
        // overflowing the bottom of the cell by a pixel, which isn't good
        // but if we try to prevent it by increasing the cell height then we
        // get line heights that are too large for most users and even more
        // inconsistent across DPIs.
        let cell_width = face_width.round();
        let cell_height = face_height.round();

        // We split our line gap in two parts, and put half of it on the top
        // of the cell and the other half on the bottom, so that our text
        // never bumps up against either edge of the cell vertically.
        let half_line_gap = face.line_gap / 2.0;

        // NOTE: Unlike all our other metrics, `cell_baseline` is relative to
        // the BOTTOM of the cell rather than the top.
        let face_baseline = half_line_gap - face.descent;
        // We calculate the baseline by trying to center the face vertically
        // in the pixel-rounded cell height, so that before rounding it will
        // be an even distance from the top and bottom of the cell, meaning
        // it either sticks out the same amount or is inset the same amount,
        // depending on whether the cell height was rounded up or down from
        // the line height. We do this by adding half the difference between
        // the cell height and the face height.
        let cell_baseline = (face_baseline - (cell_height - face_height) / 2.0).round();

        // We keep track of the offset from the bottom of the cell to the
        // bottom of the face's "true" bounding box, which at this point,
        // since nothing has been scaled yet, is equivalent to the offset
        // between the baseline we draw at (cell_baseline) and the one the
        // font wants (face_baseline).
        let face_y = cell_baseline - face_baseline;

        // We calculate a top_to_baseline to make following calculations
        // simpler.
        let top_to_baseline = cell_height - cell_baseline;

        // Get the other font metrics or their estimates. See doc comments
        // on `FaceMetrics`'s getters for explanations of the estimation
        // heuristics.
        let cap_height = face.cap_height();
        let underline_thickness = face.underline_thickness().ceil().max(1.0);
        let strikethrough_thickness = face.strikethrough_thickness().ceil().max(1.0);
        let underline_position = (top_to_baseline - face.underline_position()).round();
        let strikethrough_position = (top_to_baseline - face.strikethrough_position()).round();

        // Same heuristic as the font_patcher script. We store icon_height
        // separately from face_height such that modifiers can apply to the
        // former without affecting the latter.
        let icon_height = face_height;
        let icon_height_single = (2.0 * cap_height + face_height) / 3.0;

        let mut result = Metrics {
            cell_width: cell_width as u32,
            cell_height: cell_height as u32,
            cell_baseline: cell_baseline as u32,
            underline_position: underline_position as u32,
            underline_thickness: underline_thickness as u32,
            strikethrough_position: strikethrough_position as u32,
            strikethrough_thickness: strikethrough_thickness as u32,
            overline_position: 0,
            overline_thickness: underline_thickness as u32,
            box_thickness: underline_thickness as u32,
            cursor_thickness: 1,
            cursor_height: cell_height as u32,
            icon_height,
            icon_height_single,
            face_width,
            face_height,
            face_y,
        };

        // Ensure all metrics are within their allowable range.
        result.clamp();

        result
    }

    /// Apply a set of modifiers.
    pub fn apply(&mut self, mods: &ModifierSet) {
        for (&key, modifier) in mods.iter() {
            match key {
                // We clamp these values to a minimum of 1 to prevent
                // divide-by-zero in downstream operations.
                Key::CellWidth => {
                    let original = self.cell_width;
                    let new = modifier.apply_u32(original).max(1);
                    if new == original {
                        continue;
                    }
                    self.cell_width = new;
                }
                Key::CellHeight => {
                    let original = self.cell_height;
                    let new = modifier.apply_u32(original).max(1);
                    if new == original {
                        continue;
                    }
                    self.cell_height = new;

                    // For cell height, we have to also modify some
                    // positions that are absolute from the top of the cell.
                    // The main goal here is to center the baseline so that
                    // text is vertically centered in the cell.
                    let original_f64 = original as f64;
                    let new_f64 = new as f64;
                    let diff = new_f64 - original_f64;
                    let half_diff = diff / 2.0;

                    // If the diff is even, the number of pixels we add will
                    // be the same for the top and the bottom, but if the
                    // diff is odd then we want to add the extra pixel to
                    // the edge of the cell that needs it most.
                    //
                    // How much the edge "needs it" depends on whether the
                    // face is higher or lower than it should be to be
                    // perfectly centered in the cell.
                    //
                    // If the face were perfectly centered then face_y would
                    // be equal to half of the difference between the cell
                    // height and the face height.
                    let position_with_respect_to_center =
                        self.face_y - (original_f64 - self.face_height) / 2.0;

                    let (diff_top, diff_bottom) = if position_with_respect_to_center > 0.0 {
                        // The baseline is higher than it should be, so we
                        // add the extra to the top, or if it's a negative
                        // diff it gets added to the bottom because of how
                        // floor and ceil work.
                        (half_diff.ceil(), half_diff.floor())
                    } else {
                        // The baseline is lower than it should be, so we
                        // add the extra to the bottom, or vice versa for
                        // negative diffs.
                        (half_diff.floor(), half_diff.ceil())
                    };

                    // The cell baseline and face_y values are relative to
                    // the bottom of the cell so we add the bottom diff to
                    // them.
                    add_float_to_u32(&mut self.cell_baseline, diff_bottom);
                    self.face_y += diff_bottom;

                    // These are all relative to the top of the cell.
                    add_float_to_u32(&mut self.underline_position, diff_top);
                    add_float_to_u32(&mut self.strikethrough_position, diff_top);
                    self.overline_position = self.overline_position.saturating_add(diff_top as i32);
                }
                Key::IconHeight => {
                    self.icon_height = modifier.apply_f64(self.icon_height);
                    self.icon_height_single = modifier.apply_f64(self.icon_height_single);
                }
                // Not specially handled in Metrics.zig's modifier switch
                // either (no `.cell_baseline` case) -- it falls into the
                // generic `inline else` arm there, which just applies the
                // modifier straight to the field. Mirrored here for parity,
                // though in practice no caller is expected to target this
                // key directly (see the `Key` doc comment).
                Key::CellBaseline => {
                    self.cell_baseline = modifier.apply_u32(self.cell_baseline);
                }
                Key::UnderlinePosition => {
                    self.underline_position = modifier.apply_u32(self.underline_position);
                }
                Key::UnderlineThickness => {
                    self.underline_thickness = modifier.apply_u32(self.underline_thickness);
                }
                Key::StrikethroughPosition => {
                    self.strikethrough_position = modifier.apply_u32(self.strikethrough_position);
                }
                Key::StrikethroughThickness => {
                    self.strikethrough_thickness = modifier.apply_u32(self.strikethrough_thickness);
                }
                Key::OverlinePosition => {
                    self.overline_position = modifier.apply_i32(self.overline_position);
                }
                Key::OverlineThickness => {
                    self.overline_thickness = modifier.apply_u32(self.overline_thickness);
                }
                Key::BoxThickness => {
                    self.box_thickness = modifier.apply_u32(self.box_thickness);
                }
                Key::CursorThickness => {
                    self.cursor_thickness = modifier.apply_u32(self.cursor_thickness);
                }
                Key::CursorHeight => {
                    self.cursor_height = modifier.apply_u32(self.cursor_height);
                }
                Key::IconHeightSingle => {
                    self.icon_height_single = modifier.apply_f64(self.icon_height_single);
                }
                Key::FaceWidth => {
                    self.face_width = modifier.apply_f64(self.face_width);
                }
                Key::FaceHeight => {
                    self.face_height = modifier.apply_f64(self.face_height);
                }
                Key::FaceY => {
                    self.face_y = modifier.apply_f64(self.face_y);
                }
            }
        }

        // Prevent modifiers from pushing metrics out of their allowable
        // range.
        self.clamp();
    }

    /// Clamp all metrics to their allowable range.
    fn clamp(&mut self) {
        self.cell_width = self.cell_width.max(minimums::CELL_WIDTH);
        self.cell_height = self.cell_height.max(minimums::CELL_HEIGHT);
        self.underline_thickness = self.underline_thickness.max(minimums::UNDERLINE_THICKNESS);
        self.strikethrough_thickness = self
            .strikethrough_thickness
            .max(minimums::STRIKETHROUGH_THICKNESS);
        self.overline_thickness = self.overline_thickness.max(minimums::OVERLINE_THICKNESS);
        self.box_thickness = self.box_thickness.max(minimums::BOX_THICKNESS);
        self.cursor_thickness = self.cursor_thickness.max(minimums::CURSOR_THICKNESS);
        self.cursor_height = self.cursor_height.max(minimums::CURSOR_HEIGHT);
        self.icon_height = self.icon_height.max(minimums::ICON_HEIGHT);
        self.icon_height_single = self.icon_height_single.max(minimums::ICON_HEIGHT_SINGLE);
        self.face_height = self.face_height.max(minimums::FACE_HEIGHT);
        self.face_width = self.face_width.max(minimums::FACE_WIDTH);
    }
}

/// Helper function for adding an `f64` to a `u32`.
///
/// Performs saturating addition or subtraction depending on the sign of the
/// provided float. The float is assumed to have an integer value (this is
/// upheld by every call site: the `diff_top`/`diff_bottom` values are always
/// the result of `.floor()` or `.ceil()`).
fn add_float_to_u32(int: &mut u32, float: f64) {
    debug_assert_eq!(float.floor(), float);
    if float >= 0.0 {
        *int = int.saturating_add(float as u32);
    } else {
        *int = int.saturating_sub((-float) as u32);
    }
}

/// A set of modifiers to apply to metrics. We use a hash map because we
/// expect most metrics to be unmodified and want to take up as little space
/// as possible.
pub type ModifierSet = HashMap<Key, Modifier>;

/// A modifier to apply to a metrics value. The modifier value represents a
/// delta, so percent is a percentage to change, not a percentage of. For
/// example, "20%" is 20% larger, not 20% of the value. Likewise, an absolute
/// value of "20" is 20 larger, not literally 20.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Modifier {
    /// Stored as `1.0 + delta`, e.g. `"20%"` parses to `1.2`, `"-20%"`
    /// parses to `0.8`.
    Percent(f64),
    Absolute(i32),
}

/// Error parsing a [`Modifier`] from a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidFormat;

impl Modifier {
    /// Parses the modifier value. If the value ends in "%" it is assumed to
    /// be a percent, otherwise the value is parsed as an integer.
    pub fn parse(input: &str) -> Result<Modifier, InvalidFormat> {
        if input.is_empty() {
            return Err(InvalidFormat);
        }

        if let Some(prefix) = input.strip_suffix('%') {
            let mut percent: f64 = prefix.parse().map_err(|_| InvalidFormat)?;
            percent /= 100.0;

            if percent <= -1.0 {
                return Ok(Modifier::Percent(0.0));
            }
            // Both branches below compute the same expression as the Zig
            // source (`1 + percent`); kept as two branches to mirror the
            // original control flow exactly.
            if percent < 0.0 {
                return Ok(Modifier::Percent(1.0 + percent));
            }
            return Ok(Modifier::Percent(1.0 + percent));
        }

        input
            .parse::<i32>()
            .map(Modifier::Absolute)
            .map_err(|_| InvalidFormat)
    }

    fn apply_u32(self, v: u32) -> u32 {
        match self {
            Modifier::Percent(p) => {
                let p_clamped = p.max(0.0);
                let v_f64 = v as f64;
                (v_f64 * p_clamped).round() as u32
            }
            Modifier::Absolute(abs) => {
                let v_i64 = v as i64;
                let abs_i64 = abs as i64;
                let applied = v_i64.saturating_add(abs_i64);
                let clamped = applied.max(0);
                clamped.min(u32::MAX as i64) as u32
            }
        }
    }

    fn apply_i32(self, v: i32) -> i32 {
        match self {
            Modifier::Percent(p) => {
                let p_clamped = p.max(0.0);
                let v_f64 = v as f64;
                (v_f64 * p_clamped).round() as i32
            }
            Modifier::Absolute(abs) => v.saturating_add(abs),
        }
    }

    fn apply_f64(self, v: f64) -> f64 {
        match self {
            Modifier::Percent(p) => v * p.max(0.0),
            Modifier::Absolute(abs) => v + abs as f64,
        }
    }
}

/// Key is an enum of all the available metrics keys (every `u32`/`i32`/`f64`
/// field of [`Metrics`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    CellWidth,
    CellHeight,
    CellBaseline,
    UnderlinePosition,
    UnderlineThickness,
    StrikethroughPosition,
    StrikethroughThickness,
    OverlinePosition,
    OverlineThickness,
    BoxThickness,
    CursorThickness,
    CursorHeight,
    IconHeight,
    IconHeightSingle,
    FaceWidth,
    FaceHeight,
    FaceY,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_apply_modifiers() {
        let mut set = ModifierSet::new();
        set.insert(Key::CellWidth, Modifier::Percent(1.2));

        let mut m = Metrics::init();
        m.cell_width = 100;
        m.apply(&set);
        assert_eq!(m.cell_width, 120);
    }

    #[test]
    fn metrics_adjust_cell_height_smaller() {
        let mut set = ModifierSet::new();
        // We choose numbers such that the subtracted number of pixels is
        // odd, as that's the case that could most easily have off-by-one
        // errors. Here we're removing 25 pixels: 13 on the bottom, 12 on
        // top, split that way because we're simulating a face that's
        // 0.33px higher than it "should" be (due to rounding).
        set.insert(Key::CellHeight, Modifier::Percent(0.75));

        let mut m = Metrics::init();
        m.face_y = 0.33;
        m.cell_baseline = 50;
        m.underline_position = 55;
        m.strikethrough_position = 30;
        m.overline_position = 0;
        m.cell_height = 100;
        m.face_height = 99.67;
        m.cursor_height = 100;
        m.apply(&set);
        assert_eq!(m.face_y, -12.67);
        assert_eq!(m.cell_height, 75);
        assert_eq!(m.cell_baseline, 37);
        assert_eq!(m.underline_position, 43);
        assert_eq!(m.strikethrough_position, 18);
        assert_eq!(m.overline_position, -12);
        // Cursor height is separate from cell height and does not follow
        // it.
        assert_eq!(m.cursor_height, 100);
    }

    #[test]
    fn metrics_adjust_cell_height_larger() {
        let mut set = ModifierSet::new();
        // We choose numbers such that the added number of pixels is odd,
        // as that's the case that could most easily have off-by-one
        // errors. Here we're adding 75 pixels: 37 on the bottom, 38 on
        // top, split that way because we're simulating a face that's
        // 0.33px higher than it "should" be (due to rounding).
        set.insert(Key::CellHeight, Modifier::Percent(1.75));

        let mut m = Metrics::init();
        m.face_y = 0.33;
        m.cell_baseline = 50;
        m.underline_position = 55;
        m.strikethrough_position = 30;
        m.overline_position = 0;
        m.cell_height = 100;
        m.face_height = 99.67;
        m.cursor_height = 100;
        m.apply(&set);
        assert_eq!(m.face_y, 37.33);
        assert_eq!(m.cell_height, 175);
        assert_eq!(m.cell_baseline, 87);
        assert_eq!(m.underline_position, 93);
        assert_eq!(m.strikethrough_position, 68);
        assert_eq!(m.overline_position, 38);
        // Cursor height is separate from cell height and does not follow
        // it.
        assert_eq!(m.cursor_height, 100);
    }

    #[test]
    fn metrics_adjust_icon_height_by_percentage() {
        let mut set = ModifierSet::new();
        set.insert(Key::IconHeight, Modifier::Percent(0.75));

        let mut m = Metrics::init();
        m.icon_height = 100.0;
        m.icon_height_single = 80.0;
        m.face_height = 100.0;
        m.face_y = 1.0;
        m.apply(&set);
        assert_eq!(m.icon_height, 75.0);
        assert_eq!(m.icon_height_single, 60.0);
        // Face metrics not affected
        assert_eq!(m.face_height, 100.0);
        assert_eq!(m.face_y, 1.0);
    }

    #[test]
    fn metrics_adjust_icon_height_by_absolute_pixels() {
        let mut set = ModifierSet::new();
        set.insert(Key::IconHeight, Modifier::Absolute(-5));

        let mut m = Metrics::init();
        m.icon_height = 100.0;
        m.icon_height_single = 80.0;
        m.face_height = 100.0;
        m.face_y = 1.0;
        m.apply(&set);
        assert_eq!(m.icon_height, 95.0);
        assert_eq!(m.icon_height_single, 75.0);
        // Face metrics not affected
        assert_eq!(m.face_height, 100.0);
        assert_eq!(m.face_y, 1.0);
    }

    #[test]
    fn modifier_parse_absolute() {
        assert_eq!(Modifier::parse("100"), Ok(Modifier::Absolute(100)));
        assert_eq!(Modifier::parse("-100"), Ok(Modifier::Absolute(-100)));
    }

    #[test]
    fn modifier_parse_percent() {
        assert_eq!(Modifier::parse("20%"), Ok(Modifier::Percent(1.2)));
        assert_eq!(Modifier::parse("-20%"), Ok(Modifier::Percent(0.8)));
        assert_eq!(Modifier::parse("0%"), Ok(Modifier::Percent(1.0)));
    }

    #[test]
    fn modifier_percent() {
        {
            let m = Modifier::Percent(0.8);
            let v = m.apply_u32(100);
            assert_eq!(v, 80);
        }
        {
            let m = Modifier::Percent(1.8);
            let v = m.apply_u32(100);
            assert_eq!(v, 180);
        }
    }

    #[test]
    fn modifier_absolute() {
        {
            let m = Modifier::Absolute(-100);
            let v = m.apply_u32(100);
            assert_eq!(v, 0);
        }
        {
            let m = Modifier::Absolute(-120);
            let v = m.apply_u32(100);
            assert_eq!(v, 0);
        }
        {
            let m = Modifier::Absolute(100);
            let v = m.apply_u32(100);
            assert_eq!(v, 200);
        }
    }
}
