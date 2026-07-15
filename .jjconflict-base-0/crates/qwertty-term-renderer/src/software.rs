//! A CPU (software) rendering backend — the non-GPU implementation of the
//! [`GpuBackend`](crate::gpu::GpuBackend) trait (ADR 003 P1, PR-2).
//!
//! This is the headless render path: it composites the same frozen wire structs
//! (`Uniforms`/`CellBg`/`CellText`/`Image`) the GPU shaders consume, but on the
//! CPU into a plain BGRA `Vec<u8>` framebuffer — no GPU, no window, no `objc2`.
//! So it builds and runs on any target (Linux, and macOS for testing), which is
//! what betamax's headless-Linux rendering needs.
//!
//! It implements the trait extended in PR-1: [`GpuBuffer`]/[`GpuTexture`]/
//! [`GpuTarget`] plus [`GpuFrame`]/[`GpuRenderPass`], with buffers bound untyped
//! (`BufferHandle = [u8]`, option A). The `RenderPass::step` rasterizer selects
//! its CPU compositing routine from the pipeline (`SoftPipeline`, keyed off the
//! backend-agnostic `PipelineDescription::name`), reinterprets the bound buffers
//! as their wire type, and blends into the target.
//!
//! Scope (first cut): `bg_color`, `cell_bg`, `cell_text` (grayscale glyphs).
//! Color/emoji glyphs, kitty `image`, and `padding_extend` edge extension are
//! deferred follow-ups (documented at each site). Blending is premultiplied
//! "over" in gamma space, matching the default Metal target (`BGRA8Unorm`,
//! non-sRGB, `linear_blending = false`); linear-space blending when the uniform
//! requests it is a follow-up.

use std::cell::RefCell;
use std::rc::Rc;

use crate::gpu::{
    Attachment, FrameCompletion, GpuBackend, GpuBuffer, GpuFrame, GpuRenderPass, GpuTarget,
    GpuTexture, Health, SamplerOptions, ShaderSource, Step, TextureFormat, TextureOptions,
};
use crate::wire::{Atlas, CellBg, CellText, Uniforms};

/// Errors from the software backend. Resource creation is infallible (plain
/// allocation), so this only exists to satisfy the trait's `Error` bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftError {
    /// A build-time misuse (e.g. `build_pipeline` with an unknown pipeline
    /// name, or `ShaderSource` other than `None`).
    Unsupported,
}

impl std::fmt::Display for SoftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SoftError::Unsupported => write!(f, "unsupported software-backend operation"),
        }
    }
}

impl std::error::Error for SoftError {}

/// A typed CPU buffer (the analog of `metal::Buffer<T>`), holding a `Vec<T>`.
pub struct SoftBuffer<T> {
    data: Vec<T>,
}

impl<T: Copy + 'static> GpuBuffer<T> for SoftBuffer<T> {
    type Error = SoftError;
    type Handle = [u8];

    /// The buffer's raw bytes, for binding untyped in a [`Step`].
    fn handle(&self) -> &[u8] {
        // SAFETY: `data` is a live `Vec<T>` of `Copy` (plain-data) elements; we
        // expose its exact byte span read-only, and the pointer is `T`-aligned
        // (so a later cast back to `&Uniforms`/`&[CellText]` is well-aligned).
        unsafe {
            std::slice::from_raw_parts(
                self.data.as_ptr().cast::<u8>(),
                std::mem::size_of_val::<[T]>(&self.data),
            )
        }
    }

    fn len(&self) -> usize {
        self.data.len()
    }

    fn sync(&mut self, data: &[T]) -> Result<(), SoftError> {
        if data.len() > self.data.len() {
            self.data = data.to_vec();
        } else {
            self.data[..data.len()].copy_from_slice(data);
        }
        Ok(())
    }

    fn sync_from_slices(&mut self, lists: &[&[T]]) -> Result<usize, SoftError> {
        let total: usize = lists.iter().map(|l| l.len()).sum();
        if total > self.data.len() {
            self.data = Vec::with_capacity(total);
            for l in lists {
                self.data.extend_from_slice(l);
            }
        } else {
            let mut off = 0;
            for l in lists {
                self.data[off..off + l.len()].copy_from_slice(l);
                off += l.len();
            }
        }
        Ok(total)
    }
}

/// A CPU texture: format + dimensions + tightly-packed pixels. Interior-mutable
/// (`replace_region` takes `&self`, like the Metal texture), CPU-readable by the
/// glyph rasterizer.
pub struct SoftTexture {
    format: TextureFormat,
    width: usize,
    height: usize,
    data: RefCell<Vec<u8>>,
}

impl GpuTexture for SoftTexture {
    type Error = SoftError;

    fn width(&self) -> usize {
        self.width
    }
    fn height(&self) -> usize {
        self.height
    }

    fn replace_region(
        &self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        data: &[u8],
    ) -> Result<(), SoftError> {
        let bpp = self.format.bytes_per_pixel();
        let mut buf = self.data.borrow_mut();
        for row in 0..height {
            let src = row * width * bpp;
            let dst = ((y + row) * self.width + x) * bpp;
            let n = width * bpp;
            if src + n <= data.len() && dst + n <= buf.len() {
                buf[dst..dst + n].copy_from_slice(&data[src..src + n]);
            }
        }
        Ok(())
    }
}

/// A presentable/readable CPU render target: a BGRA framebuffer. The pixel
/// buffer is `Rc<RefCell<…>>` so a [`SoftRenderPass`] can share write access
/// without borrowing the target for a lifetime the trait's `RenderPass`
/// associated type (which carries none) can't express.
pub struct SoftTarget {
    width: usize,
    height: usize,
    pixels: Rc<RefCell<Vec<u8>>>,
}

impl GpuTarget for SoftTarget {
    fn width(&self) -> usize {
        self.width
    }
    fn height(&self) -> usize {
        self.height
    }
    /// The rendered pixels, BGRA, tightly packed — same layout as
    /// `metal::Target::read_pixels`, so consumers are backend-agnostic.
    fn read_pixels(&self) -> Vec<u8> {
        self.pixels.borrow().clone()
    }
}

/// A CPU sampler — nothing to configure (glyph/atlas reads are nearest-neighbour
/// point reads in the rasterizer).
pub struct SoftSampler;

/// A "compiled" pipeline: just which CPU compositing routine to run, selected
/// from the backend-agnostic `PipelineDescription::name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftPipeline {
    BgColor,
    CellBg,
    CellText,
    /// Kitty images — deferred; drawn as a no-op for now.
    Image,
}

/// One in-flight frame. The software backend submits nothing to a GPU; a frame
/// just fires its completion hook on `complete` (health always `Healthy`). The
/// target is taken from the render pass's attachment (as the trait shape hands
/// it through), not held here.
pub struct SoftFrame {
    completion: Option<FrameCompletion>,
}

impl GpuFrame for SoftFrame {
    type Backend = Software;

    fn render_pass(
        &self,
        attachments: &[Attachment<'_, Software>],
    ) -> Result<SoftRenderPass, SoftError> {
        // The renderer only ever uses one color attachment.
        let at = attachments.first().ok_or(SoftError::Unsupported)?;
        let target = at.texture;
        let pass = SoftRenderPass {
            pixels: Rc::clone(&target.pixels),
            width: target.width,
            height: target.height,
        };
        if let Some(c) = at.clear_color {
            pass.clear(c);
        }
        Ok(pass)
    }

    fn complete(&mut self, sync: bool) {
        if let Some(completion) = self.completion.take() {
            completion(Health::Healthy, sync);
        }
    }
}

/// A live render pass: it owns shared write access to the target framebuffer and
/// composites each [`Step`] straight in (there is no deferred GPU submission).
pub struct SoftRenderPass {
    pixels: Rc<RefCell<Vec<u8>>>,
    width: usize,
    height: usize,
}

impl SoftRenderPass {
    fn clear(&self, c: [f64; 4]) {
        let px = to_bgra_unorm(c);
        let mut buf = self.pixels.borrow_mut();
        for chunk in buf.chunks_exact_mut(4) {
            chunk.copy_from_slice(&px);
        }
    }
}

impl GpuRenderPass for SoftRenderPass {
    type Backend = Software;

    fn step(&self, step: &Step<'_, Software>) {
        if step.draw.instance_count == 0 {
            return;
        }
        let mut buf = self.pixels.borrow_mut();
        match step.pipeline {
            SoftPipeline::BgColor => raster_bg_color(&mut buf, self.width, self.height, step),
            SoftPipeline::CellBg => raster_cell_bg(&mut buf, self.width, self.height, step),
            SoftPipeline::CellText => raster_cell_text(&mut buf, self.width, self.height, step),
            // Kitty images: deferred (PR-2 follow-up).
            SoftPipeline::Image => {}
        }
    }

    fn complete(self) {}
}

/// The software graphics backend: a CPU compositor. Stateless (all resources
/// are owned by the caller); construct with [`Software::new`].
pub struct Software;

impl Software {
    #[must_use]
    pub fn new() -> Software {
        Software
    }
}

impl Default for Software {
    fn default() -> Self {
        Software::new()
    }
}

impl GpuBackend for Software {
    // One framebuffer, no multi-buffering (CPU render is synchronous).
    const SWAP_CHAIN_COUNT: usize = 1;

    type Error = SoftError;
    type Target = SoftTarget;
    type Frame = SoftFrame;
    type RenderPass = SoftRenderPass;
    type Pipeline = SoftPipeline;
    type Buffer<T: Copy + 'static> = SoftBuffer<T>;
    type Texture = SoftTexture;
    type Sampler = SoftSampler;
    type BufferHandle = [u8];

    fn max_texture_size(&self) -> u32 {
        // No hardware limit; a generous cap matching the largest GPU tier.
        32768
    }

    fn new_target(&self, width: usize, height: usize) -> Result<SoftTarget, SoftError> {
        Ok(SoftTarget {
            width,
            height,
            pixels: Rc::new(RefCell::new(vec![0u8; width * height * 4])),
        })
    }

    fn new_buffer<T: Copy + 'static>(&self, len: usize) -> Result<SoftBuffer<T>, SoftError> {
        Ok(SoftBuffer {
            data: Vec::with_capacity(len),
        })
    }

    fn new_buffer_with_data<T: Copy + 'static>(
        &self,
        data: &[T],
    ) -> Result<SoftBuffer<T>, SoftError> {
        Ok(SoftBuffer {
            data: data.to_vec(),
        })
    }

    fn new_texture(
        &self,
        options: TextureOptions,
        width: usize,
        height: usize,
        data: Option<&[u8]>,
    ) -> Result<SoftTexture, SoftError> {
        let bpp = options.format.bytes_per_pixel();
        let pixels = match data {
            Some(d) => d.to_vec(),
            None => vec![0u8; width * height * bpp],
        };
        Ok(SoftTexture {
            format: options.format,
            width,
            height,
            data: RefCell::new(pixels),
        })
    }

    fn new_sampler(&self, _options: SamplerOptions) -> Result<SoftSampler, SoftError> {
        Ok(SoftSampler)
    }

    fn begin_frame(&self, completion: FrameCompletion) -> Result<SoftFrame, SoftError> {
        Ok(SoftFrame {
            completion: Some(completion),
        })
    }

    fn build_pipeline(
        &self,
        desc: &crate::shaders::PipelineDescription,
        source: ShaderSource<'_>,
    ) -> Result<SoftPipeline, SoftError> {
        // The software backend needs no shader source.
        if !matches!(source, ShaderSource::None | ShaderSource::Msl(_)) {
            return Err(SoftError::Unsupported);
        }
        match desc.name {
            "bg_color" => Ok(SoftPipeline::BgColor),
            "cell_bg" => Ok(SoftPipeline::CellBg),
            "cell_text" => Ok(SoftPipeline::CellText),
            "image" => Ok(SoftPipeline::Image),
            _ => Err(SoftError::Unsupported),
        }
    }
}

// --- Rasterization helpers ------------------------------------------------

/// Convert a clear color (RGBA `f64` in 0..1) to a BGRA `u8` pixel.
fn to_bgra_unorm(c: [f64; 4]) -> [u8; 4] {
    let q = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [q(c[2]), q(c[1]), q(c[0]), q(c[3])]
}

/// Premultiplied "over" blend of a non-premultiplied RGBA source (with an
/// effective coverage `alpha` in 0..1) onto a BGRA destination pixel, in gamma
/// space (matching the default non-sRGB Metal target).
fn blend_over(dst: &mut [u8], src_rgb: [u8; 3], alpha: f64) {
    let a = alpha.clamp(0.0, 1.0);
    let inv = 1.0 - a;
    // dst is BGRA.
    for (i, s) in [src_rgb[2], src_rgb[1], src_rgb[0]].into_iter().enumerate() {
        dst[i] = (f64::from(s) * a + f64::from(dst[i]) * inv).round() as u8;
    }
    dst[3] = (f64::from(255) * a + f64::from(dst[3]) * inv).round() as u8;
}

/// Reinterpret bound buffer bytes as a `&Uniforms`.
fn uniforms_of(step: &Step<'_, Software>) -> Option<Uniforms> {
    let bytes = step.uniforms?;
    if bytes.len() < std::mem::size_of::<Uniforms>() {
        return None;
    }
    // SAFETY: the bytes came from a `SoftBuffer<Uniforms>` (so `Uniforms`-aligned
    // and at least one element long); `Uniforms` is `Copy` plain data.
    Some(unsafe { *bytes.as_ptr().cast::<Uniforms>() })
}

/// The grid geometry from the uniforms: cell size (px) and padding (top,right,
/// bottom,left) and grid columns/rows.
struct Grid {
    cell_w: f64,
    cell_h: f64,
    pad_top: f64,
    pad_left: f64,
    cols: usize,
    rows: usize,
}

impl Grid {
    fn from(u: &Uniforms) -> Grid {
        Grid {
            cell_w: f64::from(u.cell_size[0]),
            cell_h: f64::from(u.cell_size[1]),
            pad_top: f64::from(u.grid_padding.0[0]),
            pad_left: f64::from(u.grid_padding.0[3]),
            cols: usize::from(u.grid_size[0]),
            rows: usize::from(u.grid_size[1]),
        }
    }
}

/// `bg_color`: fill the whole target with the uniform background color (opaque,
/// blending disabled — upstream `bg_color_fragment`).
fn raster_bg_color(buf: &mut [u8], _w: usize, _h: usize, step: &Step<'_, Software>) {
    let Some(u) = uniforms_of(step) else { return };
    let px = [u.bg_color[2], u.bg_color[1], u.bg_color[0], u.bg_color[3]];
    for chunk in buf.chunks_exact_mut(4) {
        chunk.copy_from_slice(&px);
    }
}

/// `cell_bg`: blend each cell's background color over the target within the grid
/// area (upstream `cell_bg_fragment`). `padding_extend` edge extension into the
/// padding is a follow-up — padding pixels keep the `bg_color` fill.
fn raster_cell_bg(buf: &mut [u8], w: usize, h: usize, step: &Step<'_, Software>) {
    let Some(u) = uniforms_of(step) else { return };
    let Some(bytes) = step.extras.first().and_then(|e| *e) else {
        return;
    };
    let n = bytes.len() / std::mem::size_of::<CellBg>();
    // SAFETY: bytes from a `SoftBuffer<CellBg>` (`[u8;4]`, `Copy`), `CellBg`-aligned.
    let cells: &[CellBg] =
        unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<CellBg>(), n) };
    let g = Grid::from(&u);
    if g.cell_w < 1.0 || g.cell_h < 1.0 || g.cols == 0 {
        return;
    }
    for y in 0..h {
        let fy = y as f64 - g.pad_top;
        if fy < 0.0 {
            continue;
        }
        let row = (fy / g.cell_h) as usize;
        if row >= g.rows {
            continue;
        }
        for x in 0..w {
            let fx = x as f64 - g.pad_left;
            if fx < 0.0 {
                continue;
            }
            let col = (fx / g.cell_w) as usize;
            if col >= g.cols {
                continue;
            }
            let Some(c) = cells.get(row * g.cols + col) else {
                continue;
            };
            if c[3] == 0 {
                continue;
            }
            let idx = (y * w + x) * 4;
            blend_over(
                &mut buf[idx..idx + 4],
                [c[0], c[1], c[2]],
                f64::from(c[3]) / 255.0,
            );
        }
    }
}

/// `cell_text`: alpha-blit each glyph from the grayscale atlas at its cell,
/// tinted by the instance color (upstream `cell_text_vertex`/`_fragment`,
/// grayscale path). Color/emoji glyphs (the `Color` atlas) and min-contrast are
/// follow-ups; a color-atlas instance is skipped for now.
fn raster_cell_text(buf: &mut [u8], w: usize, h: usize, step: &Step<'_, Software>) {
    let Some(u) = uniforms_of(step) else { return };
    let Some(bytes) = step.vertex else { return };
    let n = bytes.len() / std::mem::size_of::<CellText>();
    // SAFETY: bytes from a `SoftBuffer<CellText>` (`Copy`, `CellText`-aligned).
    let cells: &[CellText] =
        unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<CellText>(), n) };
    // textures[0] = grayscale atlas.
    let Some(Some(atlas)) = step.textures.first() else {
        return;
    };
    let g = Grid::from(&u);
    let atlas_w = atlas.width;
    let atlas_data = atlas.data.borrow();

    let count = step.draw.instance_count.min(cells.len());
    for cell in &cells[..count] {
        if cell.atlas == Atlas::Color {
            continue; // color glyphs deferred
        }
        let gw = cell.glyph_size[0] as usize;
        let gh = cell.glyph_size[1] as usize;
        if gw == 0 || gh == 0 {
            continue;
        }
        let ax = cell.glyph_pos[0] as usize;
        let ay = cell.glyph_pos[1] as usize;
        // Cell origin (px) + bearings. `bearings.y` is cell-bottom → ink-top, so
        // the glyph top sits `cell_h - bearings.y` below the cell top.
        let cell_x = g.pad_left + f64::from(cell.grid_pos[0]) * g.cell_w;
        let cell_y = g.pad_top + f64::from(cell.grid_pos[1]) * g.cell_h;
        let dst_x0 = (cell_x + f64::from(cell.bearings[0])) as i64;
        let dst_y0 = (cell_y + g.cell_h - f64::from(cell.bearings[1])) as i64;
        let [cr, cg, cb, ca] = cell.color;

        for gy in 0..gh {
            for gx in 0..gw {
                let cov = atlas_data
                    .get((ay + gy) * atlas_w + (ax + gx))
                    .copied()
                    .unwrap_or(0);
                if cov == 0 {
                    continue;
                }
                let px = dst_x0 + gx as i64;
                let py = dst_y0 + gy as i64;
                if px < 0 || py < 0 || px as usize >= w || py as usize >= h {
                    continue;
                }
                let alpha = (f64::from(cov) / 255.0) * (f64::from(ca) / 255.0);
                let idx = (py as usize * w + px as usize) * 4;
                blend_over(&mut buf[idx..idx + 4], [cr, cg, cb], alpha);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{Draw, Primitive};
    use crate::wire::{AlignedF32x4, CellTextBools, Mat, PaddingExtend, UniformBools};

    /// A `Uniforms` for a `cols × rows` grid of `cell`-sized cells, no padding,
    /// with background `bg` (RGBA).
    fn uniforms(cell: [f32; 2], cols: u16, rows: u16, bg: [u8; 4]) -> Uniforms {
        Uniforms {
            projection_matrix: Mat::IDENTITY,
            screen_size: [cell[0] * f32::from(cols), cell[1] * f32::from(rows)],
            cell_size: cell,
            grid_size: [cols, rows],
            grid_padding: AlignedF32x4([0.0; 4]),
            padding_extend: PaddingExtend(0),
            min_contrast: 1.0,
            cursor_pos: [0, 0],
            cursor_color: [0, 0, 0, 0],
            bg_color: bg,
            bools: UniformBools::default(),
        }
    }

    fn px(pixels: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    }

    #[test]
    fn renders_bg_cellbg_and_text_end_to_end() {
        let backend = Software::new();
        // 2 cols × 1 row, 4×4px cells → 8×4 target.
        let (cw, ch) = (4.0_f32, 4.0_f32);
        let (w, h) = (8usize, 4usize);
        let target = backend.new_target(w, h).unwrap();

        let bg = backend
            .build_pipeline(desc("bg_color"), ShaderSource::None)
            .unwrap();
        let cbg = backend
            .build_pipeline(desc("cell_bg"), ShaderSource::None)
            .unwrap();
        let ctext = backend
            .build_pipeline(desc("cell_text"), ShaderSource::None)
            .unwrap();

        let u = backend
            .new_buffer_with_data(&[uniforms([cw, ch], 2, 1, [10, 20, 30, 255])])
            .unwrap();
        // Cell 0 = red, cell 1 = green.
        let cells_bg = backend
            .new_buffer_with_data::<CellBg>(&[[200, 0, 0, 255], [0, 200, 0, 255]])
            .unwrap();
        // A 4×4 grayscale atlas with a 2×2 full-coverage block at (0,0).
        let mut atlas_px = vec![0u8; 16];
        for gy in 0..2 {
            for gx in 0..2 {
                atlas_px[gy * 4 + gx] = 255;
            }
        }
        let atlas = backend
            .new_texture(
                TextureOptions {
                    format: TextureFormat::R8Unorm,
                    usage: crate::gpu::TextureUsage::SHADER_READ,
                },
                4,
                4,
                Some(&atlas_px),
            )
            .unwrap();
        // One white glyph in cell 0: 2×2, bearings (0,4) → drawn at cell top-left.
        let glyph = CellText {
            glyph_pos: [0, 0],
            glyph_size: [2, 2],
            bearings: [0, 4],
            grid_pos: [0, 0],
            color: [255, 255, 255, 255],
            atlas: Atlas::Grayscale,
            bools: CellTextBools::default(),
        };
        let cells = backend.new_buffer_with_data(&[glyph]).unwrap();

        let mut frame = backend.begin_frame(Box::new(|_h, _s| {})).unwrap();
        {
            let pass = frame
                .render_pass(&[Attachment {
                    texture: &target,
                    clear_color: Some([0.0, 0.0, 0.0, 0.0]),
                }])
                .unwrap();
            pass.step(&Step {
                pipeline: &bg,
                vertex: None,
                uniforms: Some(u.handle()),
                extras: &[],
                textures: &[],
                samplers: &[],
                draw: Draw::vertices(Primitive::Triangle, 3),
            });
            pass.step(&Step {
                pipeline: &cbg,
                vertex: None,
                uniforms: Some(u.handle()),
                extras: &[Some(cells_bg.handle())],
                textures: &[],
                samplers: &[],
                draw: Draw::vertices(Primitive::Triangle, 3),
            });
            pass.step(&Step {
                pipeline: &ctext,
                vertex: Some(cells.handle()),
                uniforms: Some(u.handle()),
                extras: &[],
                textures: &[Some(&atlas)],
                samplers: &[],
                draw: Draw {
                    primitive: Primitive::TriangleStrip,
                    vertex_count: 4,
                    instance_count: 1,
                },
            });
            pass.complete();
        }
        frame.complete(true);

        let pixels = target.read_pixels();
        // Glyph pixel (0,0): white over red = white. BGRA.
        assert_eq!(px(&pixels, w, 0, 0), [255, 255, 255, 255], "glyph pixel");
        // Cell 0 non-glyph pixel (3,3): red bg. BGRA = [0,0,200,255].
        assert_eq!(px(&pixels, w, 3, 3), [0, 0, 200, 255], "cell 0 bg (red)");
        // Cell 1 pixel (5,2): green bg. BGRA = [0,200,0,255].
        assert_eq!(px(&pixels, w, 5, 2), [0, 200, 0, 255], "cell 1 bg (green)");
    }

    #[test]
    fn bg_color_fills_when_no_cells() {
        let backend = Software::new();
        let target = backend.new_target(2, 2).unwrap();
        let bg = backend
            .build_pipeline(desc("bg_color"), ShaderSource::None)
            .unwrap();
        let u = backend
            .new_buffer_with_data(&[uniforms([2.0, 2.0], 1, 1, [1, 2, 3, 255])])
            .unwrap();
        let mut frame = backend.begin_frame(Box::new(|_h, _s| {})).unwrap();
        {
            let pass = frame
                .render_pass(&[Attachment {
                    texture: &target,
                    clear_color: None,
                }])
                .unwrap();
            pass.step(&Step {
                pipeline: &bg,
                vertex: None,
                uniforms: Some(u.handle()),
                extras: &[],
                textures: &[],
                samplers: &[],
                draw: Draw::vertices(Primitive::Triangle, 3),
            });
            pass.complete();
        }
        frame.complete(true);
        // Every pixel is the bg color, BGRA.
        assert!(
            target
                .read_pixels()
                .chunks_exact(4)
                .all(|p| p == [3, 2, 1, 255])
        );
    }

    fn desc(name: &str) -> &'static crate::shaders::PipelineDescription {
        crate::shaders::PIPELINE_DESCRIPTIONS
            .iter()
            .find(|d| d.name == name)
            .unwrap()
    }
}
