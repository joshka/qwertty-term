//! Box Drawing | U+2500..U+257F
//!
//! Ported from `src/font/sprite/draw/box.zig`. This is the shared hub for the
//! whole subsystem: [`lines_char`] (the intersection-style line renderer) and
//! [`arc`] are reused by branch, powerline, and legacy-computing glyphs.

use tiny_skia::LineCap;

use crate::common::{Corner, Shade, Thickness, hline, hline_middle, vline, vline_middle};
use crate::{Canvas, Metrics};

/// Per-edge line style for an intersection-style box-drawing char.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Lines {
    pub up: LineStyle,
    pub right: LineStyle,
    pub down: LineStyle,
    pub left: LineStyle,
}

/// The style of one edge of a box-drawing char.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LineStyle {
    /// No line on this edge.
    #[default]
    None,
    /// Light line.
    Light,
    /// Heavy line.
    Heavy,
    /// Double line.
    Double,
}

use LineStyle::{Double, Heavy, Light, None as NoLine};

/// Convenience for a [`Lines`] literal in this module.
macro_rules! lines {
    ($($edge:ident = $style:expr),* $(,)?) => {
        #[allow(clippy::needless_update)]
        Lines { $($edge: $style,)* ..Lines::default() }
    };
}

pub(crate) fn draw2500_257f(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    match cp {
        0x2500 => lines_char(m, canvas, lines!(left = Light, right = Light)),
        0x2501 => lines_char(m, canvas, lines!(left = Heavy, right = Heavy)),
        0x2502 => lines_char(m, canvas, lines!(up = Light, down = Light)),
        0x2503 => lines_char(m, canvas, lines!(up = Heavy, down = Heavy)),
        0x2504 => dash_horizontal(
            m,
            canvas,
            3,
            Thickness::Light.height(m.box_thickness),
            4.max(Thickness::Light.height(m.box_thickness)),
        ),
        0x2505 => dash_horizontal(
            m,
            canvas,
            3,
            Thickness::Heavy.height(m.box_thickness),
            4.max(Thickness::Light.height(m.box_thickness)),
        ),
        0x2506 => dash_vertical(
            m,
            canvas,
            3,
            Thickness::Light.height(m.box_thickness),
            4.max(Thickness::Light.height(m.box_thickness)),
        ),
        0x2507 => dash_vertical(
            m,
            canvas,
            3,
            Thickness::Heavy.height(m.box_thickness),
            4.max(Thickness::Light.height(m.box_thickness)),
        ),
        0x2508 => dash_horizontal(
            m,
            canvas,
            4,
            Thickness::Light.height(m.box_thickness),
            4.max(Thickness::Light.height(m.box_thickness)),
        ),
        0x2509 => dash_horizontal(
            m,
            canvas,
            4,
            Thickness::Heavy.height(m.box_thickness),
            4.max(Thickness::Light.height(m.box_thickness)),
        ),
        0x250a => dash_vertical(
            m,
            canvas,
            4,
            Thickness::Light.height(m.box_thickness),
            4.max(Thickness::Light.height(m.box_thickness)),
        ),
        0x250b => dash_vertical(
            m,
            canvas,
            4,
            Thickness::Heavy.height(m.box_thickness),
            4.max(Thickness::Light.height(m.box_thickness)),
        ),
        0x250c => lines_char(m, canvas, lines!(down = Light, right = Light)),
        0x250d => lines_char(m, canvas, lines!(down = Light, right = Heavy)),
        0x250e => lines_char(m, canvas, lines!(down = Heavy, right = Light)),
        0x250f => lines_char(m, canvas, lines!(down = Heavy, right = Heavy)),

        0x2510 => lines_char(m, canvas, lines!(down = Light, left = Light)),
        0x2511 => lines_char(m, canvas, lines!(down = Light, left = Heavy)),
        0x2512 => lines_char(m, canvas, lines!(down = Heavy, left = Light)),
        0x2513 => lines_char(m, canvas, lines!(down = Heavy, left = Heavy)),
        0x2514 => lines_char(m, canvas, lines!(up = Light, right = Light)),
        0x2515 => lines_char(m, canvas, lines!(up = Light, right = Heavy)),
        0x2516 => lines_char(m, canvas, lines!(up = Heavy, right = Light)),
        0x2517 => lines_char(m, canvas, lines!(up = Heavy, right = Heavy)),
        0x2518 => lines_char(m, canvas, lines!(up = Light, left = Light)),
        0x2519 => lines_char(m, canvas, lines!(up = Light, left = Heavy)),
        0x251a => lines_char(m, canvas, lines!(up = Heavy, left = Light)),
        0x251b => lines_char(m, canvas, lines!(up = Heavy, left = Heavy)),
        0x251c => lines_char(m, canvas, lines!(up = Light, down = Light, right = Light)),
        0x251d => lines_char(m, canvas, lines!(up = Light, down = Light, right = Heavy)),
        0x251e => lines_char(m, canvas, lines!(up = Heavy, right = Light, down = Light)),
        0x251f => lines_char(m, canvas, lines!(down = Heavy, right = Light, up = Light)),

        0x2520 => lines_char(m, canvas, lines!(up = Heavy, down = Heavy, right = Light)),
        0x2521 => lines_char(m, canvas, lines!(down = Light, right = Heavy, up = Heavy)),
        0x2522 => lines_char(m, canvas, lines!(up = Light, right = Heavy, down = Heavy)),
        0x2523 => lines_char(m, canvas, lines!(up = Heavy, down = Heavy, right = Heavy)),
        0x2524 => lines_char(m, canvas, lines!(up = Light, down = Light, left = Light)),
        0x2525 => lines_char(m, canvas, lines!(up = Light, down = Light, left = Heavy)),
        0x2526 => lines_char(m, canvas, lines!(up = Heavy, left = Light, down = Light)),
        0x2527 => lines_char(m, canvas, lines!(down = Heavy, left = Light, up = Light)),
        0x2528 => lines_char(m, canvas, lines!(up = Heavy, down = Heavy, left = Light)),
        0x2529 => lines_char(m, canvas, lines!(down = Light, left = Heavy, up = Heavy)),
        0x252a => lines_char(m, canvas, lines!(up = Light, left = Heavy, down = Heavy)),
        0x252b => lines_char(m, canvas, lines!(up = Heavy, down = Heavy, left = Heavy)),
        0x252c => lines_char(m, canvas, lines!(down = Light, left = Light, right = Light)),
        0x252d => lines_char(m, canvas, lines!(left = Heavy, right = Light, down = Light)),
        0x252e => lines_char(m, canvas, lines!(right = Heavy, left = Light, down = Light)),
        0x252f => lines_char(m, canvas, lines!(down = Light, left = Heavy, right = Heavy)),

        0x2530 => lines_char(m, canvas, lines!(down = Heavy, left = Light, right = Light)),
        0x2531 => lines_char(m, canvas, lines!(right = Light, left = Heavy, down = Heavy)),
        0x2532 => lines_char(m, canvas, lines!(left = Light, right = Heavy, down = Heavy)),
        0x2533 => lines_char(m, canvas, lines!(down = Heavy, left = Heavy, right = Heavy)),
        0x2534 => lines_char(m, canvas, lines!(up = Light, left = Light, right = Light)),
        0x2535 => lines_char(m, canvas, lines!(left = Heavy, right = Light, up = Light)),
        0x2536 => lines_char(m, canvas, lines!(right = Heavy, left = Light, up = Light)),
        0x2537 => lines_char(m, canvas, lines!(up = Light, left = Heavy, right = Heavy)),
        0x2538 => lines_char(m, canvas, lines!(up = Heavy, left = Light, right = Light)),
        0x2539 => lines_char(m, canvas, lines!(right = Light, left = Heavy, up = Heavy)),
        0x253a => lines_char(m, canvas, lines!(left = Light, right = Heavy, up = Heavy)),
        0x253b => lines_char(m, canvas, lines!(up = Heavy, left = Heavy, right = Heavy)),
        0x253c => lines_char(
            m,
            canvas,
            lines!(up = Light, down = Light, left = Light, right = Light),
        ),
        0x253d => lines_char(
            m,
            canvas,
            lines!(left = Heavy, right = Light, up = Light, down = Light),
        ),
        0x253e => lines_char(
            m,
            canvas,
            lines!(right = Heavy, left = Light, up = Light, down = Light),
        ),
        0x253f => lines_char(
            m,
            canvas,
            lines!(up = Light, down = Light, left = Heavy, right = Heavy),
        ),

        0x2540 => lines_char(
            m,
            canvas,
            lines!(up = Heavy, down = Light, left = Light, right = Light),
        ),
        0x2541 => lines_char(
            m,
            canvas,
            lines!(down = Heavy, up = Light, left = Light, right = Light),
        ),
        0x2542 => lines_char(
            m,
            canvas,
            lines!(up = Heavy, down = Heavy, left = Light, right = Light),
        ),
        0x2543 => lines_char(
            m,
            canvas,
            lines!(left = Heavy, up = Heavy, right = Light, down = Light),
        ),
        0x2544 => lines_char(
            m,
            canvas,
            lines!(right = Heavy, up = Heavy, left = Light, down = Light),
        ),
        0x2545 => lines_char(
            m,
            canvas,
            lines!(left = Heavy, down = Heavy, right = Light, up = Light),
        ),
        0x2546 => lines_char(
            m,
            canvas,
            lines!(right = Heavy, down = Heavy, left = Light, up = Light),
        ),
        0x2547 => lines_char(
            m,
            canvas,
            lines!(down = Light, up = Heavy, left = Heavy, right = Heavy),
        ),
        0x2548 => lines_char(
            m,
            canvas,
            lines!(up = Light, down = Heavy, left = Heavy, right = Heavy),
        ),
        0x2549 => lines_char(
            m,
            canvas,
            lines!(right = Light, left = Heavy, up = Heavy, down = Heavy),
        ),
        0x254a => lines_char(
            m,
            canvas,
            lines!(left = Light, right = Heavy, up = Heavy, down = Heavy),
        ),
        0x254b => lines_char(
            m,
            canvas,
            lines!(up = Heavy, down = Heavy, left = Heavy, right = Heavy),
        ),
        0x254c => dash_horizontal(
            m,
            canvas,
            2,
            Thickness::Light.height(m.box_thickness),
            Thickness::Light.height(m.box_thickness),
        ),
        0x254d => dash_horizontal(
            m,
            canvas,
            2,
            Thickness::Heavy.height(m.box_thickness),
            Thickness::Heavy.height(m.box_thickness),
        ),
        0x254e => dash_vertical(
            m,
            canvas,
            2,
            Thickness::Light.height(m.box_thickness),
            Thickness::Heavy.height(m.box_thickness),
        ),
        0x254f => dash_vertical(
            m,
            canvas,
            2,
            Thickness::Heavy.height(m.box_thickness),
            Thickness::Heavy.height(m.box_thickness),
        ),

        0x2550 => lines_char(m, canvas, lines!(left = Double, right = Double)),
        0x2551 => lines_char(m, canvas, lines!(up = Double, down = Double)),
        0x2552 => lines_char(m, canvas, lines!(down = Light, right = Double)),
        0x2553 => lines_char(m, canvas, lines!(down = Double, right = Light)),
        0x2554 => lines_char(m, canvas, lines!(down = Double, right = Double)),
        0x2555 => lines_char(m, canvas, lines!(down = Light, left = Double)),
        0x2556 => lines_char(m, canvas, lines!(down = Double, left = Light)),
        0x2557 => lines_char(m, canvas, lines!(down = Double, left = Double)),
        0x2558 => lines_char(m, canvas, lines!(up = Light, right = Double)),
        0x2559 => lines_char(m, canvas, lines!(up = Double, right = Light)),
        0x255a => lines_char(m, canvas, lines!(up = Double, right = Double)),
        0x255b => lines_char(m, canvas, lines!(up = Light, left = Double)),
        0x255c => lines_char(m, canvas, lines!(up = Double, left = Light)),
        0x255d => lines_char(m, canvas, lines!(up = Double, left = Double)),
        0x255e => lines_char(m, canvas, lines!(up = Light, down = Light, right = Double)),
        0x255f => lines_char(m, canvas, lines!(up = Double, down = Double, right = Light)),

        0x2560 => lines_char(
            m,
            canvas,
            lines!(up = Double, down = Double, right = Double),
        ),
        0x2561 => lines_char(m, canvas, lines!(up = Light, down = Light, left = Double)),
        0x2562 => lines_char(m, canvas, lines!(up = Double, down = Double, left = Light)),
        0x2563 => lines_char(m, canvas, lines!(up = Double, down = Double, left = Double)),
        0x2564 => lines_char(
            m,
            canvas,
            lines!(down = Light, left = Double, right = Double),
        ),
        0x2565 => lines_char(
            m,
            canvas,
            lines!(down = Double, left = Light, right = Light),
        ),
        0x2566 => lines_char(
            m,
            canvas,
            lines!(down = Double, left = Double, right = Double),
        ),
        0x2567 => lines_char(m, canvas, lines!(up = Light, left = Double, right = Double)),
        0x2568 => lines_char(m, canvas, lines!(up = Double, left = Light, right = Light)),
        0x2569 => lines_char(
            m,
            canvas,
            lines!(up = Double, left = Double, right = Double),
        ),
        0x256a => lines_char(
            m,
            canvas,
            lines!(up = Light, down = Light, left = Double, right = Double),
        ),
        0x256b => lines_char(
            m,
            canvas,
            lines!(up = Double, down = Double, left = Light, right = Light),
        ),
        0x256c => lines_char(
            m,
            canvas,
            lines!(up = Double, down = Double, left = Double, right = Double),
        ),
        0x256d => arc(m, canvas, Corner::Br, Thickness::Light),
        0x256e => arc(m, canvas, Corner::Bl, Thickness::Light),
        0x256f => arc(m, canvas, Corner::Tl, Thickness::Light),

        0x2570 => arc(m, canvas, Corner::Tr, Thickness::Light),
        0x2571 => light_diagonal_upper_right_to_lower_left(m, canvas),
        0x2572 => light_diagonal_upper_left_to_lower_right(m, canvas),
        0x2573 => light_diagonal_cross(m, canvas),
        0x2574 => lines_char(m, canvas, lines!(left = Light)),
        0x2575 => lines_char(m, canvas, lines!(up = Light)),
        0x2576 => lines_char(m, canvas, lines!(right = Light)),
        0x2577 => lines_char(m, canvas, lines!(down = Light)),
        0x2578 => lines_char(m, canvas, lines!(left = Heavy)),
        0x2579 => lines_char(m, canvas, lines!(up = Heavy)),
        0x257a => lines_char(m, canvas, lines!(right = Heavy)),
        0x257b => lines_char(m, canvas, lines!(down = Heavy)),
        0x257c => lines_char(m, canvas, lines!(left = Light, right = Heavy)),
        0x257d => lines_char(m, canvas, lines!(up = Light, down = Heavy)),
        0x257e => lines_char(m, canvas, lines!(left = Heavy, right = Light)),
        0x257f => lines_char(m, canvas, lines!(up = Heavy, down = Light)),

        _ => {}
    }
}

/// Draw an intersection-style box-drawing char. Shared by many glyph blocks.
///
/// The center offsets (`up_bottom`, `down_top`, `left_right`, `right_left`) are
/// computed with the same precedence as the Zig source so that lines meet
/// cleanly regardless of odd/even cell size — do not reorder the branches.
#[allow(clippy::too_many_lines)]
pub(crate) fn lines_char(m: &Metrics, canvas: &mut Canvas, lines: Lines) {
    let on = Shade::On as u8;
    let light_px = Thickness::Light.height(m.box_thickness);
    let heavy_px = Thickness::Heavy.height(m.box_thickness);

    let h_light_top = m.cell_height.saturating_sub(light_px) / 2;
    let h_light_bottom = h_light_top + light_px;
    let h_heavy_top = m.cell_height.saturating_sub(heavy_px) / 2;
    let h_heavy_bottom = h_heavy_top + heavy_px;
    let h_double_top = h_light_top.saturating_sub(light_px);
    let h_double_bottom = h_light_bottom + light_px;

    let v_light_left = m.cell_width.saturating_sub(light_px) / 2;
    let v_light_right = v_light_left + light_px;
    let v_heavy_left = m.cell_width.saturating_sub(heavy_px) / 2;
    let v_heavy_right = v_heavy_left + heavy_px;
    let v_double_left = v_light_left.saturating_sub(light_px);
    let v_double_right = v_light_right + light_px;

    let up_bottom = if lines.left == Heavy || lines.right == Heavy {
        h_heavy_bottom
    } else if lines.left != lines.right || lines.down == lines.up {
        if lines.left == Double || lines.right == Double {
            h_double_bottom
        } else {
            h_light_bottom
        }
    } else if lines.left == NoLine && lines.right == NoLine {
        h_light_bottom
    } else {
        h_light_top
    };

    let down_top = if lines.left == Heavy || lines.right == Heavy {
        h_heavy_top
    } else if lines.left != lines.right || lines.up == lines.down {
        if lines.left == Double || lines.right == Double {
            h_double_top
        } else {
            h_light_top
        }
    } else if lines.left == NoLine && lines.right == NoLine {
        h_light_top
    } else {
        h_light_bottom
    };

    let left_right = if lines.up == Heavy || lines.down == Heavy {
        v_heavy_right
    } else if lines.up != lines.down || lines.left == lines.right {
        if lines.up == Double || lines.down == Double {
            v_double_right
        } else {
            v_light_right
        }
    } else if lines.up == NoLine && lines.down == NoLine {
        v_light_right
    } else {
        v_light_left
    };

    let right_left = if lines.up == Heavy || lines.down == Heavy {
        v_heavy_left
    } else if lines.up != lines.down || lines.right == lines.left {
        if lines.up == Double || lines.down == Double {
            v_double_left
        } else {
            v_light_left
        }
    } else if lines.up == NoLine && lines.down == NoLine {
        v_light_left
    } else {
        v_light_right
    };

    let b = |c: &mut Canvas, x0: u32, y0: u32, x1: u32, y1: u32| {
        c.box_fill(x0 as i32, y0 as i32, x1 as i32, y1 as i32, on);
    };

    match lines.up {
        NoLine => {}
        Light => b(canvas, v_light_left, 0, v_light_right, up_bottom),
        Heavy => b(canvas, v_heavy_left, 0, v_heavy_right, up_bottom),
        Double => {
            let left_bottom = if lines.left == Double {
                h_light_top
            } else {
                up_bottom
            };
            let right_bottom = if lines.right == Double {
                h_light_top
            } else {
                up_bottom
            };
            b(canvas, v_double_left, 0, v_light_left, left_bottom);
            b(canvas, v_light_right, 0, v_double_right, right_bottom);
        }
    }

    match lines.right {
        NoLine => {}
        Light => b(
            canvas,
            right_left,
            h_light_top,
            m.cell_width,
            h_light_bottom,
        ),
        Heavy => b(
            canvas,
            right_left,
            h_heavy_top,
            m.cell_width,
            h_heavy_bottom,
        ),
        Double => {
            let top_left = if lines.up == Double {
                v_light_right
            } else {
                right_left
            };
            let bottom_left = if lines.down == Double {
                v_light_right
            } else {
                right_left
            };
            b(canvas, top_left, h_double_top, m.cell_width, h_light_top);
            b(
                canvas,
                bottom_left,
                h_light_bottom,
                m.cell_width,
                h_double_bottom,
            );
        }
    }

    match lines.down {
        NoLine => {}
        Light => b(canvas, v_light_left, down_top, v_light_right, m.cell_height),
        Heavy => b(canvas, v_heavy_left, down_top, v_heavy_right, m.cell_height),
        Double => {
            let left_top = if lines.left == Double {
                h_light_bottom
            } else {
                down_top
            };
            let right_top = if lines.right == Double {
                h_light_bottom
            } else {
                down_top
            };
            b(canvas, v_double_left, left_top, v_light_left, m.cell_height);
            b(
                canvas,
                v_light_right,
                right_top,
                v_double_right,
                m.cell_height,
            );
        }
    }

    match lines.left {
        NoLine => {}
        Light => b(canvas, 0, h_light_top, left_right, h_light_bottom),
        Heavy => b(canvas, 0, h_heavy_top, left_right, h_heavy_bottom),
        Double => {
            let top_right = if lines.up == Double {
                v_light_left
            } else {
                left_right
            };
            let bottom_right = if lines.down == Double {
                v_light_left
            } else {
                left_right
            };
            b(canvas, 0, h_double_top, top_right, h_light_top);
            b(canvas, 0, h_light_bottom, bottom_right, h_double_bottom);
        }
    }
}

pub(crate) fn light_diagonal_upper_right_to_lower_left(m: &Metrics, canvas: &mut Canvas) {
    let fw = f64::from(m.cell_width);
    let fh = f64::from(m.cell_height);
    let slope_x = (fw / fh).min(1.0);
    let slope_y = (fh / fw).min(1.0);
    canvas.line(
        crate::canvas::Line {
            p0: crate::canvas::Point {
                x: fw + 0.5 * slope_x,
                y: -0.5 * slope_y,
            },
            p1: crate::canvas::Point {
                x: -0.5 * slope_x,
                y: fh + 0.5 * slope_y,
            },
        },
        f64::from(Thickness::Light.height(m.box_thickness)),
        Shade::On as u8,
    );
}

pub(crate) fn light_diagonal_upper_left_to_lower_right(m: &Metrics, canvas: &mut Canvas) {
    let fw = f64::from(m.cell_width);
    let fh = f64::from(m.cell_height);
    let slope_x = (fw / fh).min(1.0);
    let slope_y = (fh / fw).min(1.0);
    canvas.line(
        crate::canvas::Line {
            p0: crate::canvas::Point {
                x: -0.5 * slope_x,
                y: -0.5 * slope_y,
            },
            p1: crate::canvas::Point {
                x: fw + 0.5 * slope_x,
                y: fh + 0.5 * slope_y,
            },
        },
        f64::from(Thickness::Light.height(m.box_thickness)),
        Shade::On as u8,
    );
}

pub(crate) fn light_diagonal_cross(m: &Metrics, canvas: &mut Canvas) {
    light_diagonal_upper_right_to_lower_left(m, canvas);
    light_diagonal_upper_left_to_lower_right(m, canvas);
}

/// Draw a rounded corner arc for `corner`. Shared with the branch glyphs.
pub(crate) fn arc(m: &Metrics, canvas: &mut Canvas, corner: Corner, thickness: Thickness) {
    let thick_px = thickness.height(m.box_thickness);
    let fw = f64::from(m.cell_width);
    let fh = f64::from(m.cell_height);
    let ft = f64::from(thick_px);
    let center_x = f64::from(m.cell_width.saturating_sub(thick_px) / 2) + ft / 2.0;
    let center_y = f64::from(m.cell_height.saturating_sub(thick_px) / 2) + ft / 2.0;
    let r = fw.min(fh) / 2.0;
    // Fraction away from the center to place control points.
    let s = 0.25;

    let mut pb = canvas.path();
    match corner {
        Corner::Tl => {
            pb.move_to(center_x as f32, 0.0);
            pb.line_to(center_x as f32, (center_y - r) as f32);
            pb.cubic_to(
                center_x as f32,
                (center_y - s * r) as f32,
                (center_x - s * r) as f32,
                center_y as f32,
                (center_x - r) as f32,
                center_y as f32,
            );
            pb.line_to(0.0, center_y as f32);
        }
        Corner::Tr => {
            pb.move_to(center_x as f32, 0.0);
            pb.line_to(center_x as f32, (center_y - r) as f32);
            pb.cubic_to(
                center_x as f32,
                (center_y - s * r) as f32,
                (center_x + s * r) as f32,
                center_y as f32,
                (center_x + r) as f32,
                center_y as f32,
            );
            pb.line_to(fw as f32, center_y as f32);
        }
        Corner::Bl => {
            pb.move_to(center_x as f32, fh as f32);
            pb.line_to(center_x as f32, (center_y + r) as f32);
            pb.cubic_to(
                center_x as f32,
                (center_y + s * r) as f32,
                (center_x - s * r) as f32,
                center_y as f32,
                (center_x - r) as f32,
                center_y as f32,
            );
            pb.line_to(0.0, center_y as f32);
        }
        Corner::Br => {
            pb.move_to(center_x as f32, fh as f32);
            pb.line_to(center_x as f32, (center_y + r) as f32);
            pb.cubic_to(
                center_x as f32,
                (center_y + s * r) as f32,
                (center_x + s * r) as f32,
                center_y as f32,
                (center_x + r) as f32,
                center_y as f32,
            );
            pb.line_to(fw as f32, center_y as f32);
        }
    }
    canvas.stroke_path(pb, ft, LineCap::Butt, Shade::On as u8);
}

fn dash_horizontal(m: &Metrics, canvas: &mut Canvas, count: u8, thick_px: u32, desired_gap: u32) {
    let count = i32::from(count);
    let gap_count = count;
    if m.cell_width < (count + gap_count) as u32 {
        hline_middle(m, canvas, Thickness::Light);
        return;
    }
    let gap_width = (desired_gap as i32).min(m.cell_width as i32 / (2 * count));
    let total_gap_width = gap_count * gap_width;
    let total_dash_width = m.cell_width as i32 - total_gap_width;
    let dash_width = total_dash_width.div_euclid(count);
    let remaining = total_dash_width.rem_euclid(count);

    let y = (m.cell_height.saturating_sub(thick_px) / 2) as i32;
    let mut x = gap_width.div_euclid(2);
    let mut extra = remaining;
    for _ in 0..count {
        let mut x1 = x + dash_width;
        if extra > 0 {
            extra -= 1;
            x1 += 1;
        }
        hline(canvas, x, x1, y, thick_px);
        x = x1 + gap_width;
    }
}

fn dash_vertical(m: &Metrics, canvas: &mut Canvas, count: u8, thick_px: u32, desired_gap: u32) {
    let count = i32::from(count);
    let gap_count = count;
    if m.cell_height < (count + gap_count) as u32 {
        vline_middle(m, canvas, Thickness::Light);
        return;
    }
    let gap_height = (desired_gap as i32).min(m.cell_height as i32 / (2 * count));
    let total_gap_height = gap_count * gap_height;
    let total_dash_height = m.cell_height as i32 - total_gap_height;
    let dash_height = total_dash_height.div_euclid(count);
    let remaining = total_dash_height.rem_euclid(count);

    let x = (m.cell_width.saturating_sub(thick_px) / 2) as i32;
    let mut y = 0;
    let mut extra = remaining;
    for _ in 0..count {
        let mut y1 = y + dash_height;
        if extra > 0 {
            extra -= 1;
            y1 += 1;
        }
        vline(canvas, y, y1, x, thick_px);
        y = y1 + gap_height;
    }
}
