//! R7 slice 1 (PR-B) acceptance test: OSC8 hyperlink HOVER UNDERLINE.
//!
//! Feeds a terminal an OSC8 link over "ab" (cols 0-1) followed by a plain "c"
//! (col 2, no link), then renders two offscreen frames: one with the mouse
//! hovering a link cell and one with no hover. Reading the pixels back, the
//! underline band (bottom of the cell) must gain ink on *both* link cells when
//! hovered — and the non-link cell must be unchanged. This proves
//! `FrameOptions.hovered_cell` forces an underline across exactly the hovered
//! link's cells.
//!
//! Skips gracefully (prints `SKIP:`) when no Metal device is present.

#![cfg(target_os = "macos")]

use qwertty_term_font::coretext::Face;
use qwertty_term_font::grid::Grid;
use qwertty_term_font::{CodepointResolver, Collection, Metrics};
use qwertty_term_renderer::engine::{Engine, FrameOptions};
use qwertty_term_renderer::metal::Metal;
use qwertty_term_renderer::snapshot::FullSnapshot;
use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};

struct Frame {
    pixels: Vec<u8>,
    width: usize,
    cell_w: usize,
    cell_h: usize,
}

impl Frame {
    /// Number of pixels in cell `(col, row)` whose color differs meaningfully
    /// from `other`'s. Used to detect that hovering added an underline: the
    /// hovered link's cells change a lot, unrelated cells not at all.
    fn cell_diff_count(&self, other: &Frame, col: usize, row: usize) -> usize {
        let mut n = 0usize;
        for dy in 0..self.cell_h {
            for dx in 0..self.cell_w {
                let x = col * self.cell_w + dx;
                let y = row * self.cell_h + dy;
                if x >= self.width {
                    continue;
                }
                let i = (y * self.width + x) * 4;
                let d = (self.pixels[i] as i32 - other.pixels[i] as i32).abs()
                    + (self.pixels[i + 1] as i32 - other.pixels[i + 1] as i32).abs()
                    + (self.pixels[i + 2] as i32 - other.pixels[i + 2] as i32).abs();
                if d > 24 {
                    n += 1;
                }
            }
        }
        n
    }
}

fn make_grid(face: Face) -> Grid {
    let metrics = Metrics::calc(face.face_metrics());
    let resolver = CodepointResolver::new(Collection::new(face));
    Grid::new(resolver, metrics).expect("grid")
}

/// Render one offscreen frame with `feed` scripted into a `cols`-wide terminal
/// and the given `hovered_cell`, returning the read-back pixels.
fn render(cols: u16, feed: &[u8], hovered_cell: Option<(usize, usize)>) -> Option<Frame> {
    let backend = match Metal::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e}); skipping link-hover test");
            return None;
        }
    };
    let text_face = Face::load_embedded(16.0).expect("embedded JetBrains Mono");
    let metrics = Metrics::calc(text_face.face_metrics());
    let (cw, ch) = (metrics.cell_width, metrics.cell_height);
    let mut grid = make_grid(text_face);

    let term = Terminal::new(Options {
        cols,
        rows: 2,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    stream.feed(feed);
    let term = stream.handler.terminal;

    let snapshot = FullSnapshot::capture(&term, 0);
    let mut engine = Engine::with_backend(backend, cw, ch).expect("engine");
    let opts = FrameOptions {
        hovered_cell,
        ..FrameOptions::default()
    };
    engine.update_frame(&snapshot, &mut grid, opts);
    engine.sync_atlas(&grid).expect("sync atlas");
    let pixels = engine.draw_frame().expect("draw frame");
    let (sw, _sh) = engine.screen_size();
    Some(Frame {
        pixels,
        width: sw,
        cell_w: cw as usize,
        cell_h: ch as usize,
    })
}

/// OSC8 link over "ab" (cols 0-1), close, then a plain "c" (col 2, no link).
const OSC8_SESSION: &[u8] = b"\x1b]8;;http://example.test\x1b\\ab\x1b]8;;\x1b\\c";

#[test]
fn hovered_link_underlines_all_its_cells_and_only_those() {
    let Some(off) = render(10, OSC8_SESSION, None) else {
        return; // skipped (no Metal)
    };
    // Hover the *second* cell of the link (col 1) to prove the underline spans
    // the whole link, not just the hovered cell.
    let on = render(10, OSC8_SESSION, Some((1, 0))).expect("second render");

    // Both link cells (col 0 and col 1) change substantially when hovered — the
    // forced underline adds a full-width horizontal line of pixels.
    let link0 = on.cell_diff_count(&off, 0, 0);
    let link1 = on.cell_diff_count(&off, 1, 0);
    assert!(
        link0 > 8,
        "link cell col 0 should gain a hover underline (changed pixels: {link0})"
    );
    assert!(
        link1 > 8,
        "link cell col 1 (whole link underlines, not just hovered) \
         should change too (changed pixels: {link1})"
    );

    // The non-link cell (col 2, 'c') must be untouched — the underline is forced
    // only across the hovered link's cells.
    let nonlink = on.cell_diff_count(&off, 2, 0);
    assert!(
        nonlink == 0,
        "non-link cell col 2 must not change on hover (changed pixels: {nonlink})"
    );
}

#[test]
fn hovered_regex_url_underlines_the_whole_span() {
    // A plain (non-OSC8) URL detected by regex. Layout: "x " then
    // "http://a.test" at cols 2..=14, a space, then "y".
    const SESSION: &[u8] = b"x http://a.test y";
    let Some(off) = render(20, SESSION, None) else {
        return; // skipped (no Metal)
    };
    // Hover col 5 ('p'), inside the URL.
    let on = render(20, SESSION, Some((5, 0))).expect("second render");

    // Both ends of the detected URL span gain the hover underline.
    let start = on.cell_diff_count(&off, 2, 0); // 'h'
    let end = on.cell_diff_count(&off, 14, 0); // last 't'
    assert!(
        start > 8,
        "URL start cell (col 2) should gain a hover underline ({start})"
    );
    assert!(
        end > 8,
        "URL end cell (col 14) should gain a hover underline ({end}) — the whole span, not just col 5"
    );

    // Text outside the URL is untouched: the leading 'x' (col 0).
    let outside = on.cell_diff_count(&off, 0, 0);
    assert!(
        outside == 0,
        "non-URL cell (col 0 'x') must not change on hover ({outside})"
    );
}
