//! R6 slice 1 acceptance test: KITTY IMAGE PIXELS.
//!
//! Drives a real `qwertty_term_vt::Terminal` through a `Stream`, transmitting a
//! solid-color RGBA image via the kitty graphics protocol (an `icat`-style
//! direct transmit-and-display), then snapshots, renders an offscreen frame,
//! and reads the pixels back — asserting the image's cells are its color and
//! cells outside the placement are background. Exercises the whole slice-1
//! path: `resolve_placements` (vt) → `FullSnapshot` → texture upload → the
//! `image` pipeline draw.
//!
//! Skips gracefully (`SKIP:`) when no Metal device is present.

#![cfg(target_os = "macos")]

use base64::Engine as _;
use qwertty_term_font::coretext::Face;
use qwertty_term_font::grid::Grid;
use qwertty_term_font::{CodepointResolver, Collection, Metrics};
use qwertty_term_renderer::engine::{Engine, FrameOptions};
use qwertty_term_renderer::metal::Metal;
use qwertty_term_renderer::snapshot::{FullSnapshot, RenderSnapshot};
use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};

#[derive(Debug, Clone, Copy)]
struct Px {
    r: u8,
    g: u8,
    b: u8,
}

struct Frame {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    cell_w: usize,
    cell_h: usize,
}

impl Frame {
    fn px(&self, x: usize, y: usize) -> Px {
        let i = (y * self.width + x) * 4;
        Px {
            b: self.pixels[i],
            g: self.pixels[i + 1],
            r: self.pixels[i + 2],
        }
    }

    fn cell_center(&self, col: usize, row: usize) -> Px {
        let x = col * self.cell_w + self.cell_w / 2;
        let y = row * self.cell_h + self.cell_h / 2;
        self.px(x.min(self.width - 1), y.min(self.height - 1))
    }
}

fn make_grid(face: Face) -> Grid {
    let metrics = Metrics::calc(face.face_metrics());
    let resolver = CodepointResolver::new(Collection::new(face));
    Grid::new(resolver, metrics).expect("grid")
}

/// Build a kitty transmit-and-display APC for a direct RGBA image, scaled to
/// `cols`×`rows` cells: `ESC _ G a=T,f=32,s=W,v=H,c=cols,r=rows,i=id ; <b64> ESC \`.
fn transmit_and_display_rgba(
    id: u32,
    w: u32,
    h: u32,
    cols: u32,
    rows: u32,
    rgba: &[u8],
) -> Vec<u8> {
    let payload = base64::engine::general_purpose::STANDARD.encode(rgba);
    format!("\x1b_Ga=T,f=32,s={w},v={h},c={cols},r={rows},i={id};{payload}\x1b\\").into_bytes()
}

#[test]
fn kitty_image_offscreen_readback() {
    let backend = match Metal::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e}); skipping kitty-image test");
            return;
        }
    };

    let text_face = Face::load_embedded(16.0).expect("embedded JetBrains Mono");
    let metrics = Metrics::calc(text_face.face_metrics());
    let (cw, ch) = (metrics.cell_width, metrics.cell_height);
    let mut grid = make_grid(text_face);

    let cols = 20u16;
    let rows = 6u16;
    let mut term = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    // The kitty placement geometry (`Placement::pixel_size`) divides the
    // terminal's pixel size by its cell count to get the cell size; set them to
    // match the renderer's cell metrics so a `c`/`r`-scaled image lines up on
    // the grid. In the app these come from the window's pixel size.
    term.width_px = u32::from(cols) * cw;
    term.height_px = u32::from(rows) * ch;

    // Home the cursor, then transmit + display a 2×2 solid-red image scaled to
    // a 6×3 cell block (top-left at 0,0). Solid color → linear upscaling stays
    // red everywhere, so any covered cell center reads red.
    let red = [255u8, 0, 0, 255].repeat(4); // 2x2 RGBA
    let img_cols = 6u32;
    let img_rows = 3u32;
    let mut stream = Stream::new(TerminalHandler::new(term));
    stream.feed(b"\x1b[H");
    stream.feed(&transmit_and_display_rgba(
        1, 2, 2, img_cols, img_rows, &red,
    ));
    let term = stream.handler.terminal;

    let snapshot = FullSnapshot::capture(&term, 0);
    // The image must have resolved into the snapshot: 1 placement, 1 image.
    assert_eq!(
        snapshot.kitty_placements().len(),
        1,
        "expected one resolved kitty placement"
    );
    assert_eq!(snapshot.kitty_images().len(), 1, "expected one kitty image");

    let mut engine = Engine::with_backend(backend, cw, ch).expect("engine");
    engine.update_frame(&snapshot, &mut grid, FrameOptions::default());
    engine.sync_atlas(&grid).expect("sync atlas");
    let pixels = engine.draw_frame().expect("draw frame");

    let (sw, sh) = engine.screen_size();
    assert_eq!(pixels.len(), sw * sh * 4, "readback size");
    let frame = Frame {
        pixels,
        width: sw,
        height: sh,
        cell_w: cw as usize,
        cell_h: ch as usize,
    };

    // Cells inside the 6×3 placement are red.
    for &(col, row) in &[(0usize, 0usize), (2, 1), (5, 2), (0, 2), (5, 0)] {
        let p = frame.cell_center(col, row);
        assert!(
            p.r > 180 && p.g < 70 && p.b < 70,
            "cell ({col},{row}) should be image-red, got {p:?}"
        );
    }

    // Cells outside the placement are the (black) background.
    for &(col, row) in &[(10usize, 4usize), (7, 1), (0, 4)] {
        let p = frame.cell_center(col, row);
        assert!(
            p.r < 40 && p.g < 40 && p.b < 40,
            "cell ({col},{row}) should be background, got {p:?}"
        );
    }
}

/// R6 slice 2 end-to-end: an image scrolled partly above the viewport renders
/// only its visible bottom rows (top clipped by the GPU), and the resolver
/// reports the expected negative `grid_row`. Exercises the scrollback-offset
/// threading through `capture` → resolve → draw.
#[test]
fn kitty_image_scrolled_clips_top() {
    let backend = match Metal::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e}); skipping scrolled kitty-image test");
            return;
        }
    };

    let text_face = Face::load_embedded(16.0).expect("embedded JetBrains Mono");
    let metrics = Metrics::calc(text_face.face_metrics());
    let (cw, ch) = (metrics.cell_width, metrics.cell_height);
    let mut grid = make_grid(text_face);

    let cols = 20u16;
    let rows = 6u16;
    let mut term = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    term.width_px = u32::from(cols) * cw;
    term.height_px = u32::from(rows) * ch;

    // A 4-cell-tall red image at home (screen rows 0..=3), then scroll it up.
    let red = [255u8, 0, 0, 255].repeat(4);
    let mut stream = Stream::new(TerminalHandler::new(term));
    stream.feed(b"\x1b[H");
    stream.feed(&transmit_and_display_rgba(1, 2, 2, 6, 4, &red));
    for _ in 0..10 {
        stream.feed(b"scroll\r\n");
    }
    let term = stream.handler.terminal;

    // Offset that puts the window top on the image's 3rd row (top_y = 2): the
    // image's first two rows clip above, rows 2..=3 show at frame rows 0..=1.
    let total = term.screen().pages.total_rows();
    let scrollback_len = total - usize::from(rows);
    let offset = scrollback_len - 2;

    let snapshot = FullSnapshot::capture(&term, offset);
    assert_eq!(snapshot.kitty_placements().len(), 1, "one placement");
    // The resolver reports the clipped (negative) row.
    assert_eq!(
        snapshot.kitty_placements()[0].instance.grid_pos[1],
        -2.0,
        "top two rows are clipped above the window"
    );

    let mut engine = Engine::with_backend(backend, cw, ch).expect("engine");
    engine.update_frame(&snapshot, &mut grid, FrameOptions::default());
    engine.sync_atlas(&grid).expect("sync atlas");
    let pixels = engine.draw_frame().expect("draw frame");
    let (sw, sh) = engine.screen_size();
    let frame = Frame {
        pixels,
        width: sw,
        height: sh,
        cell_w: cw as usize,
        cell_h: ch as usize,
    };

    // The visible bottom of the image (frame rows 0..=1) is red.
    for &(col, row) in &[(0usize, 0usize), (3, 0), (5, 1), (0, 1)] {
        let p = frame.cell_center(col, row);
        assert!(
            p.r > 180 && p.g < 70 && p.b < 70,
            "cell ({col},{row}) should be visible image-red, got {p:?}"
        );
    }
    // Below the clipped image (frame rows 2+) is scrollback text/background, not
    // the image color.
    for &(col, row) in &[(3usize, 2usize), (3, 3)] {
        let p = frame.cell_center(col, row);
        assert!(
            !(p.r > 150 && p.g < 90 && p.b < 90),
            "cell ({col},{row}) below the image should not be image-red, got {p:?}"
        );
    }
}
