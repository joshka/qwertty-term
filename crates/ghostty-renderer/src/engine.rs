//! The cell engine: snapshot → GPU buffers → drawn frame (first pixels).
//!
//! Port of the load-bearing subset of `src/renderer/generic.zig` (commit
//! `2da015cd6`): `updateFrame` (the buffer-building half, minus the threading /
//! kitty / search / custom-shader branches), `rebuildCells` / `rebuildRow` /
//! `addGlyph` / `addCursor` / `addUnderline` / `addStrikethrough` /
//! `addOverline`, `syncAtlasTexture`, and the cell-drawing structure of
//! `drawFrame` (the three first-pixels pipelines encoded into an R2 `Frame`
//! against the swap chain, minus image / overlay / post passes).
//!
//! The engine owns a [`Contents`], the [`Uniforms`], and a [`SwapChain`]; each
//! frame it consumes a [`RenderSnapshot`] and a `ghostty-font` [`Grid`] to
//! rebuild the buffers, then draws. See `docs/analysis/renderer-r4.md`.

use ghostty_font::grid::Grid;
use ghostty_font::{FontIndex, ShapedCell};
use ghostty_sprite::Sprite;
use ghostty_vt::color::{Palette, Rgb};
use ghostty_vt::snapshot::{CellStyle, SnapshotCell, SnapshotColor, SnapshotUnderline};

use crate::cells::{self, Contents, Key};
use crate::cursor::{self, Style as CursorStyle};
use crate::gpu::{GpuBackend, GpuBuffer, GpuTexture};
use crate::metal::{
    Attachment, Draw, Metal, MetalError, Pipeline, PipelineOptions, Primitive, Step,
    VertexAttribute, VertexFormat, VertexLayout, VertexStep, library_from_source,
};
use crate::shaders;
use crate::snapshot::RenderSnapshot;
use crate::swap_chain::{SwapChain, SwapChainMode};
use crate::wire::{Atlas, CellText, CellTextBools, Mat, PaddingExtend, UniformBools, Uniforms};

/// Renderer-local per-frame options the terminal snapshot can't supply itself
/// (mirrors what `updateFrame` reads off `renderer.State` / config).
#[derive(Debug, Clone, Copy)]
pub struct FrameOptions {
    /// Whether the surface is focused (drives hollow-cursor + cursor alpha).
    pub focused: bool,
    /// Whether the blink phase currently shows the cursor.
    pub cursor_blink_visible: bool,
    /// The renderer's default foreground when the snapshot has none
    /// (`Snapshot::default_fg == None`). Upstream: `config`-derived
    /// `colors.foreground`.
    pub default_fg: Rgb,
    /// The renderer's default background when the snapshot has none.
    pub default_bg: Rgb,
    /// Minimum WCAG contrast ratio for text (`> 1.0` enables the shader's
    /// min-contrast step). Upstream `config.minimum-contrast`.
    pub min_contrast: f32,
}

impl Default for FrameOptions {
    fn default() -> Self {
        Self {
            focused: true,
            cursor_blink_visible: true,
            // A conventional light-on-dark default (upstream's built-in default
            // theme); callers wire real config here later.
            default_fg: Rgb::new(0xd8, 0xd8, 0xd8),
            default_bg: Rgb::new(0x18, 0x18, 0x18),
            min_contrast: 1.0,
        }
    }
}

/// The cell engine. Owns the CPU-side [`Contents`], the [`Uniforms`], the
/// [`SwapChain`], and the three first-pixels pipelines.
pub struct Engine {
    /// The GPU backend (device/queue + resource factory).
    backend: Metal,
    /// Per-frame GPU-slot pool (targets + instance buffers + atlas textures).
    swap_chain: SwapChain<Metal>,
    /// CPU-side cell contents, rebuilt each `update_frame`.
    contents: Contents,
    /// Shader uniforms, rebuilt each `update_frame`.
    uniforms: Uniforms,
    /// `bg_color`: solid background fill (full-screen triangle).
    bg_color_pipeline: Pipeline,
    /// `cell_bg`: per-cell background color (full-screen triangle sampling the
    /// bg_cells buffer).
    cell_bg_pipeline: Pipeline,
    /// `cell_text`: instanced glyph quads.
    cell_text_pipeline: Pipeline,
    /// Cell metrics (from the font grid), cached for geometry.
    cell_width: u32,
    cell_height: u32,
    /// Per-slot "last synced atlas modified counter" so we only re-upload the
    /// grayscale atlas when it changed (upstream `frame.grayscale_modified`).
    grayscale_modified: Vec<usize>,
    /// Current pixel dimensions of the render target (grid size × cell size in
    /// the reduced cut: no window padding yet).
    screen_width: usize,
    screen_height: usize,
}

impl Engine {
    /// Build the engine over a fresh Metal context, compiling the three
    /// first-pixels pipelines from the embedded R3 shader source.
    ///
    /// `cell_width`/`cell_height` are the grid metrics (from the font grid);
    /// they fix cell geometry for the uniforms and the target size.
    pub fn new(cell_width: u32, cell_height: u32) -> Result<Engine, MetalError> {
        let backend = Metal::new()?;
        Engine::with_backend(backend, cell_width, cell_height)
    }

    /// Build the engine over an existing Metal context. Used by tests that
    /// share a backend or need to skip gracefully when no device is present.
    pub fn with_backend(
        backend: Metal,
        cell_width: u32,
        cell_height: u32,
    ) -> Result<Engine, MetalError> {
        // Day-one degenerate pacing (plan decision 3): one live slot, each frame
        // completed with `waitUntilCompleted`.
        let swap_chain = SwapChain::new(&backend, SwapChainMode::Sync)?;
        let slot_count = swap_chain.slot_count();

        let library = library_from_source(backend.device(), shaders::SOURCE)?;
        let pixel_format = backend.target_pixel_format();

        let bg_color_pipeline = build_pipeline(&backend, &library, pixel_format, "bg_color")?;
        let cell_bg_pipeline = build_pipeline(&backend, &library, pixel_format, "cell_bg")?;
        let cell_text_pipeline = build_pipeline(&backend, &library, pixel_format, "cell_text")?;

        Ok(Engine {
            backend,
            swap_chain,
            contents: Contents::default(),
            uniforms: default_uniforms(),
            bg_color_pipeline,
            cell_bg_pipeline,
            cell_text_pipeline,
            cell_width,
            cell_height,
            grayscale_modified: vec![0; slot_count],
            screen_width: 0,
            screen_height: 0,
        })
    }

    /// The Metal backend (for tests / inspection).
    pub fn backend(&self) -> &Metal {
        &self.backend
    }

    /// The pixel dimensions of the current render target.
    pub fn screen_size(&self) -> (usize, usize) {
        (self.screen_width, self.screen_height)
    }

    /// Rebuild the CPU-side [`Contents`] and [`Uniforms`] from `snapshot`,
    /// shaping and rasterizing glyphs through `grid`. Port of `updateFrame` +
    /// `rebuildCells` + `rebuildRow` (buffer-building half).
    ///
    /// The reduced cut always does a full rebuild (`DirtyStatus::Full`; the
    /// full-copy snapshot never reports partial), matching plan decision 4
    /// (day-one full redraw).
    pub fn update_frame<S: RenderSnapshot>(
        &mut self,
        snapshot: &S,
        grid: &mut Grid,
        opts: FrameOptions,
    ) {
        let cols = snapshot.cols();
        let rows = snapshot.rows();

        // Resize the contents (and thus target/uniforms geometry) if the grid
        // changed. Full-copy snapshot => always a full rebuild anyway.
        let size_changed = self.contents.cols() != cols || self.contents.rows() != rows;
        if size_changed {
            self.contents.resize(cols, rows);
            self.screen_width = cols * self.cell_width as usize;
            self.screen_height = rows * self.cell_height as usize;
        }
        self.contents.reset();

        // Resolve default fg/bg (dynamic OSC override wins, else config default).
        let default_fg = snapshot.default_fg().unwrap_or(opts.default_fg);
        let default_bg = snapshot.default_bg().unwrap_or(opts.default_bg);
        let palette = snapshot.palette();

        // Uniforms: screen/cell/grid geometry + projection + bg color.
        self.build_uniforms(cols, rows, default_bg, opts.min_contrast);

        // Cursor style resolution (renderer-local focus/blink/preedit on top of
        // the snapshot cursor). Preedit isn't wired in the reduced cut.
        let cursor = snapshot.cursor();
        let cursor_style = cursor.and_then(|c| {
            // The snapshot doesn't carry the blink *mode*; pass `blinking =
            // false` so the cursor is always shown when visible+focused
            // (blink gating comes from `opts.cursor_blink_visible` in a future
            // wiring, but the reduced cut doesn't animate).
            let state = cursor::CursorState::from_snapshot_cursor(&c, false);
            cursor::style(
                &state,
                cursor::StyleOptions {
                    preedit: false,
                    focused: opts.focused,
                    blink_visible: opts.cursor_blink_visible,
                },
            )
        });

        // Rebuild each row.
        for y in 0..rows {
            let row = snapshot.row(y);
            self.rebuild_row(y, row, grid, palette, default_fg, default_bg);
        }

        // Cursor.
        self.build_cursor(
            cursor,
            cursor_style,
            snapshot,
            grid,
            palette,
            default_fg,
            default_bg,
        );
    }

    /// Build the frame uniforms (port of `updateScreenSizeUniforms` + the
    /// bg-color / grid-size assignments in `updateFrame`/`rebuildCells`).
    ///
    /// The reduced cut has no window padding, so `grid_padding` is zero and the
    /// projection is a plain 0..width / 0..height ortho.
    fn build_uniforms(&mut self, cols: usize, rows: usize, default_bg: Rgb, min_contrast: f32) {
        let w = (cols * self.cell_width as usize) as f32;
        let h = (rows * self.cell_height as usize) as f32;

        self.uniforms.projection_matrix = Mat::ortho2d(0.0, w, h, 0.0);
        self.uniforms.screen_size = [w, h];
        self.uniforms.cell_size = [self.cell_width as f32, self.cell_height as f32];
        self.uniforms.grid_size = [cols as u16, rows as u16];
        self.uniforms.grid_padding = crate::wire::AlignedF32x4([0.0, 0.0, 0.0, 0.0]);
        // No padding extension in the reduced cut (padding_color=background).
        self.uniforms.padding_extend = PaddingExtend(0);
        self.uniforms.min_contrast = min_contrast;
        self.uniforms.bg_color = [default_bg.r, default_bg.g, default_bg.b, 255];
        self.uniforms.bools.use_display_p3 = false;
        self.uniforms.bools.use_linear_blending = false;
        self.uniforms.bools.use_linear_correction = false;
    }

    /// Rebuild one row: per-cell background colors + shaped foreground glyphs +
    /// decorations. Port of `rebuildRow` reduced to the snapshot model (no
    /// selection/search/highlight/link/preedit branches).
    #[allow(clippy::too_many_arguments)]
    fn rebuild_row(
        &mut self,
        y: usize,
        row: &[SnapshotCell],
        grid: &mut Grid,
        palette: &Palette,
        default_fg: Rgb,
        default_bg: Rgb,
    ) {
        let cols = self.contents.cols();

        // Per-cell background + decorations, plus the resolved fg used for
        // glyphs shaped below. We shape style-homogeneous runs of non-spacer
        // cells (a run breaks on a style change, matching upstream's
        // comparableStyle break; sprite cells break implicitly since they
        // shape one glyph at a time via render_codepoint).
        let mut x = 0usize;
        while x < cols && x < row.len() {
            let cell = &row[x];

            // Spacer tail of a wide glyph: no bg fill, no glyph (the lead cell
            // painted it). Advance one.
            if cell.is_spacer() {
                x += 1;
                continue;
            }

            let style = &cell.style;
            let (fg, bg, bg_alpha) =
                resolve_colors(style, cell.ch as u32, palette, default_fg, default_bg);

            // Background fill (upstream: only paint when the cell has an
            // explicit bg or is inverse; otherwise alpha 0 lets the surface bg
            // show through).
            self.contents
                .set_bg_cell(y, x, [bg.r, bg.g, bg.b, bg_alpha]);

            // Invisible cells: bg only, no foreground (matches xterm/upstream).
            if style.invisible {
                x += 1;
                continue;
            }

            let alpha: u8 = if style.faint { 128 } else { 255 };

            // Underlines draw first (underneath text).
            if style.underline != SnapshotUnderline::None {
                let underline_color = match style.underline_color {
                    SnapshotColor::Default => fg,
                    other => resolve_color(other, palette, default_fg),
                };
                self.add_decoration(
                    x,
                    y,
                    underline_sprite(style.underline),
                    underline_color,
                    alpha,
                    grid,
                    Key::Underline,
                );
            }
            if style.overline {
                self.add_decoration(x, y, Sprite::Overline, fg, alpha, grid, Key::Overline);
            }

            // Glyph. Sprite codepoints (box drawing etc.) and single glyphs are
            // handled by render_codepoint; text runs shape one contiguous run
            // of same-style cells.
            self.add_cell_glyph(x, y, row, grid, fg, alpha);

            // Strikethrough draws last (over text).
            if style.strikethrough {
                self.add_decoration(
                    x,
                    y,
                    Sprite::Strikethrough,
                    fg,
                    alpha,
                    grid,
                    Key::Strikethrough,
                );
            }

            x += 1;
        }
    }

    /// Shape+rasterize the glyph(s) for the cell at `x`, adding [`CellText`]
    /// instances. Port of the `addGlyph` path, reduced to per-cell shaping (the
    /// reduced Shaper takes a `&str`; we shape each cell's grapheme in
    /// isolation, which is exact for the monospace ASCII/CJK/box scope and
    /// avoids threading run segmentation through the snapshot model — style-run
    /// segmentation is preserved by the fact that each cell carries its own
    /// resolved fg).
    fn add_cell_glyph(
        &mut self,
        x: usize,
        y: usize,
        row: &[SnapshotCell],
        grid: &mut Grid,
        fg: Rgb,
        alpha: u8,
    ) {
        let cell = &row[x];
        let cp = cell.ch as u32;

        // Blank cell: nothing to draw (space renders as an empty glyph anyway;
        // skip to avoid a zero-size instance).
        if cp == 0 || cell.ch == ' ' {
            return;
        }

        // Resolve to a font index. Sprite codepoints route to the procedural
        // rasterizer; text codepoints shape through rustybuzz.
        let Some(index) = grid.get_index(cp) else {
            return;
        };

        match index {
            FontIndex::Sprite => {
                // Sprite: codepoint == glyph id, no shaping.
                if let Ok(Some(g)) = grid.render_codepoint(cp) {
                    if g.width == 0 || g.height == 0 {
                        return;
                    }
                    self.push_text_cell(
                        x,
                        y,
                        fg,
                        alpha,
                        &g,
                        0,
                        0,
                        Atlas::Grayscale,
                        cells::no_min_contrast(cp),
                        false,
                        Key::Text,
                    );
                }
            }
            FontIndex::Face { .. } => {
                // Shape this cell's grapheme (base char + combining marks) as a
                // one-cell run. rustybuzz maps clusters → cell 0.
                let text: String = std::iter::once(cell.ch)
                    .chain(cell.combining.iter().copied())
                    .collect();
                let shaped = match self.shape_cell(grid, &text) {
                    Some(s) => s,
                    None => return,
                };
                for sc in shaped {
                    let g = match grid.render_glyph(index, sc.glyph_index) {
                        Ok(g) => g,
                        Err(_) => continue,
                    };
                    if g.width == 0 || g.height == 0 {
                        continue;
                    }
                    self.push_text_cell(
                        x,
                        y,
                        fg,
                        alpha,
                        &g,
                        sc.x_offset,
                        sc.y_offset,
                        Atlas::Grayscale,
                        false,
                        false,
                        Key::Text,
                    );
                }
            }
        }
    }

    /// Shape one cell's text into shaped cells via the grid's primary face.
    /// Returns `None` if the face has no byte-backed shaper (name-loaded system
    /// faces; deferred).
    fn shape_cell(&self, grid: &Grid, text: &str) -> Option<Vec<ShapedCell>> {
        let face = grid.resolver().collection().primary();
        let mut shaper = ghostty_font::Shaper::new(face)?;
        Some(shaper.shape_run(text))
    }

    /// Render a decoration sprite (underline/strikethrough/overline) into the
    /// cell. Port of `addUnderline`/`addStrikethrough`/`addOverline`.
    #[allow(clippy::too_many_arguments)]
    fn add_decoration(
        &mut self,
        x: usize,
        y: usize,
        sprite: Sprite,
        color: Rgb,
        alpha: u8,
        grid: &mut Grid,
        key: Key,
    ) {
        let cp = sprite.codepoint();
        let g = match grid.render_codepoint(cp) {
            Ok(Some(g)) => g,
            _ => return,
        };
        // Decorations may legitimately be full-cell; still skip empties.
        if g.width == 0 || g.height == 0 {
            return;
        }
        self.push_text_cell(
            x,
            y,
            color,
            alpha,
            &g,
            0,
            0,
            Atlas::Grayscale,
            false,
            false,
            key,
        );
    }

    /// Push a foreground [`CellText`] instance built from a cached glyph +
    /// placement (glyph / decoration). Shared body of `addGlyph` /
    /// `add*line`. The cursor takes a separate path ([`build_cursor`] →
    /// `Contents::set_cursor`) because it lives in the reserved cursor lists.
    #[allow(clippy::too_many_arguments)]
    fn push_text_cell(
        &mut self,
        x: usize,
        y: usize,
        color: Rgb,
        alpha: u8,
        glyph: &ghostty_font::CachedGlyph,
        x_offset: i16,
        y_offset: i16,
        atlas: Atlas,
        no_min_contrast: bool,
        _is_cursor: bool,
        key: Key,
    ) {
        let mut bools = 0u8;
        if no_min_contrast {
            bools |= CellTextBools::NO_MIN_CONTRAST;
        }
        let cell = CellText {
            glyph_pos: [glyph.atlas_x, glyph.atlas_y],
            glyph_size: [glyph.width, glyph.height],
            bearings: [
                (glyph.offset_x + x_offset as i32) as i16,
                (glyph.offset_y + y_offset as i32) as i16,
            ],
            grid_pos: [x as u16, y as u16],
            color: [color.r, color.g, color.b, alpha],
            atlas,
            bools: CellTextBools(bools),
        };
        self.contents.add(key, cell);
    }

    /// Build the cursor cell + block-cursor uniforms. Port of `addCursor` + the
    /// cursor uniform block of `rebuildCells`.
    #[allow(clippy::too_many_arguments)]
    fn build_cursor<S: RenderSnapshot>(
        &mut self,
        cursor: Option<ghostty_vt::snapshot::SnapshotCursor>,
        style: Option<CursorStyle>,
        snapshot: &S,
        grid: &mut Grid,
        palette: &Palette,
        default_fg: Rgb,
        default_bg: Rgb,
    ) {
        // Default: no cursor, sentinel cursor_pos.
        self.contents.set_cursor(None, None);
        self.uniforms.cursor_pos = [u16::MAX, u16::MAX];

        let (Some(cursor), Some(style)) = (cursor, style) else {
            return;
        };
        // Lock cursor needs a nerd-font symbol not in the reduced cut; treat as
        // no cursor (documented deferral).
        if style == CursorStyle::Lock {
            return;
        }

        let cx = cursor.col;
        let cy = cursor.row;
        if cy >= self.contents.rows() || cx >= self.contents.cols() {
            return;
        }

        // Cursor color: OSC-12 not wired in the snapshot; use default fg.
        let cursor_color = default_fg;

        // Is the cursor cell wide? Look at the underlying cell.
        let row = snapshot.row(cy);
        let (wide, x) = match row.get(cx) {
            Some(c) if c.is_spacer() && cx > 0 => (true, cx - 1),
            Some(c) => (c.is_wide(), cx),
            None => (false, cx),
        };

        let sprite = match style {
            CursorStyle::Block => Sprite::CursorRect,
            CursorStyle::BlockHollow => Sprite::CursorHollowRect,
            CursorStyle::Bar => Sprite::CursorBar,
            CursorStyle::Underline => Sprite::CursorUnderline,
            CursorStyle::Lock => return,
        };

        let alpha: u8 = 255;
        let g = match grid.render_codepoint(sprite.codepoint()) {
            Ok(Some(g)) => g,
            _ => return,
        };

        let cell = CellText {
            glyph_pos: [g.atlas_x, g.atlas_y],
            glyph_size: [g.width, g.height],
            bearings: [g.offset_x as i16, g.offset_y as i16],
            grid_pos: [x as u16, cy as u16],
            color: [cursor_color.r, cursor_color.g, cursor_color.b, alpha],
            atlas: Atlas::Grayscale,
            bools: CellTextBools(CellTextBools::IS_CURSOR_GLYPH),
        };
        self.contents.set_cursor(Some(cell), Some(style));

        // Block cursor also drives the cursor_pos/cursor_color uniforms so the
        // cell_text shader flips the glyph under it to the cursor-text color.
        if style == CursorStyle::Block {
            self.uniforms.cursor_pos = [x as u16, cy as u16];
            self.uniforms.bools.cursor_wide = wide;
            // Cursor-text: use the cell background so text under a block cursor
            // reads as inverted (upstream default when cursor-text unset).
            let (_, bg, _) = row
                .get(cx)
                .map(|c| resolve_colors(&c.style, c.ch as u32, palette, default_fg, default_bg))
                .unwrap_or((default_fg, default_bg, 255));
            self.uniforms.cursor_color = [bg.r, bg.g, bg.b, 255];
        }
    }

    /// Draw the current [`Contents`] into a fresh frame against the swap chain
    /// and present (readback-ready). Port of `drawFrame`'s cell-drawing
    /// structure (bg_color → cell_bg → cell_text), sync mode.
    ///
    /// Returns the drawn slot's readback pixels (BGRA, row-padding stripped) so
    /// the caller (offscreen tests / a future window host reading the surface)
    /// can inspect them. In sync mode this is coherent after
    /// `waitUntilCompleted`.
    pub fn draw_frame(&mut self) -> Result<Vec<u8>, MetalError> {
        if self.screen_width == 0 || self.screen_height == 0 {
            return Ok(Vec::new());
        }

        // Copy/gather the CPU-side data to hand to the GPU. This ends the borrow
        // of `self.contents` / `self.uniforms` before we take disjoint borrows
        // of `self.backend`, the pipelines, and `self.swap_chain` below.
        let uniforms = self.uniforms;
        let bg_cells: Vec<crate::wire::CellBg> = self.contents.bg_cells().to_vec();
        let fg_count = self.contents.fg_count();
        // rustybuzz can emit >1 glyph per cell, but the gathered upload is a
        // flat concatenation; borrow the lists as slices for the gather.
        let fg_lists: Vec<&[CellText]> =
            self.contents.fg_lists().iter().map(Vec::as_slice).collect();

        let (sw, sh) = (self.screen_width, self.screen_height);

        // Disjoint borrows: the swap chain (for the slot), the backend (for the
        // frame + resizes), and the three pipelines. Splitting `self` into field
        // references up front lets us hold the slot guard across backend calls
        // without a self-aliasing conflict.
        let Engine {
            backend,
            swap_chain,
            bg_color_pipeline,
            cell_bg_pipeline,
            cell_text_pipeline,
            ..
        } = self;

        // Acquire a slot (sync mode: one live permit).
        let mut guard = swap_chain.next_frame().ok_or(MetalError::MetalFailed)?;
        let slot = guard.slot();

        // Ensure the slot's target matches the current size.
        if slot.target.width() != sw || slot.target.height() != sh {
            slot.resize(backend, sw, sh)?;
        }

        // Sync per-frame buffers.
        slot.uniforms.sync(&[uniforms])?;
        slot.cells_bg.sync(&bg_cells)?;
        slot.cells.sync_from_slices(&fg_lists)?;

        // Begin the frame with a sync completion (present + health report).
        let mut frame = backend.begin_frame(Box::new(|_health, _sync| {}))?;
        {
            let pass = frame.render_pass(&[Attachment {
                texture: slot.target.texture(),
                clear_color: Some([0.0, 0.0, 0.0, 0.0]),
            }])?;

            // 1. Background color (full-screen triangle; reads bg_cells at
            //    buffer index 2 for padding-extend, but primarily fills the
            //    surface bg from the uniform). No vertex buffer.
            pass.step(&Step {
                pipeline_state: bg_color_pipeline.state(),
                vertex: None,
                uniforms: Some(slot.uniforms.buffer()),
                extras: &[Some(slot.cells_bg.buffer())],
                textures: &[],
                samplers: &[],
                draw: Draw::vertices(Primitive::Triangle, 3),
            });

            // 2. Per-cell backgrounds (full-screen triangle sampling bg_cells).
            pass.step(&Step {
                pipeline_state: cell_bg_pipeline.state(),
                vertex: None,
                uniforms: Some(slot.uniforms.buffer()),
                extras: &[Some(slot.cells_bg.buffer())],
                textures: &[],
                samplers: &[],
                draw: Draw::vertices(Primitive::Triangle, 3),
            });

            // 3. Text (instanced glyph quads). Vertex buffer 0 = CellText
            //    instances; extras[0] (buffer 2) = bg_cells (for min-contrast);
            //    textures 0/1 = grayscale/color atlas.
            pass.step(&Step {
                pipeline_state: cell_text_pipeline.state(),
                vertex: Some(slot.cells.buffer()),
                uniforms: Some(slot.uniforms.buffer()),
                extras: &[Some(slot.cells_bg.buffer())],
                textures: &[Some(slot.grayscale.texture()), Some(slot.color.texture())],
                samplers: &[],
                draw: Draw {
                    primitive: Primitive::TriangleStrip,
                    vertex_count: 4,
                    instance_count: fg_count,
                },
            });

            pass.complete();
        }
        frame.complete(true);

        let pixels = slot.target.read_pixels();
        guard.release();
        Ok(pixels)
    }

    /// Upload the grayscale atlas into the *next* slot's texture if it changed
    /// since we last synced that slot. Port of `syncAtlasTexture` +
    /// `drawFrame`'s modified-counter gate. Must be called after `update_frame`
    /// (which populates the atlas via glyph rendering) and before `draw_frame`.
    ///
    /// The color-atlas seam: the reduced cut renders text into the grayscale
    /// atlas only (emoji/color glyphs are deferred with the color atlas — see
    /// `docs/analysis/font-shaping.md`). We still keep the 1×1 color texture
    /// bound so the shader's two-atlas sampling is well-formed; no color glyph
    /// is ever emitted (`Atlas::Grayscale` on every instance).
    pub fn sync_atlas(&mut self, grid: &Grid) -> Result<(), MetalError> {
        let atlas = grid.atlas();
        let size = atlas.size() as usize;
        let modified = atlas.modified();

        // The slot draw_frame will use next is (frame_index + 1) % count. We
        // sync every slot lazily: simplest correct approach for sync mode
        // (one slot in play) is to sync the slot that next_frame will hand out.
        // Since next_frame advances the index, replicate that here without
        // consuming a permit: peek the next index.
        let next_index = self.swap_chain.peek_next_index();

        if modified <= self.grayscale_modified[next_index] {
            return Ok(());
        }
        self.grayscale_modified[next_index] = modified;

        // Grow the slot's grayscale texture if the atlas outgrew it, then
        // replace the full region. We rebuild the texture via the backend since
        // GpuTexture has no resize.
        let slot = self.swap_chain.slot_mut(next_index);
        if slot.grayscale.width() != size || slot.grayscale.height() != size {
            slot.grayscale = self.backend.new_texture(
                crate::gpu::TextureOptions {
                    format: crate::gpu::TextureFormat::R8Unorm,
                    usage: crate::gpu::TextureUsage::SHADER_READ,
                },
                size,
                size,
                None,
            )?;
        }
        slot.grayscale
            .replace_region(0, 0, size, size, atlas.data())?;
        Ok(())
    }
}

/// Build one of the three first-pixels pipelines by name from the R3 table.
fn build_pipeline(
    backend: &Metal,
    library: &objc2::runtime::ProtocolObject<dyn objc2_metal::MTLLibrary>,
    pixel_format: objc2_metal::MTLPixelFormat,
    name: &str,
) -> Result<Pipeline, MetalError> {
    let desc = shaders::PIPELINE_DESCRIPTIONS
        .iter()
        .find(|d| d.name == name)
        .expect("pipeline description exists");

    // Translate the R3 vertex-attribute table (if any) into the Metal
    // backend's VertexLayout.
    let attrs: Vec<VertexAttribute> = desc
        .vertex_attributes
        .unwrap_or(&[])
        .iter()
        .map(|a| VertexAttribute {
            format: map_vertex_format(a.format),
            offset: a.offset,
        })
        .collect();

    let vertex_layout = desc.vertex_attributes.map(|_| VertexLayout {
        stride: desc.stride,
        attributes: &attrs,
        step: match desc.step_fn {
            shaders::StepFunction::PerVertex => VertexStep::PerVertex,
            shaders::StepFunction::PerInstance => VertexStep::PerInstance,
        },
    });

    backend.new_pipeline(&PipelineOptions {
        vertex_fn: desc.vertex_fn,
        fragment_fn: desc.fragment_fn,
        vertex_library: library,
        fragment_library: library,
        vertex_layout,
        attachments: &[crate::metal::ColorAttachment {
            pixel_format,
            blending_enabled: desc.blending.enabled,
        }],
    })
}

/// Map an R3 (backend-agnostic) vertex format onto the Metal backend's format.
fn map_vertex_format(f: shaders::VertexFormat) -> VertexFormat {
    match f {
        shaders::VertexFormat::UChar4 => VertexFormat::UChar4,
        shaders::VertexFormat::UShort2 => VertexFormat::UShort2,
        shaders::VertexFormat::Short2 => VertexFormat::Short2,
        shaders::VertexFormat::UInt2 => VertexFormat::UInt2,
        shaders::VertexFormat::UChar => VertexFormat::UChar,
    }
}

/// A zero-value [`Uniforms`] (all fields overwritten in `build_uniforms`).
fn default_uniforms() -> Uniforms {
    Uniforms {
        projection_matrix: Mat::IDENTITY,
        screen_size: [0.0, 0.0],
        cell_size: [0.0, 0.0],
        grid_size: [0, 0],
        grid_padding: crate::wire::AlignedF32x4([0.0; 4]),
        padding_extend: PaddingExtend(0),
        min_contrast: 1.0,
        cursor_pos: [u16::MAX, u16::MAX],
        cursor_color: [0, 0, 0, 0],
        bg_color: [0, 0, 0, 255],
        bools: UniformBools::default(),
    }
}

/// The underline sprite for a snapshot underline style.
fn underline_sprite(u: SnapshotUnderline) -> Sprite {
    match u {
        SnapshotUnderline::None => Sprite::Underline, // unreachable in caller
        SnapshotUnderline::Single => Sprite::Underline,
        SnapshotUnderline::Double => Sprite::UnderlineDouble,
        SnapshotUnderline::Curly => Sprite::UnderlineCurly,
        SnapshotUnderline::Dotted => Sprite::UnderlineDotted,
        SnapshotUnderline::Dashed => Sprite::UnderlineDashed,
    }
}

/// Resolve a [`SnapshotColor`] into a concrete [`Rgb`] through the palette /
/// default. Port of the `Color.fg`/`Color.bg` resolution reduced to the
/// snapshot color model.
fn resolve_color(c: SnapshotColor, palette: &Palette, default: Rgb) -> Rgb {
    match c {
        SnapshotColor::Default => default,
        SnapshotColor::Palette(i) => palette[i as usize],
        SnapshotColor::Rgb { r, g, b } => Rgb::new(r, g, b),
    }
}

/// Resolve a cell's final (fg, bg, bg_alpha), honoring the inverse flag and the
/// "covering glyph uses fg for bg" rule. Reduced port of `rebuildRow`'s color
/// resolution (no selection/search/highlight branches).
///
/// `bg_alpha` is 0 when the cell has no explicit bg and isn't inverse (so the
/// surface bg shows through), 255 otherwise — the CPU-side min-contrast /
/// bg-fill decision the shader uniforms feed off.
fn resolve_colors(
    style: &CellStyle,
    cp: u32,
    palette: &Palette,
    default_fg: Rgb,
    default_bg: Rgb,
) -> (Rgb, Rgb, u8) {
    let fg_style = resolve_color(style.fg, palette, default_fg);
    let has_explicit_bg = style.bg != SnapshotColor::Default;
    let bg_style = resolve_color(style.bg, palette, default_bg);

    // Two cases use fg-as-bg: the inverse flag, or a "covering" glyph (e.g.
    // FULL BLOCK) — but not both (they cancel). Upstream `rebuildRow`.
    let use_fg_for_bg = style.inverse != cells::is_covering(cp);

    let fg = if style.inverse { bg_style } else { fg_style };
    let bg = if use_fg_for_bg { fg_style } else { bg_style };

    // Paint the bg (alpha 255) when the cell is inverse, covering, or has an
    // explicit bg; otherwise leave alpha 0 so the surface bg shows through.
    let bg_alpha: u8 = if use_fg_for_bg || has_explicit_bg {
        255
    } else {
        0
    };

    (fg, bg, bg_alpha)
}
