//! Powerline + Powerline Extra Symbols | U+E0B0..U+E0D4 (geometric subset)
//!
//! Ported from `src/font/sprite/draw/powerline.zig`. Only the geometric glyphs
//! (arrows, half-circles, flames-as-triangles) are implemented; the stylized
//! ones are left to a real font, matching upstream. Several glyphs are drawn by
//! flipping another glyph horizontally.

use tiny_skia::LineCap;

use crate::canvas::{Point, Triangle};
use crate::common::{Shade, Thickness};
use crate::draw::box_drawing;
use crate::{Canvas, Metrics};

pub(crate) fn draw_e0b0_e0d4(cp: u32, canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    match cp {
        0xe0b0 => filled_triangle(canvas, width, height, TriKind::RightPoint),
        0xe0b1 => chevron(canvas, width, height, m),
        0xe0b2 => filled_triangle(canvas, width, height, TriKind::LeftPoint),
        0xe0b3 => {
            chevron(canvas, width, height, m);
            canvas.flip_horizontal();
        }
        0xe0b4 => half_circle_right(canvas, width, height, m),
        0xe0b5 => half_circle_right_outline(canvas, width, height, m),
        0xe0b6 => {
            half_circle_right(canvas, width, height, m);
            canvas.flip_horizontal();
        }
        0xe0b7 => {
            half_circle_right_outline(canvas, width, height, m);
            canvas.flip_horizontal();
        }
        0xe0b8 => filled_triangle(canvas, width, height, TriKind::LowerLeft),
        0xe0b9 => box_drawing::light_diagonal_upper_left_to_lower_right(m, canvas),
        0xe0ba => filled_triangle(canvas, width, height, TriKind::LowerRight),
        0xe0bb => box_drawing::light_diagonal_upper_right_to_lower_left(m, canvas),
        0xe0bc => filled_triangle(canvas, width, height, TriKind::UpperLeft),
        0xe0bd => box_drawing::light_diagonal_upper_right_to_lower_left(m, canvas),
        0xe0be => filled_triangle(canvas, width, height, TriKind::UpperRight),
        0xe0bf => box_drawing::light_diagonal_upper_left_to_lower_right(m, canvas),
        0xe0d2 => trapezoids(canvas, width, height, m),
        0xe0d4 => {
            trapezoids(canvas, width, height, m);
            canvas.flip_horizontal();
        }
        _ => {}
    }
}

enum TriKind {
    RightPoint,
    LeftPoint,
    LowerLeft,
    LowerRight,
    UpperLeft,
    UpperRight,
}

fn filled_triangle(canvas: &mut Canvas, width: u32, height: u32, kind: TriKind) {
    let fw = f64::from(width);
    let fh = f64::from(height);
    let t = match kind {
        TriKind::RightPoint => Triangle {
            p0: Point { x: 0.0, y: 0.0 },
            p1: Point { x: fw, y: fh / 2.0 },
            p2: Point { x: 0.0, y: fh },
        },
        TriKind::LeftPoint => Triangle {
            p0: Point { x: fw, y: 0.0 },
            p1: Point {
                x: 0.0,
                y: fh / 2.0,
            },
            p2: Point { x: fw, y: fh },
        },
        TriKind::LowerLeft => Triangle {
            p0: Point { x: 0.0, y: 0.0 },
            p1: Point { x: fw, y: fh },
            p2: Point { x: 0.0, y: fh },
        },
        TriKind::LowerRight => Triangle {
            p0: Point { x: fw, y: 0.0 },
            p1: Point { x: fw, y: fh },
            p2: Point { x: 0.0, y: fh },
        },
        TriKind::UpperLeft => Triangle {
            p0: Point { x: 0.0, y: 0.0 },
            p1: Point { x: fw, y: 0.0 },
            p2: Point { x: 0.0, y: fh },
        },
        TriKind::UpperRight => Triangle {
            p0: Point { x: 0.0, y: 0.0 },
            p1: Point { x: fw, y: 0.0 },
            p2: Point { x: fw, y: fh },
        },
    };
    canvas.triangle(t, Shade::On as u8);
}

fn chevron(canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let fw = f64::from(width);
    let fh = f64::from(height);
    let mut pb = canvas.path();
    pb.move_to(0.0, 0.0);
    pb.line_to(fw as f32, (fh / 2.0) as f32);
    pb.line_to(0.0, fh as f32);
    canvas.stroke_path(
        pb,
        f64::from(Thickness::Light.height(m.box_thickness)),
        LineCap::Butt,
        Shade::On as u8,
    );
}

fn half_circle_right(canvas: &mut Canvas, width: u32, height: u32, _m: &Metrics) {
    let fw = f64::from(width);
    let fh = f64::from(height);
    let c = (std::f64::consts::SQRT_2 - 1.0) * 4.0 / 3.0;
    let radius = fw.min(fh / 2.0);
    let mut pb = canvas.path();
    pb.move_to(0.0, 0.0);
    pb.cubic_to(
        (radius * c) as f32,
        0.0,
        radius as f32,
        (radius - radius * c) as f32,
        radius as f32,
        radius as f32,
    );
    pb.line_to(radius as f32, (fh - radius) as f32);
    pb.cubic_to(
        radius as f32,
        (fh - radius + radius * c) as f32,
        (radius * c) as f32,
        fh as f32,
        0.0,
        fh as f32,
    );
    pb.close();
    canvas.fill_path(pb, Shade::On as u8);
}

fn half_circle_right_outline(canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let fw = f64::from(width);
    let fh = f64::from(height);
    let c = (std::f64::consts::SQRT_2 - 1.0) * 4.0 / 3.0;
    let radius = fw.min(fh / 2.0);
    let mut pb = canvas.path();
    pb.move_to(0.0, 0.0);
    pb.cubic_to(
        (radius * c) as f32,
        0.0,
        radius as f32,
        (radius - radius * c) as f32,
        radius as f32,
        radius as f32,
    );
    pb.line_to(radius as f32, (fh - radius) as f32);
    pb.cubic_to(
        radius as f32,
        (fh - radius + radius * c) as f32,
        (radius * c) as f32,
        fh as f32,
        0.0,
        fh as f32,
    );
    canvas.inner_stroke_path(
        pb,
        f64::from(m.box_thickness),
        LineCap::Butt,
        Shade::On as u8,
    );
}

fn trapezoids(canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let fw = f64::from(width);
    let fh = f64::from(height);
    let ft = f64::from(m.box_thickness);

    // Top piece.
    let mut pb = canvas.path();
    pb.move_to(0.0, 0.0);
    pb.line_to(fw as f32, 0.0);
    pb.line_to((fw / 2.0) as f32, (fh / 2.0 - ft / 2.0) as f32);
    pb.line_to(0.0, (fh / 2.0 - ft / 2.0) as f32);
    pb.close();
    canvas.fill_path(pb, Shade::On as u8);

    // Bottom piece.
    let mut pb = canvas.path();
    pb.move_to(0.0, fh as f32);
    pb.line_to(fw as f32, fh as f32);
    pb.line_to((fw / 2.0) as f32, (fh / 2.0 + ft / 2.0) as f32);
    pb.line_to(0.0, (fh / 2.0 + ft / 2.0) as f32);
    pb.close();
    canvas.fill_path(pb, Shade::On as u8);
}
