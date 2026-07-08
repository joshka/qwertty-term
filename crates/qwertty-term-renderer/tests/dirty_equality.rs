//! Dirty-tracking equality-vs-full-redraw property test (the heart of the
//! per-row dirty chunk).
//!
//! For a battery of scripted scenarios — single-row edit, scroll (viewport
//! move), SGR-only change, selection change, palette change (OSC 4), and screen
//! switch (1049) — this renders the SAME final terminal state two ways and
//! asserts the readback pixels are byte-for-byte IDENTICAL:
//!
//! 1. **Dirty-tracking path**: one engine renders frame A, then renders frame B
//!    via `FullSnapshot::capture_tracking` (which reports only the rows/globals
//!    that actually changed, so the engine rebuilds a subset of rows).
//! 2. **Full-redraw reference**: a *fresh* engine renders frame B from scratch
//!    via `FullSnapshot::capture` (every row rebuilt).
//!
//! If incremental redraw ever leaves a stale row, a wrong global-rebuild
//! decision, or a mis-remapped cursor, the two frames diverge and the test
//! fails. Equality is the whole contract.
//!
//! Skips gracefully (`SKIP:`) when no Metal device is present.

#![cfg(target_os = "macos")]

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

/// Feed `bytes` into `term`, moving it through a `Stream` and back.
fn feed(term: Terminal, bytes: &[u8]) -> Terminal {
    let mut stream = Stream::new(TerminalHandler::new(term));
    stream.feed(bytes);
    stream.handler.terminal
}

/// A scenario: the setup bytes that produce frame A, the mutation bytes that
/// produce frame B, and the scrollback offsets to render A / B at.
struct Scenario {
    name: &'static str,
    cols: u16,
    rows: u16,
    setup: &'static [u8],
    /// Applied to the terminal after frame A is rendered.
    mutate: fn(Terminal) -> Terminal,
    /// Scrollback offset for frame A and frame B (a scroll scenario differs).
    offset_a: usize,
    offset_b: usize,
}

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "single-row edit",
            cols: 20,
            rows: 5,
            setup: b"row0\r\nrow1\r\nrow2\r\nrow3\r\nrow4",
            mutate: |t| feed(t, b"\x1b[3;1HEDITED"), // overwrite row index 2
            offset_a: 0,
            offset_b: 0,
        },
        Scenario {
            name: "SGR-only change (same text, new color)",
            cols: 20,
            rows: 4,
            setup: b"alpha\r\nbravo\r\ncharlie\r\ndelta",
            // Rewrite row 1 with a red foreground; same glyphs, different color.
            mutate: |t| feed(t, b"\x1b[2;1H\x1b[31mbravo\x1b[0m"),
            offset_a: 0,
            offset_b: 0,
        },
        Scenario {
            name: "scroll into history (viewport move)",
            cols: 8,
            rows: 3,
            // Push several rows into scrollback.
            setup: b"L0\r\nL1\r\nL2\r\nL3\r\nL4\r\nL5\r\nL6\r\nL7",
            mutate: |t| t, // no content change; only the render offset differs
            offset_a: 0,
            offset_b: 2, // scroll up 2 rows
        },
        Scenario {
            name: "selection change (global dirty, no visible change)",
            cols: 16,
            rows: 4,
            setup: b"select me now\r\nsecond\r\nthird\r\nfourth",
            mutate: |mut t| {
                use qwertty_term_vt::point::Point;
                use qwertty_term_vt::screen::selection::Selection;
                let s = t.screen_mut();
                let start = s.pages.pin(Point::active(0, 0)).unwrap();
                let end = s.pages.pin(Point::active(5, 0)).unwrap();
                s.select(Some(Selection::init(start, end, false)));
                t
            },
            offset_a: 0,
            offset_b: 0,
        },
        Scenario {
            name: "palette change (OSC 4)",
            cols: 16,
            rows: 3,
            // Use palette color 1 for some text, then remap it.
            setup: b"\x1b[31mred text\x1b[0m\r\nplain\r\nmore",
            mutate: |t| feed(t, b"\x1b]4;1;#00ff00\x1b\\"), // remap index 1 to green
            offset_a: 0,
            offset_b: 0,
        },
        Scenario {
            name: "screen switch (alt screen 1049)",
            cols: 16,
            rows: 4,
            setup: b"primary line 0\r\nprimary line 1\r\nprimary 2\r\nprimary 3",
            // Enter alt screen and draw different content.
            mutate: |t| feed(t, b"\x1b[?1049h\x1b[2J\x1b[HALT SCREEN\r\nsecond alt"),
            offset_a: 0,
            offset_b: 0,
        },
    ]
}

#[test]
fn dirty_tracking_equals_full_redraw() {
    let backend = match Metal::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e})");
            return;
        }
    };

    let (mut grid, cw, ch) = make_grid();
    let opts = FrameOptions::default();

    // The dirty engine accumulates state across A->B (the incremental path).
    // Each reference render below builds a fresh engine over its own backend.
    let mut dirty_engine = Engine::with_backend(backend, cw, ch).expect("dirty engine");

    let mut failures = Vec::new();

    for sc in scenarios() {
        // --- Build terminal state A. ---
        let term_a = Terminal::new(Options {
            cols: sc.cols,
            rows: sc.rows,
            ..Default::default()
        });
        let term_a = feed(term_a, sc.setup);

        // Render frame A on the dirty engine (tracking) so it holds stale rows.
        let mut term_a = term_a;
        let snap_a = FullSnapshot::capture_tracking(&mut term_a, sc.offset_a);
        dirty_engine.update_frame(&snap_a, &mut grid, opts);
        dirty_engine.sync_atlas(&grid).expect("sync a");
        let _ = dirty_engine.draw_frame().expect("draw a");

        // --- Apply the mutation to produce state B. ---
        let mut term_b = (sc.mutate)(term_a);

        // Dirty-tracking render of B (reuses dirty_engine's A contents).
        let snap_b = FullSnapshot::capture_tracking(&mut term_b, sc.offset_b);
        dirty_engine.update_frame(&snap_b, &mut grid, opts);
        dirty_engine.sync_atlas(&grid).expect("sync b dirty");
        let dirty_pixels = dirty_engine.draw_frame().expect("draw b dirty");

        // --- Full-redraw reference render of the SAME state B. ---
        // Fresh engine + backend, read-only capture (all rows dirty => full
        // rebuild).
        let ref_backend = Metal::new().expect("ref backend");
        let mut ref_engine = Engine::with_backend(ref_backend, cw, ch).expect("ref engine");
        let snap_ref = FullSnapshot::capture(&term_b, sc.offset_b);
        ref_engine.update_frame(&snap_ref, &mut grid, opts);
        ref_engine.sync_atlas(&grid).expect("sync b ref");
        let ref_pixels = ref_engine.draw_frame().expect("draw b ref");

        // === EQUALITY: pixels must be byte-for-byte identical. ===
        if dirty_pixels != ref_pixels {
            let n = dirty_pixels.len().min(ref_pixels.len());
            let diffs = (0..n).filter(|&i| dirty_pixels[i] != ref_pixels[i]).count();
            failures.push(format!(
                "scenario '{}': dirty-tracking != full-redraw ({} byte diffs of {} bytes; \
                 lens dirty={} ref={})",
                sc.name,
                diffs,
                n,
                dirty_pixels.len(),
                ref_pixels.len()
            ));
        } else {
            println!(
                "scenario '{}': OK ({} bytes identical)",
                sc.name,
                dirty_pixels.len()
            );
        }
    }

    assert!(
        failures.is_empty(),
        "equality failures:\n{}",
        failures.join("\n")
    );
}
