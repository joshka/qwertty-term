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

use qwertty_term_font::grid::Grid;
use qwertty_term_font::{CodepointResolver, Collection, Face, Metrics};
use qwertty_term_renderer::engine::{Engine, FrameOptions};
use qwertty_term_renderer::opengl::OpenGL;
use qwertty_term_renderer::snapshot::FullSnapshot;
use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};

/// The per-surface renderer core: engine + grid + terminal, sized to the
/// GLArea.
pub struct SurfaceState {
    engine: Engine<OpenGL>,
    grid: Grid,
    stream: Stream<TerminalHandler>,
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
        let engine = Engine::with_backend_for_grid(backend, &grid).ok()?;

        let terminal = Terminal::new(Options {
            cols: cols as u16,
            rows: rows as u16,
            ..Default::default()
        });
        let stream = Stream::new(TerminalHandler::new(terminal));

        Some(Self {
            engine,
            grid,
            stream,
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

    /// Feed raw pty/VT bytes into the terminal. Returns whether anything was
    /// fed (so the caller can `queue_render` only on change).
    pub fn feed(&mut self, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }
        self.stream.feed(bytes);
        true
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
        let snap = FullSnapshot::capture_live(self.stream.terminal());
        self.engine
            .update_frame(&snap, &mut self.grid, FrameOptions::default());
        self.engine
            .sync_atlas(&self.grid)
            .map_err(|e| e.to_string())?;
        // Point the OpenGL backend's present at the GLArea's framebuffer, then
        // draw + blit into it in one call.
        self.engine.backend().set_default_framebuffer(dst);
        self.engine.draw_and_present().map_err(|e| e.to_string())?;
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

        let mut subprocess = Subprocess::init(Config::default());
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
