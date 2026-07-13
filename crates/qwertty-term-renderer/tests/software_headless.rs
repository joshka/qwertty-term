//! Headless render acceptance test: `Engine<Software>` produces real pixels
//! with **no GPU, no window, no Metal device** — the betamax headless-Linux
//! path (ADR 003 P1).
//!
//! This is the end-to-end proof that PR-3's `Engine<B>` genericization pays
//! off: the exact same cell engine that drives Metal on macOS, parameterized
//! over the platform-free [`Software`] backend, feeds a terminal, snapshots it,
//! rasterizes glyphs through the CPU compositor, and reads back a BGRA frame.
//!
//! Because the Software backend is pure Rust (no `objc2`, no device), this test
//! **never skips** — it is the render path Linux CI will run. The font
//! substrate here is CoreText only because the test is compiled on macOS (where
//! the renderer's integration tests live); on Linux the identical flow runs
//! over the FreeType `Grid`, which is why the assertions below are about
//! backend-agnostic facts (dimensions + ink coverage), not Apple specifics.

#![cfg(target_os = "macos")]

use qwertty_term_font::coretext::Face;
use qwertty_term_font::grid::Grid;
use qwertty_term_font::{CodepointResolver, Collection, Metrics};
use qwertty_term_renderer::engine::{Engine, FrameOptions};
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

/// Max absolute per-channel delta between two pixels — "how different" two
/// pixels are, independent of which backend produced them.
fn delta(a: Px, b: Px) -> i32 {
    (a.r as i32 - b.r as i32)
        .abs()
        .max((a.g as i32 - b.g as i32).abs())
        .max((a.b as i32 - b.b as i32).abs())
}

/// A read-back frame with cell-aware pixel sampling.
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

    /// Total ink coverage over a cell: sum of per-pixel deltas vs `bg`. A cell
    /// that holds a glyph darkens/colors many pixels, so its coverage is large;
    /// a blank cell is ~0.
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
}

/// Render `bytes` on a `cols`×`rows` terminal through `Engine<Software>` and
/// read back the frame. No Metal, no window — pure CPU.
fn render_headless(cols: u16, rows: u16, bytes: &[u8]) -> Readback {
    // Embedded JetBrains Mono → deterministic metrics + atlas.
    let face = Face::load_embedded(16.0).expect("embedded face");
    let metrics = Metrics::calc(face.face_metrics());
    let resolver = CodepointResolver::new(Collection::new(face));
    let mut grid = Grid::new(resolver, metrics).expect("grid");
    let (cell_w, cell_h) = {
        let m = grid.metrics();
        (m.cell_width as usize, m.cell_height as usize)
    };

    let terminal = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(terminal));
    stream.feed(bytes);

    // The whole point of PR-3: the cell engine over the platform-free backend.
    let mut engine =
        Engine::with_backend_for_grid(Software::new(), &grid).expect("software engine");
    let snapshot = FullSnapshot::capture_live(stream.terminal());
    let frame = engine
        .render(&snapshot, &mut grid, FrameOptions::default())
        .expect("headless render");

    Readback {
        pixels: frame.bgra().to_vec(),
        width: frame.width(),
        height: frame.height(),
        cell_w,
        cell_h,
    }
}

/// The headless path renders a real, correctly-sized frame in which text cells
/// carry ink and blank cells are the uniform background — the CPU compositor
/// end-to-end, no GPU involved.
#[test]
fn software_backend_renders_text_headless() {
    // Two glyphs at the start of the top row; the rest of the grid is blank.
    let rb = render_headless(10, 3, b"hi");

    // Dimensions are grid × cell metrics (reduced cut: no window padding).
    assert_eq!(rb.width, 10 * rb.cell_w, "frame width = cols * cell_w");
    assert_eq!(rb.height, 3 * rb.cell_h, "frame height = rows * cell_h");
    assert_eq!(
        rb.pixels.len(),
        rb.width * rb.height * 4,
        "tightly-packed BGRA readback",
    );

    // Sample the background from a cell we know is empty (bottom-right corner).
    let bg = rb.px(rb.width - 1, rb.height - 1);

    // The 'h' and 'i' cells carry ink; a blank cell on the same row does not.
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

/// A colored foreground run comes through the Software `cell_text` raster with
/// the right hue — proving color resolution + the alpha-blit composite, not
/// just "some pixels changed".
#[test]
fn software_backend_renders_colored_text() {
    // SGR 32 = green foreground on the default background.
    let rb = render_headless(8, 1, b"\x1b[32mA\x1b[0m");
    let bg = rb.px(rb.width - 1, rb.height / 2);

    // Find the inkiest pixel in the 'A' cell and check it reads greener than it
    // does red or blue (the green channel dominates the delta vs background).
    let mut best = bg;
    let mut best_cov = 0;
    for dy in 0..rb.cell_h {
        for dx in 0..rb.cell_w {
            let p = rb.px(dx, dy);
            let c = delta(p, bg);
            if c > best_cov {
                best_cov = c;
                best = p;
            }
        }
    }
    assert!(best_cov > 0, "the 'A' cell must carry ink");
    assert!(
        best.g >= best.r && best.g >= best.b,
        "green foreground must be green-dominant (got {best:?})",
    );
}
