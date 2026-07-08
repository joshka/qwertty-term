//! Headless smoke test of the full terminal stack, minus the NSWindow.
//!
//! `--offscreen-smoke` runs this: spawn a real PTY + shell, drive a scripted
//! command through it, feed the output into a real `ghostty-vt` engine, render a
//! frame through the R4 cell engine into an IOSurface-backed target, read the
//! pixels back, and assert the frame is non-trivial (real glyph coverage over
//! the default background). This exercises everything the window path does
//! except CoreAnimation presentation and event handling — so a green
//! `--offscreen-smoke` on a machine with a Metal device proves the
//! engine→PTY→renderer pipeline end to end without a GUI.
//!
//! Exits `0` on success, non-zero on failure (so it doubles as a CI gate).
//! Skips gracefully (exit `0`, prints `SKIP:`) when no Metal device is present.

#![cfg(target_os = "macos")]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ghostty_renderer::engine::{Engine as RenderEngine, FrameOptions};
use ghostty_renderer::snapshot::FullSnapshot;

use crate::engine::Engine;
use crate::font;
use crate::geometry;
use crate::termio::TabIo;

/// One BGRA pixel.
#[derive(Clone, Copy)]
struct Px {
    r: u8,
    g: u8,
    b: u8,
}

/// Run the offscreen smoke. Returns `Ok(true)` on a verified render, `Ok(false)`
/// when skipped (no Metal device), and `Err` on a real failure.
pub fn run() -> Result<bool, String> {
    // Font grid at a fixed size (no display scale in the offscreen path).
    let fg = font::build(None, 16.0).map_err(|e| format!("font grid: {e}"))?;
    let (cw, ch) = (fg.cell_width, fg.cell_height);
    let mut grid = fg.grid;

    // A modest grid.
    let (cols, rows) = (40usize, 12usize);

    // Real engine (shared with the termio parse thread) + the real termio
    // stack (rustix pty + read pipeline + writer loop). This is the exact IO
    // path the window uses, minus the NSWindow.
    let engine = Arc::new(Mutex::new(Engine::new(cols, rows)));
    let io = TabIo::spawn(Arc::clone(&engine), cols as u16, rows as u16, cw, ch, None)
        .map_err(|e| format!("spawn termio: {e}"))?;

    // Give the shell a beat to draw its prompt, then drive a scripted command
    // that emits deterministic visible text.
    std::thread::sleep(Duration::from_millis(200));
    io.write(b"printf 'GHOSTTY-RS-SMOKE-OK\\n'\n");

    // The parse thread feeds the engine off-thread; poll the shared engine for
    // the marker (and drain any engine replies back to the pty).
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_marker = false;
    while Instant::now() < deadline {
        {
            let mut e = engine.lock().unwrap();
            let out = e.take_output();
            if !out.is_empty() {
                io.write(&out);
            }
            if e.screen_dump().contains("GHOSTTY-RS-SMOKE-OK") {
                saw_marker = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !saw_marker {
        return Err("timed out waiting for shell output marker".to_string());
    }

    // Build the render engine (skip if no Metal device).
    let mut render = match RenderEngine::new(cw, ch) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e}); offscreen smoke skipped");
            return Ok(false);
        }
    };

    // Snapshot → render → readback. Take the snapshot under the engine lock,
    // then drop it before the Metal draw.
    let window = engine.lock().unwrap().snapshot_window(0);
    let snapshot = FullSnapshot::from_window(window);
    render.update_frame(&snapshot, &mut grid, FrameOptions::default());
    render
        .sync_atlas(&grid)
        .map_err(|e| format!("sync atlas: {e}"))?;
    let pixels = render
        .draw_frame()
        .map_err(|e| format!("draw frame: {e}"))?;

    let (sw, sh) = render.screen_size();
    if pixels.len() != sw * sh * 4 {
        return Err(format!(
            "readback size mismatch: got {}, want {}",
            pixels.len(),
            sw * sh * 4
        ));
    }
    // Expected pixel size from geometry math must match what the engine sized.
    let (want_w, want_h) = geometry::pixel_size(cols, rows, cw, ch);
    if (sw, sh) != (want_w, want_h) {
        return Err(format!(
            "screen size {sw}x{sh} != geometry {want_w}x{want_h}"
        ));
    }

    // Assert real glyph coverage: some pixel must differ substantially from the
    // default background (proof that text was rasterized, not a blank clear).
    let bg = Px {
        r: 0x18,
        g: 0x18,
        b: 0x18,
    };
    let mut max_delta = 0i32;
    for chunk in pixels.chunks_exact(4) {
        let (b, g, r) = (chunk[0] as i32, chunk[1] as i32, chunk[2] as i32);
        let d = (r - bg.r as i32).abs() + (g - bg.g as i32).abs() + (b - bg.b as i32).abs();
        max_delta = max_delta.max(d);
    }
    if max_delta <= 40 {
        return Err(format!(
            "rendered frame has no glyph coverage (max delta {max_delta})"
        ));
    }

    // --- Search match-highlight ink assertion ---------------------------
    //
    // Re-snapshot the same content, tint a one-cell match highlight over a
    // known cell (the amber match background), render it, and assert that
    // cell's pixels now differ substantially from the *plain* render at the
    // same cell. This proves the match-highlight tint reaches real ink,
    // distinct from normal rendering — using the exact `tint_matches` path the
    // app render loop uses. The marker "GHOSTTY-RS-SMOKE-OK" occupies row 0
    // (printed by the scripted command's echo); tint its first character cell.
    {
        use crate::selection::{MatchColors, ScreenRange, tint_matches};

        // The default render at the target cell (row 0, col 0 region).
        let (tx, ty) = (0usize, 0usize); // cell coordinates
        let plain = cell_avg(&pixels, sw, sh, cw as usize, ch as usize, tx, ty);

        let mut window = engine.lock().unwrap().snapshot_window(0);
        let match_range = ScreenRange {
            top_left: (tx, ty),
            bottom_right: (tx, ty),
            rectangle: false,
        };
        tint_matches(&mut window, &[match_range], Some(0), MatchColors::default());
        let snapshot = FullSnapshot::from_window(window);
        render.update_frame(&snapshot, &mut grid, FrameOptions::default());
        render
            .sync_atlas(&grid)
            .map_err(|e| format!("sync atlas (highlight): {e}"))?;
        let hl_pixels = render
            .draw_frame()
            .map_err(|e| format!("draw frame (highlight): {e}"))?;
        let highlighted = cell_avg(&hl_pixels, sw, sh, cw as usize, ch as usize, tx, ty);

        // The amber match background (#FFE082) is far from the #181818 default
        // background and from the plain glyph render; require a substantial
        // shift at the tinted cell.
        let delta = (highlighted.r as i32 - plain.r as i32).abs()
            + (highlighted.g as i32 - plain.g as i32).abs()
            + (highlighted.b as i32 - plain.b as i32).abs();
        if delta <= 60 {
            return Err(format!(
                "search match-highlight produced no distinct ink at the match \
                 cell (plain rgb=({},{},{}) vs highlighted rgb=({},{},{}), delta \
                 {delta})",
                plain.r, plain.g, plain.b, highlighted.r, highlighted.g, highlighted.b,
            ));
        }
        eprintln!(
            "offscreen: search match-highlight ink present at the match cell \
             (plain→highlighted rgb delta {delta})"
        );
    }

    Ok(true)
}

/// Average BGRA pixel over the cell at cell-coordinate `(cx, cy)` in a readback
/// buffer of `sw`×`sh` pixels with cell size `cw`×`ch`. Used to compare the
/// plain vs match-highlighted render of a single cell.
fn cell_avg(pixels: &[u8], sw: usize, sh: usize, cw: usize, ch: usize, cx: usize, cy: usize) -> Px {
    let x0 = cx * cw;
    let y0 = cy * ch;
    let (mut sr, mut sg, mut sb, mut n) = (0u64, 0u64, 0u64, 0u64);
    for y in y0..(y0 + ch).min(sh) {
        for x in x0..(x0 + cw).min(sw) {
            let i = (y * sw + x) * 4;
            if i + 3 >= pixels.len() {
                continue;
            }
            sb += pixels[i] as u64;
            sg += pixels[i + 1] as u64;
            sr += pixels[i + 2] as u64;
            n += 1;
        }
    }
    if n == 0 {
        return Px { r: 0, g: 0, b: 0 };
    }
    Px {
        r: (sr / n) as u8,
        g: (sg / n) as u8,
        b: (sb / n) as u8,
    }
}
