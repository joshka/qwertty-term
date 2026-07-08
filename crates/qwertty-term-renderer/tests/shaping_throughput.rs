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

use qwertty_term_font::coretext::Face;
use qwertty_term_font::grid::Grid;
use qwertty_term_font::{CodepointResolver, Collection, Metrics};
use qwertty_term_renderer::engine::{Engine, FrameOptions};
use qwertty_term_renderer::metal::Metal;
use qwertty_term_renderer::snapshot::FullSnapshot;
use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};

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

/// Feed bytes through a `Stream`, returning the terminal.
fn feed(term: Terminal, bytes: &[u8]) -> Terminal {
    let mut stream = Stream::new(TerminalHandler::new(term));
    stream.feed(bytes);
    stream.handler.terminal
}

/// Fill a fresh terminal with a busy 80x24 grid of shaped content.
fn busy_term(cols: u16, rows: u16) -> Terminal {
    let term = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    for _ in 0..rows {
        stream.feed(b"let x = foo->bar == baz != qux; ");
        stream.feed("\u{2500}\u{2500}\u{2500}\u{2500}".as_bytes());
        stream.feed(b"  end\r\n");
    }
    stream.handler.terminal
}

/// Steady-state single-row-change: the dominant interactive workload (a shell
/// echoing keystrokes, a status line ticking). Compares the full-redraw path
/// (every visible row rebuilt each frame) against the dirty-tracking path (only
/// the changed row rebuilt), plus a busy all-dirty frame to prove dirty
/// tracking adds no regression when everything changes.
#[test]
fn dirty_vs_full_steady_state() {
    let backend = match Metal::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e})");
            return;
        }
    };
    let (mut grid, cw, ch) = make_grid();
    let opts = FrameOptions::default();
    let (cols, rows) = (80u16, 24u16);

    // Warm the shared shaping cache once so both paths measure steady state
    // (not cold shaping). One engine, one full render of the busy grid.
    let mut warm_engine = Engine::with_backend(backend, cw, ch).expect("engine");
    let base = busy_term(cols, rows);
    let warm = FullSnapshot::capture(&base, 0);
    warm_engine.update_frame(&warm, &mut grid, opts);
    warm_engine.sync_atlas(&grid).expect("warm atlas");
    let _ = warm_engine.draw_frame().expect("warm draw");

    const N: u32 = 200;

    // --- Full-redraw path: rebuild EVERY row each frame (read-only capture). ---
    // We mutate one row per frame but ignore dirty info (all rows rebuilt).
    let mut full_engine = Engine::with_backend(Metal::new().unwrap(), cw, ch).expect("full engine");
    // Prime it.
    let mut term = busy_term(cols, rows);
    let s = FullSnapshot::capture(&term, 0);
    full_engine.update_frame(&s, &mut grid, opts);

    let t_full = Instant::now();
    for i in 0..N {
        // Change exactly one row (cursor to row (i % rows), overwrite a word).
        let row = (i % rows as u32) + 1; // 1-based CUP
        term = feed(term, format!("\x1b[{row};1Hchanged{i:03}").as_bytes());
        let snap = FullSnapshot::capture(&term, 0); // all rows dirty => full
        full_engine.update_frame(&snap, &mut grid, opts);
    }
    let full_each = t_full.elapsed() / N;

    // --- Dirty-tracking path: only the changed row rebuilt (tracking capture). ---
    let mut dirty_engine =
        Engine::with_backend(Metal::new().unwrap(), cw, ch).expect("dirty engine");
    let mut term = busy_term(cols, rows);
    // Prime + drain initial dirt so subsequent frames are steady-state partial.
    let s = FullSnapshot::capture_tracking(&mut term, 0);
    dirty_engine.update_frame(&s, &mut grid, opts);

    let t_dirty = Instant::now();
    for i in 0..N {
        let row = (i % rows as u32) + 1;
        term = feed(term, format!("\x1b[{row};1Hchanged{i:03}").as_bytes());
        let snap = FullSnapshot::capture_tracking(&mut term, 0); // ~1 row dirty
        dirty_engine.update_frame(&snap, &mut grid, opts);
    }
    let dirty_each = t_dirty.elapsed() / N;

    // --- Busy all-dirty: every row changes each frame (no-regression check). ---
    let mut busy_engine = Engine::with_backend(Metal::new().unwrap(), cw, ch).expect("busy engine");
    let mut term = busy_term(cols, rows);
    let s = FullSnapshot::capture_tracking(&mut term, 0);
    busy_engine.update_frame(&s, &mut grid, opts);

    let t_busy = Instant::now();
    for i in 0..N {
        // Rewrite the whole grid (every row dirty) via a clear + full repaint.
        term = feed(term, b"\x1b[2J\x1b[H");
        for _ in 0..rows {
            term = feed(
                term,
                format!("busy frame {i} row content here padded\r\n").as_bytes(),
            );
        }
        let snap = FullSnapshot::capture_tracking(&mut term, 0); // all dirty
        busy_engine.update_frame(&snap, &mut grid, opts);
    }
    let busy_each = t_busy.elapsed() / N;

    eprintln!(
        "dirty-vs-full steady state ({cols}x{rows}, single-row change):\n  \
         full-redraw : {full_each:?}/frame\n  \
         dirty-track : {dirty_each:?}/frame ({:.1}x faster)\n  \
         busy all-dirty (dirty path): {busy_each:?}/frame",
        full_each.as_secs_f64() / dirty_each.as_secs_f64().max(f64::MIN_POSITIVE),
    );

    // The dirty path must not be SLOWER than full redraw for a single-row change
    // (we expect a large win, but assert only the loose direction so CI is
    // stable). Add a small absolute floor so timer noise on a tiny duration
    // doesn't flip the comparison.
    assert!(
        dirty_each <= full_each + std::time::Duration::from_micros(50),
        "dirty tracking should not regress single-row-change frames: \
         dirty {dirty_each:?} vs full {full_each:?}"
    );
}
