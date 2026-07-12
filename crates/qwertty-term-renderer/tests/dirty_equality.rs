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

/// Kitty-image equality property (R6 slice 1) across the image transitions that
/// touch the engine's *persistent* image state — the non-evicting texture cache
/// (`images`) and the grown-not-shrunk instance-buffer pool (`image_instances`).
///
/// A single stateful "dirty" engine is walked through a sequence — no image →
/// add → re-transmit same id with new content → delete — and at each step its
/// output is compared byte-for-byte against a *fresh* engine rendering that same
/// terminal state from scratch. If the dirty engine ever let a stale texture, a
/// missed generation-bump re-upload, or a leftover instance buffer bleed into a
/// frame, the two diverge. This locks the invariant that the draw loop is bounded
/// by `pending_placements` (stale map/pool entries are invisible), which today
/// holds only by construction.
///
/// Native-size images (no `c`/`r` scaling), so no terminal pixel geometry needed.
#[test]
fn dirty_tracking_equals_full_redraw_with_image() {
    use base64::Engine as _;

    let backend = match Metal::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e})");
            return;
        }
    };
    let (mut grid, cw, ch) = make_grid();
    let opts = FrameOptions::default();

    // Two distinct 6×6 solid images (blue, then green) transmitted as id=1.
    let blue = [0u8, 0, 255, 255].repeat(36); // 6x6 RGBA
    let green = [0u8, 255, 0, 255].repeat(36); // 6x6 RGBA
    let display = |rgba: &[u8]| -> Vec<u8> {
        let payload = base64::engine::general_purpose::STANDARD.encode(rgba);
        format!("\x1b[H\x1b_Ga=T,f=32,s=6,v=6,i=1;{payload}\x1b\\").into_bytes()
    };

    let mut dirty = Engine::with_backend(backend, cw, ch).expect("dirty engine");

    // Render `term`'s state on the stateful `dirty` engine (carrying all prior
    // frames' image state) and on a fresh engine, and assert identical pixels.
    let step = |dirty: &mut Engine, term: &mut Terminal, grid: &mut Grid, label: &str| {
        let snap = FullSnapshot::capture_tracking(term, 0);
        dirty.update_frame(&snap, grid, opts);
        dirty.sync_atlas(grid).expect("sync dirty");
        let dirty_px = dirty.draw_frame().expect("draw dirty");

        let ref_backend = Metal::new().expect("ref backend");
        let mut fresh = Engine::with_backend(ref_backend, cw, ch).expect("fresh engine");
        let ref_snap = FullSnapshot::capture(term, 0);
        fresh.update_frame(&ref_snap, grid, opts);
        fresh.sync_atlas(grid).expect("sync fresh");
        let fresh_px = fresh.draw_frame().expect("draw fresh");

        let n = dirty_px.len().min(fresh_px.len());
        let diffs = (0..n).filter(|&i| dirty_px[i] != fresh_px[i]).count();
        assert_eq!(
            dirty_px, fresh_px,
            "kitty-image '{label}': dirty != fresh ({diffs} byte diffs of {n})"
        );
    };

    // 1. Plain text, no image.
    let mut term = feed(
        Terminal::new(Options {
            cols: 16,
            rows: 5,
            ..Default::default()
        }),
        b"before image\r\nsecond",
    );
    step(&mut dirty, &mut term, &mut grid, "no image");

    // 2. Add image id=1 (blue). Dirty engine uploads a new texture.
    term = feed(term, &display(&blue));
    step(&mut dirty, &mut term, &mut grid, "add image");

    // 3. Re-transmit id=1 with new content (green). Generation bumps → the dirty
    //    engine must re-upload; the fresh engine only ever sees green.
    term = feed(term, &display(&green));
    step(&mut dirty, &mut term, &mut grid, "re-generation");

    // 4. Delete the image. The dirty engine keeps a now-stale texture + instance
    //    buffer; the fresh engine never had one. Equal output proves stale
    //    entries are never drawn.
    term = feed(term, b"\x1b_Ga=d,d=A\x1b\\");
    step(&mut dirty, &mut term, &mut grid, "delete image");
}
