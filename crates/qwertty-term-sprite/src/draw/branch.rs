//! Branch Drawing Characters | U+F5D0..U+F60D
//!
//! Ported from `src/font/sprite/draw/branch.zig`. Git-graph glyphs: circular
//! nodes (filled or hollow) with optional stubs to each edge, plus arcs and
//! fading lines. Reuses [`box_drawing::arc`] and the centered line helpers so
//! branch glyphs align with box-drawing characters.

use crate::common::Corner;
use crate::common::{Edge, Shade, Thickness, hline_middle, vline_middle};
use crate::draw::box_drawing::arc;
use crate::{Canvas, Metrics};

/// A branch node: a circle plus optional edge stubs.
#[derive(Debug, Clone, Copy, Default)]
struct BranchNode {
    up: bool,
    right: bool,
    down: bool,
    left: bool,
    filled: bool,
}

pub(crate) fn draw_f5d0_f60d(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    let node = |up, right, down, left, filled| BranchNode {
        up,
        right,
        down,
        left,
        filled,
    };
    match cp {
        0x0f5d0 => hline_middle(m, canvas, Thickness::Light),
        0x0f5d1 => vline_middle(m, canvas, Thickness::Light),
        0x0f5d2 => fading_line(m, canvas, Edge::Right, Thickness::Light),
        0x0f5d3 => fading_line(m, canvas, Edge::Left, Thickness::Light),
        0x0f5d4 => fading_line(m, canvas, Edge::Bottom, Thickness::Light),
        0x0f5d5 => fading_line(m, canvas, Edge::Top, Thickness::Light),
        0x0f5d6 => arc(m, canvas, Corner::Br, Thickness::Light),
        0x0f5d7 => arc(m, canvas, Corner::Bl, Thickness::Light),
        0x0f5d8 => arc(m, canvas, Corner::Tr, Thickness::Light),
        0x0f5d9 => arc(m, canvas, Corner::Tl, Thickness::Light),
        0x0f5da => {
            vline_middle(m, canvas, Thickness::Light);
            arc(m, canvas, Corner::Tr, Thickness::Light);
        }
        0x0f5db => {
            vline_middle(m, canvas, Thickness::Light);
            arc(m, canvas, Corner::Br, Thickness::Light);
        }
        0x0f5dc => {
            arc(m, canvas, Corner::Tr, Thickness::Light);
            arc(m, canvas, Corner::Br, Thickness::Light);
        }
        0x0f5dd => {
            vline_middle(m, canvas, Thickness::Light);
            arc(m, canvas, Corner::Tl, Thickness::Light);
        }
        0x0f5de => {
            vline_middle(m, canvas, Thickness::Light);
            arc(m, canvas, Corner::Bl, Thickness::Light);
        }
        0x0f5df => {
            arc(m, canvas, Corner::Tl, Thickness::Light);
            arc(m, canvas, Corner::Bl, Thickness::Light);
        }
        0x0f5e0 => {
            arc(m, canvas, Corner::Bl, Thickness::Light);
            hline_middle(m, canvas, Thickness::Light);
        }
        0x0f5e1 => {
            arc(m, canvas, Corner::Br, Thickness::Light);
            hline_middle(m, canvas, Thickness::Light);
        }
        0x0f5e2 => {
            arc(m, canvas, Corner::Br, Thickness::Light);
            arc(m, canvas, Corner::Bl, Thickness::Light);
        }
        0x0f5e3 => {
            arc(m, canvas, Corner::Tl, Thickness::Light);
            hline_middle(m, canvas, Thickness::Light);
        }
        0x0f5e4 => {
            arc(m, canvas, Corner::Tr, Thickness::Light);
            hline_middle(m, canvas, Thickness::Light);
        }
        0x0f5e5 => {
            arc(m, canvas, Corner::Tr, Thickness::Light);
            arc(m, canvas, Corner::Tl, Thickness::Light);
        }
        0x0f5e6 => {
            vline_middle(m, canvas, Thickness::Light);
            arc(m, canvas, Corner::Tl, Thickness::Light);
            arc(m, canvas, Corner::Tr, Thickness::Light);
        }
        0x0f5e7 => {
            vline_middle(m, canvas, Thickness::Light);
            arc(m, canvas, Corner::Bl, Thickness::Light);
            arc(m, canvas, Corner::Br, Thickness::Light);
        }
        0x0f5e8 => {
            hline_middle(m, canvas, Thickness::Light);
            arc(m, canvas, Corner::Bl, Thickness::Light);
            arc(m, canvas, Corner::Tl, Thickness::Light);
        }
        0x0f5e9 => {
            hline_middle(m, canvas, Thickness::Light);
            arc(m, canvas, Corner::Tr, Thickness::Light);
            arc(m, canvas, Corner::Br, Thickness::Light);
        }
        0x0f5ea => {
            vline_middle(m, canvas, Thickness::Light);
            arc(m, canvas, Corner::Tl, Thickness::Light);
            arc(m, canvas, Corner::Br, Thickness::Light);
        }
        0x0f5eb => {
            vline_middle(m, canvas, Thickness::Light);
            arc(m, canvas, Corner::Tr, Thickness::Light);
            arc(m, canvas, Corner::Bl, Thickness::Light);
        }
        0x0f5ec => {
            hline_middle(m, canvas, Thickness::Light);
            arc(m, canvas, Corner::Tl, Thickness::Light);
            arc(m, canvas, Corner::Br, Thickness::Light);
        }
        0x0f5ed => {
            hline_middle(m, canvas, Thickness::Light);
            arc(m, canvas, Corner::Tr, Thickness::Light);
            arc(m, canvas, Corner::Bl, Thickness::Light);
        }
        0x0f5ee => branch_node(m, canvas, node(false, false, false, false, true)),
        0x0f5ef => branch_node(m, canvas, node(false, false, false, false, false)),

        0x0f5f0 => branch_node(m, canvas, node(false, true, false, false, true)),
        0x0f5f1 => branch_node(m, canvas, node(false, true, false, false, false)),
        0x0f5f2 => branch_node(m, canvas, node(false, false, false, true, true)),
        0x0f5f3 => branch_node(m, canvas, node(false, false, false, true, false)),
        0x0f5f4 => branch_node(m, canvas, node(false, true, false, true, true)),
        0x0f5f5 => branch_node(m, canvas, node(false, true, false, true, false)),
        0x0f5f6 => branch_node(m, canvas, node(false, false, true, false, true)),
        0x0f5f7 => branch_node(m, canvas, node(false, false, true, false, false)),
        0x0f5f8 => branch_node(m, canvas, node(true, false, false, false, true)),
        0x0f5f9 => branch_node(m, canvas, node(true, false, false, false, false)),
        0x0f5fa => branch_node(m, canvas, node(true, false, true, false, true)),
        0x0f5fb => branch_node(m, canvas, node(true, false, true, false, false)),
        0x0f5fc => branch_node(m, canvas, node(false, true, true, false, true)),
        0x0f5fd => branch_node(m, canvas, node(false, true, true, false, false)),
        0x0f5fe => branch_node(m, canvas, node(false, false, true, true, true)),
        0x0f5ff => branch_node(m, canvas, node(false, false, true, true, false)),

        0x0f600 => branch_node(m, canvas, node(true, true, false, false, true)),
        0x0f601 => branch_node(m, canvas, node(true, true, false, false, false)),
        0x0f602 => branch_node(m, canvas, node(true, false, false, true, true)),
        0x0f603 => branch_node(m, canvas, node(true, false, false, true, false)),
        0x0f604 => branch_node(m, canvas, node(true, true, true, false, true)),
        0x0f605 => branch_node(m, canvas, node(true, true, true, false, false)),
        0x0f606 => branch_node(m, canvas, node(true, false, true, true, true)),
        0x0f607 => branch_node(m, canvas, node(true, false, true, true, false)),
        0x0f608 => branch_node(m, canvas, node(false, true, true, true, true)),
        0x0f609 => branch_node(m, canvas, node(false, true, true, true, false)),
        0x0f60a => branch_node(m, canvas, node(true, true, false, true, true)),
        0x0f60b => branch_node(m, canvas, node(true, true, false, true, false)),
        0x0f60c => branch_node(m, canvas, node(true, true, true, true, true)),
        0x0f60d => branch_node(m, canvas, node(true, true, true, true, false)),

        _ => {}
    }
}

fn branch_node(m: &Metrics, canvas: &mut Canvas, node: BranchNode) {
    let on = Shade::On as u8;
    let thick_px = Thickness::Light.height(m.box_thickness);
    let fw = f64::from(m.cell_width);
    let fh = f64::from(m.cell_height);
    let ft = f64::from(thick_px);

    let h_top = m.cell_height.saturating_sub(thick_px) / 2;
    let h_bottom = h_top + thick_px;
    let v_left = m.cell_width.saturating_sub(thick_px) / 2;
    let v_right = v_left + thick_px;

    // Center chosen to align with box-drawing chars (lines are sometimes off
    // center to avoid splitting a pixel).
    let cx = f64::from(v_left) + ft / 2.0;
    let cy = f64::from(h_top) + ft / 2.0;
    let r = (cx.min(cy)).min((fw - cx).min(fh - cy));

    if node.up {
        canvas.box_fill(
            v_left as i32,
            0,
            v_right as i32,
            (cy - r + ft / 2.0).ceil() as i32,
            on,
        );
    }
    if node.right {
        canvas.box_fill(
            (cx + r - ft / 2.0).floor() as i32,
            h_top as i32,
            m.cell_width as i32,
            h_bottom as i32,
            on,
        );
    }
    if node.down {
        canvas.box_fill(
            v_left as i32,
            (cy + r - ft / 2.0).floor() as i32,
            v_right as i32,
            m.cell_height as i32,
            on,
        );
    }
    if node.left {
        canvas.box_fill(
            0,
            h_top as i32,
            (cx - r + ft / 2.0).ceil() as i32,
            h_bottom as i32,
            on,
        );
    }

    let mut ctx = canvas.context();
    ctx.line_width = ft;
    if node.filled {
        ctx.circle(cx, cy, r);
        ctx.fill(on);
    } else {
        ctx.circle(cx, cy, r - ft / 2.0);
        ctx.stroke(on);
    }
}

fn fading_line(m: &Metrics, canvas: &mut Canvas, to: Edge, thickness: Thickness) {
    let thick_px = thickness.height(m.box_thickness);
    let fw = f64::from(m.cell_width);
    let fh = f64::from(m.cell_height);

    let h_top = m.cell_height.saturating_sub(thick_px) / 2;
    let h_bottom = h_top + thick_px;
    let v_left = m.cell_width.saturating_sub(thick_px) / 2;
    let v_right = v_left + thick_px;

    let mut color: f64 = match to {
        Edge::Top | Edge::Left => 0.0,
        Edge::Bottom | Edge::Right => 255.0,
    };
    let inc: f64 = 255.0
        / match to {
            Edge::Top => fh,
            Edge::Bottom => -fh,
            Edge::Left => fw,
            Edge::Right => -fw,
        };

    match to {
        Edge::Top | Edge::Bottom => {
            for y in 0..m.cell_height {
                for x in v_left..v_right {
                    canvas.pixel(x as i32, y as i32, color.round() as u8);
                }
                color += inc;
            }
        }
        Edge::Left | Edge::Right => {
            for x in 0..m.cell_width {
                for y in h_top..h_bottom {
                    canvas.pixel(x as i32, y as i32, color.round() as u8);
                }
                color += inc;
            }
        }
    }
}
