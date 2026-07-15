//! Regression: a font-size zoom (Cmd-+/-) rebuilds the font grid, and the render
//! engine must adopt it via
//! [`Engine::on_font_rebuilt`](qwertty_term_renderer::engine::Engine::on_font_rebuilt).
//! Two pieces of engine state outlive a rebuild and would otherwise render the
//! zoom garbled — the "janky resize":
//!
//! 1. **Cached cell metrics.** The engine caches cell width/height at
//!    construction for the projection/target/placement. When the grid dimensions
//!    (cols×rows) don't change — a small step, or one whose pixel viewport still
//!    fits the same grid — `update_frame`'s own resize path is a no-op, so
//!    without `on_font_rebuilt` the target keeps the old pixel size and glyphs
//!    land on the old pitch. (`font_zoom_adopts_new_cell_metrics`.)
//!
//! 2. **Per-slot atlas-upload trackers.** The bug that actually reached the
//!    screen. A rebuild makes a *fresh* atlas whose `modified()` counter restarts
//!    at 0; each swap-chain slot records the last counter it uploaded (large
//!    after a real session), and `sync_atlas` skips the upload when
//!    `modified <= recorded`. So the fresh atlas fails the gate and every slot
//!    keeps sampling the STALE old-size atlas → garbled glyphs. The offscreen
//!    readback path hides it (it syncs and draws the same slot from a 0 counter),
//!    which is why every prior readback test — and the offscreen smoke — passed
//!    while the live window was broken. (`zoom_after_pumped_session_is_crisp`.)
//!
//! All pinned over the platform-free Software backend (no GPU, no window — runs
//! everywhere), since both bugs live in the backend-agnostic engine.

use qwertty_term_font::grid::Grid;
use qwertty_term_font::{CodepointResolver, Collection, Face, Metrics};
use qwertty_term_renderer::engine::{Engine, FrameOptions};
use qwertty_term_renderer::snapshot::FullSnapshot;
use qwertty_term_renderer::software::Software;
use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options, Terminal};

/// Build a fresh atlas grid over the embedded face at `size_px`, returning it
/// with its cell metrics — exactly what `qwertty-term::font::build` does on a
/// zoom, minus the AppKit surface.
fn grid_at(size_px: f64) -> (Grid, u32, u32) {
    let face = Face::load_embedded(size_px).expect("embedded face");
    let metrics = Metrics::calc(face.face_metrics());
    let resolver = CodepointResolver::new(Collection::new(face));
    let grid = Grid::new(resolver, metrics).expect("grid");
    let (cw, ch) = {
        let m = grid.metrics();
        (m.cell_width, m.cell_height)
    };
    (grid, cw, ch)
}

/// A font zoom that keeps the same grid dimensions still resizes the render
/// target to the new cell pitch — the metrics the engine uses for projection,
/// target sizing, and per-cell placement all follow the rebuilt grid.
#[test]
fn font_zoom_adopts_new_cell_metrics() {
    let (cols, rows) = (10u16, 3u16);
    let terminal = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(terminal));
    stream.feed(b"hi");

    // Render at the small size first.
    let (mut small, small_cw, small_ch) = grid_at(12.0);
    let mut engine = Engine::with_backend_for_grid(Software::new(), &small).expect("engine");
    let snap = FullSnapshot::capture_live(stream.terminal());
    engine
        .render(&snap, &mut small, FrameOptions::default())
        .expect("render small");
    assert_eq!(
        engine.screen_size(),
        (
            cols as usize * small_cw as usize,
            rows as usize * small_ch as usize
        ),
        "baseline target is grid × small cell metrics",
    );

    // Zoom in: rebuild the atlas grid at a larger size, then hand the engine the
    // new metrics (the fix). The terminal — and thus cols/rows — is unchanged,
    // so `update_frame`'s size-diff path does NOT fire; only `on_font_rebuilt`
    // updates the pitch.
    let (mut large, large_cw, large_ch) = grid_at(24.0);
    assert!(
        large_cw > small_cw && large_ch > small_ch,
        "a larger font must yield larger cells ({small_cw}x{small_ch} -> {large_cw}x{large_ch})",
    );
    engine.on_font_rebuilt(large_cw, large_ch);

    let snap = FullSnapshot::capture_live(stream.terminal());
    let frame = engine
        .render(&snap, &mut large, FrameOptions::default())
        .expect("render large");

    // Target now reflects the NEW cell pitch. Before the fix the engine kept its
    // construction-time metrics (cols/rows unchanged → no resize), leaving this
    // at the small pitch: garbled, wrongly-sized output.
    let want = (
        cols as usize * large_cw as usize,
        rows as usize * large_ch as usize,
    );
    assert_eq!(
        engine.screen_size(),
        want,
        "font zoom must resize the target to the new cell metrics",
    );
    assert_eq!(
        (frame.width(), frame.height()),
        want,
        "the read-back frame is sized at the new cell pitch",
    );
}

/// `on_font_rebuilt` is a no-op when the metrics are unchanged (a clamped step at
/// the min/max bound rebuilds the same grid): it must not disturb the target.
#[test]
fn unchanged_metrics_leave_the_target_alone() {
    let (cols, rows) = (8u16, 2u16);
    let terminal = Terminal::new(Options {
        cols,
        rows,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(terminal));
    stream.feed(b"ok");

    let (mut grid, cw, ch) = grid_at(16.0);
    let mut engine = Engine::with_backend_for_grid(Software::new(), &grid).expect("engine");
    let snap = FullSnapshot::capture_live(stream.terminal());
    engine
        .render(&snap, &mut grid, FrameOptions::default())
        .expect("render");
    let before = engine.screen_size();

    engine.on_font_rebuilt(cw, ch);
    assert_eq!(engine.screen_size(), before, "same metrics → no-op");
}

/// Render `content` on a fresh `cols`×`rows` terminal at `size_px` and return the
/// BGRA frame — the ground-truth crisp render.
fn render_fresh(cols: u16, rows: u16, size_px: f64, content: &[u8]) -> Vec<u8> {
    let (mut grid, _, _) = grid_at(size_px);
    let mut engine = Engine::with_backend_for_grid(Software::new(), &grid).expect("engine");
    let term = {
        let t = Terminal::new(Options {
            cols,
            rows,
            ..Default::default()
        });
        let mut s = Stream::new(TerminalHandler::new(t));
        s.feed(content);
        s.handler.terminal
    };
    engine
        .render(
            &FullSnapshot::capture(&term, 0),
            &mut grid,
            FrameOptions::default(),
        )
        .expect("render")
        .bgra()
        .to_vec()
}

/// The on-screen bug: after a session has pumped the atlas `modified()` counter
/// high, a zoom to a fresh atlas (counter restarts at 0) must still re-upload the
/// atlas into the swap-chain slots. Otherwise the slots keep sampling the stale
/// old-size atlas and the zoomed frame is garbled. Verified by comparing against
/// a fresh-engine render of the same content at the same size (pixel-for-pixel).
#[test]
fn zoom_after_pumped_session_is_crisp() {
    let (cols, rows) = (20u16, 5u16);
    let content = b"\x1b[2J\x1b[HAB\r\nCD";

    // Ground truth: a fresh 24px engine rendering the content.
    let reference = render_fresh(cols, rows, 24.0, content);

    // Victim: start at 12px and pump the atlas by rendering a screen of many
    // distinct glyphs across several frames — each distinct glyph is an atlas
    // insert that bumps modified(), and each frame syncs another swap-chain slot,
    // so every slot's recorded counter ends up high.
    let (mut small, _, _) = grid_at(12.0);
    let mut engine = Engine::with_backend_for_grid(Software::new(), &small).expect("engine");
    let junk: Vec<u8> = b"\x1b[2J\x1b[H"
        .iter()
        .copied()
        .chain(0x21u8..0x7f)
        .collect();
    let junk_term = {
        let t = Terminal::new(Options {
            cols,
            rows,
            ..Default::default()
        });
        let mut s = Stream::new(TerminalHandler::new(t));
        s.feed(&junk);
        s.handler.terminal
    };
    for _ in 0..4 {
        let snap = FullSnapshot::capture(&junk_term, 0);
        engine.update_frame(&snap, &mut small, FrameOptions::default());
        engine.sync_atlas(&small).expect("sync junk");
        let _ = engine.draw_frame().expect("draw junk");
    }

    // Zoom to 24px: fresh atlas (modified restarts at 0), same content as the
    // reference. `on_font_rebuilt` must force every slot to re-upload it.
    let (mut large, lcw, lch) = grid_at(24.0);
    engine.on_font_rebuilt(lcw, lch);
    let victim_term = {
        let t = Terminal::new(Options {
            cols,
            rows,
            ..Default::default()
        });
        let mut s = Stream::new(TerminalHandler::new(t));
        s.feed(content);
        s.handler.terminal
    };
    let victim = engine
        .render(
            &FullSnapshot::capture(&victim_term, 0),
            &mut large,
            FrameOptions::default(),
        )
        .expect("victim render")
        .bgra()
        .to_vec();

    assert_eq!(victim.len(), reference.len(), "same dimensions");
    let diff = victim
        .iter()
        .zip(&reference)
        .filter(|(a, b)| a.abs_diff(**b) > 8)
        .count();
    assert_eq!(
        diff, 0,
        "zoomed render must match the fresh render pixel-for-pixel ({diff} \
         differing bytes) — a stale atlas garbles the glyphs",
    );
}
