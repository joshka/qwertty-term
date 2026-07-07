//! Perf sanity for run-based engine shaping.
//!
//! The reduced engine rebuilds every visible row every frame (full-redraw), so
//! run shaping happens per-frame. The shaped-run cache keeps steady-state cost
//! low: an unchanged row re-uses cached glyphs instead of re-shaping. This test
//! measures `update_frame` throughput over a filled grid and reports the cold
//! (first, cache-empty) vs warm (cached) per-frame cost, so a regression shows
//! up as a slow warm frame.
//!
//! It's a reporting test (asserts only a loose upper bound so CI doesn't flake
//! on a busy machine); the printed numbers are the evidence. Run with
//! `--release --nocapture` for meaningful figures.
//!
//! Skips (`SKIP:`) when no Metal device is present.
#![cfg(target_os = "macos")]

use std::time::Instant;

use ghostty_font::coretext::Face;
use ghostty_font::grid::Grid;
use ghostty_font::{CodepointResolver, Collection, Metrics};
use ghostty_renderer::engine::{Engine, FrameOptions};
use ghostty_renderer::metal::Metal;
use ghostty_renderer::snapshot::FullSnapshot;
use ghostty_vt::stream::{Stream, TerminalHandler};
use ghostty_vt::terminal::{Options, Terminal};

fn make_grid() -> (Grid, u32, u32) {
    let face = Face::load_embedded(16.0).expect("embedded");
    let metrics = Metrics::calc(face.face_metrics());
    let (cw, ch) = (metrics.cell_width, metrics.cell_height);
    let resolver = CodepointResolver::new(Collection::new(face));
    (Grid::new(resolver, metrics).expect("grid"), cw, ch)
}

#[test]
fn update_frame_throughput() {
    let backend = match Metal::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e})");
            return;
        }
    };

    let (mut grid, cw, ch) = make_grid();

    // A busy 80x24 screen: a repeating mix of letters (runs), a couple of
    // ligature-ish operator pairs, and box-drawing sprites (run breaks).
    let cols = 80u16;
    let rows = 24u16;
    let term = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    for _ in 0..rows {
        // 80 columns of assorted content that segments into several runs.
        stream.feed(b"let x = foo->bar == baz != qux; ");
        stream.feed("\u{2500}\u{2500}\u{2500}\u{2500}".as_bytes()); // box drawing
        stream.feed(b"  end\r\n");
    }
    let term = stream.handler.terminal;
    let snapshot = FullSnapshot::capture(&term, 0);

    let mut engine = Engine::with_backend(backend, cw, ch).expect("engine");
    let opts = FrameOptions::default();

    // Cold frame (cache empty: every run shaped once).
    let t0 = Instant::now();
    engine.update_frame(&snapshot, &mut grid, opts);
    let cold = t0.elapsed();

    // Warm frames (cache populated: runs re-used, no re-shaping).
    const N: u32 = 200;
    let t1 = Instant::now();
    for _ in 0..N {
        engine.update_frame(&snapshot, &mut grid, opts);
    }
    let warm_total = t1.elapsed();
    let warm_each = warm_total / N;

    eprintln!(
        "update_frame throughput ({cols}x{rows}): cold {cold:?}, warm {warm_each:?}/frame \
         ({:.0} frames/s warm)",
        1.0 / warm_each.as_secs_f64()
    );

    // Loose ceiling so CI doesn't flake, but catches a gross regression: a warm
    // full-grid rebuild should be well under 10ms even in debug builds.
    assert!(
        warm_each.as_millis() < 50,
        "warm update_frame unexpectedly slow: {warm_each:?}/frame"
    );
}
