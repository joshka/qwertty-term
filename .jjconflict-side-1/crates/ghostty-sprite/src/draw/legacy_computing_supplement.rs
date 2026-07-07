//! Symbols for Legacy Computing Supplement | U+1CC00..U+1CEBF (implemented subset)
//!
//! Ported from `src/font/sprite/draw/symbols_for_legacy_computing_supplement.zig`.
//! Contains the octant lookup table (parsed from the embedded `octants.txt`),
//! separated block quadrants/sextants, the sixteenth-block grid, box-drawing
//! combinations, and [`circle_piece`] — the sub-rectangle ellipse-arc routine
//! used to build twelfth/quarter circles and half-ellipses.

use std::sync::LazyLock;

use tiny_skia::LineCap;

use crate::common::{Corner, Fraction, Shade, fill};
use crate::draw::box_drawing;
use crate::draw::legacy_computing::circle;
use crate::{Canvas, Metrics};

const OCTANT_MIN: u32 = 0x1cd00;
const OCTANT_MAX: u32 = 0x1cde5;

/// One octant entry: which of the 8 vertical eighths are filled.
#[derive(Clone, Copy, Default)]
struct Octant {
    bits: u8,
}

/// The octant table, parsed once from the embedded `octants.txt`. Each line is
/// `BLOCK OCTANT-<digits>`, where the digits (1..=8) index vertical eighths
/// (1,2 = top row halves; ... 7,8 = bottom row halves). The order in the file
/// *is* the codepoint order, so index = `cp - OCTANT_MIN`.
static OCTANTS: LazyLock<Vec<Octant>> = LazyLock::new(|| {
    let data = include_str!("octants.txt");
    let mut result = Vec::new();
    for raw in data.lines() {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let idx = line.find('-').expect("octant line has a '-'");
        let mut oct = Octant::default();
        for c in line[idx + 1..].chars() {
            let d = c.to_digit(10).expect("octant digit") as u8;
            debug_assert!((1..=8).contains(&d));
            oct.bits |= 1 << (d - 1);
        }
        result.push(oct);
    }
    debug_assert_eq!(result.len() as u32, OCTANT_MAX - OCTANT_MIN + 1);
    result
});

/// Octants (U+1CD00..U+1CDE5).
pub(crate) fn draw1cd00_1cde5(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    use Fraction as F;
    let oct = OCTANTS[(cp - OCTANT_MIN) as usize];
    let b = oct.bits;
    if b & 0b0000_0001 != 0 {
        fill(m, canvas, F::Zero, F::Half, F::Zero, F::OneQuarter);
    }
    if b & 0b0000_0010 != 0 {
        fill(m, canvas, F::Half, F::One, F::Zero, F::OneQuarter);
    }
    if b & 0b0000_0100 != 0 {
        fill(m, canvas, F::Zero, F::Half, F::OneQuarter, F::Half);
    }
    if b & 0b0000_1000 != 0 {
        fill(m, canvas, F::Half, F::One, F::OneQuarter, F::Half);
    }
    if b & 0b0001_0000 != 0 {
        fill(m, canvas, F::Zero, F::Half, F::Half, F::ThreeQuarters);
    }
    if b & 0b0010_0000 != 0 {
        fill(m, canvas, F::Half, F::One, F::Half, F::ThreeQuarters);
    }
    if b & 0b0100_0000 != 0 {
        fill(m, canvas, F::Zero, F::Half, F::ThreeQuarters, F::One);
    }
    if b & 0b1000_0000 != 0 {
        fill(m, canvas, F::Half, F::One, F::ThreeQuarters, F::One);
    }
}

/// Separated Block Quadrants (U+1CC21..U+1CC2F).
pub(crate) fn draw1cc21_1cc2f(cp: u32, canvas: &mut Canvas, width: u32, height: u32, _m: &Metrics) {
    let bits = ((cp - 0x1cc20) & 0xf) as u8;
    let tl = bits & 0b0001 != 0;
    let tr = bits & 0b0010 != 0;
    let bl = bits & 0b0100 != 0;
    let br = bits & 0b1000 != 0;

    let gap = (width / 12).max(1) as i32;
    let mid_gap_x = gap * 2 + (width % 2) as i32;
    let mid_gap_y = gap * 2 + (height % 2) as i32;
    let w = (width as i32 - gap * 2 - mid_gap_x) / 2;
    let h = (height as i32 - gap * 2 - mid_gap_y) / 2;
    let on = Shade::On as u8;

    if tl {
        canvas.box_fill(gap, gap, gap + w, gap + h, on);
    }
    if tr {
        canvas.box_fill(
            gap + w + mid_gap_x,
            gap,
            gap + w + mid_gap_x + w,
            gap + h,
            on,
        );
    }
    if bl {
        canvas.box_fill(
            gap,
            gap + h + mid_gap_y,
            gap + w,
            gap + h + mid_gap_y + h,
            on,
        );
    }
    if br {
        canvas.box_fill(
            gap + w + mid_gap_x,
            gap + h + mid_gap_y,
            gap + w + mid_gap_x + w,
            gap + h + mid_gap_y + h,
            on,
        );
    }
}

/// Twelfth and quarter circle pieces (U+1CC30..U+1CC3F). These are ellipse arcs
/// sized to touch the edge of an enclosing set of cells.
pub(crate) fn draw1cc30_1cc3f(cp: u32, canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let p = |canvas: &mut Canvas, x, y, w, h, corner| {
        circle_piece(canvas, width, height, m, x, y, w, h, corner);
    };
    match cp {
        0x1cc30 => p(canvas, 0.0, 0.0, 2.0, 2.0, Corner::Tl),
        0x1cc31 => p(canvas, 1.0, 0.0, 2.0, 2.0, Corner::Tl),
        0x1cc32 => p(canvas, 2.0, 0.0, 2.0, 2.0, Corner::Tr),
        0x1cc33 => p(canvas, 3.0, 0.0, 2.0, 2.0, Corner::Tr),
        0x1cc34 => p(canvas, 0.0, 1.0, 2.0, 2.0, Corner::Tl),
        0x1cc35 => p(canvas, 0.0, 0.0, 1.0, 1.0, Corner::Tl),
        0x1cc36 => p(canvas, 1.0, 0.0, 1.0, 1.0, Corner::Tr),
        0x1cc37 => p(canvas, 3.0, 1.0, 2.0, 2.0, Corner::Tr),
        0x1cc38 => p(canvas, 0.0, 2.0, 2.0, 2.0, Corner::Bl),
        0x1cc39 => p(canvas, 0.0, 1.0, 1.0, 1.0, Corner::Bl),
        0x1cc3a => p(canvas, 1.0, 1.0, 1.0, 1.0, Corner::Br),
        0x1cc3b => p(canvas, 3.0, 2.0, 2.0, 2.0, Corner::Br),
        0x1cc3c => p(canvas, 0.0, 3.0, 2.0, 2.0, Corner::Bl),
        0x1cc3d => p(canvas, 1.0, 3.0, 2.0, 2.0, Corner::Bl),
        0x1cc3e => p(canvas, 2.0, 3.0, 2.0, 2.0, Corner::Br),
        0x1cc3f => p(canvas, 3.0, 3.0, 2.0, 2.0, Corner::Br),
        _ => {}
    }
}

/// Box drawings light horizontal/vertical with a corner stub (U+1CC1B..U+1CC1E).
pub(crate) fn draw1cc1b_1cc1e(cp: u32, canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let w = width as i32;
    let h = height as i32;
    let t = m.box_thickness as i32;
    let on = Shade::On as u8;
    match cp {
        0x1cc1b => {
            box_drawing::lines_char(
                m,
                canvas,
                box_drawing::Lines {
                    left: box_drawing::LineStyle::Light,
                    right: box_drawing::LineStyle::Light,
                    ..box_drawing::Lines::default()
                },
            );
            canvas.box_fill(w - t, 0, w, h / 2, on);
        }
        0x1cc1c => {
            box_drawing::lines_char(
                m,
                canvas,
                box_drawing::Lines {
                    left: box_drawing::LineStyle::Light,
                    right: box_drawing::LineStyle::Light,
                    ..box_drawing::Lines::default()
                },
            );
            canvas.box_fill(w - t, h / 2, w, h, on);
        }
        0x1cc1d => {
            canvas.box_fill(0, 0, w, t, on);
            canvas.box_fill(0, 0, t, h / 2, on);
        }
        0x1cc1e => {
            canvas.box_fill(0, h - t, w, h, on);
            canvas.box_fill(0, h / 2, t, h, on);
        }
        _ => {}
    }
}

/// 𜸀 right half + left half white circle (U+1CE00).
pub(crate) fn draw1ce00(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    use crate::common::Alignment as A;
    circle(m, canvas, A::LEFT, false);
    circle(m, canvas, A::RIGHT, false);
}

/// 𜸁 lower half + upper half white circle (U+1CE01).
pub(crate) fn draw1ce01(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    use crate::common::Alignment as A;
    circle(m, canvas, A::UPPER, false);
    circle(m, canvas, A::LOWER, false);
}

/// 𜸋 left half white ellipse (U+1CE0B).
pub(crate) fn draw1ce0b(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    circle_piece(canvas, width, height, m, 0.0, 0.0, 1.0, 0.5, Corner::Tl);
    circle_piece(canvas, width, height, m, 0.0, 0.0, 1.0, 0.5, Corner::Bl);
}

/// 𜸌 right half white ellipse (U+1CE0C).
pub(crate) fn draw1ce0c(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    circle_piece(canvas, width, height, m, 1.0, 0.0, 1.0, 0.5, Corner::Tr);
    circle_piece(canvas, width, height, m, 1.0, 0.0, 1.0, 0.5, Corner::Br);
}

/// Box drawings light vertical with a corner stub (U+1CE16..U+1CE19).
pub(crate) fn draw1ce16_1ce19(cp: u32, canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let w = width as i32;
    let h = height as i32;
    let t = m.box_thickness as i32;
    let on = Shade::On as u8;
    let vert = |m: &Metrics, c: &mut Canvas| {
        box_drawing::lines_char(
            m,
            c,
            box_drawing::Lines {
                up: box_drawing::LineStyle::Light,
                down: box_drawing::LineStyle::Light,
                ..box_drawing::Lines::default()
            },
        );
    };
    match cp {
        0x1ce16 => {
            vert(m, canvas);
            canvas.box_fill(w / 2, 0, w, t, on);
        }
        0x1ce17 => {
            vert(m, canvas);
            canvas.box_fill(w / 2, h - t, w, h, on);
        }
        0x1ce18 => {
            vert(m, canvas);
            canvas.box_fill(0, 0, w / 2, t, on);
        }
        0x1ce19 => {
            vert(m, canvas);
            canvas.box_fill(0, h - t, w / 2, h, on);
        }
        _ => {}
    }
}

/// Separated Block Sextants (U+1CE51..U+1CE8F).
pub(crate) fn draw1ce51_1ce8f(cp: u32, canvas: &mut Canvas, width: u32, height: u32, _m: &Metrics) {
    let bits = ((cp - 0x1ce50) & 0x3f) as u8;
    let tl = bits & 0b00_0001 != 0;
    let tr = bits & 0b00_0010 != 0;
    let ml = bits & 0b00_0100 != 0;
    let mr = bits & 0b00_1000 != 0;
    let bl = bits & 0b01_0000 != 0;
    let br = bits & 0b10_0000 != 0;

    let gap = (width / 12).max(1) as i32;
    let mid_gap_x = gap * 2 + (width % 2) as i32;
    let y_extra = (height % 3) as i32;
    let mid_gap_y = gap * 2 + y_extra / 2;
    let w = (width as i32 - gap * 2 - mid_gap_x) / 2;
    let h = (height as i32 - gap * 2 - mid_gap_y * 2) / 3;
    let h_m = height as i32 - gap * 2 - mid_gap_y * 2 - h * 2;
    let on = Shade::On as u8;

    if tl {
        canvas.box_fill(gap, gap, gap + w, gap + h, on);
    }
    if tr {
        canvas.box_fill(
            gap + w + mid_gap_x,
            gap,
            gap + w + mid_gap_x + w,
            gap + h,
            on,
        );
    }
    if ml {
        canvas.box_fill(
            gap,
            gap + h + mid_gap_y,
            gap + w,
            gap + h + mid_gap_y + h_m,
            on,
        );
    }
    if mr {
        canvas.box_fill(
            gap + w + mid_gap_x,
            gap + h + mid_gap_y,
            gap + w + mid_gap_x + w,
            gap + h + mid_gap_y + h_m,
            on,
        );
    }
    if bl {
        canvas.box_fill(
            gap,
            gap + h + mid_gap_y + h_m + mid_gap_y,
            gap + w,
            gap + h + mid_gap_y + h_m + mid_gap_y + h,
            on,
        );
    }
    if br {
        canvas.box_fill(
            gap + w + mid_gap_x,
            gap + h + mid_gap_y + h_m + mid_gap_y,
            gap + w + mid_gap_x + w,
            gap + h + mid_gap_y + h_m + mid_gap_y + h,
            on,
        );
    }
}

/// Sixteenth blocks (U+1CE90..U+1CEAF).
pub(crate) fn draw1ce90_1ceaf(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    let q = Fraction::quarters();
    let f = |c: &mut Canvas, a: usize, b: usize, cc: usize, d: usize| {
        fill(m, c, q[a], q[b], q[cc], q[d]);
    };
    match cp {
        0x1ce90 => f(canvas, 0, 1, 0, 1),
        0x1ce91 => f(canvas, 1, 2, 0, 1),
        0x1ce92 => f(canvas, 2, 3, 0, 1),
        0x1ce93 => f(canvas, 3, 4, 0, 1),
        0x1ce94 => f(canvas, 0, 1, 1, 2),
        0x1ce95 => f(canvas, 1, 2, 1, 2),
        0x1ce96 => f(canvas, 2, 3, 1, 2),
        0x1ce97 => f(canvas, 3, 4, 1, 2),
        0x1ce98 => f(canvas, 0, 1, 2, 3),
        0x1ce99 => f(canvas, 1, 2, 2, 3),
        0x1ce9a => f(canvas, 2, 3, 2, 3),
        0x1ce9b => f(canvas, 3, 4, 2, 3),
        0x1ce9c => f(canvas, 0, 1, 3, 4),
        0x1ce9d => f(canvas, 1, 2, 3, 4),
        0x1ce9e => f(canvas, 2, 3, 3, 4),
        0x1ce9f => f(canvas, 3, 4, 3, 4),
        0x1cea0 => f(canvas, 2, 4, 3, 4),
        0x1cea1 => f(canvas, 1, 4, 3, 4),
        0x1cea2 => f(canvas, 0, 3, 3, 4),
        0x1cea3 => f(canvas, 0, 2, 3, 4),
        0x1cea4 => f(canvas, 0, 1, 2, 4),
        0x1cea5 => f(canvas, 0, 1, 1, 4),
        0x1cea6 => f(canvas, 0, 1, 0, 3),
        0x1cea7 => f(canvas, 0, 1, 0, 2),
        0x1cea8 => f(canvas, 0, 2, 0, 1),
        0x1cea9 => f(canvas, 0, 3, 0, 1),
        0x1ceaa => f(canvas, 1, 4, 0, 1),
        0x1ceab => f(canvas, 2, 4, 0, 1),
        0x1ceac => f(canvas, 3, 4, 0, 2),
        0x1cead => f(canvas, 3, 4, 0, 3),
        0x1ceae => f(canvas, 3, 4, 1, 4),
        0x1ceaf => f(canvas, 3, 4, 2, 4),
        _ => {}
    }
}

/// Ellipse-arc sub-rectangle stroke used by circle/ellipse pieces.
///
/// `(x, y)` is the cell offset (in cells) of this piece's sub-rectangle within
/// its enclosing arc, `(w, h)` its size in cells; `corner` selects which corner
/// of the (2*w x 2*h) ellipse this arc belongs to. A single cubic Bézier
/// approximates the quarter-ellipse, offset by `(xp, yp)` so only the visible
/// slice lands in this cell.
#[allow(clippy::too_many_arguments)]
fn circle_piece(
    canvas: &mut Canvas,
    width: u32,
    height: u32,
    m: &Metrics,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    corner: Corner,
) {
    let wdth = f64::from(width) * w;
    let hght = f64::from(height) * h;
    let xp = f64::from(width) * x;
    let yp = f64::from(height) * y;

    canvas.clip_to_cell();

    let c = (std::f64::consts::SQRT_2 - 1.0) * 4.0 / 3.0;
    let cw = c * wdth;
    let ch = c * hght;
    let thick = f64::from(m.box_thickness);
    let ht = thick * 0.5;

    let mut pb = canvas.path();
    match corner {
        Corner::Tl => {
            pb.move_to((wdth - xp) as f32, (ht - yp) as f32);
            pb.cubic_to(
                (wdth - cw - xp) as f32,
                (ht - yp) as f32,
                (ht - xp) as f32,
                (hght - ch - yp) as f32,
                (ht - xp) as f32,
                (hght - yp) as f32,
            );
        }
        Corner::Tr => {
            pb.move_to((wdth - xp) as f32, (ht - yp) as f32);
            pb.cubic_to(
                (wdth + cw - xp) as f32,
                (ht - yp) as f32,
                (wdth * 2.0 - ht - xp) as f32,
                (hght - ch - yp) as f32,
                (wdth * 2.0 - ht - xp) as f32,
                (hght - yp) as f32,
            );
        }
        Corner::Bl => {
            pb.move_to((ht - xp) as f32, (hght - yp) as f32);
            pb.cubic_to(
                (ht - xp) as f32,
                (hght + ch - yp) as f32,
                (wdth - cw - xp) as f32,
                (hght * 2.0 - ht - yp) as f32,
                (wdth - xp) as f32,
                (hght * 2.0 - ht - yp) as f32,
            );
        }
        Corner::Br => {
            pb.move_to((wdth * 2.0 - ht - xp) as f32, (hght - yp) as f32);
            pb.cubic_to(
                (wdth * 2.0 - ht - xp) as f32,
                (hght + ch - yp) as f32,
                (wdth + cw - xp) as f32,
                (hght * 2.0 - ht - yp) as f32,
                (wdth - xp) as f32,
                (hght * 2.0 - ht - yp) as f32,
            );
        }
    }
    canvas.stroke_path(
        pb,
        f64::from(m.box_thickness),
        LineCap::Butt,
        Shade::On as u8,
    );
}
