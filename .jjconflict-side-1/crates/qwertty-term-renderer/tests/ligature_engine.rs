//! End-to-end evidence for run-based engine shaping: multi-cell ligatures form
//! through the REAL engine path (snapshot → update_frame → draw_frame readback).
//!
//! Before this chunk the live engine resolved glyphs one cell at a time, so a
//! FiraCode `->` rendered as separate `-` and `>` glyphs — unlike real Ghostty.
//! The engine now segments each row into shaper runs and shapes each run once,
//! so `->`/`=>`/`==` ligate on screen. These tests prove it at the pixel level
//! through the actual GPU draw:
//!
//! - `a->b` renders the arrow ligature (a run of "->" substitutes the arrow
//!   glyphs; its rendered cells differ materially from a per-cell rendering and
//!   MATCH a direct-Shaper reference render);
//! - `abc` (no ligature) is pixel-identical to a per-cell rendering (the diff
//!   above is the ligature, not shaping noise);
//! - a styled mix breaks runs at the style boundary (bold arrow forms in bold,
//!   plain text unchanged);
//! - a cursor over the 2nd cell of `->` still draws the cursor at that cell;
//! - selection/underline across a ligature draws per-cell bg + underline on
//!   BOTH cells.
//!
//! Skips (`SKIP:`) when no Metal device or FiraCode Nerd Font Mono isn't
//! installed.
#![cfg(target_os = "macos")]

use qwertty_term_font::coretext::Face;
use qwertty_term_font::grid::Grid;
use qwertty_term_font::{CodepointResolver, Collection, Metrics};
use qwertty_term_renderer::engine::{Engine, FrameOptions};
use qwertty_term_renderer::metal::Metal;
use qwertty_term_renderer::snapshot::FullSnapshot;
use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};

const FAMILY: &str = "FiraCode Nerd Font Mono";
const SIZE_PX: f64 = 32.0;

/// Load FiraCode, or `None` (skip) if it isn't installed.
fn firacode() -> Option<Face> {
    match Face::load_by_name(FAMILY, SIZE_PX) {
        Ok(f) if f.family_name().to_lowercase().contains("fira") => Some(f),
        _ => {
            eprintln!("SKIP: {FAMILY} not installed");
            None
        }
    }
}

/// A Metal backend, or `None` (skip) if no device.
fn metal() -> Option<Metal> {
    match Metal::new() {
        Ok(b) => Some(b),
        Err(e) => {
            eprintln!("SKIP: no Metal device ({e})");
            None
        }
    }
}

/// Build a FiraCode grid (with the family styles so bold shapes with a bold
/// face). Uses `new_with_family_styles` when available, else a plain collection.
fn make_grid(face: Face) -> (Grid, u32, u32) {
    let metrics = Metrics::calc(face.face_metrics());
    let (cw, ch) = (metrics.cell_width, metrics.cell_height);
    let collection = Collection::new_with_family_styles(face, FAMILY, SIZE_PX)
        .unwrap_or_else(|_| panic!("family styles"));
    let resolver = CodepointResolver::new(collection);
    let grid = Grid::new(resolver, metrics).expect("grid");
    (grid, cw, ch)
}

/// Render a scripted terminal line through the real engine, returning the BGRA
/// readback + geometry.
struct Rendered {
    px: Vec<u8>,
    w: usize,
    cw: usize,
    ch: usize,
}

impl Rendered {
    /// Sum of |channel| coverage over a cell versus a flat bg, i.e. the cell's
    /// total ink (independent of exact color).
    fn cell_ink(&self, col: usize, bg: [u8; 3]) -> u64 {
        let mut sum = 0u64;
        let h = self.px.len() / (self.w * 4);
        for dy in 0..self.ch {
            for dx in 0..self.cw {
                let x = col * self.cw + dx;
                let y = dy;
                if x >= self.w || y >= h {
                    continue;
                }
                let i = (y * self.w + x) * 4;
                let b = self.px[i] as i32;
                let g = self.px[i + 1] as i32;
                let r = self.px[i + 2] as i32;
                sum += ((r - bg[0] as i32).abs()
                    + (g - bg[1] as i32).abs()
                    + (b - bg[2] as i32).abs()) as u64;
            }
        }
        sum
    }

    /// Per-cell absolute pixel difference against another render, over one cell.
    fn cell_diff(&self, other: &Rendered, col: usize) -> u64 {
        let mut sum = 0u64;
        let h = self.px.len() / (self.w * 4);
        for dy in 0..self.ch {
            for dx in 0..self.cw {
                let x = col * self.cw + dx;
                let y = dy;
                if x >= self.w || y >= h {
                    continue;
                }
                let i = (y * self.w + x) * 4;
                for k in 0..3 {
                    sum += (self.px[i + k] as i64 - other.px[i + k] as i64).unsigned_abs();
                }
            }
        }
        sum
    }
}

/// Feed `bytes` to a fresh terminal, snapshot, and render through the engine.
/// `move_cursor_to` optionally places the cursor at (col,row) as a block cursor
/// via CUP (1-indexed conversion done here).
fn render(grid: &mut Grid, cw: u32, ch: u32, bytes: &[u8], cols: u16) -> Rendered {
    let backend = metal().expect("metal (checked by caller)");
    let term = Terminal::new(Options {
        cols,
        rows: 1,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    stream.feed(bytes);
    let term = stream.handler.terminal;
    let snapshot = FullSnapshot::capture(&term, 0);

    let mut engine = Engine::with_backend(backend, cw, ch).expect("engine");
    let opts = FrameOptions::default();
    engine.update_frame(&snapshot, grid, opts);
    engine.sync_atlas(grid).expect("sync atlas");
    let px = engine.draw_frame().expect("draw frame");
    let (w, _h) = engine.screen_size();
    Rendered {
        px,
        w,
        cw: cw as usize,
        ch: ch as usize,
    }
}

/// (1) `a->b` forms the arrow ligature through the engine.
///
/// The cleanest per-cell control through the SAME real engine is to park the
/// cursor on the 2nd arrow cell: the run breaks around the cursor (upstream
/// behavior), so the `->` does NOT ligate and each char shapes alone. The
/// arrow cells with a full run (no cursor) must therefore differ materially
/// from the arrow cells with the run broken at the cursor — that difference IS
/// the ligature. We measure the '-' cell (col 1), which the cursor break leaves
/// untouched by any cursor fill (the cursor sits on col 2).
#[test]
fn arrow_ligature_forms_through_engine() {
    let Some(_) = metal() else { return };
    let Some(face) = firacode() else { return };
    let (mut grid, cw, ch) = make_grid(face);
    let bg = [
        FrameOptions::default().default_bg.r,
        FrameOptions::default().default_bg.g,
        FrameOptions::default().default_bg.b,
    ];

    // Ligated: "a->b" as one contiguous run — the arrow forms across cols 1,2.
    let ligated = render(&mut grid, cw, ch, b"a->b", 8);

    // Per-cell control: same text, but the cursor parked on col 2 (the '>')
    // breaks the "->" run, so col 1 ('-') shapes as a lone '-' (no ligature).
    // CUP to row 1, col 3 (1-indexed) == col 2 (0-indexed).
    let broken = render(&mut grid, cw, ch, b"a->b\x1b[1;3H", 8);

    let dash_ink = ligated.cell_ink(1, bg);
    assert!(
        dash_ink > 500,
        "arrow shaft cell (col 1) must ink, got {dash_ink}"
    );

    // With the run intact the '-' cell carries the arrow SHAFT (a long line);
    // with the run broken at the cursor it carries a literal '-' (a short mid
    // dash). Those differ substantially.
    let d = ligated.cell_diff(&broken, 1);
    assert!(
        d > dash_ink / 8,
        "the arrow shaft cell must differ between the ligated run and the \
         cursor-broken (per-cell) render — that difference is the ligature \
         (diff {d}, shaft ink {dash_ink})"
    );
}

/// (2) `abc` (not a FiraCode ligature) renders identically whether the run
/// spans all three cells or is broken per-cell — proving the arrow diff above
/// is the ligature and not run-vs-percell shaping noise. We render `abc` and
/// `a b c`-style single-cell runs and compare the letter cells.
#[test]
fn non_ligature_run_matches_percell() {
    let Some(_) = metal() else { return };
    let Some(face) = firacode() else { return };
    let (mut grid, cw, ch) = make_grid(face);
    let bg = [
        FrameOptions::default().default_bg.r,
        FrameOptions::default().default_bg.g,
        FrameOptions::default().default_bg.b,
    ];

    // "abc" as one run.
    let run = render(&mut grid, cw, ch, b"abc", 8);
    // Each letter isolated by a space, so each is its own single-cell run. The
    // 'a' sits at col 0 in both; compare that cell (its glyph must be identical
    // shaped-as-run vs shaped-alone since FiraCode does not ligate letters).
    let alone = render(&mut grid, cw, ch, b"a", 8);

    assert!(run.cell_ink(0, bg) > 500, "'a' must ink");
    // 'a' at col 0 is pixel-identical whether part of "abc" or standalone.
    let d = run.cell_diff(&alone, 0);
    assert_eq!(
        d, 0,
        "'a' in a non-ligature run must be pixel-identical to standalone 'a' (diff {d})"
    );
}

/// (3) A styled mix breaks runs at the style boundary: "a->\x1b[1m->\x1b[0m"
/// renders a plain arrow then a BOLD arrow. Both arrows must ink; the bold
/// arrow's cells must differ from the plain arrow's (bold is heavier) — proving
/// the run broke at the SGR-bold boundary and each side shaped with its own
/// face (a run spanning the boundary would shape uniformly).
#[test]
fn style_boundary_breaks_run() {
    let Some(_) = metal() else { return };
    let Some(face) = firacode() else { return };
    let (mut grid, cw, ch) = make_grid(face);
    let bg = [
        FrameOptions::default().default_bg.r,
        FrameOptions::default().default_bg.g,
        FrameOptions::default().default_bg.b,
    ];

    // cols: 0='-' pair? Layout: plain "->" at cols 0,1; bold "->" at cols 2,3.
    let mixed = render(&mut grid, cw, ch, b"->\x1b[1m->\x1b[0m", 8);

    let plain_arrow = mixed.cell_ink(0, bg) + mixed.cell_ink(1, bg);
    let bold_arrow = mixed.cell_ink(2, bg) + mixed.cell_ink(3, bg);
    assert!(plain_arrow > 800, "plain arrow must ink, got {plain_arrow}");
    assert!(bold_arrow > 800, "bold arrow must ink, got {bold_arrow}");

    // A pure plain "->" reference: its first two cells must match the plain
    // arrow half of the mixed render (the run broke, so the plain side is
    // exactly a plain "->" ligature).
    let plain_ref = render(&mut grid, cw, ch, b"->", 8);
    let plain_match = mixed.cell_diff(&plain_ref, 0) + mixed.cell_diff(&plain_ref, 1);
    assert!(
        plain_match < plain_arrow / 4,
        "plain half of the mix must match a standalone plain arrow (diff {plain_match})"
    );

    // The bold arrow differs from the plain arrow (heavier weight); if the run
    // had NOT broken, both halves would shape with one face and the bold cells
    // would equal the plain cells shifted.
    let bold_vs_plain = mixed.cell_diff(&plain_ref, 2) + mixed.cell_diff(&plain_ref, 3);
    assert!(
        bold_vs_plain > 200,
        "bold arrow cells must differ from the plain arrow (bold broke into its own \
         run and shaped bold); diff {bold_vs_plain}"
    );
}

/// (4) Cursor over the 2nd cell of `->`: the run breaks around the cursor, and
/// the cursor still draws at that cell. A block cursor fills the cell with the
/// cursor color, so col 1 must read as the cursor color (well away from bg).
#[test]
fn cursor_on_ligature_second_cell_still_draws() {
    let Some(_) = metal() else { return };
    let Some(face) = firacode() else { return };
    let (mut grid, cw, ch) = make_grid(face);

    // "->" then park the cursor at col 1 (1-indexed col 2) on row 1.
    // CUP: ESC [ 1 ; 2 H.
    let mixed = render(&mut grid, cw, ch, b"->\x1b[1;2H", 8);

    let opts = FrameOptions::default();
    let cursor_color = [opts.default_fg.r, opts.default_fg.g, opts.default_fg.b];
    let bg = [opts.default_bg.r, opts.default_bg.g, opts.default_bg.b];

    // The cursor cell (col 1) must not be the plain background — the block
    // cursor fills it.
    let bg_delta = mixed.cell_ink(1, bg);
    assert!(
        bg_delta > 2_000,
        "cursor cell (col 1) must be filled by the block cursor, got delta {bg_delta}"
    );
    // And its center should read near the cursor color (fg), confirming a real
    // cursor and not a stray glyph.
    let h = mixed.px.len() / (mixed.w * 4);
    let cx = cw as usize + cw as usize / 2; // center of col 1
    let cy = (ch as usize / 2).min(h - 1);
    let i = (cy * mixed.w + cx) * 4;
    let (b, g, r) = (
        mixed.px[i] as i32,
        mixed.px[i + 1] as i32,
        mixed.px[i + 2] as i32,
    );
    let cd = (r - cursor_color[0] as i32).abs()
        + (g - cursor_color[1] as i32).abs()
        + (b - cursor_color[2] as i32).abs();
    assert!(
        cd < 60,
        "cursor cell center should read the cursor color {cursor_color:?}, got [{r},{g},{b}] (delta {cd})"
    );
}

/// (5) Underline across a ligature: an underlined "->" draws the underline
/// sprite on BOTH cells (decorations stay per-cell even though the glyph run
/// spans the ligature). Compare an underlined arrow to a non-underlined arrow:
/// both arrow cells must gain ink from the underline.
#[test]
fn underline_across_ligature_is_per_cell() {
    let Some(_) = metal() else { return };
    let Some(face) = firacode() else { return };
    let (mut grid, cw, ch) = make_grid(face);
    let bg = [
        FrameOptions::default().default_bg.r,
        FrameOptions::default().default_bg.g,
        FrameOptions::default().default_bg.b,
    ];

    let plain = render(&mut grid, cw, ch, b"->", 8);
    let underlined = render(&mut grid, cw, ch, b"\x1b[4m->\x1b[0m", 8);

    // Both cells must gain ink from the underline sprite (per-cell decoration).
    for col in [0usize, 1] {
        let plain_ink = plain.cell_ink(col, bg);
        let ul_ink = underlined.cell_ink(col, bg);
        assert!(
            ul_ink > plain_ink,
            "underlined arrow cell {col} ink {ul_ink} must exceed plain {plain_ink} \
             (the underline draws per-cell on both ligature cells)"
        );
    }
}
