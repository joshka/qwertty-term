//! Regression: default-foreground (SGR-free) text must produce ink — for the
//! embedded font AND for a name-loaded system family.
//!
//! Field bug (fg-invisible): with a config `font-family` naming a real system
//! font (e.g. "FiraCode Nerd Font Mono") on a themed background, whole classes
//! of plain default-fg text rendered as NO INK AT ALL — the word "via" in a
//! prompt, eza's column headers (their underline sprite drew, the header glyph
//! did not), plain filenames and dates — while bold/italic/colored text drew
//! fine.
//!
//! Original root cause: a name-loaded [`Face`] carried no `source_bytes`, so the
//! shaper ([`ghostty_font::Shaper::new`]) returned `None`, and the renderer's
//! primary-face glyph path (`Engine::add_cell_glyph`) dropped the glyph
//! entirely instead of falling back to per-codepoint CoreText rendering.
//! Decorations survived (sprite path), and bold/italic survived (the default
//! style table fills those slots with *embedded*, byte-backed faces) — so only
//! default-fg regular text on the primary face went dark.
//!
//! Fix history: this first landed by adding an *unshaped* per-codepoint CoreText
//! fallback for byte-less faces (so the glyph drew even without a shaper). The
//! byte-backed-named-faces work then gave name-loaded faces their backing bytes
//! (read via the CoreText font URL attribute), so a named family like FiraCode
//! now takes the **shaped** path (`Shaper::new` succeeds) — enabling ligatures —
//! and still inks. The unshaped fallback remains for faces that genuinely lack
//! bytes (e.g. a purely synthesized system face with no file URL).
//!
//! These assertions lock the FiraCode (named-family) path so it can never go
//! silently dark again: a plain, SGR-free glyph on the Aardvark-Ink themed
//! background must ink — now through the shaped path — with the embedded font
//! and (skip-if-not-installed) a named family alike. The eza-header case
//! (underline + text) is asserted too: the cell's ink must exceed a
//! bare-underline-only baseline.
//!
//! Skips gracefully (`SKIP:`) when no Metal device is present.

#![cfg(target_os = "macos")]

use ghostty_font::coretext::Face;
use ghostty_font::grid::Grid;
use ghostty_font::{CodepointResolver, Collection, Metrics};
use ghostty_renderer::engine::{Engine, FrameOptions};
use ghostty_renderer::metal::Metal;
use ghostty_renderer::snapshot::FullSnapshot;
use ghostty_vt::color::Rgb;
use ghostty_vt::stream::{Stream, TerminalHandler};
use ghostty_vt::terminal::{Options, Terminal};

/// The maintainer's real theme (Aardvark Ink): a low-contrast dark background
/// with a soft off-white foreground. Using the real values keeps the assertion
/// honest about the exact field configuration.
const THEME_FG: Rgb = Rgb::new(0xb4, 0xbc, 0xca);
const THEME_BG: Rgb = Rgb::new(0x0f, 0x14, 0x1f);

/// Sum of per-pixel coverage (max abs channel delta vs `bg`) over one cell.
/// ~0 means the cell is indistinguishable from the background (INVISIBLE).
fn cell_ink(pixels: &[u8], width: usize, cell_w: usize, cell_h: usize, col: usize, bg: Rgb) -> i64 {
    let mut sum = 0i64;
    for dy in 0..cell_h {
        for dx in 0..cell_w {
            let x = col * cell_w + dx;
            let y = dy;
            let i = (y * width + x) * 4;
            if i + 3 >= pixels.len() {
                continue;
            }
            let (b, g, r) = (pixels[i] as i32, pixels[i + 1] as i32, pixels[i + 2] as i32);
            let d = (r - bg.r as i32)
                .abs()
                .max((g - bg.g as i32).abs())
                .max((b - bg.b as i32).abs());
            sum += d as i64;
        }
    }
    sum
}

/// Render `line` through a fresh grid built over `face`, on the themed
/// background, and return `(pixels, width, cell_w, cell_h)`.
fn render(face: Face, size_px: f64, line: &str) -> (Vec<u8>, usize, usize, usize) {
    let metrics = Metrics::calc(face.face_metrics());
    let (cw, ch) = (metrics.cell_width, metrics.cell_height);
    let collection =
        Collection::new_with_default_fallbacks(face, size_px).expect("default style table");
    let resolver = CodepointResolver::new(collection);
    let mut grid = Grid::new(resolver, metrics).expect("grid");

    let term = Terminal::new(Options {
        cols: 40,
        rows: 1,
        ..Default::default()
    });
    let mut stream = Stream::new(TerminalHandler::new(term));
    stream.feed(line.as_bytes());
    let term = stream.handler.terminal;

    let snapshot = FullSnapshot::capture(&term, 0);
    let mut engine = Engine::with_backend(Metal::new().unwrap(), cw, ch).expect("engine");
    let opts = FrameOptions {
        cursor_blink_visible: false,
        default_fg: THEME_FG,
        default_bg: THEME_BG,
        ..FrameOptions::default()
    };
    engine.update_frame(&snapshot, &mut grid, opts);
    engine.sync_atlas(&grid).expect("sync atlas");
    let pixels = engine.draw_frame().expect("draw frame");
    let (sw, _sh) = engine.screen_size();
    (pixels, sw, cw as usize, ch as usize)
}

/// Assert every plain default-fg glyph in "via" inks, and the underlined header
/// glyph inks *beyond* a bare underline. `label` names the font for messages.
fn assert_default_fg_visible(face: Face, label: &str) {
    let size_px = 32.0;
    // Cols: 0..2 "via" (plain default-fg), 3 space, 4 underlined 'A' (eza-header
    // case: decoration + glyph), 5 space, 6 plain default-fg 'A' (same glyph, no
    // underline — the baseline the underlined cell must exceed).
    let line = "via \x1b[4mA\x1b[0m A";
    let (px, w, cw, ch) = render(face, size_px, line);

    // 1. Plain default-fg text ("via") must ink — the core symptom.
    for (col, name) in [(0usize, 'v'), (1, 'i'), (2, 'a')] {
        let ink = cell_ink(&px, w, cw, ch, col, THEME_BG);
        assert!(
            ink > 1_000,
            "[{label}] default-fg '{name}' (col {col}) must ink on the themed bg, got {ink} \
             (~0 == invisible; this is the field bug)"
        );
    }

    // 2. The eza-header case: an underlined default-fg glyph must ink beyond a
    //    bare underline. Compare the underlined 'A' (col 4) to the plain 'A'
    //    (col 6): the underlined cell carries the same glyph PLUS the underline
    //    sprite, so it must have strictly more ink than the plain glyph alone —
    //    proving the glyph draws, not just the underline.
    let plain_a = cell_ink(&px, w, cw, ch, 6, THEME_BG);
    let underlined_a = cell_ink(&px, w, cw, ch, 4, THEME_BG);
    assert!(
        plain_a > 1_000,
        "[{label}] plain default-fg 'A' (col 6) must ink, got {plain_a}"
    );
    assert!(
        underlined_a > plain_a,
        "[{label}] underlined 'A' ink {underlined_a} must exceed plain 'A' ink {plain_a} \
         (the header glyph must draw on top of its underline, not the underline alone)"
    );
}

#[test]
fn embedded_default_fg_inks_on_theme() {
    if Metal::new().is_err() {
        eprintln!("SKIP: no Metal device");
        return;
    }
    let face = Face::load_embedded(32.0).expect("embedded JetBrains Mono");
    assert_default_fg_visible(face, "embedded");
}

#[test]
fn named_family_default_fg_inks_on_theme() {
    if Metal::new().is_err() {
        eprintln!("SKIP: no Metal device");
        return;
    }
    // A name-loaded system family — the path that went dark in the field. Skip
    // (don't fail) when it isn't installed: `load_by_name` falls back to the
    // embedded face on a miss, which would silently retest the embedded path, so
    // verify the family actually resolved before asserting.
    let family = "FiraCode Nerd Font Mono";
    let face = match Face::load_by_name(family, 32.0) {
        Ok(f) if f.family_name().to_lowercase().contains("fira") => f,
        _ => {
            eprintln!("SKIP: {family} not installed; skipping named-family default-fg test");
            return;
        }
    };
    assert_default_fg_visible(face, family);
}
