//! Per-surface terminal render state — the platform-free core the GTK GLArea
//! drives each frame.
//!
//! Owns the vt [`Terminal`] (fed VT bytes), the font [`Grid`] (shape +
//! rasterize), and the [`Engine<OpenGL>`] that renders the terminal into the
//! GLArea's framebuffer via the on-screen present seam
//! ([`Engine::draw_and_present`] → [`OpenGL::present`]). This is the Linux
//! analog of the core the macOS host owns; it mirrors the pieces upstream
//! initializes lazily on a surface's first resize (`class/surface.zig:3419`,
//! so the terminal gets correct initial dimensions) and draws in `glareaRender`
//! (`class/surface.zig:3347`).
//!
//! A live [`Terminal`] is fed either directly (the headless text smoke feeds
//! known bytes) or from a pty running the user's shell ([`Pty::spawn`], the
//! interactive path). Keyboard input the other direction is the next chunk.

use qwertty_term::engine::Engine as TermEngine;
use qwertty_term::selection::{SelectionColors, tint_selection};
use qwertty_term_font::grid::Grid;
use qwertty_term_font::{CodepointResolver, Collection, Face, Metrics};
use qwertty_term_renderer::engine::{Engine, FrameOptions};
use qwertty_term_renderer::opengl::OpenGL;
use qwertty_term_renderer::snapshot::FullSnapshot;

/// The per-surface renderer core: GPU engine + font grid + the terminal
/// [`TermEngine`], sized to the GLArea.
///
/// The vt terminal is the macOS app crate's [`qwertty_term::engine::Engine`]
/// wrapper (not the raw vt [`Stream`](qwertty_term_vt::stream::Stream)) so the
/// GTK host reuses its whole platform-free selection surface: `select_*` /
/// `screen_range` / `selection_string` / `bracketed_paste`, and the
/// `snapshot_window` → [`tint_selection`] → [`FullSnapshot::from_window`] render
/// path that draws highlighted selected cells (the same path the macOS view
/// uses — `qwertty-term/src/app.rs:765-798`). This is the reuse the P4 selection
/// slice is built on.
pub struct SurfaceState {
    /// The GPU renderer (`Engine<OpenGL>`), renamed from `engine` so it doesn't
    /// collide with the terminal [`TermEngine`].
    renderer: Engine<OpenGL>,
    grid: Grid,
    /// The terminal + selection state (app-crate wrapper over the vt terminal).
    engine: TermEngine,
    cols: usize,
    rows: usize,
    cell_w: usize,
    cell_h: usize,
}

impl SurfaceState {
    /// Lazily initialize the per-surface core once the GLArea has a real pixel
    /// size, over the **GTK-owned, already-current** GL context `gl` (the
    /// context GTK made current for `realize`/`render`). Mirrors upstream's
    /// lazy surface init on first resize (`class/surface.zig:3419`).
    ///
    /// `gl` is consumed by the [`OpenGL`] backend ([`OpenGL::from_glow`]); the
    /// caller keeps its own `glow::Context` for direct GL (querying the draw
    /// framebuffer, readback). Returns `None` if the font substrate or the GL
    /// engine can't be built (treated as "not yet initialized" by the caller).
    pub fn new(gl: glow::Context, width_px: i32, height_px: i32, font_size: f64) -> Option<Self> {
        let face = Face::load_embedded(font_size).ok()?;
        let metrics = Metrics::calc(face.face_metrics());
        let resolver = CodepointResolver::new(Collection::new(face));
        let grid = Grid::new(resolver, metrics).ok()?;
        let (cell_w, cell_h) = {
            let m = grid.metrics();
            (m.cell_width as usize, m.cell_height as usize)
        };
        // Grid dims from the GLArea size and cell metrics (reduced cut: no
        // window padding). At least 1×1 so a zero-size GLArea still builds.
        let cols = (width_px.max(1) as usize / cell_w.max(1)).max(1);
        let rows = (height_px.max(1) as usize / cell_h.max(1)).max(1);

        let backend = OpenGL::from_glow(gl);
        let renderer = Engine::with_backend_for_grid(backend, &grid).ok()?;

        let engine = TermEngine::new(cols, rows);

        Some(Self {
            renderer,
            grid,
            engine,
            cols,
            rows,
            cell_w,
            cell_h,
        })
    }

    /// Grid columns/rows.
    pub fn grid_size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// Cell width/height in pixels.
    pub fn cell_size(&self) -> (usize, usize) {
        (self.cell_w, self.cell_h)
    }

    /// The terminal engine (selection queries, `bracketed_paste`, …).
    pub fn engine(&self) -> &TermEngine {
        &self.engine
    }

    /// The terminal engine, mutably (drive the selection: `select_screen_points`,
    /// `clear_selection`, …).
    pub fn engine_mut(&mut self) -> &mut TermEngine {
        &mut self.engine
    }

    /// Feed raw pty/VT bytes into the terminal. Returns whether anything was
    /// fed (so the caller can `queue_render` only on change).
    pub fn feed(&mut self, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }
        self.engine.write(bytes);
        true
    }

    /// Drain any reply bytes the terminal queued in response to fed input
    /// (DA/DSR/CPR/DECRQSS/…), destined for the pty master. The interactive
    /// path writes these back so query-driven programs (vim, tmux) work.
    pub fn take_pty_replies(&mut self) -> Vec<u8> {
        self.engine.take_output()
    }

    /// Capture the live screen, rebuild this frame's GPU buffers + atlas, and
    /// **present** it into the host default framebuffer `dst` (the GLArea's FBO,
    /// bound + current on the GTK render thread). Port of the render seam
    /// (`glareaRender` → `drawFrame` → `present`).
    ///
    /// `dst` is the framebuffer the present blit targets (the GLArea binds a
    /// fresh FBO before each `render`; the caller passes the current
    /// `GL_DRAW_FRAMEBUFFER_BINDING`). Must run with that framebuffer bound and
    /// the GL context current.
    pub fn render(&mut self, dst: Option<glow::NativeFramebuffer>) -> Result<(), String> {
        // Resolve the selection range first (immutable borrow), then take the
        // window snapshot and overlay the selection tint CPU-side before wrapping
        // it in a `FullSnapshot` — the exact render path the macOS view runs
        // (`qwertty-term/src/app.rs:765-798`). No scrollback UI is wired, so the
        // window is always `snapshot_window(0)`.
        let range = self
            .engine
            .selection()
            .and_then(|(start, end, rect)| self.engine.screen_range(start, end, rect));
        let mut window = self.engine.snapshot_window(0);
        if let Some(range) = range {
            // No theme is wired in the GTK host yet, so highlight selected cells
            // with inverse video (the terminal-convention unthemed selection).
            tint_selection(&mut window, range, SelectionColors::Inverse);
        }
        let snap = FullSnapshot::from_window(window);

        self.renderer
            .update_frame(&snap, &mut self.grid, FrameOptions::default());
        self.renderer
            .sync_atlas(&self.grid)
            .map_err(|e| e.to_string())?;
        // Point the OpenGL backend's present at the GLArea's framebuffer, then
        // draw + blit into it in one call.
        self.renderer.backend().set_default_framebuffer(dst);
        self.renderer
            .draw_and_present()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// A pty child feeding the terminal — the interactive path's byte source and
/// the keystroke sink.
///
/// Spawns the user's shell (via `qwertty-term-termio`'s POSIX
/// [`Subprocess`](qwertty_term_termio::Subprocess)) on a pty sized to the grid,
/// and a reader thread that appends the child's output to a shared buffer the
/// GTK main loop drains (a lock-and-poll wakeup, so no glib channel-version
/// churn). Keystrokes go the other way: [`Pty::write`] sends the encoder's
/// bytes to the pty master (a clone of the master fd kept for writing, since
/// the reader thread owns its own clone).
pub struct Pty {
    /// Bytes read from the pty master, awaiting a drain on the main thread.
    buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    /// The pty master, for writing keystrokes to the child. A clone of the fd
    /// the reader thread reads from (the master is bidirectional).
    writer: std::fs::File,
    /// Kept alive so the child isn't dropped/reaped while the window lives.
    _subprocess: qwertty_term_termio::Subprocess,
}

impl Pty {
    /// Spawn the default shell on a pty of `cols`×`rows` (`w`×`h` px) and start
    /// reading it. `None` on any spawn failure (the caller falls back to a
    /// static banner so the window still shows real glyphs).
    pub fn spawn(cols: u16, rows: u16, w: u32, h: u32) -> Option<Self> {
        use qwertty_term_termio::size::{GridSize, ScreenSize};
        use qwertty_term_termio::{Config, Subprocess};

        // The pty's shell must inherit our environment (PATH, HOME, SHELL, …);
        // termio's `execvpe` runs the child with exactly `Config.env` (it does
        // not merge the parent env), and `Config::default()` leaves it empty —
        // which gives the shell no PATH, so it can't find any binary. Seed it
        // from the current process environment (termio still overrides TERM /
        // COLORTERM / TERMINFO on top).
        let config = Config {
            env: std::env::vars().collect(),
            ..Config::default()
        };
        let mut subprocess = Subprocess::init(config);
        subprocess
            .resize(
                GridSize {
                    columns: cols,
                    rows,
                },
                ScreenSize {
                    width: w,
                    height: h,
                },
            )
            .ok()?;
        let master = subprocess.start().ok()?;
        // Clone the master fd: one handle for the reader thread, one for
        // writing keystrokes back (the pty master is bidirectional).
        let writer = std::fs::File::from(master.try_clone().ok()?);

        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let reader = std::sync::Arc::clone(&buf);
        std::thread::Builder::new()
            .name("qwertty-gtk-pty-reader".into())
            .spawn(move || {
                use std::io::Read;
                let mut file = std::fs::File::from(master);
                let mut chunk = [0u8; 4096];
                loop {
                    match file.read(&mut chunk) {
                        Ok(0) | Err(_) => break, // child exited / HUP
                        Ok(n) => {
                            if let Ok(mut b) = reader.lock() {
                                b.extend_from_slice(&chunk[..n]);
                            }
                        }
                    }
                }
            })
            .ok()?;

        Some(Self {
            buf,
            writer,
            _subprocess: subprocess,
        })
    }

    /// Take everything read from the pty since the last drain (empties the
    /// buffer). Called on the GTK main thread before feeding the terminal.
    pub fn drain(&self) -> Vec<u8> {
        match self.buf.lock() {
            Ok(mut b) => std::mem::take(&mut *b),
            Err(_) => Vec::new(),
        }
    }

    /// Write encoded keystroke bytes to the pty master (the child's stdin).
    /// Best-effort: a short write or error is ignored (the child may have
    /// exited). Called on the GTK main thread from the key handler.
    pub fn write(&self, bytes: &[u8]) {
        use std::io::Write;
        // `File::write_all` takes `&mut File`, but `write` on the underlying fd
        // is a plain syscall; clone the handle so the shared `&self` API holds.
        if let Ok(mut w) = self.writer.try_clone() {
            let _ = w.write_all(bytes);
        }
    }
}
