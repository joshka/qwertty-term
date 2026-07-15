//! The drawing canvas: a padded 8-bit alpha coverage buffer with primitives
//! for rects, paths, strokes, and the per-pixel compositing tricks the sprite
//! glyphs need.
//!
//! Ported from `src/font/sprite/canvas.zig`. Upstream builds on the `z2d`
//! vector library; here we use [`tiny_skia`] for path fill/stroke and operate
//! directly on the alpha buffer for axis-aligned rects, single pixels, and the
//! manual composite operations (`innerStrokePath`'s multiply, `invert`, the
//! flips) — mirroring the fact that upstream also bypasses z2d for those "for
//! performance".
//!
//! # Backend note
//!
//! tiny-skia renders into a premultiplied RGBA [`tiny_skia::Pixmap`]. We paint
//! every path in opaque white and then read back the alpha (equivalently, any
//! channel) into our own alpha8 buffer, compositing `src_over` onto whatever is
//! already there. This keeps a single source of truth (the alpha buffer) that
//! `trim`/`invert`/`flip`/atlas-extraction all operate on, matching the Zig
//! design where the z2d surface *is* the alpha8 buffer.

use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::{Glyph, Metrics};

/// A point in 2D space.
#[derive(Debug, Clone, Copy)]
pub struct Point {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
}

/// A line segment.
#[derive(Debug, Clone, Copy)]
pub struct Line {
    /// Start point.
    pub p0: Point,
    /// End point.
    pub p1: Point,
}

/// A triangle.
#[derive(Debug, Clone, Copy)]
pub struct Triangle {
    /// First vertex.
    pub p0: Point,
    /// Second vertex.
    pub p1: Point,
    /// Third vertex.
    pub p2: Point,
}

/// A quadrilateral.
#[derive(Debug, Clone, Copy)]
pub struct Quad {
    /// First vertex.
    pub p0: Point,
    /// Second vertex.
    pub p1: Point,
    /// Third vertex.
    pub p2: Point,
    /// Fourth vertex.
    pub p3: Point,
}

/// The drawing canvas.
///
/// Coordinates passed to drawing methods are relative to the cell's top-left;
/// the canvas adds `padding_x`/`padding_y` internally so glyphs may extend a
/// quarter-cell beyond the cell in any direction (used by decorations and
/// overshooting diagonals).
pub struct Canvas {
    /// Full buffer width including padding on both sides.
    buf_width: u32,
    /// Full buffer height including padding on both sides.
    buf_height: u32,
    /// Horizontal padding added on each side.
    padding_x: u32,
    /// Vertical padding added on each side.
    padding_y: u32,
    /// Row-major alpha coverage, `buf_width * buf_height` bytes.
    buf: Vec<u8>,

    /// Clip margins, trimmed automatically before atlas extraction. Also set
    /// explicitly by a few glyphs to exclude the padding region.
    clip_top: u32,
    clip_left: u32,
    clip_right: u32,
    clip_bottom: u32,
}

impl Canvas {
    /// Create a canvas for a cell of `width` x `height`, padded on every side.
    #[must_use]
    pub fn new(width: u32, height: u32, padding_x: u32, padding_y: u32) -> Self {
        let buf_width = width + 2 * padding_x;
        let buf_height = height + 2 * padding_y;
        Self {
            buf_width,
            buf_height,
            padding_x,
            padding_y,
            buf: vec![0u8; (buf_width * buf_height) as usize],
            clip_top: 0,
            clip_left: 0,
            clip_right: 0,
            clip_bottom: 0,
        }
    }

    /// Horizontal padding.
    #[must_use]
    pub fn padding_x(&self) -> u32 {
        self.padding_x
    }

    /// Vertical padding.
    #[must_use]
    pub fn padding_y(&self) -> u32 {
        self.padding_y
    }

    /// Set the clip margins to exactly exclude the padding region (i.e. clip to
    /// the cell). Several legacy-computing glyphs call this so overshoot from
    /// paths doesn't leak into adjacent cells.
    pub fn clip_to_cell(&mut self) {
        self.clip_left = self.padding_x;
        self.clip_right = self.padding_x;
        self.clip_top = self.padding_y;
        self.clip_bottom = self.padding_y;
    }

    /// Set a single pixel (cell-relative coordinates) to `alpha`, replacing
    /// whatever was there (matches z2d `putPixel`).
    pub fn pixel(&mut self, x: i32, y: i32, alpha: u8) {
        let px = x + self.padding_x as i32;
        let py = y + self.padding_y as i32;
        if px < 0 || py < 0 || px >= self.buf_width as i32 || py >= self.buf_height as i32 {
            return;
        }
        let idx = py as usize * self.buf_width as usize + px as usize;
        self.buf[idx] = alpha;
    }

    /// Fill an axis-aligned rectangle with `alpha` (replacing existing values,
    /// matching z2d `Canvas.rect` which writes each pixel directly).
    pub fn rect(&mut self, x: i32, y: i32, width: i32, height: i32, alpha: u8) {
        let mut yy = y;
        while yy < y + height {
            let mut xx = x;
            while xx < x + width {
                self.pixel(xx, yy, alpha);
                xx += 1;
            }
            yy += 1;
        }
    }

    /// Fill the box between the two corner points (order-independent).
    pub fn box_fill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, alpha: u8) {
        let tl_x = x0.min(x1);
        let tl_y = y0.min(y1);
        let br_x = x0.max(x1);
        let br_y = y0.max(y1);
        self.rect(tl_x, tl_y, br_x - tl_x, br_y - tl_y, alpha);
    }

    // --- path primitives (tiny-skia backed) ---

    /// Fill a triangle.
    pub fn triangle(&mut self, t: Triangle, alpha: u8) {
        let mut pb = PathBuilder::new();
        pb.move_to(t.p0.x as f32, t.p0.y as f32);
        pb.line_to(t.p1.x as f32, t.p1.y as f32);
        pb.line_to(t.p2.x as f32, t.p2.y as f32);
        pb.close();
        self.fill_builder(pb, alpha);
    }

    /// Fill a quadrilateral.
    pub fn quad(&mut self, q: Quad, alpha: u8) {
        let mut pb = PathBuilder::new();
        pb.move_to(q.p0.x as f32, q.p0.y as f32);
        pb.line_to(q.p1.x as f32, q.p1.y as f32);
        pb.line_to(q.p2.x as f32, q.p2.y as f32);
        pb.line_to(q.p3.x as f32, q.p3.y as f32);
        pb.close();
        self.fill_builder(pb, alpha);
    }

    /// Stroke a single line with butt caps.
    pub fn line(&mut self, l: Line, thickness: f64, alpha: u8) {
        let mut pb = PathBuilder::new();
        pb.move_to(l.p0.x as f32, l.p0.y as f32);
        pb.line_to(l.p1.x as f32, l.p1.y as f32);
        self.stroke_builder(pb, thickness, LineCap::Butt, alpha);
    }

    /// Start building a path with cell-relative coordinates. Fill or stroke it
    /// via [`Canvas::fill_path`] / [`Canvas::stroke_path`] /
    /// [`Canvas::inner_stroke_path`].
    #[must_use]
    pub fn path(&self) -> PathBuilder {
        PathBuilder::new()
    }

    /// Fill a built path with the non-zero winding rule.
    pub fn fill_path(&mut self, pb: PathBuilder, alpha: u8) {
        self.fill_builder(pb, alpha);
    }

    /// Stroke a built path.
    pub fn stroke_path(&mut self, pb: PathBuilder, thickness: f64, cap: LineCap, alpha: u8) {
        self.stroke_builder(pb, thickness, cap, alpha);
    }

    fn transform(&self) -> Transform {
        Transform::from_translate(self.padding_x as f32, self.padding_y as f32)
    }

    fn fill_builder(&mut self, pb: PathBuilder, alpha: u8) {
        let Some(path) = pb.finish() else { return };
        let mut pm = Pixmap::new(self.buf_width, self.buf_height).expect("nonzero canvas size");
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        paint.anti_alias = true;
        pm.fill_path(&path, &paint, FillRule::Winding, self.transform(), None);
        self.composite_pixmap(&pm, alpha);
    }

    fn stroke_builder(&mut self, pb: PathBuilder, thickness: f64, cap: LineCap, alpha: u8) {
        let Some(path) = pb.finish() else { return };
        let mut pm = Pixmap::new(self.buf_width, self.buf_height).expect("nonzero canvas size");
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        paint.anti_alias = true;
        let stroke = Stroke {
            width: thickness as f32,
            line_cap: cap,
            ..Stroke::default()
        };
        pm.stroke_path(&path, &paint, &stroke, self.transform(), None);
        self.composite_pixmap(&pm, alpha);
    }

    /// The `innerStrokePath` dual-surface multiply trick: fill a closed copy of
    /// the path in white on one surface, stroke the (open) path at *double*
    /// width on another, multiply them per-pixel so only the half of the stroke
    /// inside the fill survives, then composite that onto the main buffer.
    ///
    /// This yields an "inner stroke" (a stroke that stays inside the shape's
    /// outline) which z2d/tiny-skia don't offer natively.
    pub fn inner_stroke_path(&mut self, pb: PathBuilder, thickness: f64, cap: LineCap, alpha: u8) {
        let Some(open_path) = pb.finish() else { return };

        // Fill mask: filling an open path implicitly closes it, giving us the
        // closed-path fill the Zig code builds explicitly.
        let mut fill_pm = Pixmap::new(self.buf_width, self.buf_height).expect("nonzero size");
        let mut mask_paint = Paint::default();
        mask_paint.set_color_rgba8(255, 255, 255, 255);
        mask_paint.anti_alias = true;
        fill_pm.fill_path(
            &open_path,
            &mask_paint,
            FillRule::Winding,
            self.transform(),
            None,
        );

        let mut stroke_pm = Pixmap::new(self.buf_width, self.buf_height).expect("nonzero size");
        let mut stroke_paint = Paint::default();
        stroke_paint.set_color_rgba8(255, 255, 255, 255);
        stroke_paint.anti_alias = true;
        let stroke = Stroke {
            width: (thickness * 2.0) as f32,
            line_cap: cap,
            ..Stroke::default()
        };
        stroke_pm.stroke_path(&open_path, &stroke_paint, &stroke, self.transform(), None);

        // Multiply the stroke coverage by the fill mask, per pixel, then
        // composite the result (scaled by `alpha`) onto our buffer.
        let fill_px = fill_pm.data();
        let stroke_px = stroke_pm.data();
        let a_frac = f64::from(alpha) / 255.0;
        for i in 0..(self.buf_width * self.buf_height) as usize {
            // Premultiplied white: alpha == any color channel; read alpha byte.
            let f = f64::from(fill_px[i * 4 + 3]) / 255.0;
            let s = f64::from(stroke_px[i * 4 + 3]) / 255.0;
            let coverage = (255.0 * s * f * a_frac).round() as u8;
            self.buf[i] = src_over(self.buf[i], coverage);
        }
    }

    /// Composite a tiny-skia pixmap's alpha (scaled by `alpha`) onto our buffer
    /// with `src_over`.
    fn composite_pixmap(&mut self, pm: &Pixmap, alpha: u8) {
        let px = pm.data();
        let a_frac = f64::from(alpha) / 255.0;
        for i in 0..(self.buf_width * self.buf_height) as usize {
            let cov = px[i * 4 + 3];
            if cov == 0 {
                continue;
            }
            let scaled = if alpha == 255 {
                cov
            } else {
                (f64::from(cov) * a_frac).round() as u8
            };
            self.buf[i] = src_over(self.buf[i], scaled);
        }
    }

    // --- whole-buffer transforms ---

    /// Invert every pixel (`v -> 255 - v`).
    pub fn invert(&mut self) {
        for v in &mut self.buf {
            *v = 255 - *v;
        }
    }

    /// Mirror the buffer horizontally, swapping the left/right clip margins.
    pub fn flip_horizontal(&mut self) {
        let w = self.buf_width as usize;
        let h = self.buf_height as usize;
        let clone = self.buf.clone();
        for y in 0..h {
            for x in 0..w {
                self.buf[y * w + x] = clone[y * w + (w - x - 1)];
            }
        }
        std::mem::swap(&mut self.clip_left, &mut self.clip_right);
    }

    /// Mirror the buffer vertically, swapping the top/bottom clip margins.
    pub fn flip_vertical(&mut self) {
        let w = self.buf_width as usize;
        let h = self.buf_height as usize;
        let clone = self.buf.clone();
        for y in 0..h {
            for x in 0..w {
                self.buf[y * w + x] = clone[(h - y - 1) * w + x];
            }
        }
        std::mem::swap(&mut self.clip_top, &mut self.clip_bottom);
    }

    /// A drawing context bound to this canvas that offsets by padding, for
    /// glyphs that draw arcs/curves via an imperative API (undercurl, dotted
    /// underline, branch circles).
    #[must_use]
    pub fn context(&mut self) -> Context<'_> {
        Context {
            canvas: self,
            builder: PathBuilder::new(),
            line_width: 1.0,
            line_cap: LineCap::Butt,
        }
    }

    // --- trimming & extraction ---

    /// Trim clip margins to drop fully-transparent border rows/columns, then
    /// build the final [`Glyph`] with placement offsets. Mirrors
    /// `writeAtlas` + the offset math in `Face.renderGlyph`.
    #[must_use]
    pub(crate) fn into_glyph(mut self, metrics: &Metrics, draw_height: u32) -> Glyph {
        self.trim();

        let sfc_w = self.buf_width;
        let sfc_h = self.buf_height;
        let region_w = sfc_w
            .saturating_sub(self.clip_left)
            .saturating_sub(self.clip_right);
        let region_h = sfc_h
            .saturating_sub(self.clip_top)
            .saturating_sub(self.clip_bottom);

        let mut alpha = Vec::with_capacity((region_w * region_h) as usize);
        if region_w > 0 && region_h > 0 {
            for y in 0..region_h {
                let src_y = y + self.clip_top;
                let row_start = (src_y * sfc_w + self.clip_left) as usize;
                alpha.extend_from_slice(&self.buf[row_start..row_start + region_w as usize]);
            }
        }

        let offset_x = self.clip_left as i32 - self.padding_x as i32;
        let offset_y = (region_h + self.clip_bottom) as i32 - self.padding_y as i32
            + (metrics.cell_height as i32 - draw_height as i32) / 2;

        Glyph {
            width: region_w,
            height: region_h,
            offset_x,
            offset_y,
            alpha,
        }
    }

    /// Grow the clip margins inward past any fully-transparent border lines.
    fn trim(&mut self) {
        let w = self.buf_width;
        let h = self.buf_height;

        while self.clip_top < h.saturating_sub(self.clip_bottom) {
            let y = self.clip_top;
            let x0 = self.clip_left;
            let x1 = w - self.clip_right;
            let row = &self.buf[(y * w) as usize..][x0 as usize..x1 as usize];
            if row.iter().any(|&v| v != 0) {
                break;
            }
            self.clip_top += 1;
        }

        while self.clip_bottom < h.saturating_sub(self.clip_top) {
            let y = h.saturating_sub(self.clip_bottom).saturating_sub(1);
            let x0 = self.clip_left;
            let x1 = w - self.clip_right;
            let row = &self.buf[(y * w) as usize..][x0 as usize..x1 as usize];
            if row.iter().any(|&v| v != 0) {
                break;
            }
            self.clip_bottom += 1;
        }

        while self.clip_left < w.saturating_sub(self.clip_right) {
            let x = self.clip_left;
            let y0 = self.clip_top;
            let y1 = h - self.clip_bottom;
            if (y0..y1).any(|y| self.buf[(y * w + x) as usize] != 0) {
                break;
            }
            self.clip_left += 1;
        }

        while self.clip_right < w.saturating_sub(self.clip_left) {
            let x = w.saturating_sub(self.clip_right).saturating_sub(1);
            let y0 = self.clip_top;
            let y1 = h - self.clip_bottom;
            if (y0..y1).any(|y| self.buf[(y * w + x) as usize] != 0) {
                break;
            }
            self.clip_right += 1;
        }
    }
}

/// `src_over` compositing of two coverage (alpha) values.
fn src_over(dst: u8, src: u8) -> u8 {
    if src == 255 {
        return 255;
    }
    if src == 0 {
        return dst;
    }
    // out = src + dst * (1 - src)
    let s = f64::from(src) / 255.0;
    let d = f64::from(dst) / 255.0;
    ((s + d * (1.0 - s)) * 255.0).round() as u8
}

/// An imperative path-drawing context that offsets by the canvas padding, used
/// by glyphs that build arcs/curves incrementally (undercurl, dotted underline,
/// branch/circle nodes). Mirrors the subset of z2d's `Context` those glyphs
/// use.
pub struct Context<'a> {
    canvas: &'a mut Canvas,
    builder: PathBuilder,
    /// Stroke line width. Public so glyph code can set it like the Zig context.
    pub line_width: f64,
    /// Stroke line cap.
    pub line_cap: LineCap,
}

impl Context<'_> {
    /// Move the pen to a point.
    pub fn move_to(&mut self, x: f64, y: f64) {
        self.builder.move_to(x as f32, y as f32);
    }

    /// Add a cubic Bézier segment.
    pub fn curve_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) {
        self.builder.cubic_to(
            x1 as f32, y1 as f32, x2 as f32, y2 as f32, x3 as f32, y3 as f32,
        );
    }

    /// Add a full circle centered at `(cx, cy)` with radius `r`.
    pub fn circle(&mut self, cx: f64, cy: f64, r: f64) {
        self.builder.push_circle(cx as f32, cy as f32, r as f32);
    }

    /// Fill the accumulated path (non-zero winding).
    pub fn fill(&mut self, alpha: u8) {
        let pb = std::mem::take(&mut self.builder);
        self.canvas.fill_builder(pb, alpha);
    }

    /// Stroke the accumulated path with the current width and cap.
    pub fn stroke(&mut self, alpha: u8) {
        let pb = std::mem::take(&mut self.builder);
        self.canvas
            .stroke_builder(pb, self.line_width, self.line_cap, alpha);
    }
}
