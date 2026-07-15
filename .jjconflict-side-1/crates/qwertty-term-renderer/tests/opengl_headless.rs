//! Headless OpenGL render acceptance test (ADR 005 P4, slice 1).
//!
//! The offscreen-GL analog of `software_headless.rs`: the *same* cell engine
//! (`Engine<B>`) drives the OpenGL [`GpuBackend`] over a **surfaceless EGL**
//! context — no display server, no real GPU — renders a known cell grid into an
//! FBO, and reads the BGRA pixels back. It asserts the same backend-agnostic
//! facts the Software test does (frame dimensions + per-cell ink coverage), and
//! then a **differential** check: the OpenGL backend must ink the *same cells*
//! as the Software backend for the same input — the parity the ADR-003 /
//! ADR-005 methodology uses across backends.
//!
//! It **skips with a note** (never hard-fails) when no headless GL context can
//! be created — `libEGL` missing, no surfaceless display, or the driver is too
//! old — exactly like `metal`'s "no device" skip. This is the evidence the ADR
//! calls for: runs in CI/Docker under Mesa software GL (`LIBGL_ALWAYS_SOFTWARE=1`,
//! `EGL_PLATFORM=surfaceless`), reproducing the Software backend's grid.
//!
//! Whole file is `cfg(target_os = "linux")`: the `opengl` backend only exists
//! there. On other targets this test compiles to nothing.
#![cfg(target_os = "linux")]

use qwertty_term_font::grid::Grid;
use qwertty_term_font::{CodepointResolver, Collection, Face, Metrics};
use qwertty_term_renderer::engine::{Engine, FrameOptions};
use qwertty_term_renderer::opengl::OpenGL;
use qwertty_term_renderer::snapshot::FullSnapshot;
use qwertty_term_renderer::software::Software;
use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};

/// One rendered pixel, unpacked from the BGRA readback buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Px {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

/// Max absolute per-channel delta between two pixels.
fn delta(a: Px, b: Px) -> i32 {
    (a.r as i32 - b.r as i32)
        .abs()
        .max((a.g as i32 - b.g as i32).abs())
        .max((a.b as i32 - b.b as i32).abs())
}

/// A read-back frame with cell-aware pixel sampling (same shape as the Software
/// test's `Readback`).
struct Readback {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    cell_w: usize,
    cell_h: usize,
}

impl Readback {
    fn px(&self, x: usize, y: usize) -> Px {
        let i = (y * self.width + x) * 4;
        Px {
            b: self.pixels[i],
            g: self.pixels[i + 1],
            r: self.pixels[i + 2],
            a: self.pixels[i + 3],
        }
    }

    /// Total ink coverage over a cell: sum of per-pixel deltas vs `bg`.
    fn cell_ink(&self, col: usize, row: usize, bg: Px) -> i64 {
        let mut sum = 0i64;
        for dy in 0..self.cell_h {
            for dx in 0..self.cell_w {
                let x = col * self.cell_w + dx;
                let y = row * self.cell_h + dy;
                if x < self.width && y < self.height {
                    sum += delta(self.px(x, y), bg) as i64;
                }
            }
        }
        sum
    }

    fn bg(&self) -> Px {
        // Bottom-right corner is always a blank cell for our inputs.
        self.px(self.width - 1, self.height - 1)
    }
}

/// Build a fresh font grid from the embedded face (deterministic metrics/atlas).
fn make_grid() -> (Grid, usize, usize) {
    let face = Face::load_embedded(16.0).expect("embedded face");
    let metrics = Metrics::calc(face.face_metrics());
    let resolver = CodepointResolver::new(Collection::new(face));
    let grid = Grid::new(resolver, metrics).expect("grid");
    let (cell_w, cell_h) = {
        let m = grid.metrics();
        (m.cell_width as usize, m.cell_height as usize)
    };
    (grid, cell_w, cell_h)
}

/// Feed `bytes` to a fresh `cols`×`rows` terminal and capture a full snapshot.
fn snapshot(cols: u16, rows: u16, bytes: &[u8]) -> FullSnapshot {
    let terminal = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(terminal));
    stream.feed(bytes);
    FullSnapshot::capture_live(stream.terminal())
}

/// Render `bytes` through `Engine<OpenGL>`; `None` if no GL context (skip).
fn render_opengl(cols: u16, rows: u16, bytes: &[u8]) -> Option<Readback> {
    let backend = match OpenGL::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: no usable OpenGL context ({e}); skipping GL headless test");
            return None;
        }
    };
    let (mut grid, cell_w, cell_h) = make_grid();
    let snap = snapshot(cols, rows, bytes);
    let mut engine = Engine::with_backend_for_grid(backend, &grid).expect("gl engine");
    let frame = engine
        .render(&snap, &mut grid, FrameOptions::default())
        .expect("gl headless render");
    Some(Readback {
        pixels: frame.bgra().to_vec(),
        width: frame.width(),
        height: frame.height(),
        cell_w,
        cell_h,
    })
}

/// Render `bytes` through `Engine<Software>` (the differential reference).
fn render_software(cols: u16, rows: u16, bytes: &[u8]) -> Readback {
    let (mut grid, cell_w, cell_h) = make_grid();
    let snap = snapshot(cols, rows, bytes);
    let mut engine = Engine::with_backend_for_grid(Software::new(), &grid).expect("sw engine");
    let frame = engine
        .render(&snap, &mut grid, FrameOptions::default())
        .expect("sw headless render");
    Readback {
        pixels: frame.bgra().to_vec(),
        width: frame.width(),
        height: frame.height(),
        cell_w,
        cell_h,
    }
}

/// The OpenGL backend renders a correctly-sized frame in which text cells carry
/// ink and blank cells are the uniform background — the whole GL pipeline
/// (surfaceless EGL → FBO → readback) end-to-end, no display, no GPU.
#[test]
fn opengl_backend_renders_text_headless() {
    let Some(rb) = render_opengl(10, 3, b"hi") else {
        return; // skipped: no GL context
    };

    // Dimensions are grid × cell metrics (reduced cut: no window padding).
    assert_eq!(rb.width, 10 * rb.cell_w, "frame width = cols * cell_w");
    assert_eq!(rb.height, 3 * rb.cell_h, "frame height = rows * cell_h");
    assert_eq!(
        rb.pixels.len(),
        rb.width * rb.height * 4,
        "tightly-packed BGRA readback",
    );

    let bg = rb.bg();
    let h_ink = rb.cell_ink(0, 0, bg);
    let i_ink = rb.cell_ink(1, 0, bg);
    let blank_ink = rb.cell_ink(8, 0, bg);

    assert!(
        h_ink > 0 && i_ink > 0,
        "glyph cells must carry ink (h={h_ink}, i={i_ink})",
    );
    assert!(
        h_ink > blank_ink * 8 + 100,
        "a glyph cell (h={h_ink}) must be far inkier than a blank cell (blank={blank_ink})",
    );
}

/// Differential parity: the OpenGL backend inks the *same cells* the Software
/// backend does for the same input. Orientation, cell geometry, and glyph
/// placement must all agree — a vertical flip or an off-by-one in the GL
/// projection/readback would light up different cells and fail here.
#[test]
fn opengl_matches_software_inky_cells() {
    let cols = 12u16;
    let rows = 2u16;
    let input = b"Hello";

    let Some(gl) = render_opengl(cols, rows, input) else {
        return; // skipped: no GL context
    };
    let sw = render_software(cols, rows, input);

    assert_eq!((gl.width, gl.height), (sw.width, sw.height), "frame sizes");

    let gl_bg = gl.bg();
    let sw_bg = sw.bg();
    // A cell is "inky" if its coverage clears a small noise floor. The exact
    // colors/AA can differ slightly (GL's sRGB conversion vs the CPU
    // compositor), so we compare the *set of inky cells*, not pixels.
    let threshold = 200i64;
    let mut compared = 0;
    for row in 0..rows as usize {
        for col in 0..cols as usize {
            let gl_inky = gl.cell_ink(col, row, gl_bg) > threshold;
            let sw_inky = sw.cell_ink(col, row, sw_bg) > threshold;
            assert_eq!(
                gl_inky,
                sw_inky,
                "cell ({col},{row}) inky mismatch: gl={} sw={}",
                gl.cell_ink(col, row, gl_bg),
                sw.cell_ink(col, row, sw_bg),
            );
            compared += 1;
        }
    }
    assert_eq!(compared, cols as usize * rows as usize);

    // Sanity: the first five cells of row 0 ("Hello") are inky in both.
    for col in 0..5 {
        assert!(
            gl.cell_ink(col, 0, gl_bg) > threshold,
            "GL cell ({col},0) should be inky",
        );
    }
}
