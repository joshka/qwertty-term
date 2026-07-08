//! Geometric Shapes | U+25A0..U+25FF (partial)
//!
//! Ported from `src/font/sprite/draw/geometric_shapes.zig`. Only the
//! sprite-viable corner-triangle subset is implemented, matching upstream.
//! [`corner_triangle_shade`] is reused by the legacy-computing glyphs.

use tiny_skia::LineCap;

use crate::canvas::{Point, Triangle};
use crate::common::{Corner, Shade, Thickness};
use crate::{Canvas, Metrics};

/// ◢ ◣ ◤ ◥
pub(crate) fn draw25e2_25e5(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    match cp {
        0x25e2 => corner_triangle_shade(m, canvas, Corner::Br, Shade::On),
        0x25e3 => corner_triangle_shade(m, canvas, Corner::Bl, Shade::On),
        0x25e4 => corner_triangle_shade(m, canvas, Corner::Tl, Shade::On),
        0x25e5 => corner_triangle_shade(m, canvas, Corner::Tr, Shade::On),
        _ => {}
    }
}

/// ◸ ◹ ◺
pub(crate) fn draw25f8_25fa(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    match cp {
        0x25f8 => corner_triangle_outline(m, canvas, Corner::Tl),
        0x25f9 => corner_triangle_outline(m, canvas, Corner::Tr),
        0x25fa => corner_triangle_outline(m, canvas, Corner::Bl),
        _ => {}
    }
}

/// ◿
pub(crate) fn draw25ff(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    corner_triangle_outline(m, canvas, Corner::Br);
}

fn corner_points(m: &Metrics, corner: Corner) -> Triangle {
    let fw = f64::from(m.cell_width);
    let fh = f64::from(m.cell_height);
    let (x0, y0, x1, y1, x2, y2) = match corner {
        Corner::Tl => (0.0, 0.0, 0.0, fh, fw, 0.0),
        Corner::Tr => (0.0, 0.0, fw, fh, fw, 0.0),
        Corner::Bl => (0.0, 0.0, 0.0, fh, fw, fh),
        Corner::Br => (0.0, fh, fw, fh, fw, 0.0),
    };
    Triangle {
        p0: Point { x: x0, y: y0 },
        p1: Point { x: x1, y: y1 },
        p2: Point { x: x2, y: y2 },
    }
}

/// Filled/shaded right triangle occupying one corner of the cell.
pub(crate) fn corner_triangle_shade(
    m: &Metrics,
    canvas: &mut Canvas,
    corner: Corner,
    shade: Shade,
) {
    canvas.triangle(corner_points(m, corner), shade as u8);
}

/// Outlined (inner-stroked) right triangle occupying one corner of the cell.
pub(crate) fn corner_triangle_outline(m: &Metrics, canvas: &mut Canvas, corner: Corner) {
    let t = corner_points(m, corner);
    let float_thick = f64::from(Thickness::Light.height(m.box_thickness));
    let mut pb = canvas.path();
    pb.move_to(t.p0.x as f32, t.p0.y as f32);
    pb.line_to(t.p1.x as f32, t.p1.y as f32);
    pb.line_to(t.p2.x as f32, t.p2.y as f32);
    pb.close();
    canvas.inner_stroke_path(pb, float_thick, LineCap::Butt, Shade::On as u8);
}
