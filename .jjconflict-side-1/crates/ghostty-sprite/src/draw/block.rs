//! Block Elements | U+2580..U+259F
//!
//! Ported from `src/font/sprite/draw/block.zig`. [`block`], [`block_shade`],
//! and [`full_block_shade`] are reused by the legacy-computing glyphs.

use crate::common::{Alignment, Horizontal, Quads, Shade, Vertical, fill};
use crate::{Canvas, Metrics};

const ONE_EIGHTH: f64 = 0.125;
const ONE_QUARTER: f64 = 0.25;
const THREE_EIGHTHS: f64 = 0.375;
const HALF: f64 = 0.5;
const FIVE_EIGHTHS: f64 = 0.625;
const THREE_QUARTERS: f64 = 0.75;
const SEVEN_EIGHTHS: f64 = 0.875;

pub(crate) fn draw2580_259f(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &Metrics) {
    match cp {
        0x2580 => block(m, canvas, Alignment::UPPER, 1.0, HALF),
        0x2581 => block(m, canvas, Alignment::LOWER, 1.0, ONE_EIGHTH),
        0x2582 => block(m, canvas, Alignment::LOWER, 1.0, ONE_QUARTER),
        0x2583 => block(m, canvas, Alignment::LOWER, 1.0, THREE_EIGHTHS),
        0x2584 => block(m, canvas, Alignment::LOWER, 1.0, HALF),
        0x2585 => block(m, canvas, Alignment::LOWER, 1.0, FIVE_EIGHTHS),
        0x2586 => block(m, canvas, Alignment::LOWER, 1.0, THREE_QUARTERS),
        0x2587 => block(m, canvas, Alignment::LOWER, 1.0, SEVEN_EIGHTHS),
        0x2588 => full_block_shade(m, canvas, Shade::On),
        0x2589 => block(m, canvas, Alignment::LEFT, SEVEN_EIGHTHS, 1.0),
        0x258a => block(m, canvas, Alignment::LEFT, THREE_QUARTERS, 1.0),
        0x258b => block(m, canvas, Alignment::LEFT, FIVE_EIGHTHS, 1.0),
        0x258c => block(m, canvas, Alignment::LEFT, HALF, 1.0),
        0x258d => block(m, canvas, Alignment::LEFT, THREE_EIGHTHS, 1.0),
        0x258e => block(m, canvas, Alignment::LEFT, ONE_QUARTER, 1.0),
        0x258f => block(m, canvas, Alignment::LEFT, ONE_EIGHTH, 1.0),

        0x2590 => block(m, canvas, Alignment::RIGHT, HALF, 1.0),
        0x2591 => full_block_shade(m, canvas, Shade::Light),
        0x2592 => full_block_shade(m, canvas, Shade::Medium),
        0x2593 => full_block_shade(m, canvas, Shade::Dark),
        0x2594 => block(m, canvas, Alignment::UPPER, 1.0, ONE_EIGHTH),
        0x2595 => block(m, canvas, Alignment::RIGHT, ONE_EIGHTH, 1.0),
        0x2596 => quadrant(
            m,
            canvas,
            Quads {
                bl: true,
                ..Quads::default()
            },
        ),
        0x2597 => quadrant(
            m,
            canvas,
            Quads {
                br: true,
                ..Quads::default()
            },
        ),
        0x2598 => quadrant(
            m,
            canvas,
            Quads {
                tl: true,
                ..Quads::default()
            },
        ),
        0x2599 => quadrant(
            m,
            canvas,
            Quads {
                tl: true,
                bl: true,
                br: true,
                ..Quads::default()
            },
        ),
        0x259a => quadrant(
            m,
            canvas,
            Quads {
                tl: true,
                br: true,
                ..Quads::default()
            },
        ),
        0x259b => quadrant(
            m,
            canvas,
            Quads {
                tl: true,
                tr: true,
                bl: true,
                ..Quads::default()
            },
        ),
        0x259c => quadrant(
            m,
            canvas,
            Quads {
                tl: true,
                tr: true,
                br: true,
                ..Quads::default()
            },
        ),
        0x259d => quadrant(
            m,
            canvas,
            Quads {
                tr: true,
                ..Quads::default()
            },
        ),
        0x259e => quadrant(
            m,
            canvas,
            Quads {
                tr: true,
                bl: true,
                ..Quads::default()
            },
        ),
        0x259f => quadrant(
            m,
            canvas,
            Quads {
                tr: true,
                bl: true,
                br: true,
                ..Quads::default()
            },
        ),

        _ => {}
    }
}

/// Fully opaque block aligned within the cell, sized as a fraction of the cell.
pub(crate) fn block(
    m: &Metrics,
    canvas: &mut Canvas,
    alignment: Alignment,
    width: f64,
    height: f64,
) {
    block_shade(m, canvas, alignment, width, height, Shade::On);
}

/// Shaded block aligned within the cell, sized as a fraction of the cell.
pub(crate) fn block_shade(
    m: &Metrics,
    canvas: &mut Canvas,
    alignment: Alignment,
    width: f64,
    height: f64,
    shade: Shade,
) {
    let fw = f64::from(m.cell_width);
    let fh = f64::from(m.cell_height);
    let w = (fw * width).round() as u32;
    let h = (fh * height).round() as u32;

    let x = match alignment.horizontal {
        Horizontal::Left => 0,
        Horizontal::Right => m.cell_width - w,
        Horizontal::Center => (m.cell_width - w) / 2,
    };
    let y = match alignment.vertical {
        Vertical::Top => 0,
        Vertical::Bottom => m.cell_height - h,
        Vertical::Middle => (m.cell_height - h) / 2,
    };

    canvas.rect(x as i32, y as i32, w as i32, h as i32, shade as u8);
}

/// Fill the entire cell with a shade.
pub(crate) fn full_block_shade(m: &Metrics, canvas: &mut Canvas, shade: Shade) {
    canvas.box_fill(0, 0, m.cell_width as i32, m.cell_height as i32, shade as u8);
}

fn quadrant(m: &Metrics, canvas: &mut Canvas, quads: Quads) {
    use crate::common::Fraction as F;
    if quads.tl {
        fill(m, canvas, F::Zero, F::Half, F::Zero, F::Half);
    }
    if quads.tr {
        fill(m, canvas, F::Half, F::One, F::Zero, F::Half);
    }
    if quads.bl {
        fill(m, canvas, F::Zero, F::Half, F::Half, F::One);
    }
    if quads.br {
        fill(m, canvas, F::Half, F::One, F::Half, F::One);
    }
}
