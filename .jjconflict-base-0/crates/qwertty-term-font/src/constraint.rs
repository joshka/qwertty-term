//! Glyph constraint sizing/positioning — the full port of upstream
//! `RenderOptions.Constraint` (`src/font/Glyph.zig`, commit `2da015cd6`).
//!
//! A [`Constraint`] scales and positions a glyph's ink box within its cell(s)
//! according to a sizing rule ([`Size`]), per-axis alignment ([`Align`]), a
//! scale-group definition (`relative_*`), padding, and a height metric
//! ([`Height`]). Nerd Fonts PUA icons carry a per-codepoint constraint (the
//! generated [`crate::nerd_font_constraints`] table); emoji use a fixed `.cover`
//! + center constraint. Both flow through [`Constraint::constrain`].
//!
//! This supersedes the earlier emoji-only `EmojiConstraint` reduction: the emoji
//! constraint is now just `Constraint { size: Cover, align_*: Center, pad_left/
//! right: 0.025 }` ([`Constraint::EMOJI`]), and PUA icons use the table entries,
//! all sharing one exact port of the constraint math.
//!
//! Math parity: `constrain` / `constrain_inner` / `scale_factors` / `aligned_y`
//! / `aligned_x` mirror `Glyph.zig:180-433` line-for-line (f64 throughout, no
//! design-unit rounding — we stay in f64 straight to rasterization, as upstream
//! notes).

use crate::metrics::Metrics;

/// A glyph's ink box: size and origin (CoreGraphics space, +Y up, cell-relative
/// after the baseline fold). The analog of upstream `Glyph.Size`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphSize {
    pub width: f64,
    pub height: f64,
    pub x: f64,
    pub y: f64,
}

/// Sizing rule (`Constraint.Size`, Glyph.zig:121-139).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Size {
    /// Don't change the size of this glyph.
    #[default]
    None,
    /// Scale down if needed to fit within the bounds (aspect-preserving).
    Fit,
    /// Scale up or down to exactly match the bounds (aspect-preserving).
    Cover,
    /// Scale down to fit; if under one cell, scale up; if over one cell but
    /// within bounds, do nothing. (Nerd Font specific.)
    FitCover1,
    /// Stretch to exactly fit the bounds in both directions (ignores aspect).
    Stretch,
}

/// Per-axis alignment rule (`Constraint.Align`, Glyph.zig:141-156).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// Don't move the glyph on this axis.
    #[default]
    None,
    /// Align the leading (bottom/left) edge to the axis's leading edge.
    Start,
    /// Align the trailing (top/right) edge to the axis's trailing edge.
    End,
    /// Center on this axis.
    Center,
    /// Center on this axis w.r.t. the first cell even for multi-cell
    /// constraints. (Nerd Font specific.)
    Center1,
}

/// Height metric to constrain against (`Constraint.Height`, Glyph.zig:158-167).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Height {
    /// Full line height of the primary face.
    #[default]
    Cell,
    /// The icon height from grid metrics (depends on constraint width and the
    /// `adjust-icon-height` config option).
    Icon,
}

/// A glyph constraint (`RenderOptions.Constraint`, Glyph.zig:83-434).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Constraint {
    pub size: Size,
    pub align_vertical: Align,
    pub align_horizontal: Align,
    pub pad_top: f64,
    pub pad_left: f64,
    pub pad_right: f64,
    pub pad_bottom: f64,
    pub relative_width: f64,
    pub relative_height: f64,
    pub relative_x: f64,
    pub relative_y: f64,
    pub max_xy_ratio: Option<f64>,
    pub max_constraint_width: u32,
    pub height: Height,
}

impl Constraint {
    /// The no-op constraint (`Constraint.none`), also the defaults the generated
    /// table's `..Constraint::NONE` fills in for omitted fields.
    pub const NONE: Constraint = Constraint {
        size: Size::None,
        align_vertical: Align::None,
        align_horizontal: Align::None,
        pad_top: 0.0,
        pad_left: 0.0,
        pad_right: 0.0,
        pad_bottom: 0.0,
        relative_width: 1.0,
        relative_height: 1.0,
        relative_x: 0.0,
        relative_y: 0.0,
        max_xy_ratio: None,
        max_constraint_width: 2,
        height: Height::Cell,
    };

    /// The fixed emoji constraint upstream hardcodes in `SharedGrid.renderGlyph`
    /// (`.cover`, centered on both axes, 2.5% horizontal pad).
    pub const EMOJI: Constraint = Constraint {
        size: Size::Cover,
        align_vertical: Align::Center,
        align_horizontal: Align::Center,
        pad_left: 0.025,
        pad_right: 0.025,
        ..Constraint::NONE
    };

    /// True if the constraint sizes or positions the glyph at all
    /// (`doesAnything`, Glyph.zig:172-176).
    pub fn does_anything(&self) -> bool {
        self.size != Size::None
            || self.align_horizontal != Align::None
            || self.align_vertical != Align::None
    }

    /// Apply this constraint to `glyph` given the available `constraint_width`
    /// cells (`constrain`, Glyph.zig:180-214).
    pub fn constrain(
        &self,
        glyph: GlyphSize,
        metrics: &Metrics,
        constraint_width: u32,
    ) -> GlyphSize {
        if !self.does_anything() {
            return glyph;
        }

        match self.size {
            Size::Stretch => {
                // Stretched glyphs align to the grid, not the face: fib the
                // metrics so face_* == cell_* and face_y == 0.
                let mut m = *metrics;
                m.face_width = m.cell_width as f64;
                m.face_height = m.cell_height as f64;
                m.face_y = 0.0;

                // Clamp negative padding to 0 (grid-aligned stretch needs no
                // negative-pad band-aid).
                let mut c = *self;
                c.pad_bottom = c.pad_bottom.max(0.0);
                c.pad_top = c.pad_top.max(0.0);
                c.pad_left = c.pad_left.max(0.0);
                c.pad_right = c.pad_right.max(0.0);

                c.constrain_inner(glyph, &m, constraint_width)
            }
            _ => self.constrain_inner(glyph, metrics, constraint_width),
        }
    }

    fn constrain_inner(
        &self,
        glyph: GlyphSize,
        metrics: &Metrics,
        constraint_width: u32,
    ) -> GlyphSize {
        // Never stretch across two cells for extra-wide faces (mirrors
        // font_patcher).
        let min_constraint_width: u32 =
            if self.size == Size::Stretch && metrics.face_width > 0.9 * metrics.face_height {
                1
            } else {
                self.max_constraint_width.min(constraint_width)
            };

        // The scale-group bounding box (glyph relative to its group).
        let group_width = glyph.width / self.relative_width;
        let group_height = glyph.height / self.relative_height;
        let mut group = GlyphSize {
            width: group_width,
            height: group_height,
            x: glyph.x - (group_width * self.relative_x),
            y: glyph.y - (group_height * self.relative_y),
        };

        // Prescribed scaling, preserving the group center.
        let (width_factor, height_factor) =
            self.scale_factors(group, metrics, min_constraint_width);
        let center_x = group.x + group.width / 2.0;
        let center_y = group.y + group.height / 2.0;
        group.width *= width_factor;
        group.height *= height_factor;
        group.x = center_x - group.width / 2.0;
        group.y = center_y - group.height / 2.0;

        // Prescribed alignment.
        group.y = self.aligned_y(group, metrics);
        group.x = self.aligned_x(group, metrics, min_constraint_width);

        // Transfer scaling + alignment back to the glyph.
        GlyphSize {
            width: width_factor * glyph.width,
            height: height_factor * glyph.height,
            x: group.x + (group.width * self.relative_x),
            y: group.y + (group.height * self.relative_y),
        }
    }

    /// Width/height scale factors for the group (`scale_factors`,
    /// Glyph.zig:272-349).
    fn scale_factors(
        &self,
        group: GlyphSize,
        metrics: &Metrics,
        min_constraint_width: u32,
    ) -> (f64, f64) {
        if self.size == Size::None {
            return (1.0, 1.0);
        }

        let multi_cell = min_constraint_width > 1;

        let pad_width_factor = min_constraint_width as f64 - (self.pad_left + self.pad_right);
        let pad_height_factor = 1.0 - (self.pad_bottom + self.pad_top);

        let target_width = pad_width_factor * metrics.face_width;
        let target_height = pad_height_factor
            * match self.height {
                Height::Cell => metrics.face_height,
                Height::Icon => {
                    if multi_cell {
                        metrics.icon_height
                    } else {
                        metrics.icon_height_single
                    }
                }
            };

        let mut width_factor = target_width / group.width;
        let mut height_factor = target_height / group.height;

        match self.size {
            Size::None => unreachable!(),
            Size::Fit => {
                height_factor = 1.0_f64.min(width_factor).min(height_factor);
                width_factor = height_factor;
            }
            Size::Cover => {
                height_factor = width_factor.min(height_factor);
                width_factor = height_factor;
            }
            Size::FitCover1 => {
                height_factor = width_factor.min(height_factor);
                if multi_cell && height_factor > 1.0 {
                    // Recompute single-cell factors; use the height factor
                    // (width may be modified by max_xy_ratio).
                    let (_, single_height_factor) = self.scale_factors(group, metrics, 1);
                    height_factor = 1.0_f64.max(single_height_factor);
                }
                width_factor = height_factor;
            }
            Size::Stretch => {}
        }

        // Reduce aspect ratio if required.
        if let Some(ratio) = self.max_xy_ratio
            && group.width * width_factor > group.height * height_factor * ratio
        {
            width_factor = group.height * height_factor * ratio / group.width;
        }

        (width_factor, height_factor)
    }

    /// Vertical bearing for aligning the group (`aligned_y`, Glyph.zig:351-387).
    fn aligned_y(&self, group: GlyphSize, metrics: &Metrics) -> f64 {
        if self.size == Size::None && self.align_vertical == Align::None {
            return group.y;
        }
        let pad_bottom_dy = self.pad_bottom * metrics.face_height;
        let pad_top_dy = self.pad_top * metrics.face_height;
        let start_y = metrics.face_y + pad_bottom_dy;
        let end_y = metrics.face_y + (metrics.face_height - group.height - pad_top_dy);
        let center_y = (start_y + end_y) / 2.0;
        match self.align_vertical {
            Align::None => {
                if end_y < start_y {
                    center_y
                } else {
                    start_y.max(group.y.min(end_y))
                }
            }
            Align::Start => start_y,
            Align::End => end_y,
            Align::Center | Align::Center1 => center_y,
        }
    }

    /// Horizontal bearing for aligning the group (`aligned_x`,
    /// Glyph.zig:389-433).
    fn aligned_x(&self, group: GlyphSize, metrics: &Metrics, min_constraint_width: u32) -> f64 {
        if self.size == Size::None && self.align_horizontal == Align::None {
            return group.x;
        }
        let full_face_span =
            metrics.face_width + ((min_constraint_width - 1) * metrics.cell_width) as f64;
        let pad_left_dx = self.pad_left * metrics.face_width;
        let pad_right_dx = self.pad_right * metrics.face_width;
        let start_x = pad_left_dx;
        let end_x = full_face_span - group.width - pad_right_dx;
        match self.align_horizontal {
            Align::None => start_x.max(group.x.min(end_x)),
            Align::Start => start_x,
            Align::End => start_x.max(end_x),
            Align::Center => start_x.max((start_x + end_x) / 2.0),
            Align::Center1 => {
                let end1_x = metrics.face_width - group.width - pad_right_dx;
                start_x.max((start_x + end1_x) / 2.0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nerd_font_constraints::get_constraint;

    /// The exact grid metrics upstream's `Glyph.zig` "Constraints" test uses
    /// (CoreText at size 12, DPI 96; `font-family = JetBrains Mono`). Only the
    /// fields the constraint math reads are meaningful here.
    fn test_metrics() -> Metrics {
        Metrics {
            cell_width: 10,
            cell_height: 22,
            cell_baseline: 5,
            underline_position: 19,
            underline_thickness: 1,
            strikethrough_position: 12,
            strikethrough_thickness: 1,
            overline_position: 0,
            overline_thickness: 1,
            box_thickness: 1,
            cursor_thickness: 1,
            cursor_height: 22,
            icon_height: 21.12,
            icon_height_single: 44.48 / 3.0,
            face_width: 9.6,
            face_height: 21.12,
            face_y: 0.2,
        }
    }

    fn approx(a: GlyphSize, b: GlyphSize) {
        const EPS: f64 = 1e-6;
        assert!(
            (a.width - b.width).abs() < EPS
                && (a.height - b.height).abs() < EPS
                && (a.x - b.x).abs() < EPS
                && (a.y - b.y).abs() < EPS,
            "glyph sizes differ:\n  got {a:?}\n  want {b:?}"
        );
    }

    /// Upstream oracle: ASCII 'x' (no constraint) is unchanged at either width.
    #[test]
    fn ascii_no_constraint() {
        let m = test_metrics();
        let c = Constraint::NONE;
        let glyph_x = GlyphSize {
            width: 6.784,
            height: 15.28,
            x: 1.408,
            y: 4.84,
        };
        for cw in [1, 2] {
            approx(c.constrain(glyph_x, &m, cw), glyph_x);
        }
    }

    /// Upstream oracle: the `.fit` "symbol" constraint on '■' (0x25A0), a
    /// two-cell-designed glyph.
    #[test]
    fn symbol_fit() {
        let m = test_metrics();
        let c = Constraint {
            size: Size::Fit,
            ..Constraint::NONE
        };
        let glyph = GlyphSize {
            width: 10.272,
            height: 10.272,
            x: 2.864,
            y: 5.304,
        };
        // Width 1: scale down + shift to fit one cell.
        approx(
            c.constrain(glyph, &m, 1),
            GlyphSize {
                width: m.face_width,
                height: m.face_width,
                x: 0.0,
                y: 5.64,
            },
        );
        // Width 2: unchanged.
        approx(c.constrain(glyph, &m, 2), glyph);
    }

    /// Upstream oracle: the emoji constraint (`.cover` + center, 2.5% pad) on
    /// '🥸' (0x1F978) at width 2.
    #[test]
    fn emoji_cover_center() {
        let m = test_metrics();
        let glyph = GlyphSize {
            width: 20.0,
            height: 20.0,
            x: 0.46,
            y: 1.0,
        };
        approx(
            Constraint::EMOJI.constrain(glyph, &m, 2),
            GlyphSize {
                width: 18.72,
                height: 18.72,
                x: 0.44,
                y: 1.4,
            },
        );
    }

    /// Upstream oracle: the Nerd Fonts `.fit_cover1` table entry for 0xEA61
    /// (nf-cod-lightbulb), which is part of a scale group (`relative_*` != id).
    /// This validates the generated table entry AND the group/scale/align math.
    #[test]
    fn nerd_fit_cover1_ea61() {
        let m = test_metrics();
        let c = get_constraint(0xEA61).expect("0xEA61 has a constraint");
        assert_eq!(c.size, Size::FitCover1);
        assert_eq!(c.height, Height::Icon);
        assert_eq!(c.align_horizontal, Align::Center1);
        assert_eq!(c.align_vertical, Align::Center1);

        let glyph = GlyphSize {
            width: 9.015625,
            height: 13.015625,
            x: 3.015625,
            y: 3.76525,
        };
        // Width 1: scale + shift group to fit one cell.
        approx(
            c.constrain(glyph, &m, 1),
            GlyphSize {
                width: 7.2125,
                height: 10.4125,
                x: 0.8125,
                y: 5.950695224719102,
            },
        );
        // Width 2: no scaling; left-align + vertically center group.
        approx(
            c.constrain(glyph, &m, 2),
            GlyphSize {
                width: glyph.width,
                height: glyph.height,
                x: 1.015625,
                y: 4.7483690308988775,
            },
        );
    }

    /// Upstream oracle: the Nerd Fonts `.stretch` table entry for 0xE0C0
    /// (nf-ple-flame_thick) — stretches to exactly cover the cell span.
    #[test]
    fn nerd_stretch_e0c0() {
        let m = test_metrics();
        let c = get_constraint(0xE0C0).expect("0xE0C0 has a constraint");
        assert_eq!(c.size, Size::Stretch);
        assert_eq!(c.height, Height::Cell);
        assert_eq!(c.align_horizontal, Align::Start);
        assert_eq!(c.align_vertical, Align::Center1);

        let glyph = GlyphSize {
            width: 16.796875,
            height: 16.46875,
            x: -0.796875,
            y: 1.7109375,
        };
        // Width 1: stretch to exactly one cell.
        approx(
            c.constrain(glyph, &m, 1),
            GlyphSize {
                width: m.cell_width as f64,
                height: m.cell_height as f64,
                x: 0.0,
                y: 0.0,
            },
        );
        // Width 2: stretch to exactly two cells.
        approx(
            c.constrain(glyph, &m, 2),
            GlyphSize {
                width: (2 * m.cell_width) as f64,
                height: m.cell_height as f64,
                x: 0.0,
                y: 0.0,
            },
        );
    }
}
