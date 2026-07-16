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
//! The engine owns a [`Contents`](crate::cells::Contents), the
//! [`Uniforms`](crate::wire::Uniforms), and a [`SwapChain`](crate::swap_chain::SwapChain);
//! each frame it consumes a [`RenderSnapshot`](crate::snapshot::RenderSnapshot) and a
//! `qwertty-term-font` [`Grid`](qwertty_term_font::grid::Grid) to
//! rebuild the buffers, then draws. See `docs/analysis/renderer-r4.md`.
//!
//! # Embeddability quickstart
//!
//! The whole headless "VT bytes in, pixels out" story — feed a terminal,
//! snapshot it, render one offscreen frame, read back the pixels. This is the
//! shape a recorder like [betamax](https://github.com/joshka/betamax) embeds;
//! `examples/frame-capture` is the same flow wired to a PNG encoder.
//!
//! This example uses the platform-free [`Software`](crate::software::Software)
//! backend, so it renders the same way on macOS and headless Linux — no GPU, no
//! window. `Face` is the cfg-selected platform face (CoreText on macOS, FreeType
//! on Linux); the flow below is identical either way.
//!
//! ```no_run
//! use qwertty_term_font::{CodepointResolver, Collection, Face, Grid, Metrics};
//! use qwertty_term_renderer::engine::{Engine, FrameOptions};
//! use qwertty_term_renderer::snapshot::FullSnapshot;
//! use qwertty_term_renderer::software::Software;
//! use qwertty_term_vt::stream::{Stream, TerminalHandler};
//! use qwertty_term_vt::terminal::{Options, Terminal};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // 1. Font substrate: embedded JetBrains Mono → deterministic metrics + atlas.
//! let face = Face::load_embedded(16.0)?;
//! let metrics = Metrics::calc(face.face_metrics());
//! let resolver = CodepointResolver::new(Collection::new(face));
//! let mut grid = Grid::new(resolver, metrics)?;
//!
//! // 2. Terminal state machine: feed it raw VT bytes.
//! let terminal = Terminal::new(Options { cols: 20, rows: 4, ..Default::default() });
//! let mut stream = Stream::new(TerminalHandler::new(terminal));
//! stream.feed(b"\x1b[1;32mhello\x1b[0m");
//!
//! // 3. Engine over the headless CPU backend, cell geometry read from the grid.
//! let mut engine = Engine::with_backend_for_grid(Software::new(), &grid)?;
//!
//! // 4. Snapshot the live screen and render one frame in a single call.
//! let snapshot = FullSnapshot::capture_live(stream.terminal());
//! let frame = engine.render(&snapshot, &mut grid, FrameOptions::default())?;
//!
//! // 5. Pixels, format stated by the accessor: RGBA for PNG/`image` buffers.
//! assert_eq!(frame.bgra().len(), frame.width() * frame.height() * 4);
//! let _rgba = frame.to_rgba();
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;

use qwertty_term_font::grid::Grid;
use qwertty_term_font::{AtlasKind, FontIndex, ShapedCell, Style};
use qwertty_term_sprite::Sprite;
use qwertty_term_vt::color::{Palette, Rgb};
use qwertty_term_vt::page::size::CellCountInt;
use qwertty_term_vt::snapshot::{CellStyle, SnapshotCell, SnapshotColor, SnapshotUnderline};

use crate::cells::{self, Contents, Key};
use crate::cursor::{self, Style as CursorStyle};
use crate::gpu::{
    Attachment, Draw, GpuBackend, GpuBuffer, GpuFrame, GpuRenderPass, GpuTarget, GpuTexture,
    Primitive, ShaderSource, Step,
};
#[cfg(target_os = "macos")]
use crate::metal::{Buffer, Metal, MetalError, Pipeline};
use crate::shaders;
use crate::snapshot::{KittyImage, KittyPlacement, RenderSnapshot};
use crate::swap_chain::{SwapChain, SwapChainMode};
use crate::wire::{
    Atlas, CellText, CellTextBools, Image, Mat, PaddingExtend, UniformBools, Uniforms,
};

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
    /// The viewport cell `(col, row)` the mouse is hovering, if any. When it
    /// lands on an OSC8 hyperlink, every visible cell of that link gets a
    /// forced underline (R7). `None` disables the hover underline. The host
    /// supplies this from its mouse plumbing.
    pub hovered_cell: Option<(usize, usize)>,
    /// Force a full cell rebuild this frame regardless of the snapshot's dirty
    /// signals. The host sets this when a purely host-side overlay that tints
    /// existing cells changed without moving the viewport or dirtying any engine
    /// row — e.g. the search-match highlight when the needle changes but the
    /// matches are already on screen. Without it the partial-rebuild path skips
    /// the clean rows and the tint never reaches the GPU. Analogous to the
    /// engine's `selection` global-dirty bit, but for overlays the engine
    /// doesn't know about.
    pub force_full_rebuild: bool,
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
            hovered_cell: None,
            force_full_rebuild: false,
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

/// The default render backend for an unparameterized [`Engine`]: `Metal` on
/// macOS (the GPU path), the platform-free [`Software`](crate::software::Software)
/// CPU compositor everywhere else (the headless render path, ADR 003). Keeping
/// this as a cfg-selected alias means plain `Engine` resolves to `Engine<Metal>`
/// on macOS (so the app crate + macOS tests are unchanged) and to
/// `Engine<Software>` on Linux (so headless callers get the CPU backend for free).
#[cfg(target_os = "macos")]
pub type DefaultBackend = Metal;
#[cfg(not(target_os = "macos"))]
pub type DefaultBackend = crate::software::Software;

/// The cell engine. Owns the CPU-side [`Contents`], the [`Uniforms`], the
/// [`SwapChain`], and the three first-pixels pipelines.
pub struct Engine<B: GpuBackend = DefaultBackend> {
    /// The GPU backend (device/queue + resource factory).
    backend: B,
    /// Per-frame GPU-slot pool (targets + instance buffers + atlas textures).
    swap_chain: SwapChain<B>,
    /// CPU-side cell contents, rebuilt each `update_frame`.
    contents: Contents,
    /// Shader uniforms, rebuilt each `update_frame`.
    uniforms: Uniforms,
    /// `bg_color`: solid background fill (full-screen triangle).
    bg_color_pipeline: B::Pipeline,
    /// `cell_bg`: per-cell background color (full-screen triangle sampling the
    /// bg_cells buffer).
    cell_bg_pipeline: B::Pipeline,
    /// `cell_text`: instanced glyph quads.
    cell_text_pipeline: B::Pipeline,
    /// `image`: kitty graphics image quads (R6 slice 1).
    image_pipeline: B::Pipeline,
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
    /// The `hovered_cell` of the last frame. When it changes, the hover
    /// underline set changes (R7) but no row dirty bit fires, so a change here
    /// forces a full rebuild to add/remove the link underline.
    prev_hovered_cell: Option<(usize, usize)>,
    /// GPU textures for transmitted kitty images, keyed by image id. Content
    /// (not per-slot): textures are read-only after upload. Re-uploaded only
    /// when an id's `generation` changes (upstream's staleness protocol). R6
    /// slice 3 adds eviction; slice 1 keeps referenced images cached.
    images: HashMap<u32, ImageEntry<B>>,
    /// Per-placement instance buffers (one `Image` quad each), grown on demand
    /// and reused across frames. Engine-level (not per-slot): correct under the
    /// day-one `Sync` swap-chain mode (one live frame); moves per-slot when
    /// async pacing lands.
    image_instances: Vec<B::Buffer<Image>>,
    /// Resolved kitty placements for the current frame (set by `update_frame`,
    /// drawn by `draw_frame`/present). Drawn in resolve order; z-bucketing is
    /// R6 slice 4.
    pending_placements: Vec<KittyPlacement>,
    /// Kitty images referenced this frame (set by `update_frame`); uploaded to
    /// `images` in `prepare_image_frame`.
    pending_images: Vec<KittyImage>,
    /// Ids of all images the terminal still holds this frame (set by
    /// `update_frame`); `prepare_image_frame` evicts cached textures whose id
    /// left this set (R6 slice 3 — delete + `image-storage-limit` eviction).
    pending_live_ids: Vec<u32>,
    /// Z-order bucket boundaries into the (z-sorted) `pending_placements` (R6
    /// slice 4): `[0, image_bg_end)` draws below the cell backgrounds,
    /// `[image_bg_end, image_text_end)` below text, `[image_text_end, ..)` above
    /// text. Set by `update_frame`.
    image_bg_end: usize,
    image_text_end: usize,
    /// Env-gated present-smoothness recorder (#141). `None` unless
    /// `QWERTTY_TERM_PRESENT_STATS` is set; fed from the readback present path.
    #[cfg(target_os = "macos")]
    present_recorder: Option<crate::present_stats::PresentStatsRecorder>,
}

/// A GPU-resident kitty image: its texture plus the `generation` it was
/// uploaded at, so a re-upload is skipped while the content is unchanged.
pub(crate) struct ImageEntry<B: GpuBackend = DefaultBackend> {
    generation: u64,
    texture: B::Texture,
}

#[cfg(target_os = "macos")]
impl Engine<Metal> {
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
}

impl<B: GpuBackend> Engine<B> {
    /// [`Engine::for_grid`] over an existing backend context (shared backends,
    /// graceful no-device skip paths). Generic over the backend, so a headless
    /// `Engine<Software>` is built the same way as an `Engine<Metal>`.
    pub fn with_backend_for_grid(backend: B, grid: &Grid) -> Result<Engine<B>, B::Error> {
        let metrics = grid.metrics();
        Engine::with_backend(backend, metrics.cell_width, metrics.cell_height)
    }

    /// Build the engine over an existing backend context. Used by tests that
    /// share a backend or need to skip gracefully when no device is present.
    pub fn with_backend(
        backend: B,
        cell_width: u32,
        cell_height: u32,
    ) -> Result<Engine<B>, B::Error> {
        // Day-one degenerate pacing (plan decision 3): one live slot, each frame
        // completed with `waitUntilCompleted`.
        let swap_chain = SwapChain::new(&backend, SwapChainMode::Sync)?;
        let slot_count = swap_chain.slot_count();

        // The backend compiles each pipeline from the backend-agnostic
        // description + MSL source (Metal builds the shader library + reads its
        // target pixel format internally — no longer leaked here).
        let desc = |name: &str| {
            shaders::PIPELINE_DESCRIPTIONS
                .iter()
                .find(|d| d.name == name)
                .expect("pipeline description exists")
        };
        let bg_color_pipeline =
            backend.build_pipeline(desc("bg_color"), ShaderSource::Msl(shaders::SOURCE))?;
        let cell_bg_pipeline =
            backend.build_pipeline(desc("cell_bg"), ShaderSource::Msl(shaders::SOURCE))?;
        let cell_text_pipeline =
            backend.build_pipeline(desc("cell_text"), ShaderSource::Msl(shaders::SOURCE))?;
        let image_pipeline =
            backend.build_pipeline(desc("image"), ShaderSource::Msl(shaders::SOURCE))?;

        Ok(Engine {
            backend,
            swap_chain,
            contents: Contents::default(),
            uniforms: default_uniforms(),
            bg_color_pipeline,
            cell_bg_pipeline,
            cell_text_pipeline,
            image_pipeline,
            cell_width,
            cell_height,
            grayscale_modified: vec![0; slot_count],
            color_modified: vec![0; slot_count],
            screen_width: 0,
            screen_height: 0,
            run_cache: RunCache::new(),
            prev_frame_key: None,
            prev_hovered_cell: None,
            images: HashMap::new(),
            image_instances: Vec::new(),
            pending_placements: Vec::new(),
            pending_images: Vec::new(),
            pending_live_ids: Vec::new(),
            image_bg_end: 0,
            image_text_end: 0,
            #[cfg(target_os = "macos")]
            present_recorder: crate::present_stats::PresentStatsRecorder::from_env(),
        })
    }

    /// Feed the presented frame's BGRA readback to the present-smoothness
    /// recorder, if `QWERTTY_TERM_PRESENT_STATS` enabled it (#141). No-op
    /// otherwise. Called from the readback present path.
    #[cfg(target_os = "macos")]
    pub(crate) fn record_present(&mut self, bgra: &[u8]) {
        if let Some(rec) = self.present_recorder.as_mut() {
            rec.record(bgra);
        }
    }

    /// The GPU backend (for tests / inspection).
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// The pixel dimensions of the current render target.
    pub fn screen_size(&self) -> (usize, usize) {
        (self.screen_width, self.screen_height)
    }

    /// Adopt a rebuilt font [`Grid`] — after a Cmd-+/- font zoom, a
    /// backing-scale (display DPI) change, or a `font-family` change. The host
    /// passes the new grid's cell metrics; the grid itself is handed to the next
    /// [`update_frame`](Self::update_frame) / [`sync_atlas`](Self::sync_atlas).
    ///
    /// Two pieces of engine state survive a rebuild and must be invalidated:
    ///
    /// 1. **Cached cell metrics.** The engine caches cell width/height at
    ///    construction for the projection, the target size, and per-cell
    ///    placement (`build_uniforms`). Left stale, the next frame places
    ///    new-sized glyphs on the old cell pitch (overlapping text) and keeps
    ///    the target at the old pixel size.
    ///
    /// 2. **The per-slot atlas-upload trackers.** A rebuild produces a *fresh*
    ///    atlas whose [`modified`](qwertty_term_font::atlas::Atlas::modified)
    ///    counter restarts at 0, unrelated to the previous atlas. Each swap-chain
    ///    slot records the last counter it uploaded (large after a real session),
    ///    and [`sync_atlas`](Self::sync_atlas) skips the upload when
    ///    `modified <= recorded`. So the fresh atlas's low counter fails the gate
    ///    and every slot keeps sampling the STALE old-size atlas — garbled glyphs
    ///    on screen. (The offscreen readback path hides this: it syncs and draws
    ///    the same slot from a counter of 0, so its gate always passes.) Reset
    ///    every slot's tracker to 0 to force a re-upload of the new atlas.
    ///
    /// Also drops the shaped-run cache (its advances are size-dependent yet the
    /// [`RunKey`] does not encode size) and the frame key (forcing a full
    /// rebuild). Cheap and called only on a font rebuild (never per frame), so
    /// the invalidation runs unconditionally even when the metrics are unchanged
    /// (a same-size `font-family` change still swaps the atlas).
    pub fn on_font_rebuilt(&mut self, cell_width: u32, cell_height: u32) {
        // Force the fresh atlas to re-upload into every slot (see #2 above).
        self.grayscale_modified.iter_mut().for_each(|m| *m = 0);
        self.color_modified.iter_mut().for_each(|m| *m = 0);
        self.run_cache.clear();
        self.prev_frame_key = None;

        if self.cell_width == cell_width && self.cell_height == cell_height {
            return;
        }
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.screen_width = self.contents.cols() * cell_width as usize;
        self.screen_height = self.contents.rows() * cell_height as usize;
    }

    /// Number of kitty image textures currently resident in the GPU cache.
    /// Reflects eviction (R6 slice 3): drops when the terminal deletes an image
    /// or `image-storage-limit` evicts one. For tests / diagnostics.
    #[must_use]
    pub fn image_cache_len(&self) -> usize {
        self.images.len()
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
        // A hover-cell change moves the OSC8 link underline (R7) without
        // dirtying any row, so force a full rebuild to re-evaluate it.
        let hover_changed = opts.hovered_cell != self.prev_hovered_cell;
        self.prev_hovered_cell = opts.hovered_cell;
        let full_rebuild = size_changed
            || signals.global_forces_full
            || hover_changed
            || opts.force_full_rebuild
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

        // R7: resolve which OSC8 hyperlink the mouse is hovering, if any, so
        // every visible cell sharing that link gets a forced underline. The
        // hovered cell's `LinkKey` identifies the link; cells match by value.
        let hovered_link = opts.hovered_cell.and_then(|(hc, hr)| {
            (hr < rows)
                .then(|| snapshot.row(hr))
                .and_then(|r| r.get(hc))
                .and_then(|c| c.link.clone())
        });

        // R7 slice 2: if the mouse isn't over an OSC8 link, try regex URL
        // detection on the hovered row — a matched URL span underlines like an
        // OSC8 link. Per visual row; no modifier gate yet (slice 3 adds
        // cmd+click and can gate both there).
        let hovered_url: Option<(usize, std::ops::Range<usize>)> = if hovered_link.is_none() {
            opts.hovered_cell.and_then(|(hc, hr)| {
                (hr < rows)
                    .then(|| hovered_url_cols(snapshot.row(hr), hc))
                    .flatten()
                    .map(|range| (hr, range))
            })
        } else {
            None
        };

        // Cursor style resolution (renderer-local focus/blink/preedit on top of
        // the snapshot cursor). Preedit isn't wired in the reduced cut.
        let cursor = snapshot.cursor();
        let cursor_style = cursor.and_then(|c| {
            // The snapshot carries the blink *mode* (DEC mode 12) via
            // `SnapshotCursor.blinking` (#57); pass it through so
            // `opts.cursor_blink_visible` actually gates the cursor's blink-off
            // phase. When mode 12 is off, `blinking` is false and the cursor is
            // drawn steady regardless of the blink phase.
            let state = cursor::CursorState::from_snapshot_cursor(&c, c.blinking);
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
            let url_range = hovered_url
                .as_ref()
                .filter(|(r, _)| *r == y)
                .map(|(_, rng)| rng);
            self.rebuild_row(
                y,
                row,
                grid,
                palette,
                default_fg,
                default_bg,
                cursor_x,
                hovered_link.as_ref(),
                url_range,
            );
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

        // Stash this frame's resolved kitty placements + images for the draw
        // pass (upstream `kittyUpdate` runs during frame prep, then `draw`
        // walks the placement list). The GPU upload happens in
        // `prepare_image_frame` (it needs the backend).
        self.pending_placements.clear();
        self.pending_placements
            .extend_from_slice(snapshot.kitty_placements());
        self.pending_images.clear();
        self.pending_images
            .extend_from_slice(snapshot.kitty_images());
        self.pending_live_ids.clear();
        self.pending_live_ids
            .extend_from_slice(snapshot.kitty_live_ids());

        // R6 slice 4: sort placements by z (tie-break image id) and split into
        // the three z-order buckets upstream draws at different points in the
        // pass. Port of `image.zig`'s sort + `kitty_bg_end`/`kitty_text_end`:
        //   below-bg    z < i32::MIN/2   (drawn after bg_color)
        //   below-text  i32::MIN/2..0    (drawn after cell_bg)
        //   above-text  z >= 0           (drawn after cell_text)
        self.pending_placements
            .sort_by(|a, b| a.z.cmp(&b.z).then(a.image_id.cmp(&b.image_id)));
        const BG_LIMIT: i32 = i32::MIN / 2;
        self.image_bg_end = self.pending_placements.partition_point(|p| p.z < BG_LIMIT);
        self.image_text_end = self.pending_placements.partition_point(|p| p.z < 0);
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
        hovered_link: Option<&qwertty_term_vt::page::hyperlink::LinkKey>,
        hovered_url_range: Option<&std::ops::Range<usize>>,
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

            // R7 hover: a cell of the hovered link gets a *forced* underline —
            // single normally, double if the cell already draws a single (so it
            // stays visible), mirroring upstream `rebuildCells`. The hovered
            // link is either an OSC8 link (matched by `LinkKey`) or a
            // regex-detected URL span on this row (slice 2).
            let is_hovered_link = (hovered_link.is_some() && cell.link.as_ref() == hovered_link)
                || hovered_url_range.is_some_and(|rng| rng.contains(&x));
            let effective_underline = if is_hovered_link {
                if style.underline == SnapshotUnderline::Single {
                    SnapshotUnderline::Double
                } else {
                    SnapshotUnderline::Single
                }
            } else {
                style.underline
            };

            // Underlines draw first (underneath text).
            if effective_underline != SnapshotUnderline::None {
                let underline_color = match style.underline_color {
                    SnapshotColor::Default => fg,
                    other => resolve_color(other, palette, default_fg),
                };
                self.add_decoration(
                    x,
                    y,
                    underline_sprite(effective_underline),
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
    ) -> Result<Frame, B::Error> {
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

    /// Upload/refresh GPU textures for this frame's kitty images and sync the
    /// per-placement instance buffers. Runs before the draw pass in both the
    /// offscreen (`draw_frame`) and on-screen (`draw_and_present_inner`) paths,
    /// since it needs the backend to create textures/buffers. Port of the
    /// `images.upload` pump + per-image vertex-buffer fill in upstream
    /// `image.zig` (`upload`/`draw`). Texture re-upload is skipped while an
    /// image's `generation` (and dimensions) are unchanged.
    ///
    /// **Sync-mode invariant:** unlike the per-slot uniforms/cells/atlas
    /// buffers, the image texture cache (`images`) and instance-buffer pool
    /// (`image_instances`) are *engine-level*, shared across slots. That is
    /// correct only because `Sync` mode keeps exactly one frame in flight and
    /// each draw waits for completion (`frame.complete(true)`) before the next
    /// reuses them. Under `Async` pacing a next frame could overwrite an
    /// instance buffer — or `insert` could drop an `ImageEntry` texture — while
    /// the GPU still reads it (tearing / use-after-free). Guard it so enabling
    /// async can't make this path silently unsafe; moving image state per-slot
    /// is the fix when async lands.
    pub(crate) fn prepare_image_frame(&mut self) -> Result<(), B::Error> {
        debug_assert_eq!(
            self.swap_chain.mode(),
            SwapChainMode::Sync,
            "engine-level kitty image buffers are only safe in Sync mode; \
             move `images`/`image_instances` per-slot before enabling Async",
        );
        let Engine {
            backend,
            images,
            image_instances,
            pending_images,
            pending_placements,
            pending_live_ids,
            ..
        } = self;

        // Evict cached textures for images the terminal no longer holds (R6
        // slice 3): a deleted or storage-limit-evicted image drops out of the
        // live-id set, so its GPU texture is freed here. On the `from_window`
        // path the live set is empty but `images` was never populated, so this
        // is a no-op. (`pending_live_ids` is small — a handful of images.)
        if !images.is_empty() {
            images.retain(|id, _| pending_live_ids.contains(id));
        }

        for img in pending_images.iter() {
            let (w, h) = (img.width as usize, img.height as usize);
            let stale = match images.get(&img.id) {
                Some(entry) => {
                    entry.generation != img.generation
                        || entry.texture.width() != w
                        || entry.texture.height() != h
                }
                None => true,
            };
            if stale {
                let texture = backend.new_texture(
                    crate::gpu::TextureOptions {
                        // `*_srgb` so the GPU linearizes on sample; the image
                        // fragment shader unlinearizes back when not using
                        // linear blending (matches upstream
                        // `imageTextureOptions(.rgba, srgb = true)`).
                        format: crate::gpu::TextureFormat::Rgba8UnormSrgb,
                        usage: crate::gpu::TextureUsage::SHADER_READ,
                    },
                    w,
                    h,
                    Some(&img.rgba),
                )?;
                images.insert(
                    img.id,
                    ImageEntry {
                        generation: img.generation,
                        texture,
                    },
                );
            }
        }

        // Grow the instance-buffer pool to cover every placement, then fill one
        // `Image` instance per placement (a 4-vertex quad drawn per placement).
        while image_instances.len() < pending_placements.len() {
            image_instances.push(backend.new_buffer::<Image>(1)?);
        }
        for (i, placement) in pending_placements.iter().enumerate() {
            image_instances[i].sync(&[placement.instance])?;
        }
        Ok(())
    }
    /// Draw the current [`Contents`] into a fresh frame against the swap chain
    /// and read the pixels back. Port of `drawFrame`'s cell-drawing structure
    /// (bg_color → cell_bg → cell_text → image), sync mode.
    ///
    /// Returns the drawn slot's readback pixels (BGRA, row-padding stripped) so
    /// the caller (offscreen tests / a window host reading the surface) can
    /// inspect them. In sync mode this is coherent after `waitUntilCompleted`.
    pub fn draw_frame(&mut self) -> Result<Vec<u8>, B::Error> {
        Ok(self
            .draw_into_slot(|_backend, target| Ok(target.read_pixels()))?
            .unwrap_or_default())
    }

    /// Draw one frame into the swap-chain target and **present it on screen**
    /// via the backend's [`GpuBackend::present`] — the generic analog of
    /// upstream `generic.zig`'s draw-then-`api.present(target)`.
    ///
    /// Identical GPU work to [`draw_frame`](Self::draw_frame), except that
    /// instead of reading the target's pixels back it hands the drawn target to
    /// the backend to present: OpenGL blits it onto the host's bound default
    /// framebuffer (the `GtkGLArea`'s FBO, current on the GTK render thread);
    /// Software no-ops. The Metal on-screen path is the dedicated
    /// `draw_and_present(layer)` in [`crate::present`] (IOSurface/CALayer), so
    /// this generic method is compiled off macOS to avoid the name clash and is
    /// never needed there.
    ///
    /// Must be called with the host's default framebuffer bound and current.
    /// Returns `false` (nothing presented) when the target has zero area — no
    /// `update_frame` has sized it yet — mirroring `draw_frame`'s early-out.
    #[cfg(not(target_os = "macos"))]
    pub fn draw_and_present(&mut self) -> Result<bool, B::Error> {
        Ok(self
            .draw_into_slot(|backend, target| backend.present(target))?
            .is_some())
    }

    /// Shared body of the draw paths: rebuild the GPU buffers for this frame,
    /// encode the cell passes into a fresh frame against the swap chain, and
    /// complete it (sync). Then, while the drawn slot target is still live,
    /// invoke `after(backend, target)` — the readback ([`draw_frame`]) or the
    /// on-screen present ([`draw_and_present`]). Returns `Ok(None)` when the
    /// target has zero area (no frame drawn); otherwise `Ok(Some(after(..)))`.
    fn draw_into_slot<R>(
        &mut self,
        after: impl FnOnce(&B, &B::Target) -> Result<R, B::Error>,
    ) -> Result<Option<R>, B::Error> {
        if self.screen_width == 0 || self.screen_height == 0 {
            return Ok(None);
        }

        // Upload kitty image textures + sync placement instance buffers before
        // taking the disjoint field borrows below (this needs `&mut self`).
        self.prepare_image_frame()?;

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
        // Kitty z-order bucket boundaries (into the z-sorted placements).
        let (img_bg_end, img_text_end) = (self.image_bg_end, self.image_text_end);

        // Disjoint borrows: the swap chain (for the slot), the backend (for the
        // frame + resizes), the pipelines, and the kitty image state. Splitting
        // `self` into field references up front lets us hold the slot guard
        // across backend calls without a self-aliasing conflict.
        let Engine {
            backend,
            swap_chain,
            bg_color_pipeline,
            cell_bg_pipeline,
            cell_text_pipeline,
            image_pipeline,
            images,
            image_instances,
            pending_placements,
            ..
        } = self;

        // Acquire a slot (sync mode: one live permit). The chain is only marked
        // defunct by `deinit`, which never runs during a live `draw_frame`, so
        // `next_frame` always yields here (the swap-chain tests rely on the same
        // invariant). Generic code can't mint a `B::Error`, and there is no real
        // error to report, so this is an internal invariant, not a fallible path.
        let mut guard = swap_chain
            .next_frame()
            .expect("swap chain is live during draw_frame");
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
                texture: &slot.target,
                clear_color: Some([0.0, 0.0, 0.0, 0.0]),
            }])?;

            // 1. Background color (full-screen triangle; reads bg_cells at
            //    buffer index 2 for padding-extend, but primarily fills the
            //    surface bg from the uniform). No vertex buffer.
            pass.step(&Step {
                pipeline: bg_color_pipeline,
                vertex: None,
                uniforms: Some(slot.uniforms.handle()),
                extras: &[Some(slot.cells_bg.handle())],
                textures: &[],
                samplers: &[],
                draw: Draw::vertices(Primitive::Triangle, 3),
            });

            // 1b. Kitty images below the cell backgrounds (z < i32::MIN/2).
            encode_image_steps(
                &pass,
                image_pipeline,
                slot.uniforms.handle(),
                images,
                image_instances,
                pending_placements,
                0..img_bg_end,
            );

            // 2. Per-cell backgrounds (full-screen triangle sampling bg_cells).
            pass.step(&Step {
                pipeline: cell_bg_pipeline,
                vertex: None,
                uniforms: Some(slot.uniforms.handle()),
                extras: &[Some(slot.cells_bg.handle())],
                textures: &[],
                samplers: &[],
                draw: Draw::vertices(Primitive::Triangle, 3),
            });

            // 2b. Kitty images below text (i32::MIN/2 <= z < 0).
            encode_image_steps(
                &pass,
                image_pipeline,
                slot.uniforms.handle(),
                images,
                image_instances,
                pending_placements,
                img_bg_end..img_text_end,
            );

            // 3. Text (instanced glyph quads). Vertex buffer 0 = CellText
            //    instances; extras[0] (buffer 2) = bg_cells (for min-contrast);
            //    textures 0/1 = grayscale/color atlas.
            pass.step(&Step {
                pipeline: cell_text_pipeline,
                vertex: Some(slot.cells.handle()),
                uniforms: Some(slot.uniforms.handle()),
                extras: &[Some(slot.cells_bg.handle())],
                textures: &[Some(&slot.grayscale), Some(&slot.color)],
                samplers: &[],
                draw: Draw {
                    primitive: Primitive::TriangleStrip,
                    vertex_count: 4,
                    instance_count: fg_count,
                },
            });

            // 3b. Kitty images above text (z >= 0). R6 slice 4: the three
            //     buckets are drawn at bg / below-text / above-text points; the
            //     placements were z-sorted in `update_frame`.
            encode_image_steps(
                &pass,
                image_pipeline,
                slot.uniforms.handle(),
                images,
                image_instances,
                pending_placements,
                img_text_end..pending_placements.len(),
            );

            pass.complete();
        }
        frame.complete(true);

        // While the drawn slot target is still live, hand it to the caller for
        // readback (`draw_frame`) or on-screen present (`draw_and_present`).
        let out = after(backend, &slot.target)?;
        guard.release();
        Ok(Some(out))
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
    pub fn sync_atlas(&mut self, grid: &Grid) -> Result<(), B::Error> {
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
    fn sync_grayscale(&mut self, grid: &Grid, next_index: usize) -> Result<(), B::Error> {
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
    fn sync_color(&mut self, grid: &Grid, next_index: usize) -> Result<(), B::Error> {
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
/// changing any R4 behavior. Metal-only: the presentation path is macOS
/// IOSurface (`present.rs`), so these accessors have no non-macOS caller.
#[cfg(target_os = "macos")]
impl Engine<Metal> {
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
    /// chain, the four pipelines, and the kitty image state (texture cache +
    /// instance buffers + this frame's placements). Mirrors the destructuring
    /// in `draw_frame` so the presentation path can hold the slot guard across
    /// backend calls without a self-aliasing conflict.
    #[allow(clippy::type_complexity)]
    pub(crate) fn present_parts(
        &mut self,
    ) -> (
        &Metal,
        &mut SwapChain<Metal>,
        &Pipeline,
        &Pipeline,
        &Pipeline,
        &Pipeline,
        &HashMap<u32, ImageEntry>,
        &[Buffer<Image>],
        &[KittyPlacement],
        usize,
        usize,
    ) {
        (
            &self.backend,
            &mut self.swap_chain,
            &self.bg_color_pipeline,
            &self.cell_bg_pipeline,
            &self.cell_text_pipeline,
            &self.image_pipeline,
            &self.images,
            &self.image_instances,
            &self.pending_placements,
            self.image_bg_end,
            self.image_text_end,
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

/// Encode one image draw step for each placement in `range` (an index range
/// into the z-sorted `placements` / `image_instances` — one z-order bucket, R6
/// slice 4), in the given render pass. Each placement is a 4-vertex
/// triangle-strip quad reading its own `Image` instance buffer and binding its
/// image's texture (port of upstream `image.zig`'s per-image `pass.step`).
/// Placements whose image isn't uploaded yet, or that lack an instance buffer,
/// are skipped.
pub(crate) fn encode_image_steps<B: GpuBackend>(
    pass: &B::RenderPass,
    pipeline: &B::Pipeline,
    uniforms: &B::BufferHandle,
    images: &HashMap<u32, ImageEntry<B>>,
    image_instances: &[B::Buffer<Image>],
    placements: &[KittyPlacement],
    range: std::ops::Range<usize>,
) {
    for i in range {
        let Some(placement) = placements.get(i) else {
            continue;
        };
        let Some(entry) = images.get(&placement.image_id) else {
            continue;
        };
        let Some(instance) = image_instances.get(i) else {
            continue;
        };
        pass.step(&Step {
            pipeline,
            vertex: Some(instance.handle()),
            uniforms: Some(uniforms),
            extras: &[],
            textures: &[Some(&entry.texture)],
            samplers: &[],
            draw: Draw {
                primitive: Primitive::TriangleStrip,
                vertex_count: 4,
                instance_count: 1,
            },
        });
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

/// The column range of the regex-detected URL under `hover_col` on `row`, if
/// any (R7 slice 2). Builds the row's text, finds a URL span containing the
/// hovered column via [`crate::link`], and maps the byte span back to columns.
/// Per visual row — URLs that wrap across rows aren't joined yet.
fn hovered_url_cols(row: &[SnapshotCell], hover_col: usize) -> Option<std::ops::Range<usize>> {
    let mut text = String::new();
    // Parallel char-indexed maps: the column each char came from, and its byte
    // offset in `text` (a wide glyph's spacer cells contribute no char).
    let mut col_of_char: Vec<usize> = Vec::new();
    let mut byte_of_char: Vec<usize> = Vec::new();
    for (x, cell) in row.iter().enumerate() {
        if cell.is_spacer() {
            continue;
        }
        byte_of_char.push(text.len());
        col_of_char.push(x);
        text.push(cell.ch);
        for &c in &cell.combining {
            text.push(c);
        }
    }

    // Byte offset of the hovered column's char, then the URL span covering it.
    let hover_byte = col_of_char
        .iter()
        .position(|&c| c == hover_col)
        .map(|i| byte_of_char[i])?;
    let span = crate::link::url_span_at(&text, hover_byte)?;

    // Map the byte span back to the contiguous column range it covers.
    let mut lo = usize::MAX;
    let mut hi = 0usize;
    for (i, &b) in byte_of_char.iter().enumerate() {
        if span.contains(&b) {
            lo = lo.min(col_of_char[i]);
            hi = hi.max(col_of_char[i]);
        }
    }
    (lo != usize::MAX).then_some(lo..hi + 1)
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
