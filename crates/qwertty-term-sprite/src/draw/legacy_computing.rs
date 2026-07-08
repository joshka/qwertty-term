//! Symbols for Legacy Computing | U+1FB00..U+1FBEF
//!
//! Ported from `src/font/sprite/draw/symbols_for_legacy_computing.zig`. This
//! block is where the data-driven pieces live: the sextant bit trick, the
//! 44-entry [`SmoothMosaic`] polygon lookup table (transcribed exactly), and a
//! grab-bag of block/diagonal/circle glyphs delegating to the other modules.

use crate::canvas::{Line, Point};
use crate::common::{Alignment, Corner, Edge, Fraction, Quads, Shade, Thickness, fill};
use crate::draw::{block, box_drawing, geometric_shapes};
use crate::{Canvas, Metrics};

const ONE_EIGHTH: f64 = 0.125;
const ONE_QUARTER: f64 = 0.25;
const ONE_THIRD: f64 = 1.0 / 3.0;
const THREE_EIGHTHS: f64 = 0.375;
const HALF: f64 = 0.5;
const FIVE_EIGHTHS: f64 = 0.625;
const TWO_THIRDS: f64 = 2.0 / 3.0;
const THREE_QUARTERS: f64 = 0.75;
const SEVEN_EIGHTHS: f64 = 0.875;

/// The ten anchor points of a smooth mosaic polygon (matches the packed struct
/// in the Zig source).
#[derive(Debug, Clone, Copy, Default)]
struct SmoothMosaic {
    tl: bool,
    ul: bool,
    ll: bool,
    bl: bool,
    bc: bool,
    br: bool,
    lr: bool,
    ur: bool,
    tr: bool,
    tc: bool,
}

impl SmoothMosaic {
    /// Parse a 3-wide, 4-tall ASCII pattern into the ten anchor flags, using
    /// the exact index/adjacency rules from the Zig `from`. `pattern` is the
    /// four rows concatenated with newlines stripped conceptually — we index
    /// by the flat positions the Zig code uses (row-major, 4-char stride minus
    /// the newline: positions 0,1,2 / 4,5,6 / 8,9,10 / 12,13,14).
    fn from(rows: [&str; 4]) -> SmoothMosaic {
        let b = rows.as_flat();
        let hash = |i: usize| b[i] == b'#';
        SmoothMosaic {
            tl: hash(0),
            ul: b[4] == b'#' && (b[0] != b'#' || b[8] != b'#'),
            ll: b[8] == b'#' && (b[4] != b'#' || b[12] != b'#'),
            bl: hash(12),
            bc: b[13] == b'#' && (b[12] != b'#' || b[14] != b'#'),
            br: hash(14),
            lr: b[10] == b'#' && (b[14] != b'#' || b[6] != b'#'),
            ur: b[6] == b'#' && (b[10] != b'#' || b[2] != b'#'),
            tr: hash(2),
            tc: b[1] == b'#' && (b[2] != b'#' || b[0] != b'#'),
        }
    }
}

/// Helper trait to flatten the 4 pattern rows into a 15-byte grid indexable the
/// way the Zig multiline-string literal was.
trait FlatPattern {
    fn as_flat(&self) -> [u8; 15];
}

impl FlatPattern for [&str; 4] {
    fn as_flat(&self) -> [u8; 15] {
        // The Zig literal is a 4x3 grid joined by newlines; flat index = row*4
        // + col (col in 0..3, index 3 is the newline slot, unused).
        let mut out = [b' '; 15];
        for (row, s) in self.iter().enumerate() {
            let bytes = s.as_bytes();
            for col in 0..3 {
                out[row * 4 + col] = bytes.get(col).copied().unwrap_or(b' ');
            }
        }
        out
    }
}

/// Sextants (U+1FB00..U+1FB3B).
pub(crate) fn draw1fb00_1fb3b(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    use Fraction as F;
    let idx = cp - 0x1fb00;
    // Sextant bit pattern with the two "full" codepoints skipped (hence the
    // idx/0x14 + 1 offset).
    let bits = (idx + (idx / 0x14) + 1) as u8;
    let tl = bits & 0b00_0001 != 0;
    let tr = bits & 0b00_0010 != 0;
    let ml = bits & 0b00_0100 != 0;
    let mr = bits & 0b00_1000 != 0;
    let bl = bits & 0b01_0000 != 0;
    let br = bits & 0b10_0000 != 0;
    if tl {
        fill(m, canvas, F::Zero, F::Half, F::Zero, F::OneThird);
    }
    if tr {
        fill(m, canvas, F::Half, F::One, F::Zero, F::OneThird);
    }
    if ml {
        fill(m, canvas, F::Zero, F::Half, F::OneThird, F::TwoThirds);
    }
    if mr {
        fill(m, canvas, F::Half, F::One, F::OneThird, F::TwoThirds);
    }
    if bl {
        fill(m, canvas, F::Zero, F::Half, F::TwoThirds, F::One);
    }
    if br {
        fill(m, canvas, F::Half, F::One, F::TwoThirds, F::One);
    }
}

/// Smooth Mosaics (U+1FB3C..U+1FB67).
pub(crate) fn draw1fb3c_1fb67(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    let mosaic = match cp {
        0x1fb3c => SmoothMosaic::from(["...", "...", "#..", "##."]),
        0x1fb3d => SmoothMosaic::from(["...", "...", "#\\.", "###"]),
        0x1fb3e => SmoothMosaic::from(["...", "#..", "#\\.", "##."]),
        0x1fb3f => SmoothMosaic::from(["...", "#..", "##.", "###"]),
        0x1fb40 => SmoothMosaic::from(["#..", "#..", "##.", "##."]),
        0x1fb41 => SmoothMosaic::from(["/##", "###", "###", "###"]),
        0x1fb42 => SmoothMosaic::from(["./#", "###", "###", "###"]),
        0x1fb43 => SmoothMosaic::from([".##", ".##", "###", "###"]),
        0x1fb44 => SmoothMosaic::from(["..#", ".##", "###", "###"]),
        0x1fb45 => SmoothMosaic::from([".##", ".##", ".##", "###"]),
        0x1fb46 => SmoothMosaic::from(["...", "./#", "###", "###"]),
        0x1fb47 => SmoothMosaic::from(["...", "...", "..#", ".##"]),
        0x1fb48 => SmoothMosaic::from(["...", "...", "./#", "###"]),
        0x1fb49 => SmoothMosaic::from(["...", "..#", "./#", ".##"]),
        0x1fb4a => SmoothMosaic::from(["...", "..#", ".##", "###"]),
        0x1fb4b => SmoothMosaic::from(["..#", "..#", ".##", ".##"]),
        0x1fb4c => SmoothMosaic::from(["##\\", "###", "###", "###"]),
        0x1fb4d => SmoothMosaic::from(["#\\.", "###", "###", "###"]),
        0x1fb4e => SmoothMosaic::from(["##.", "##.", "###", "###"]),
        0x1fb4f => SmoothMosaic::from(["#..", "##.", "###", "###"]),
        0x1fb50 => SmoothMosaic::from(["##.", "##.", "##.", "###"]),
        0x1fb51 => SmoothMosaic::from(["...", "#\\.", "###", "###"]),
        0x1fb52 => SmoothMosaic::from(["###", "###", "###", "\\##"]),
        0x1fb53 => SmoothMosaic::from(["###", "###", "###", ".\\#"]),
        0x1fb54 => SmoothMosaic::from(["###", "###", ".##", ".##"]),
        0x1fb55 => SmoothMosaic::from(["###", "###", ".##", "..#"]),
        0x1fb56 => SmoothMosaic::from(["###", ".##", ".##", ".##"]),
        0x1fb57 => SmoothMosaic::from(["##.", "#..", "...", "..."]),
        0x1fb58 => SmoothMosaic::from(["###", "#/.", "...", "..."]),
        0x1fb59 => SmoothMosaic::from(["##.", "#/.", "#..", "..."]),
        0x1fb5a => SmoothMosaic::from(["###", "##.", "#..", "..."]),
        0x1fb5b => SmoothMosaic::from(["##.", "##.", "#..", "#.."]),
        0x1fb5c => SmoothMosaic::from(["###", "###", "#/.", "..."]),
        0x1fb5d => SmoothMosaic::from(["###", "###", "###", "##/"]),
        0x1fb5e => SmoothMosaic::from(["###", "###", "###", "#/."]),
        0x1fb5f => SmoothMosaic::from(["###", "###", "##.", "##."]),
        0x1fb60 => SmoothMosaic::from(["###", "###", "##.", "#.."]),
        0x1fb61 => SmoothMosaic::from(["###", "##.", "##.", "##."]),
        0x1fb62 => SmoothMosaic::from([".##", "..#", "...", "..."]),
        0x1fb63 => SmoothMosaic::from(["###", ".\\#", "...", "..."]),
        0x1fb64 => SmoothMosaic::from([".##", ".\\#", "..#", "..."]),
        0x1fb65 => SmoothMosaic::from(["###", ".##", "..#", "..."]),
        0x1fb66 => SmoothMosaic::from([".##", ".##", "..#", "..#"]),
        0x1fb67 => SmoothMosaic::from(["###", "###", ".\\#", "..."]),
        _ => return,
    };

    let top = 0.0;
    let upper = Fraction::OneThird.float(m.cell_height);
    let lower = Fraction::TwoThirds.float(m.cell_height);
    let bottom = f64::from(m.cell_height);
    let left = 0.0;
    let center = Fraction::Half.float(m.cell_width);
    let right = f64::from(m.cell_width);

    let mut pb = canvas.path();
    let mut started = false;
    let line = |pb: &mut tiny_skia::PathBuilder, started: &mut bool, x: f64, y: f64| {
        if *started {
            pb.line_to(x as f32, y as f32);
        } else {
            pb.move_to(x as f32, y as f32);
            *started = true;
        }
    };
    if mosaic.tl {
        line(&mut pb, &mut started, left, top);
    }
    if mosaic.ul {
        line(&mut pb, &mut started, left, upper);
    }
    if mosaic.ll {
        line(&mut pb, &mut started, left, lower);
    }
    if mosaic.bl {
        line(&mut pb, &mut started, left, bottom);
    }
    if mosaic.bc {
        line(&mut pb, &mut started, center, bottom);
    }
    if mosaic.br {
        line(&mut pb, &mut started, right, bottom);
    }
    if mosaic.lr {
        line(&mut pb, &mut started, right, lower);
    }
    if mosaic.ur {
        line(&mut pb, &mut started, right, upper);
    }
    if mosaic.tr {
        line(&mut pb, &mut started, right, top);
    }
    if mosaic.tc {
        line(&mut pb, &mut started, center, top);
    }
    pb.close();
    canvas.fill_path(pb, Shade::On as u8);
}

/// Edge triangles, some inverted (U+1FB68..U+1FB6F).
pub(crate) fn draw1fb68_1fb6f(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    let inverted = |canvas: &mut Canvas, edge: Edge| {
        edge_triangle(m, canvas, edge);
        canvas.invert();
        canvas.clip_to_cell();
    };
    match cp {
        0x1fb68 => inverted(canvas, Edge::Left),
        0x1fb69 => inverted(canvas, Edge::Top),
        0x1fb6a => inverted(canvas, Edge::Right),
        0x1fb6b => inverted(canvas, Edge::Bottom),
        0x1fb6c => edge_triangle(m, canvas, Edge::Left),
        0x1fb6d => edge_triangle(m, canvas, Edge::Top),
        0x1fb6e => edge_triangle(m, canvas, Edge::Right),
        0x1fb6f => edge_triangle(m, canvas, Edge::Bottom),
        _ => {}
    }
}

/// Vertical one-eighth blocks (U+1FB70..U+1FB75).
pub(crate) fn draw1fb70_1fb75(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    let n = (cp + 1 - 0x1fb70) as usize;
    let e = Fraction::eighths();
    fill(m, canvas, e[n], e[n + 1], Fraction::Zero, Fraction::One);
}

/// Horizontal one-eighth blocks (U+1FB76..U+1FB7B).
pub(crate) fn draw1fb76_1fb7b(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    let n = (cp + 1 - 0x1fb76) as usize;
    let e = Fraction::eighths();
    fill(m, canvas, Fraction::Zero, Fraction::One, e[n], e[n + 1]);
}

/// Mixed block/quarter/shade glyphs (U+1FB7C..U+1FB97).
pub(crate) fn draw1fb7c_1fb97(cp: u32, canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let up = Alignment::UPPER;
    let lo = Alignment::LOWER;
    let le = Alignment::LEFT;
    let ri = Alignment::RIGHT;
    match cp {
        0x1fb7c => {
            block::block(m, canvas, le, ONE_EIGHTH, 1.0);
            block::block(m, canvas, lo, 1.0, ONE_EIGHTH);
        }
        0x1fb7d => {
            block::block(m, canvas, le, ONE_EIGHTH, 1.0);
            block::block(m, canvas, up, 1.0, ONE_EIGHTH);
        }
        0x1fb7e => {
            block::block(m, canvas, ri, ONE_EIGHTH, 1.0);
            block::block(m, canvas, up, 1.0, ONE_EIGHTH);
        }
        0x1fb7f => {
            block::block(m, canvas, ri, ONE_EIGHTH, 1.0);
            block::block(m, canvas, lo, 1.0, ONE_EIGHTH);
        }
        0x1fb80 => {
            block::block(m, canvas, up, 1.0, ONE_EIGHTH);
            block::block(m, canvas, lo, 1.0, ONE_EIGHTH);
        }
        0x1fb81 => {
            // Horizontal one-eighth blocks at rows 1, 3, 5, 8.
            draw1fb76_1fb7b(0x1fb74 + 1, canvas, width, height, m);
            draw1fb76_1fb7b(0x1fb74 + 3, canvas, width, height, m);
            draw1fb76_1fb7b(0x1fb74 + 5, canvas, width, height, m);
            draw1fb76_1fb7b(0x1fb74 + 8, canvas, width, height, m);
        }
        0x1fb82 => block::block(m, canvas, up, 1.0, ONE_QUARTER),
        0x1fb83 => block::block(m, canvas, up, 1.0, THREE_EIGHTHS),
        0x1fb84 => block::block(m, canvas, up, 1.0, FIVE_EIGHTHS),
        0x1fb85 => block::block(m, canvas, up, 1.0, THREE_QUARTERS),
        0x1fb86 => block::block(m, canvas, up, 1.0, SEVEN_EIGHTHS),
        0x1fb87 => block::block(m, canvas, ri, ONE_QUARTER, 1.0),
        0x1fb88 => block::block(m, canvas, ri, THREE_EIGHTHS, 1.0),
        0x1fb89 => block::block(m, canvas, ri, FIVE_EIGHTHS, 1.0),
        0x1fb8a => block::block(m, canvas, ri, THREE_QUARTERS, 1.0),
        0x1fb8b => block::block(m, canvas, ri, SEVEN_EIGHTHS, 1.0),
        0x1fb8c => block::block_shade(m, canvas, le, HALF, 1.0, Shade::Medium),
        0x1fb8d => block::block_shade(m, canvas, ri, HALF, 1.0, Shade::Medium),
        0x1fb8e => block::block_shade(m, canvas, up, 1.0, HALF, Shade::Medium),
        0x1fb8f => block::block_shade(m, canvas, lo, 1.0, HALF, Shade::Medium),
        0x1fb90 => block::full_block_shade(m, canvas, Shade::Medium),
        0x1fb91 => {
            block::full_block_shade(m, canvas, Shade::Medium);
            block::block(m, canvas, up, 1.0, HALF);
        }
        0x1fb92 => {
            block::full_block_shade(m, canvas, Shade::Medium);
            block::block(m, canvas, lo, 1.0, HALF);
        }
        0x1fb93 => {
            // Unallocated hole in the block; render blank.
        }
        0x1fb94 => {
            block::full_block_shade(m, canvas, Shade::Medium);
            block::block(m, canvas, ri, HALF, 1.0);
        }
        0x1fb95 => checkerboard_fill(m, canvas, 0),
        0x1fb96 => checkerboard_fill(m, canvas, 1),
        0x1fb97 => {
            canvas.box_fill(
                0,
                (height / 4) as i32,
                width as i32,
                (2 * height / 4) as i32,
                Shade::On as u8,
            );
            canvas.box_fill(
                0,
                (3 * height / 4) as i32,
                width as i32,
                height as i32,
                Shade::On as u8,
            );
        }
        _ => {}
    }
}

/// Upper-left to lower-right diagonal fill (U+1FB98).
pub(crate) fn draw1fb98(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    canvas.clip_to_cell();
    let thick_px = Thickness::Light.height(m.box_thickness);
    let line_count = m.cell_width / (2 * thick_px);
    let fw = f64::from(m.cell_width);
    let fh = f64::from(m.cell_height);
    let ft = f64::from(thick_px);
    let stride = (fw / f64::from(line_count)).round();
    for i_raw in 0..(line_count * 2 + 1) {
        let i = i_raw as i32 - line_count as i32;
        let top_x = f64::from(i) * stride;
        let bottom_x = fw + top_x;
        canvas.line(
            Line {
                p0: Point { x: top_x, y: 0.0 },
                p1: Point { x: bottom_x, y: fh },
            },
            ft,
            Shade::On as u8,
        );
    }
}

/// Upper-right to lower-left diagonal fill (U+1FB99).
pub(crate) fn draw1fb99(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    canvas.clip_to_cell();
    let thick_px = Thickness::Light.height(m.box_thickness);
    let line_count = m.cell_width / (2 * thick_px);
    let fw = f64::from(m.cell_width);
    let fh = f64::from(m.cell_height);
    let ft = f64::from(thick_px);
    let stride = (fw / f64::from(line_count)).round();
    for i_raw in 0..(line_count * 2 + 1) {
        let i = i_raw as i32 - line_count as i32;
        let bottom_x = f64::from(i) * stride;
        let top_x = fw + bottom_x;
        canvas.line(
            Line {
                p0: Point { x: top_x, y: 0.0 },
                p1: Point { x: bottom_x, y: fh },
            },
            ft,
            Shade::On as u8,
        );
    }
}

/// Edge-triangle pairs and shaded corner triangles (U+1FB9A..U+1FB9F).
pub(crate) fn draw1fb9a_1fb9f(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    match cp {
        0x1fb9a => {
            edge_triangle(m, canvas, Edge::Top);
            edge_triangle(m, canvas, Edge::Bottom);
        }
        0x1fb9b => {
            edge_triangle(m, canvas, Edge::Left);
            edge_triangle(m, canvas, Edge::Right);
        }
        0x1fb9c => geometric_shapes::corner_triangle_shade(m, canvas, Corner::Tl, Shade::Medium),
        0x1fb9d => geometric_shapes::corner_triangle_shade(m, canvas, Corner::Tr, Shade::Medium),
        0x1fb9e => geometric_shapes::corner_triangle_shade(m, canvas, Corner::Br, Shade::Medium),
        0x1fb9f => geometric_shapes::corner_triangle_shade(m, canvas, Corner::Bl, Shade::Medium),
        _ => {}
    }
}

/// Corner diagonal line glyphs (U+1FBA0..U+1FBAE).
pub(crate) fn draw1fba0_1fbae(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    let q = |tl, tr, bl, br| Quads { tl, tr, bl, br };
    match cp {
        0x1fba0 => corner_diagonal_lines(m, canvas, q(true, false, false, false)),
        0x1fba1 => corner_diagonal_lines(m, canvas, q(false, true, false, false)),
        0x1fba2 => corner_diagonal_lines(m, canvas, q(false, false, true, false)),
        0x1fba3 => corner_diagonal_lines(m, canvas, q(false, false, false, true)),
        0x1fba4 => corner_diagonal_lines(m, canvas, q(true, false, true, false)),
        0x1fba5 => corner_diagonal_lines(m, canvas, q(false, true, false, true)),
        0x1fba6 => corner_diagonal_lines(m, canvas, q(false, false, true, true)),
        0x1fba7 => corner_diagonal_lines(m, canvas, q(true, true, false, false)),
        0x1fba8 => corner_diagonal_lines(m, canvas, q(true, false, false, true)),
        0x1fba9 => corner_diagonal_lines(m, canvas, q(false, true, true, false)),
        0x1fbaa => corner_diagonal_lines(m, canvas, q(false, true, true, true)),
        0x1fbab => corner_diagonal_lines(m, canvas, q(true, false, true, true)),
        0x1fbac => corner_diagonal_lines(m, canvas, q(true, true, false, true)),
        0x1fbad => corner_diagonal_lines(m, canvas, q(true, true, true, false)),
        0x1fbae => corner_diagonal_lines(m, canvas, q(true, true, true, true)),
        _ => {}
    }
}

/// 🮯 heavy vertical + light horizontal (U+1FBAF).
pub(crate) fn draw1fbaf(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    box_drawing::lines_char(
        m,
        canvas,
        box_drawing::Lines {
            up: box_drawing::LineStyle::Heavy,
            down: box_drawing::LineStyle::Heavy,
            left: box_drawing::LineStyle::Light,
            right: box_drawing::LineStyle::Light,
        },
    );
}

/// 🮽 inverted light diagonal cross (U+1FBBD).
pub(crate) fn draw1fbbd(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    box_drawing::light_diagonal_cross(m, canvas);
    canvas.invert();
    canvas.clip_to_cell();
}

/// 🮾 inverted br corner diagonal (U+1FBBE).
pub(crate) fn draw1fbbe(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    corner_diagonal_lines(
        m,
        canvas,
        Quads {
            br: true,
            ..Quads::default()
        },
    );
    canvas.invert();
    canvas.clip_to_cell();
}

/// 🮿 inverted all-corner diagonals (U+1FBBF).
pub(crate) fn draw1fbbf(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    corner_diagonal_lines(
        m,
        canvas,
        Quads {
            tl: true,
            tr: true,
            bl: true,
            br: true,
        },
    );
    canvas.invert();
    canvas.clip_to_cell();
}

/// 🯎 left two-thirds block (U+1FBCE).
pub(crate) fn draw1fbce(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    block::block(m, canvas, Alignment::LEFT, TWO_THIRDS, 1.0);
}

/// 🯏 left one-third block (U+1FBCF).
pub(crate) fn draw1fbcf(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    block::block(m, canvas, Alignment::LEFT, ONE_THIRD, 1.0);
}

/// Cell diagonals (U+1FBD0..U+1FBDF).
pub(crate) fn draw1fbd0_1fbdf(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    let d = |c: &mut Canvas, from: Alignment, to: Alignment| cell_diagonal(m, c, from, to);
    use Alignment as A;
    match cp {
        0x1fbd0 => d(canvas, A::RIGHT, A::LOWER_LEFT),
        0x1fbd1 => d(canvas, A::UPPER_RIGHT, A::LEFT),
        0x1fbd2 => d(canvas, A::UPPER_LEFT, A::RIGHT),
        0x1fbd3 => d(canvas, A::LEFT, A::LOWER_RIGHT),
        0x1fbd4 => d(canvas, A::UPPER_LEFT, A::LOWER),
        0x1fbd5 => d(canvas, A::UPPER, A::LOWER_RIGHT),
        0x1fbd6 => d(canvas, A::UPPER_RIGHT, A::LOWER),
        0x1fbd7 => d(canvas, A::UPPER, A::LOWER_LEFT),
        0x1fbd8 => {
            d(canvas, A::UPPER_LEFT, A::CENTER);
            d(canvas, A::CENTER, A::UPPER_RIGHT);
        }
        0x1fbd9 => {
            d(canvas, A::UPPER_RIGHT, A::CENTER);
            d(canvas, A::CENTER, A::LOWER_RIGHT);
        }
        0x1fbda => {
            d(canvas, A::LOWER_LEFT, A::CENTER);
            d(canvas, A::CENTER, A::LOWER_RIGHT);
        }
        0x1fbdb => {
            d(canvas, A::UPPER_LEFT, A::CENTER);
            d(canvas, A::CENTER, A::LOWER_LEFT);
        }
        0x1fbdc => {
            d(canvas, A::UPPER_LEFT, A::LOWER);
            d(canvas, A::LOWER, A::UPPER_RIGHT);
        }
        0x1fbdd => {
            d(canvas, A::UPPER_RIGHT, A::LEFT);
            d(canvas, A::LEFT, A::LOWER_RIGHT);
        }
        0x1fbde => {
            d(canvas, A::LOWER_LEFT, A::UPPER);
            d(canvas, A::UPPER, A::LOWER_RIGHT);
        }
        0x1fbdf => {
            d(canvas, A::UPPER_LEFT, A::RIGHT);
            d(canvas, A::RIGHT, A::LOWER_LEFT);
        }
        _ => {}
    }
}

/// Circles and half-cells (U+1FBE0..U+1FBEF).
pub(crate) fn draw1fbe0_1fbef(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    use Alignment as A;
    match cp {
        0x1fbe0 => circle(m, canvas, A::UPPER, false),
        0x1fbe1 => circle(m, canvas, A::RIGHT, false),
        0x1fbe2 => circle(m, canvas, A::LOWER, false),
        0x1fbe3 => circle(m, canvas, A::LEFT, false),
        0x1fbe4 => block::block(m, canvas, A::UPPER, 0.5, 0.5),
        0x1fbe5 => block::block(m, canvas, A::LOWER, 0.5, 0.5),
        0x1fbe6 => block::block(m, canvas, A::LEFT, 0.5, 0.5),
        0x1fbe7 => block::block(m, canvas, A::RIGHT, 0.5, 0.5),
        0x1fbe8 => circle(m, canvas, A::UPPER, true),
        0x1fbe9 => circle(m, canvas, A::RIGHT, true),
        0x1fbea => circle(m, canvas, A::LOWER, true),
        0x1fbeb => circle(m, canvas, A::LEFT, true),
        0x1fbec => circle(m, canvas, A::UPPER_RIGHT, true),
        0x1fbed => circle(m, canvas, A::LOWER_LEFT, true),
        0x1fbee => circle(m, canvas, A::LOWER_RIGHT, true),
        0x1fbef => circle(m, canvas, A::UPPER_LEFT, true),
        _ => {}
    }
}

fn edge_triangle(m: &Metrics, canvas: &mut Canvas, edge: Edge) {
    let upper = 0.0;
    let middle = (f64::from(m.cell_height) / 2.0).round();
    let lower = f64::from(m.cell_height);
    let left = 0.0;
    let center = (f64::from(m.cell_width) / 2.0).round();
    let right = f64::from(m.cell_width);

    let (x0, y0, x1, y1) = match edge {
        Edge::Top => (right, upper, left, upper),
        Edge::Left => (left, upper, left, lower),
        Edge::Bottom => (left, lower, right, lower),
        Edge::Right => (right, lower, right, upper),
    };

    let mut pb = canvas.path();
    pb.move_to(center as f32, middle as f32);
    pb.line_to(x0 as f32, y0 as f32);
    pb.line_to(x1 as f32, y1 as f32);
    pb.close();
    canvas.fill_path(pb, Shade::On as u8);
}

fn corner_diagonal_lines(m: &Metrics, canvas: &mut Canvas, corners: Quads) {
    let thick_px = Thickness::Light.height(m.box_thickness);
    let fw = f64::from(m.cell_width);
    let fh = f64::from(m.cell_height);
    let ft = f64::from(thick_px);
    let center_x = f64::from(m.cell_width / 2 + m.cell_width % 2);
    let center_y = f64::from(m.cell_height / 2 + m.cell_height % 2);
    let on = Shade::On as u8;
    let l = |c: &mut Canvas, x0: f64, y0: f64, x1: f64, y1: f64| {
        c.line(
            Line {
                p0: Point { x: x0, y: y0 },
                p1: Point { x: x1, y: y1 },
            },
            ft,
            on,
        );
    };
    if corners.tl {
        l(canvas, center_x, 0.0, 0.0, center_y);
    }
    if corners.tr {
        l(canvas, center_x, 0.0, fw, center_y);
    }
    if corners.bl {
        l(canvas, center_x, fh, 0.0, center_y);
    }
    if corners.br {
        l(canvas, center_x, fh, fw, center_y);
    }
}

fn cell_diagonal(m: &Metrics, canvas: &mut Canvas, from: Alignment, to: Alignment) {
    use crate::common::{Horizontal, Vertical};
    let fw = f64::from(m.cell_width);
    let fh = f64::from(m.cell_height);
    let hx = |h: Horizontal| match h {
        Horizontal::Left => 0.0,
        Horizontal::Right => fw,
        Horizontal::Center => fw / 2.0,
    };
    let vy = |v: Vertical| match v {
        Vertical::Top => 0.0,
        Vertical::Bottom => fh,
        Vertical::Middle => fh / 2.0,
    };
    canvas.line(
        Line {
            p0: Point {
                x: hx(from.horizontal),
                y: vy(from.vertical),
            },
            p1: Point {
                x: hx(to.horizontal),
                y: vy(to.vertical),
            },
        },
        f64::from(Thickness::Light.height(m.box_thickness)),
        Shade::On as u8,
    );
}

fn checkerboard_fill(m: &Metrics, canvas: &mut Canvas, parity: u8) {
    let fw = f64::from(m.cell_width);
    let fh = f64::from(m.cell_height);
    let x_size = 4usize;
    let y_size = (4.0 * (fh / fw)).round() as usize;
    for x in 0..x_size {
        let x0 = (m.cell_width as usize * x) / x_size;
        let x1 = (m.cell_width as usize * (x + 1)) / x_size;
        for y in 0..y_size {
            let y0 = (m.cell_height as usize * y) / y_size;
            let y1 = (m.cell_height as usize * (y + 1)) / y_size;
            if ((x + y) % 2) as u8 == parity {
                canvas.rect(
                    x0 as i32,
                    y0 as i32,
                    x1.saturating_sub(x0) as i32,
                    y1.saturating_sub(y0) as i32,
                    Shade::On as u8,
                );
            }
        }
    }
}

/// Draw a circle (filled or outlined) whose center sits at a cell alignment
/// anchor and whose radius is half the smaller cell dimension. Reused by the
/// supplement's half-circle/ellipse glyphs.
pub(crate) fn circle(m: &Metrics, canvas: &mut Canvas, position: Alignment, filled: bool) {
    use crate::common::{Horizontal, Vertical};
    canvas.clip_to_cell();
    let fw = f64::from(m.cell_width);
    let fh = f64::from(m.cell_height);
    let x = match position.horizontal {
        Horizontal::Left => 0.0,
        Horizontal::Right => fw,
        Horizontal::Center => fw / 2.0,
    };
    let y = match position.vertical {
        Vertical::Top => 0.0,
        Vertical::Bottom => fh,
        Vertical::Middle => fh / 2.0,
    };
    let r = 0.5 * fw.min(fh);
    let line_width = f64::from(Thickness::Light.height(m.box_thickness));

    let mut ctx = canvas.context();
    ctx.line_width = line_width;
    if filled {
        ctx.circle(x, y, r);
        ctx.fill(Shade::On as u8);
    } else {
        ctx.circle(x, y, r - line_width / 2.0);
        ctx.stroke(Shade::On as u8);
    }
}
