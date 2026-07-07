//! Extraction of [`FaceMetrics`](crate::metrics::FaceMetrics) from a parsed
//! font face, via `ttf-parser`.
//!
//! Ports the *derivation logic* of Ghostty's CoreText backend
//! (`src/font/face/coretext.zig`'s `getMetrics`, commit `2da015cd6`) against
//! `ttf-parser`'s table access rather than CoreText's Objective-C bridge —
//! see `docs/analysis/font-foundations.md` for why `Face::ascender()` /
//! `descender()` / `line_gap()` are *not* used directly (ttf-parser bakes in
//! its own, subtly different fallback ladder) and for the bare-glyf finding
//! that lets the cell-width/ascii-height/ic-width *measurements* (steps with
//! no table equivalent, originally driven by CoreText glyph queries) be
//! reimplemented generically against `ttf-parser`'s glyph/advance/outline
//! API.

use crate::metrics::FaceMetrics;
use ttf_parser::Face;

/// Extract [`FaceMetrics`] from a parsed font face at a given pixel-per-em
/// size.
///
/// `px_per_em` plays the role CoreText's `ct_font.getSize()` plays in the
/// Zig source: the point size (in pixels) the metrics should be scaled to.
pub fn face_metrics(face: &Face, px_per_em: f64) -> FaceMetrics {
    let units_per_em = face.units_per_em() as f64;
    let px_per_unit = px_per_em / units_per_em;

    let (ascent, descent, line_gap) = vertical_metrics(face, px_per_unit);

    let (underline_position, underline_thickness) = underline_metrics(face, px_per_unit);
    let (strikethrough_position, strikethrough_thickness) =
        strikethrough_metrics(face, px_per_unit);

    let (cap_height, ex_height) = cap_ex_height(face, px_per_unit);

    let (cell_width, ascii_height) = ascii_measurements(face, px_per_unit);

    let ic_width = ic_width(face, px_per_unit, cell_width);

    FaceMetrics {
        px_per_em,
        cell_width,
        ascent,
        descent,
        line_gap,
        underline_position,
        underline_thickness,
        strikethrough_position,
        strikethrough_thickness,
        cap_height: Some(cap_height),
        ex_height: Some(ex_height),
        ascii_height: Some(ascii_height),
        ic_width,
    }
}

/// Replicates `coretext.zig::getMetrics`'s vertical-metrics fallback chain
/// (Metrics.zig `vertical_metrics: { ... }` block), reading the raw `hhea`
/// and `OS/2` tables rather than delegating to `ttf-parser`'s own
/// `Face::ascender`/`descender`/`line_gap` (which implements a different,
/// non-interchangeable ladder — see module docs).
fn vertical_metrics(face: &Face, px_per_unit: f64) -> (f64, f64, f64) {
    let hhea = face.tables().hhea;
    let hhea_ascent = hhea.ascender as f64;
    let hhea_descent = hhea.descender as f64;
    let hhea_line_gap = hhea.line_gap as f64;

    // If our font has no OS/2 table, blindly use the hhea table.
    let Some(os2) = face.tables().os2 else {
        return (
            hhea_ascent * px_per_unit,
            hhea_descent * px_per_unit,
            hhea_line_gap * px_per_unit,
        );
    };

    let os2_ascent = os2.typographic_ascender() as f64;
    let os2_descent = os2.typographic_descender() as f64;
    let os2_line_gap = os2.typographic_line_gap() as f64;

    // If the font says to use typo metrics, trust it.
    if os2.use_typographic_metrics() {
        return (
            os2_ascent * px_per_unit,
            os2_descent * px_per_unit,
            os2_line_gap * px_per_unit,
        );
    }

    // Otherwise we prefer the height metrics from `hhea` if they are
    // available, or else OS/2 `sTypo*` metrics, and if all else fails then
    // we use OS/2 `usWin*` metrics.
    if hhea.ascender != 0 || hhea.descender != 0 {
        return (
            hhea_ascent * px_per_unit,
            hhea_descent * px_per_unit,
            hhea_line_gap * px_per_unit,
        );
    }

    if os2_ascent != 0.0 || os2_descent != 0.0 {
        return (
            os2_ascent * px_per_unit,
            os2_descent * px_per_unit,
            os2_line_gap * px_per_unit,
        );
    }

    let win_ascent = os2.windows_ascender() as f64;
    let win_descent = os2.windows_descender() as f64;
    (
        win_ascent * px_per_unit,
        // usWinDescent is *positive*-down -> down unlike sTypoDescender and
        // hhea.descender, so we flip its sign to fix this.
        -win_descent * px_per_unit,
        0.0,
    )
}

/// Replicates the "broken underline" guard from `coretext.zig::getMetrics`:
/// some fonts have degenerate `post` tables where the underline thickness
/// (and often position) are 0; we treat both as absent in that case unless
/// the position is nonzero, in which case the position alone is still
/// trusted.
fn underline_metrics(face: &Face, px_per_unit: f64) -> (Option<f64>, Option<f64>) {
    let Some(metrics) = face.underline_metrics() else {
        return (None, None);
    };

    let has_broken_underline = metrics.thickness == 0;

    let position = if has_broken_underline && metrics.position == 0 {
        None
    } else {
        Some(metrics.position as f64 * px_per_unit)
    };

    let thickness = if has_broken_underline {
        None
    } else {
        Some(metrics.thickness as f64 * px_per_unit)
    };

    (position, thickness)
}

/// Same broken-value guard pattern as [`underline_metrics`], applied to
/// OS/2's strikeout metrics.
fn strikethrough_metrics(face: &Face, px_per_unit: f64) -> (Option<f64>, Option<f64>) {
    let Some(metrics) = face.strikeout_metrics() else {
        return (None, None);
    };

    let has_broken_strikethrough = metrics.thickness == 0;

    let position = if has_broken_strikethrough && metrics.position == 0 {
        None
    } else {
        Some(metrics.position as f64 * px_per_unit)
    };

    let thickness = if has_broken_strikethrough {
        None
    } else {
        Some(metrics.thickness as f64 * px_per_unit)
    };

    (position, thickness)
}

/// Cap/ex height from OS/2 if present and nonzero after scaling, else
/// measured from glyph bounding boxes (capital H / lowercase x), replacing
/// CoreText's `getCapHeight()`/`getXHeight()` fallback with a portable
/// measurement against `ttf-parser`'s outline API (see module docs: this is
/// the one piece of `getMetrics` with no direct table equivalent).
fn cap_ex_height(face: &Face, px_per_unit: f64) -> (f64, f64) {
    let units_per_em = face.units_per_em() as f64;
    let px_per_em = px_per_unit * units_per_em;

    let os2_cap = face
        .capital_height()
        .map(|v| v as f64 * px_per_unit)
        .filter(|&v| v > 0.0);
    let os2_ex = face
        .x_height()
        .map(|v| v as f64 * px_per_unit)
        .filter(|&v| v > 0.0);

    let cap_height =
        os2_cap.unwrap_or_else(|| measure_glyph_height(face, 'H').unwrap_or(0.0) * px_per_em);
    let ex_height =
        os2_ex.unwrap_or_else(|| measure_glyph_height(face, 'x').unwrap_or(0.0) * px_per_em);

    (cap_height, ex_height)
}

/// Measure a single glyph's bounding-box height, in font design units
/// normalized to ems (i.e. divided by `units_per_em`), or `None` if the
/// character has no glyph or no outline (e.g. a bitmap-only glyph).
///
/// Callers scale the result by the caller's own `px_per_em` (not
/// `px_per_unit`, since this returns em-normalized units, not design units).
fn measure_glyph_height(face: &Face, c: char) -> Option<f64> {
    let units_per_em = face.units_per_em() as f64;
    let glyph_id = face.glyph_index(c)?;
    let bbox = face.glyph_bounding_box(glyph_id)?;
    let height_units = (bbox.y_max - bbox.y_min) as f64;
    Some(height_units / units_per_em)
}

/// Cell width is the widest advance among printable ASCII glyphs (0x20..=0x7E);
/// ASCII height is the height of the overall bounding box of the same
/// glyphs. Replicates `coretext.zig::getMetrics`'s `measurements:` block
/// using `ttf-parser`'s glyph index/advance/bbox API instead of CoreText's
/// glyph-array queries.
fn ascii_measurements(face: &Face, px_per_unit: f64) -> (f64, f64) {
    let mut max_advance: f64 = 0.0;
    let mut min_y: Option<i16> = None;
    let mut max_y: Option<i16> = None;

    for c in (0x20u32..=0x7E).filter_map(char::from_u32) {
        let Some(glyph_id) = face.glyph_index(c) else {
            continue;
        };

        if let Some(advance) = face.glyph_hor_advance(glyph_id) {
            max_advance = max_advance.max(advance as f64);
        }

        if let Some(bbox) = face.glyph_bounding_box(glyph_id) {
            min_y = Some(min_y.map_or(bbox.y_min, |v| v.min(bbox.y_min)));
            max_y = Some(max_y.map_or(bbox.y_max, |v| v.max(bbox.y_max)));
        }
    }

    let cell_width = max_advance * px_per_unit;
    let ascii_height = match (min_y, max_y) {
        (Some(min_y), Some(max_y)) => (max_y - min_y) as f64 * px_per_unit,
        _ => 0.0,
    };

    (cell_width, ascii_height)
}

/// Measure the advance and bounding box of U+6C34 (CJK water ideograph),
/// discarding the measurement if the glyph's bbox width exceeds its advance
/// (a patched-font corruption guard, ported verbatim from
/// `coretext.zig::getMetrics`'s `ic_width:` block).
fn ic_width(face: &Face, px_per_unit: f64, _cell_width: f64) -> Option<f64> {
    let glyph_id = face.glyph_index('水')?;
    let advance = face.glyph_hor_advance(glyph_id)? as f64;
    let bbox = face.glyph_bounding_box(glyph_id)?;
    let width = (bbox.x_max - bbox.x_min) as f64;

    if width > advance {
        return None;
    }

    Some(advance * px_per_unit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::JETBRAINS_MONO_VARIABLE;

    #[test]
    fn extracts_plausible_metrics() {
        let face = Face::parse(JETBRAINS_MONO_VARIABLE, 0).unwrap();
        let metrics = face_metrics(&face, 16.0);

        assert!(metrics.cell_width > 0.0);
        assert!(metrics.ascent > 0.0);
        assert!(metrics.descent < 0.0);
        assert!(metrics.line_gap >= 0.0);
    }
}
