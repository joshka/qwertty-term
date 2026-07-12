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
//! frame it consumes a [`RenderSnapshot`] and a `qwertty-term-font` [`Grid`] to
//! rebuild the buffers, then draws. See `docs/analysis/renderer-r4.md`.

use std::collections::HashMap;

use qwertty_term_font::grid::Grid;
use qwertty_term_font::{AtlasKind, FontIndex, ShapedCell, Style};
use qwertty_term_sprite::Sprite;
use qwertty_term_vt::color::{Palette, Rgb};
use qwertty_term_vt::page::size::CellCountInt;
use qwertty_term_vt::snapshot::{CellStyle, SnapshotCell, SnapshotColor, SnapshotUnderline};

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

/// Cache key for a shaped text run: the resolved face (a [`FontIndex`], which
/// uniquely names the styled face including its `wght`/style) plus the exact
/// codepoint sequence fed to the shaper. Two runs with the same face and the
/// same codepoints shape identically regardless of where they sit on the grid,
/// so the key is position-independent — the analog of upstream's
/// `font/shaper/Cache.zig` run hash (font index + codepoints + style), reduced
/// to our single-size single-collection scope.
#[derive(Clone, PartialEq, Eq, Hash)]
struct RunKey {
    index: FontIndex,
    codepoints: Vec<char>,
}

/// A memoized shaped run: the shaper output for a [`RunKey`]. Caching the
/// *output* (not a live `Shaper`, which borrows the face bytes) lets an
/// unchanged run skip re-shaping on the next full-redraw frame. Upstream caches
/// `[]font.shape.Cell` keyed by the run hash; this is the same idea.
type RunCache = HashMap<RunKey, Vec<ShapedCell>>;

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
    /// Per-slot "last synced color-atlas modified counter" (upstream
    /// `frame.color_modified`), the color-atlas analog of `grayscale_modified`.
    color_modified: Vec<usize>,
    /// Current pixel dimensions of the render target (grid size × cell size in
    /// the reduced cut: no window padding yet).
    screen_width: usize,
    screen_height: usize,
    /// Shaped-run memoization (upstream `font_shaper_cache`). Persists across
    /// frames so an unchanged run is re-shaped only when its content changes;
    /// keeps steady-state cost near the per-cell path even though the reduced
    /// cut rebuilds every visible row every frame.
    run_cache: RunCache,
    /// The [`FrameKey`] of the last frame we built cell contents for, or `None`
    /// before the first frame. Persisting this across frames is what lets the
    /// engine detect the cross-frame full-rebuild triggers (screen switch,
    /// viewport move, resize) the way upstream's `RenderState` compares against
    /// its own `self.screen`/`self.viewport_pin`/`self.rows`/`self.cols`.
    prev_frame_key: Option<crate::snapshot::FrameKey>,
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

    /// Build the engine over a fresh Metal context with cell geometry read
    /// from `grid`'s own metrics — the grid that will shape and rasterize
    /// every frame is the single source of truth for cell size, so the two
    /// can never disagree. Prefer this over [`Engine::new`] when a font
    /// `Grid` already exists (an embedder always has one).
    pub fn for_grid(grid: &Grid) -> Result<Engine, MetalError> {
        let backend = Metal::new()?;
        Engine::with_backend_for_grid(backend, grid)
    }

    /// [`Engine::for_grid`] over an existing Metal context (shared backends,
    /// graceful no-device skip paths).
    pub fn with_backend_for_grid(backend: Metal, grid: &Grid) -> Result<Engine, MetalError> {
        let metrics = grid.metrics();
        Engine::with_backend(backend, metrics.cell_width, metrics.cell_height)
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
            color_modified: vec![0; slot_count],
            screen_width: 0,
            screen_height: 0,
            run_cache: RunCache::new(),
            prev_frame_key: None,
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
    /// **Dirty tracking** (port of `rebuildCells`'s `rebuild`/`row_dirty`
    /// logic): the engine persists the previous frame's [`crate::snapshot::
    /// FrameKey`] so it can detect the cross-frame full-rebuild triggers
    /// (screen switch, viewport move, resize) the way upstream's `RenderState`
    /// compares `self.screen`/`self.viewport_pin`/`self.rows`/`self.cols`. When
    /// a full rebuild is forced (first frame, resize, screen switch, viewport
    /// move, or a global dirty signal — palette/selection/clear/etc), it clears
    /// and rebuilds every row (upstream `self.cells.reset()` + rebuild-all).
    /// Otherwise it clears and rebuilds *only* the per-row dirty rows the
    /// snapshot reports, leaving clean rows' GPU cells untouched (upstream
    /// `if (!dirty.*) continue; self.cells.clear(y);`).
    pub fn update_frame<S: RenderSnapshot>(
        &mut self,
        snapshot: &S,
        grid: &mut Grid,
        opts: FrameOptions,
    ) {
        let cols = snapshot.cols();
        let rows = snapshot.rows();
        let signals = snapshot.dirty_signals();

        // Resize the contents (and thus target/uniforms geometry) if the grid
        // changed. A size change forces a full rebuild (upstream `grid_size_diff`).
        let size_changed = self.contents.cols() != cols || self.contents.rows() != rows;
        if size_changed {
            self.contents.resize(cols, rows);
            self.screen_width = cols * self.cell_width as usize;
            self.screen_height = rows * self.cell_height as usize;
        }

        // Decide full-vs-partial. Mirrors upstream `RenderState.update`'s
        // `redraw` (global flags OR screen-key OR viewport-pin OR dims changed)
        // combined with `rebuildCells`'s `grid_size_diff`.
        let full_rebuild = size_changed
            || signals.global_forces_full
            || self.prev_frame_key != Some(signals.frame_key);
        self.prev_frame_key = Some(signals.frame_key);

        if full_rebuild {
            // Full rebuild clears the entire cell buffer (upstream
            // `self.cells.reset()`); every row is then rebuilt below.
            self.contents.reset();
        }

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

        // The cursor column, when the cursor is drawn and on this row. Upstream
        // breaks shaper runs at the cursor position (`run.zig` cursor_x
        // handling) so a ligature never spans the cursor — the cursor overlay
        // then draws over separate, un-ligated cells. We mirror that: a run
        // breaks before and after `cursor_x` on the cursor's row. Only applies
        // when a cursor is actually shown (`cursor_style` resolved).
        let cursor_row_col = match (cursor.as_ref(), cursor_style.as_ref()) {
            (Some(c), Some(_)) => Some((c.row, c.col)),
            _ => None,
        };

        // Rebuild each row. On a full rebuild every row is rebuilt (the buffer
        // was just reset). On a partial rebuild only the dirty rows are cleared
        // and rebuilt; clean rows keep their existing GPU cells (upstream
        // `if (!dirty.*) continue; self.cells.clear(y);`).
        for y in 0..rows {
            if !full_rebuild {
                let is_dirty = signals.row_dirty.get(y).copied().unwrap_or(true);
                if !is_dirty {
                    continue;
                }
                // Clear this row's cells before rebuilding (upstream
                // `self.cells.clear(y)`); a full rebuild already reset all rows.
                self.contents.clear(y as CellCountInt);
            }
            let row = snapshot.row(y);
            let cursor_x = cursor_row_col.and_then(|(cr, cc)| (cr == y).then_some(cc));
            self.rebuild_row(y, row, grid, palette, default_fg, default_bg, cursor_x);
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

    /// Rebuild one row: per-cell background colors + decorations (per-cell,
    /// like upstream), then shaped foreground glyphs placed by *run*. Port of
    /// `rebuildRow` reduced to the snapshot model (no selection/search/
    /// highlight/link/preedit branches).
    ///
    /// Decorations (underline/overline/strikethrough) and background stay
    /// per-cell — upstream draws them per-cell too, independent of run
    /// segmentation. Foreground glyphs go through [`Engine::rebuild_row_runs`],
    /// which groups consecutive cells into shaper runs so multi-cell ligatures
    /// (`->`, `=>`, `==`) form on screen.
    #[allow(clippy::too_many_arguments)]
    fn rebuild_row(
        &mut self,
        y: usize,
        row: &[SnapshotCell],
        grid: &mut Grid,
        palette: &Palette,
        default_fg: Rgb,
        default_bg: Rgb,
        cursor_x: Option<usize>,
    ) {
        let cols = self.contents.cols();

        // --- Per-cell background + decorations (unchanged from the per-cell
        //     path; upstream keeps these per-cell regardless of runs). ---
        let mut x = 0usize;
        while x < cols && x < row.len() {
            let cell = &row[x];

            // Spacer tail of a wide glyph: no bg fill, no decoration (the lead
            // cell painted it). Advance one.
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

            // Invisible cells: bg only, no decoration.
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
            // Strikethrough draws last (over text). It's added after the run
            // glyphs below (glyph order within a row list doesn't affect the
            // GPU blend — each instance is an independent quad — but keep the
            // upstream ordering intent: strikethrough over text).
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

        // --- Foreground glyphs by run (multi-cell ligatures form here). ---
        self.rebuild_row_runs(y, row, grid, palette, default_fg, default_bg, cursor_x);
    }

    /// Segment `row` into shaper runs and emit glyphs for each. A run is a
    /// maximal span of consecutive non-spacer cells that share:
    ///
    /// - the same **foreground style** (bold/italic/fg color/faint/invisible),
    ///   mirroring upstream's `comparableStyle` break (background differences
    ///   don't break a run — they're per-cell above);
    /// - the same **resolved font index** (a style change or a codepoint that
    ///   resolves to a different face breaks the run — this also isolates
    ///   sprite cells and fallback/emoji cells into their own runs, matching
    ///   upstream where a font-index change breaks the run and special/sprite
    ///   fonts shape trivially as `codepoint == glyph`);
    /// - and it does **not span the cursor**: the run breaks before and after
    ///   `cursor_x` (upstream `run.zig` cursor handling) so a ligature never
    ///   forms across the cursor cell.
    ///
    /// Each text run is shaped once (through the run cache) with the run's face;
    /// glyphs are placed by cluster (`cell_x`), leaving ligature-continuation
    /// cells glyph-less. Sprite / fallback runs take the non-shaped per-cell
    /// path (they're single-cell runs).
    #[allow(clippy::too_many_arguments)]
    fn rebuild_row_runs(
        &mut self,
        y: usize,
        row: &[SnapshotCell],
        grid: &mut Grid,
        palette: &Palette,
        default_fg: Rgb,
        default_bg: Rgb,
        cursor_x: Option<usize>,
    ) {
        let cols = self.contents.cols().min(row.len());

        let mut x = 0usize;
        while x < cols {
            let cell = &row[x];

            // Spacer: covered by a prior lead cell's glyph; never starts a run.
            if cell.is_spacer() {
                x += 1;
                continue;
            }

            let style = &cell.style;
            // Blank / invisible cells emit no glyph and don't extend a run.
            let cp = cell.ch as u32;
            if style.invisible || cp == 0 || cell.ch == ' ' {
                x += 1;
                continue;
            }

            let fg = resolve_colors(style, cp, palette, default_fg, default_bg).0;
            let alpha: u8 = if style.faint { 128 } else { 255 };
            let font_style = style_of(style);

            // Resolve the lead cell's font index; it classifies the run.
            let Some(index) = grid.get_index_styled(cp, font_style) else {
                x += 1;
                continue;
            };

            // Sprite and non-primary (fallback/emoji) cells are single-cell:
            // they don't shape as a run (upstream shapes special fonts as
            // codepoint==glyph, and a fallback face's glyph ids don't map into
            // the primary shaping). Emit one glyph and advance.
            let is_shapeable_primary = matches!(index, FontIndex::Face { slot: 0, .. });
            if !is_shapeable_primary {
                self.emit_nonprimary_cell(x, y, index, cp, font_style, grid, fg, alpha);
                x += 1;
                continue;
            }

            // Grow a text run over consecutive cells that keep the same
            // foreground style, resolve to the SAME primary index, and don't
            // cross the cursor. The run breaks at BOTH edges of the cursor cell
            // (upstream `run.zig`): the extension from cell `k` to `k+1` is
            // forbidden if either `k` or `k+1` is the cursor, so the cursor cell
            // is always alone or at a run edge and no ligature spans it.
            let run_start = x;
            let mut run_end = x + 1; // exclusive
            while run_end < cols {
                // Cursor break at either edge of the boundary being crossed:
                // last-included cell (run_end-1) is the cursor, or the next cell
                // (run_end) is the cursor.
                if cursor_x == Some(run_end) || cursor_x == Some(run_end - 1) {
                    break;
                }
                let next = &row[run_end];
                if next.is_spacer() {
                    // A spacer belongs to the wide lead already inside the run;
                    // include it so the run's cell span stays contiguous (the
                    // shaper gets no codepoint for it — see codepoint gather).
                    run_end += 1;
                    continue;
                }
                let ncp = next.ch as u32;
                if next.style.invisible || ncp == 0 || next.ch == ' ' {
                    break;
                }
                // Same foreground style?
                if !comparable_style(style, &next.style) {
                    break;
                }
                // Same primary face for this codepoint?
                match grid.get_index_styled(ncp, style_of(&next.style)) {
                    Some(FontIndex::Face { slot: 0, style: s }) if s == font_style => {}
                    _ => break,
                }
                run_end += 1;
            }

            // Gather the run's codepoints with **run-relative** cell-X clusters
            // (`cx - run_start`), so the shaper output — and thus the cache
            // entry — is position-independent within the row (upstream hashes
            // relative cluster positions). The placement offset `run_start` is
            // added back in `emit_text_run`. Spacers contribute no codepoint
            // (the wide lead's advance covers them).
            let mut clusters: Vec<(char, u32)> = Vec::with_capacity(run_end - run_start);
            let mut codepoints: Vec<char> = Vec::with_capacity(run_end - run_start);
            for (cx, c) in row.iter().enumerate().take(run_end).skip(run_start) {
                if c.is_spacer() {
                    continue;
                }
                let rel = (cx - run_start) as u32;
                clusters.push((c.ch, rel));
                codepoints.push(c.ch);
                // Grapheme continuation codepoints share the cell's cluster
                // (upstream adds each grapheme codepoint with the same cluster).
                for &g in &c.combining {
                    clusters.push((g, rel));
                    codepoints.push(g);
                }
            }

            self.emit_text_run(
                y,
                run_start,
                index,
                &clusters,
                &codepoints,
                grid,
                fg,
                alpha,
                row,
            );

            x = run_end;
        }
    }

    /// Emit the glyph for a single non-primary cell (sprite, or a
    /// fallback/emoji face). Mirrors the old per-cell path's sprite and
    /// `slot != 0` branches.
    #[allow(clippy::too_many_arguments)]
    fn emit_nonprimary_cell(
        &mut self,
        x: usize,
        y: usize,
        index: FontIndex,
        cp: u32,
        font_style: Style,
        grid: &mut Grid,
        fg: Rgb,
        alpha: u8,
    ) {
        match index {
            FontIndex::Sprite => {
                // Sprite: codepoint == glyph id, no shaping. Apply the Nerd
                // constraint via render_codepoint (which routes nerd PUA cps).
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
            // Fallback/emoji face: resolve the glyph through the face's own
            // cmap (shaping against the primary would sample wrong ids).
            _ => {
                if let Ok(Some(g)) = grid.render_codepoint_styled(cp, font_style) {
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
                        atlas_from_kind(g.atlas),
                        false,
                        false,
                        Key::Text,
                    );
                }
            }
        }
    }

    /// Shape a primary-face text run once (through the run cache) and emit its
    /// glyphs, each placed at its cluster's cell-X. Port of the primary-face
    /// arm of upstream's shaped-glyph emission: one glyph per output cluster,
    /// continuation cells (2nd half of a whole-ligature) left glyph-less.
    ///
    /// Falls back to the unshaped per-codepoint path for a byte-less face
    /// (name-loaded system faces whose primary `Face` has no `source_bytes`),
    /// preserving the default-fg-ink fix for named families that can't shape.
    #[allow(clippy::too_many_arguments)]
    fn emit_text_run(
        &mut self,
        y: usize,
        run_start: usize,
        index: FontIndex,
        clusters: &[(char, u32)],
        codepoints: &[char],
        grid: &mut Grid,
        fg: Rgb,
        alpha: u8,
        row: &[SnapshotCell],
    ) {
        if clusters.is_empty() {
            return;
        }

        // Shape (or reuse) the run. `None` => the face has no byte-backed
        // shaper; fall back to per-codepoint CoreText rendering per cell.
        // Clusters/`cell_x` are run-relative; the absolute grid column is
        // `run_start + rel`.
        let shaped = match self.shape_run_cached(grid, index, clusters, codepoints) {
            Some(s) => s,
            None => {
                for &(ch, rel) in clusters {
                    let cx = run_start + rel as usize;
                    // Only render the base char of each cell (combining marks
                    // can't be placed without shaping; deferred, as before).
                    // Detect a base cell by matching the row cell's `ch`.
                    if row.get(cx).map(|c| c.ch) == Some(ch) {
                        self.render_cell_unshaped(
                            cx,
                            y,
                            ch as u32,
                            grid_style_of(index),
                            grid,
                            fg,
                            alpha,
                        );
                    }
                }
                return;
            }
        };

        for sc in shaped {
            let cx = run_start + sc.cell_x as usize;
            // Apply the Nerd Fonts per-codepoint constraint for PUA icon cells
            // (keyed by the ORIGINATING codepoint at this cell, matching
            // upstream where the codepoint range gates the constraint). A PUA
            // icon shapes as a single-cell run so this is well-defined.
            let cp_here = row.get(cx).map(|c| c.ch as u32).unwrap_or(0);
            let g = match grid.render_glyph_nerd(index, sc.glyph_index, cp_here) {
                Ok(g) => g,
                Err(_) => continue,
            };
            if g.width == 0 || g.height == 0 {
                continue;
            }
            self.push_text_cell(
                cx,
                y,
                fg,
                alpha,
                &g,
                sc.x_offset,
                sc.y_offset,
                atlas_from_kind(g.atlas),
                cells::no_min_contrast(cp_here),
                false,
                Key::Text,
            );
        }
    }

    /// Look up (or shape and cache) a run's glyphs. Returns `None` for a face
    /// with no byte-backed shaper.
    fn shape_run_cached(
        &mut self,
        grid: &Grid,
        index: FontIndex,
        clusters: &[(char, u32)],
        codepoints: &[char],
    ) -> Option<Vec<ShapedCell>> {
        let key = RunKey {
            index,
            codepoints: codepoints.to_vec(),
        };
        if let Some(cached) = self.run_cache.get(&key) {
            return Some(cached.clone());
        }
        // Cache miss: shape now. A byte-less face yields `None` (not cached, so
        // a later byte-backed load could still shape — but faces don't change
        // identity mid-session, so this is just correctness insurance).
        let shaped = self.shape_run(grid, index, clusters)?;
        self.run_cache.insert(key, shaped.clone());
        Some(shaped)
    }

    /// Render one cell's glyph without shaping, resolving the codepoint through
    /// the styled face's own cmap and rasterizing via CoreText
    /// (`render_codepoint_styled`). This is the fallback for name-loaded system
    /// faces whose primary `Face` has no `source_bytes` (so [`Engine::shape_cell`]
    /// returns `None`): shaping needs the bytes, but CoreText glyph lookup +
    /// rasterization does not, so the glyph still draws.
    ///
    /// The tradeoff vs the shaped path is no ligatures / kerning / GSUB for
    /// named faces (a documented deferral of the single-font reduced cut), but
    /// for a monospace terminal the per-cell advance is fixed anyway, so the
    /// visible result is correct for the common (non-ligature) case. Sprites and
    /// non-primary fallback faces already take their own non-shaped paths in the
    /// caller, so this only ever runs for the primary face.
    #[allow(clippy::too_many_arguments)]
    fn render_cell_unshaped(
        &mut self,
        x: usize,
        y: usize,
        cp: u32,
        style: Style,
        grid: &mut Grid,
        fg: Rgb,
        alpha: u8,
    ) {
        if let Ok(Some(g)) = grid.render_codepoint_styled(cp, style) {
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
                atlas_from_kind(g.atlas),
                false,
                false,
                Key::Text,
            );
        }
    }

    /// Shape a run's `(char, cell_x)` cluster sequence into shaped cells via the
    /// face resolved at `index`. Returns `None` if the face has no byte-backed
    /// shaper (name-loaded system faces; the caller then renders unshaped).
    ///
    /// Shaping against the *resolved* (styled) face means a bold/italic run
    /// shapes with its own face, so the shaper applies that face's `wght`
    /// variation (see [`qwertty_term_font::Shaper::new`]) and its own cmap/GSUB. The
    /// caller supplies cell-X as the cluster (upstream's cluster == cell X under
    /// `BufferClusterLevel::Characters`), so a multi-cell ligature keeps one
    /// glyph per output cluster placed at the originating cell.
    fn shape_run(
        &self,
        grid: &Grid,
        index: FontIndex,
        clusters: &[(char, u32)],
    ) -> Option<Vec<ShapedCell>> {
        let face = grid
            .resolver()
            .collection()
            .get_face(index)
            .unwrap_or_else(|| grid.resolver().collection().primary());
        let mut shaper = qwertty_term_font::Shaper::new(face)?;
        Some(shaper.shape_run_with_clusters(clusters.iter().copied()))
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
        glyph: &qwertty_term_font::CachedGlyph,
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
        cursor: Option<qwertty_term_vt::snapshot::SnapshotCursor>,
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

    /// Render one complete offscreen frame: [`Engine::update_frame`] +
    /// [`Engine::sync_atlas`] + [`Engine::draw_frame`] in the only order that
    /// works, returning the readback as a [`Frame`] (dimensions + pixels
    /// together). The one-call embedder path; the three underlying steps stay
    /// public for hosts that need to interleave them (e.g. a window host that
    /// presents instead of reading back).
    pub fn render<S: RenderSnapshot>(
        &mut self,
        snapshot: &S,
        grid: &mut Grid,
        opts: FrameOptions,
    ) -> Result<Frame, MetalError> {
        self.update_frame(snapshot, grid, opts);
        self.sync_atlas(grid)?;
        let bgra = self.draw_frame()?;
        let (width, height) = self.screen_size();
        Ok(Frame {
            width,
            height,
            bgra,
        })
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

    /// Upload the grayscale **and** color atlases into the *next* slot's
    /// textures if they changed since we last synced that slot. Port of
    /// `syncAtlasTexture` (called once per atlas) + `drawFrame`'s
    /// modified-counter gate (`grayscale_modified` / `color_modified`). Must be
    /// called after `update_frame` (which populates both atlases via glyph
    /// rendering) and before `draw_frame`.
    ///
    /// The grayscale atlas holds text outlines + sprites (1-byte alpha texels,
    /// `R8Unorm`); the color atlas holds emoji / color glyphs (4-byte
    /// premultiplied BGRA texels, `Bgra8Unorm`). Each glyph instance's
    /// `CellText.atlas` selects which texture the shader samples.
    pub fn sync_atlas(&mut self, grid: &Grid) -> Result<(), MetalError> {
        // The slot draw_frame will use next is (frame_index + 1) % count. We
        // sync the slot that next_frame will hand out; peek it without
        // consuming a permit.
        let next_index = self.swap_chain.peek_next_index();

        self.sync_grayscale(grid, next_index)?;
        self.sync_color(grid, next_index)?;
        Ok(())
    }

    /// Sync the grayscale atlas (`R8Unorm`, 1 byte/texel) into slot
    /// `next_index`. Port of `syncAtlasTexture` for the grayscale atlas.
    fn sync_grayscale(&mut self, grid: &Grid, next_index: usize) -> Result<(), MetalError> {
        let atlas = grid.atlas();
        let size = atlas.size() as usize;
        let modified = atlas.modified();

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

    /// Sync the color atlas (`Bgra8Unorm`, 4 bytes/texel) into slot
    /// `next_index`. Port of `syncAtlasTexture` for the color atlas — identical
    /// to the grayscale path except the pixel format is BGRA and the texel is
    /// 4 bytes wide (the atlas's `data()` is already the tightly-packed BGRA
    /// buffer `replace_region` expects).
    fn sync_color(&mut self, grid: &Grid, next_index: usize) -> Result<(), MetalError> {
        let atlas = grid.color_atlas();
        let size = atlas.size() as usize;
        let modified = atlas.modified();

        if modified <= self.color_modified[next_index] {
            return Ok(());
        }
        self.color_modified[next_index] = modified;

        let slot = self.swap_chain.slot_mut(next_index);
        if slot.color.width() != size || slot.color.height() != size {
            slot.color = self.backend.new_texture(
                crate::gpu::TextureOptions {
                    // `*_srgb` so the GPU auto-linearizes on sample: the
                    // ATLAS_COLOR shader branch assumes premultiplied *linear*
                    // texels. Matches upstream `initAtlasTexture`
                    // (`.bgra => .bgra8unorm_srgb`).
                    format: crate::gpu::TextureFormat::Bgra8UnormSrgb,
                    usage: crate::gpu::TextureUsage::SHADER_READ,
                },
                size,
                size,
                None,
            )?;
        }
        slot.color.replace_region(0, 0, size, size, atlas.data())?;
        Ok(())
    }
}

/// Accessors used by the additive presentation path (`crate::present`, R5).
/// These expose the per-frame CPU data and the disjoint field borrows the
/// on-screen draw needs, without duplicating the private field layout or
/// changing any R4 behavior.
impl Engine {
    /// Current render-target pixel width (0 until the first `update_frame`).
    pub(crate) fn screen_width(&self) -> usize {
        self.screen_width
    }

    /// Current render-target pixel height (0 until the first `update_frame`).
    pub(crate) fn screen_height(&self) -> usize {
        self.screen_height
    }

    /// A copy of the current frame uniforms (cheap `Copy` struct).
    pub(crate) fn uniforms_snapshot(&self) -> Uniforms {
        self.uniforms
    }

    /// A copy of the current per-cell background instances.
    pub(crate) fn bg_cells_snapshot(&self) -> Vec<crate::wire::CellBg> {
        self.contents.bg_cells().to_vec()
    }

    /// The number of foreground (glyph) instances in the current frame.
    pub(crate) fn fg_count(&self) -> usize {
        self.contents.fg_count()
    }

    /// The per-row foreground instance lists, cloned into an owned Vec of Vecs
    /// so the borrow of `self.contents` ends before the disjoint field borrows
    /// in [`Engine::present_parts`].
    pub(crate) fn fg_lists_snapshot(&self) -> Vec<Vec<CellText>> {
        self.contents.fg_lists().to_vec()
    }

    /// Disjoint field borrows for the presentation draw: the backend, the swap
    /// chain, and the three pipelines. Mirrors the destructuring in
    /// `draw_frame` so the presentation path can hold the slot guard across
    /// backend calls without a self-aliasing conflict.
    pub(crate) fn present_parts(
        &mut self,
    ) -> (
        &Metal,
        &mut SwapChain<Metal>,
        &Pipeline,
        &Pipeline,
        &Pipeline,
    ) {
        (
            &self.backend,
            &mut self.swap_chain,
            &self.bg_color_pipeline,
            &self.cell_bg_pipeline,
            &self.cell_text_pipeline,
        )
    }
}

/// A rendered frame read back from the offscreen target, as returned by
/// [`Engine::render`]: pixel dimensions and pixels travel together, and the
/// pixel format is stated by the accessor you call instead of assumed.
///
/// The readback is stored as it comes off the GPU — BGRA8, row-major, tightly
/// packed (row padding already stripped) — so [`Frame::bgra`] is free and
/// [`Frame::to_rgba`] pays one swizzled copy (the layout PNG encoders and
/// `image`-crate buffers expect).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    width: usize,
    height: usize,
    bgra: Vec<u8>,
}

impl Frame {
    /// Width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// The raw readback pixels: BGRA8, row-major, tightly packed.
    pub fn bgra(&self) -> &[u8] {
        &self.bgra
    }

    /// Consume the frame, taking the raw BGRA8 buffer without a copy.
    pub fn into_bgra(self) -> Vec<u8> {
        self.bgra
    }

    /// The pixels swizzled to RGBA8 (one copy), row-major, tightly packed.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(self.bgra.len());
        for px in self.bgra.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
        rgba
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

/// Map a font-grid [`AtlasKind`] onto the frozen wire [`Atlas`] selector. The
/// grid tags each cached glyph with the atlas it was uploaded to; the shader
/// reads the wire `atlas` field to pick `textureGrayscale` (0) vs
/// `textureColor` (1).
fn atlas_from_kind(kind: AtlasKind) -> Atlas {
    match kind {
        AtlasKind::Grayscale => Atlas::Grayscale,
        AtlasKind::Color => Atlas::Color,
    }
}

/// Map a cell's bold/italic attributes to a font [`Style`] (upstream
/// `renderer/cell.zig` derives the style from the cell's `bold`/`italic` flags
/// the same way).
fn style_of(style: &CellStyle) -> Style {
    match (style.bold, style.italic) {
        (false, false) => Style::Regular,
        (true, false) => Style::Bold,
        (false, true) => Style::Italic,
        (true, true) => Style::BoldItalic,
    }
}

/// The font [`Style`] a resolved [`FontIndex`] belongs to (for the byte-less
/// fallback path, which needs the style to re-resolve the codepoint's glyph
/// through the styled face's cmap). Sprite indices are style-agnostic; report
/// `Regular` (the fallback path never runs for sprites anyway).
fn grid_style_of(index: FontIndex) -> Style {
    match index {
        FontIndex::Face { style, .. } => style,
        FontIndex::Sprite => Style::Regular,
    }
}

/// Whether two cells share a *comparable* foreground style for run-breaking
/// purposes — the analog of upstream `run.zig`'s `comparableStyle`, adapted to
/// our model where a run carries **one** resolved `fg` for all its glyphs.
///
/// Upstream computes each glyph cell's fg independently, so its `comparableStyle`
/// can ignore background. We share one fg across the run, so we must break
/// whenever the *effective* foreground would differ. The effective fg depends on
/// `fg`, `inverse` (swaps in `bg`), and — when inverse is set — `bg` too; and
/// on `faint` (alpha). We therefore break on any of `bold`/`italic` (font face),
/// `faint`/`invisible` (alpha/skip), `inverse`, `fg`, and `bg`. This is stricter
/// than upstream (it may split a run where upstream wouldn't, e.g. a background
/// color change), which only loses ligature opportunities across those
/// boundaries — never produces a wrong glyph or color.
///
/// Decorations (underline/overline/strikethrough) draw per-cell independently
/// and don't need to break the glyph run (upstream keys `comparableStyle` on
/// shaping/fg attributes, not decorations).
fn comparable_style(a: &CellStyle, b: &CellStyle) -> bool {
    a.bold == b.bold
        && a.italic == b.italic
        && a.faint == b.faint
        && a.invisible == b.invisible
        && a.inverse == b.inverse
        && a.fg == b.fg
        && a.bg == b.bg
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
