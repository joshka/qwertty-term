//! Cursor-hide-when-scrolled-back offscreen pixel test.
//!
//! When the viewport is scrolled into history, upstream draws NO cursor
//! (`render.zig` sets `cursor.viewport = null`; `cursor.zig` priority #1). The
//! Rust `FullSnapshot::cursor()` mirrors this by suppressing the cursor when
//! its active-area position falls outside the visible window.
//!
//! This test renders the same terminal at scrollback offset 0 (cursor visible)
//! and at a nonzero offset (cursor scrolled out) and asserts, in real
//! read-back pixels, that a block cursor's characteristic full-cell ink is
//! present at the cursor cell when at the bottom and ABSENT once scrolled back.
//!
//! Skips gracefully (`SKIP:`) when no Metal device is present.

#![cfg(target_os = "macos")]

use ghostty_font::coretext::Face;
use ghostty_font::grid::Grid;
use ghostty_font::{CodepointResolver, Collection, Metrics};
use ghostty_renderer::engine::{Engine, FrameOptions};
use ghostty_renderer::metal::Metal;
use ghostty_renderer::snapshot::{FullSnapshot, RenderSnapshot};
use ghostty_vt::stream::{Stream, TerminalHandler};
use ghostty_vt::terminal::{Options, Terminal};

fn make_grid() -> (Grid, u32, u32) {
    let face = Face::load_embedded(16.0).expect("embedded");
    let metrics = Metrics::calc(face.face_metrics());
    let (cw, ch) = (metrics.cell_width, metrics.cell_height);
    let resolver = CodepointResolver::new(Collection::new(face));
    (Grid::new(resolver, metrics).expect("grid"), cw, ch)
}

/// Max abs-channel delta from `bg` over an entire cell's pixels.
fn cell_max_delta(
    pixels: &[u8],
    width: usize,
    cw: usize,
    ch: usize,
    col: usize,
    row: usize,
    bg: [u8; 3],
) -> i32 {
    let mut max = 0i32;
    for dy in 0..ch {
        for dx in 0..cw {
            let x = col * cw + dx;
            let y = row * ch + dy;
            let i = (y * width + x) * 4;
            if i + 2 >= pixels.len() {
                continue;
            }
            let (b, g, r) = (pixels[i], pixels[i + 1], pixels[i + 2]);
            let d = (r as i32 - bg[0] as i32).abs()
                + (g as i32 - bg[1] as i32).abs()
                + (b as i32 - bg[2] as i32).abs();
            max = max.max(d);
        }
    }
    max
}

#[test]
fn cursor_hidden_when_scrolled_back() {
    let backend = match Metal::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e})");
            return;
        }
    };

    let (mut grid, cw, ch) = make_grid();
    let opts = FrameOptions::default();
    let bg = [opts.default_bg.r, opts.default_bg.g, opts.default_bg.b];

    // A 6-col x 3-row screen with enough history to scroll. The cursor lands at
    // the end of the last written text on the bottom active row.
    let cols = 6u16;
    let rows = 3u16;
    let term = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    // Push several rows into scrollback, then leave the cursor at a known empty
    // cell on the last active row.
    stream.feed(b"h0\r\nh1\r\nh2\r\nh3\r\nh4\r\n");
    // Cursor now on the last active row, col 0 (a blank cell => a block cursor
    // fills it with cursor color, unmistakable ink).
    let term = stream.handler.terminal;

    // Confirm there IS scrollback to scroll into.
    let sb = term.snapshot().scrollback_len();
    assert!(
        sb >= 1,
        "need scrollback to test scroll-back hide (got {sb})"
    );

    // The cursor is on the bottom active row (row index rows-1 in the window at
    // offset 0). Find its column from the snapshot cursor.
    let win0 = term.snapshot_window(0);
    let cur_row = win0.cursor.row; // active-area row
    let cur_col = win0.cursor.col;

    // --- Render at offset 0: cursor visible. ---
    let mut engine = Engine::with_backend(backend, cw, ch).expect("engine");
    let snap0 = FullSnapshot::capture(&term, 0);
    engine.update_frame(&snap0, &mut grid, opts);
    engine.sync_atlas(&grid).expect("sync 0");
    let px0 = engine.draw_frame().expect("draw 0");
    let (sw, _sh) = engine.screen_size();

    let ink_at_bottom = cell_max_delta(&px0, sw, cw as usize, ch as usize, cur_col, cur_row, bg);
    assert!(
        ink_at_bottom > 40,
        "cursor cell ({cur_col},{cur_row}) should have block-cursor ink at offset 0; \
         delta {ink_at_bottom}"
    );

    // --- Render scrolled fully into history: cursor suppressed. ---
    // At the max offset the active area (and thus the cursor) is entirely out
    // of view. Every visible cell should be a history row with NO cursor ink at
    // the stale (col,row) position.
    let snap_up = FullSnapshot::capture(&term, sb);
    assert!(
        snap_up.cursor().is_none(),
        "cursor must be suppressed when scrolled into history"
    );

    engine.update_frame(&snap_up, &mut grid, opts);
    engine.sync_atlas(&grid).expect("sync up");
    let px_up = engine.draw_frame().expect("draw up");

    // The stale cursor cell position: a block cursor would fill it with cursor
    // color. Whatever history text is there, it must NOT be a full-cell block of
    // the cursor color. We assert the cell is not saturated the way a block
    // cursor makes it: the simplest robust check is that the rendered frame here
    // equals a full-redraw reference that also draws no cursor (proving no stale
    // cursor ink leaked in). Compare against a fresh full-redraw.
    let ref_backend = Metal::new().expect("ref backend");
    let mut ref_engine = Engine::with_backend(ref_backend, cw, ch).expect("ref engine");
    let ref_snap = FullSnapshot::capture(&term, sb);
    ref_engine.update_frame(&ref_snap, &mut grid, opts);
    ref_engine.sync_atlas(&grid).expect("sync ref");
    let px_ref = ref_engine.draw_frame().expect("draw ref");
    assert_eq!(
        px_up, px_ref,
        "scrolled-back frame must match full-redraw (no stale cursor ink)"
    );

    // Belt-and-suspenders: the history rows visible at max offset are the OLD
    // rows h0..h2 which end at column 2; the bottom-right region where the
    // cursor used to sit must read as background (no ink), since history rows
    // are short.
    if cur_col + 1 >= cols as usize {
        // Cursor was at the far column; check it directly.
        let stale_ink = cell_max_delta(
            &px_up,
            sw,
            cw as usize,
            ch as usize,
            cur_col,
            (rows - 1) as usize,
            bg,
        );
        assert!(
            stale_ink <= 40,
            "no cursor ink should remain at the stale cursor cell; delta {stale_ink}"
        );
    }
}
