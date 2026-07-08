//! Braille Patterns | U+2800..U+28FF
//!
//! Ported from `src/font/sprite/draw/braille.zig`. The five-pass dot-sizing
//! algorithm below is transcribed exactly: it greedily grows dot width, then
//! margins, then spacing, then more margins, then dot width again, so dots stay
//! crisp and evenly spread at any cell size.

use crate::common::Shade;
use crate::{Canvas, Metrics};

pub(crate) fn draw2800_28ff(cp: u32, canvas: &mut Canvas, width: u32, height: u32, _m: &Metrics) {
    let on = Shade::On as u8;

    let mut w = (width / 4).min(height / 8) as i32;
    let mut x_spacing = (width / 4) as i32;
    let mut y_spacing = (height / 8) as i32;
    let mut x_margin = x_spacing.div_euclid(2);
    let mut y_margin = y_spacing.div_euclid(2);

    let mut x_px_left = width as i32 - 2 * x_margin - x_spacing - 2 * w;
    let mut y_px_left = height as i32 - 2 * y_margin - 3 * y_spacing - 4 * w;

    // 1: ensure the dot width is non-zero.
    if x_px_left >= 2 && y_px_left >= 4 && w == 0 {
        w += 1;
        x_px_left -= 2;
        y_px_left -= 4;
    }
    // 2: prefer a non-zero margin.
    if x_px_left >= 2 && x_margin == 0 {
        x_margin = 1;
        x_px_left -= 2;
    }
    if y_px_left >= 2 && y_margin == 0 {
        y_margin = 1;
        y_px_left -= 2;
    }
    // 3: increase spacing.
    if x_px_left >= 1 {
        x_spacing += 1;
        x_px_left -= 1;
    }
    if y_px_left >= 3 {
        y_spacing += 1;
        y_px_left -= 3;
    }
    // 4: margins ("spacing", but on the sides).
    if x_px_left >= 2 {
        x_margin += 1;
        x_px_left -= 2;
    }
    if y_px_left >= 2 {
        y_margin += 1;
        y_px_left -= 2;
    }
    // 5: increase dot width.
    if x_px_left >= 2 && y_px_left >= 4 {
        w += 1;
        x_px_left -= 2;
        y_px_left -= 4;
    }

    // Same invariants the Zig source asserts once the greedy passes settle.
    debug_assert!(x_px_left <= 1 || y_px_left <= 1);
    debug_assert!(2 * x_margin + 2 * w + x_spacing <= width as i32);
    debug_assert!(2 * y_margin + 4 * w + 3 * y_spacing <= height as i32);

    let x = [x_margin, x_margin + w + x_spacing];
    let y = {
        let mut y = [0i32; 4];
        y[0] = y_margin;
        y[1] = y[0] + w + y_spacing;
        y[2] = y[1] + w + y_spacing;
        y[3] = y[2] + w + y_spacing;
        y
    };

    // The low byte of the codepoint is the dot bit pattern. Bit order matches
    // the packed struct: tl, ul, ll, tr, ur, lr, bl, br.
    let bits = (cp & 0xff) as u8;
    let dot = |c: &mut Canvas, xi: usize, yi: usize| {
        c.box_fill(x[xi], y[yi], x[xi] + w, y[yi] + w, on);
    };
    if bits & 0b0000_0001 != 0 {
        dot(canvas, 0, 0);
    } // tl
    if bits & 0b0000_0010 != 0 {
        dot(canvas, 0, 1);
    } // ul
    if bits & 0b0000_0100 != 0 {
        dot(canvas, 0, 2);
    } // ll
    if bits & 0b0000_1000 != 0 {
        dot(canvas, 1, 0);
    } // tr
    if bits & 0b0001_0000 != 0 {
        dot(canvas, 1, 1);
    } // ur
    if bits & 0b0010_0000 != 0 {
        dot(canvas, 1, 2);
    } // lr
    if bits & 0b0100_0000 != 0 {
        dot(canvas, 0, 3);
    } // bl
    if bits & 0b1000_0000 != 0 {
        dot(canvas, 1, 3);
    } // br
}
