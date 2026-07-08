//! Special (non-Unicode) sprites: text decorations and cursors.
//!
//! Ported from `src/font/sprite/draw/special.zig`. Dispatched by the
//! [`Sprite`](crate::Sprite) pseudo-codepoints rather than by Unicode range.

use tiny_skia::LineCap;

use crate::common::Shade;
use crate::sprite::Sprite;
use crate::{Canvas, Metrics};

/// Draw the special sprite for `cp` (must be a [`Sprite`] pseudo-codepoint).
pub(crate) fn draw(cp: u32, canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let Some(sprite) = Sprite::from_codepoint(cp) else {
        return;
    };
    match sprite {
        Sprite::Underline => underline(canvas, width, height, m),
        Sprite::UnderlineDouble => underline_double(canvas, width, height, m),
        Sprite::UnderlineDotted => underline_dotted(canvas, width, height, m),
        Sprite::UnderlineDashed => underline_dashed(canvas, width, height, m),
        Sprite::UnderlineCurly => underline_curly(canvas, width, height, m),
        Sprite::Strikethrough => strikethrough(canvas, width, m),
        Sprite::Overline => overline(canvas, width, m),
        Sprite::CursorRect => cursor_rect(canvas, width, height),
        Sprite::CursorHollowRect => cursor_hollow_rect(canvas, width, height, m),
        Sprite::CursorBar => cursor_bar(canvas, height, m),
        Sprite::CursorUnderline => cursor_underline(canvas, width, height, m),
    }
}

fn underline(canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let y = m
        .underline_position
        .min(height + canvas.padding_y() - m.underline_thickness);
    canvas.rect(
        0,
        y as i32,
        width as i32,
        m.underline_thickness as i32,
        Shade::On as u8,
    );
}

fn underline_double(canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let y = m
        .underline_position
        .min(height + canvas.padding_y() - 2 * m.underline_thickness);
    canvas.rect(
        0,
        y.saturating_sub(m.underline_thickness) as i32,
        width as i32,
        m.underline_thickness as i32,
        Shade::On as u8,
    );
    canvas.rect(
        0,
        (y + m.underline_thickness) as i32,
        width as i32,
        m.underline_thickness as i32,
        Shade::On as u8,
    );
}

fn underline_dotted(canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let fw = f64::from(width);
    let fh = f64::from(height);
    let float_pos = f64::from(m.underline_position);
    let float_thick = f64::from(m.underline_thickness);

    // Diameter is sqrt(1/2) * usual thickness, otherwise dotted looks anemic.
    let radius = std::f64::consts::FRAC_1_SQRT_2 * float_thick;
    let padding = f64::from(canvas.padding_y());
    let y = (float_pos + 0.5 * float_thick).min(fh + padding - radius.ceil());

    let dot_count = (fw / (4.0 * radius))
        .ceil()
        .min((fw / (3.0 * radius)).floor())
        .min((fw / (2.0 * radius + 1.0)).floor())
        .max(1.0);

    let mut x = (fw / dot_count) / 2.0;
    let mut ctx = canvas.context();
    for _ in 0..(dot_count as usize) {
        ctx.circle(x, y, radius);
        x += fw / dot_count;
    }
    ctx.fill(Shade::On as u8);
}

fn underline_dashed(canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let y = m
        .underline_position
        .min(height + canvas.padding_y() - m.underline_thickness);
    let dash_width = width / 3 + 1;
    let dash_count = (width / dash_width) + 1;
    let mut i = 0;
    while i < dash_count {
        let x = i * dash_width;
        canvas.rect(
            x as i32,
            y as i32,
            dash_width as i32,
            m.underline_thickness as i32,
            Shade::On as u8,
        );
        i += 2;
    }
}

fn underline_curly(canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let fw = f64::from(width);
    let fh = f64::from(height);
    let float_pos = f64::from(m.underline_position);
    let line_width = f64::from(m.underline_thickness);

    let amplitude = fw / std::f64::consts::PI;
    let padding = f64::from(canvas.padding_y());
    let top = float_pos.min(fh + padding - amplitude - line_width);
    let bottom = top + amplitude;

    // Curvature multiplier (0.4 gives a smooth wiggle).
    let r = 0.4;
    let center = 0.5 * fw;

    let mut ctx = canvas.context();
    ctx.line_width = line_width;
    ctx.line_cap = LineCap::Round;
    ctx.move_to(0.0, bottom);
    ctx.curve_to(center * r, bottom, center - center * r, top, center, top);
    ctx.curve_to(
        center + center * r,
        top,
        fw - center * r,
        bottom,
        fw,
        bottom,
    );
    ctx.stroke(Shade::On as u8);
}

fn strikethrough(canvas: &mut Canvas, width: u32, m: &Metrics) {
    canvas.rect(
        0,
        m.strikethrough_position as i32,
        width as i32,
        m.strikethrough_thickness as i32,
        Shade::On as u8,
    );
}

fn overline(canvas: &mut Canvas, width: u32, m: &Metrics) {
    let y = m.overline_position.max(-(canvas.padding_y() as i32));
    canvas.rect(
        0,
        y,
        width as i32,
        m.overline_thickness as i32,
        Shade::On as u8,
    );
}

fn cursor_rect(canvas: &mut Canvas, width: u32, height: u32) {
    canvas.rect(0, 0, width as i32, height as i32, Shade::On as u8);
}

fn cursor_hollow_rect(canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    canvas.rect(0, 0, width as i32, height as i32, Shade::On as u8);
    canvas.rect(
        m.cursor_thickness as i32,
        m.cursor_thickness as i32,
        width.saturating_sub(m.cursor_thickness * 2) as i32,
        height.saturating_sub(m.cursor_thickness * 2) as i32,
        Shade::Off as u8,
    );
}

fn cursor_bar(canvas: &mut Canvas, height: u32, m: &Metrics) {
    // Half its thickness over the left edge so it sits centered between chars.
    // Round up (Zig `(t + 1) / 2`); kept explicit to match that rounding.
    #[allow(clippy::manual_div_ceil)]
    let offset = ((m.cursor_thickness + 1) / 2) as i32;
    canvas.rect(
        -offset,
        0,
        m.cursor_thickness as i32,
        height as i32,
        Shade::On as u8,
    );
}

fn cursor_underline(canvas: &mut Canvas, width: u32, height: u32, m: &Metrics) {
    let y = m
        .underline_position
        .min(height + canvas.padding_y() - m.underline_thickness);
    canvas.rect(
        0,
        y as i32,
        width as i32,
        m.cursor_thickness as i32,
        Shade::On as u8,
    );
}
