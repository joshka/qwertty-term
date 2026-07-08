//! Offscreen evidence (Item 2): byte-backed named faces produce real ligature
//! PIXELS that differ from unshaped per-character rendering.
//!
//! The byte-backed-named-faces fix lets `Shaper::new` build a rustybuzz face for
//! a name-loaded family (e.g. FiraCode Nerd Font Mono), which is the enabling
//! precondition for OpenType shaping (ligatures). This test proves the payoff at
//! the pixel level: shaping the run "->" with FiraCode substitutes the arrow's
//! ligature glyphs, so a composited cell strip of the SHAPED run renders
//! materially different pixels than the same cells rendered UNSHAPED (each
//! character's own cmap glyph). A control run of "abc" — which FiraCode does not
//! ligate — renders identically shaped vs unshaped, proving the diff above is
//! the ligature and not shaping noise.
//!
//! Rendering here is the font crate's real CoreText rasterization
//! (`Face::rasterize`) composited into a cell strip and read back — an offscreen
//! readback without a GPU. (The renderer's live engine shapes one cell at a time
//! for the monospace scope, so cross-cell ligatures are exercised here through
//! the run shaper directly; wiring multi-cell run shaping into the engine is a
//! separate, out-of-territory change.)
//!
//! Skips (`SKIP:`) if FiraCode Nerd Font Mono isn't installed.
#![cfg(target_os = "macos")]

use ghostty_font::Shaper;
use ghostty_font::coretext::{Face, PixelFormat};

const FAMILY: &str = "FiraCode Nerd Font Mono";
const SIZE_PX: f64 = 32.0;
const CELL_W: usize = 20;
const CELL_H: usize = 40;

fn firacode() -> Option<Face> {
    match Face::load_by_name(FAMILY, SIZE_PX) {
        Ok(f) if f.family_name().to_lowercase().contains("fira") => Some(f),
        _ => {
            eprintln!("SKIP: {FAMILY} not installed");
            None
        }
    }
}

/// A grayscale cell strip: one row of `cols` cells, `CELL_W` px each.
struct Strip {
    px: Vec<u8>,
    cols: usize,
}

impl Strip {
    fn new(cols: usize) -> Strip {
        Strip {
            px: vec![0u8; cols * CELL_W * CELL_H],
            cols,
        }
    }

    fn width(&self) -> usize {
        self.cols * CELL_W
    }

    /// Blit a rasterized glyph bitmap into the strip at cell `col`, baseline-ish
    /// placement (bearing applied), clamped to the strip bounds. Only the alpha
    /// (coverage) channel is composited — this is grayscale text.
    fn blit(&mut self, col: usize, bmp: &ghostty_font::coretext::Bitmap) {
        if bmp.format != PixelFormat::Alpha8 {
            return; // text glyphs are Alpha8; skip color (not expected here)
        }
        let bpp = bmp.bytes_per_pixel() as usize;
        // Place the ink box: left edge at cell origin + bearing_x; top edge a
        // fixed inset from the top so both strips use identical placement.
        let cell_x0 = col * CELL_W;
        let x0 = cell_x0 as i32 + bmp.bearing_x;
        let y0 = (CELL_H as i32 - CELL_H as i32 / 4) - bmp.bearing_y; // shared baseline
        for gy in 0..bmp.height as i32 {
            for gx in 0..bmp.width as i32 {
                let sx = x0 + gx;
                let sy = y0 + gy;
                if sx < 0 || sy < 0 || sx as usize >= self.width() || sy as usize >= CELL_H {
                    continue;
                }
                let src = ((gy * bmp.width as i32 + gx) as usize) * bpp;
                let cov = bmp.data[src];
                let dst = sy as usize * self.width() + sx as usize;
                self.px[dst] = self.px[dst].max(cov);
            }
        }
    }

    /// Total absolute per-pixel difference against another strip.
    fn diff(&self, other: &Strip) -> u64 {
        self.px
            .iter()
            .zip(other.px.iter())
            .map(|(a, b)| (*a as i64 - *b as i64).unsigned_abs())
            .sum()
    }

    /// Total ink (sum of coverage).
    fn ink(&self) -> u64 {
        self.px.iter().map(|&p| p as u64).sum()
    }
}

/// Rasterize `text` into a cell strip, either SHAPED as one run (ligatures form)
/// or UNSHAPED (each char shaped in isolation → its own cmap glyph, no
/// cross-char ligature). Both paths rasterize through the same `Face`, so the
/// only variable is whether the run ligates.
fn render_strip(face: &Face, text: &str, shaped_as_run: bool) -> Strip {
    let mut shaper = Shaper::new(face).expect("byte-backed shaper");
    let n = text.chars().count();
    let mut strip = Strip::new(n);

    if shaped_as_run {
        // Shape the whole string as one run: cluster == cell X, ligatures apply.
        for cell in shaper.shape_run(text) {
            if let Ok(bmp) = face.rasterize(cell.glyph_index) {
                strip.blit(cell.cell_x as usize, &bmp);
            }
        }
    } else {
        // Unshaped per-char: shape each character alone (no adjacent context →
        // no ligature), place at its column.
        for (col, ch) in text.chars().enumerate() {
            let one = shaper.shape_run(&ch.to_string());
            if let Some(cell) = one.first()
                && let Ok(bmp) = face.rasterize(cell.glyph_index)
            {
                strip.blit(col, &bmp);
            }
        }
    }
    strip
}

/// "a->b": the shaped run ligates the arrow, so its pixels differ materially
/// from the unshaped per-char rendering. This is the Item-2 payoff assertion.
#[test]
fn arrow_shaped_run_differs_from_unshaped_pixels() {
    let Some(face) = firacode() else { return };

    let shaped = render_strip(&face, "a->b", true);
    let unshaped = render_strip(&face, "a->b", false);

    let d = shaped.diff(&unshaped);
    let base_ink = unshaped.ink().max(1);
    // The arrow ligature redraws the two middle cells entirely (shaft + head vs
    // literal '-' '>'), so the difference must be a large fraction of the ink.
    assert!(
        d > base_ink / 10,
        "shaped 'a->b' pixels should differ substantially from unshaped \
         (diff {d}, unshaped ink {base_ink}); the arrow ligature did not form"
    );
    assert!(shaped.ink() > 0, "shaped strip must have ink");
}

/// Control: "abc" is not a ligature in FiraCode, so shaped-as-run and unshaped
/// per-char produce IDENTICAL pixels. This isolates the arrow diff above as the
/// ligature, not a shaping artifact.
#[test]
fn non_ligature_run_is_pixel_identical() {
    let Some(face) = firacode() else { return };

    let shaped = render_strip(&face, "abc", true);
    let unshaped = render_strip(&face, "abc", false);

    assert_eq!(
        shaped.diff(&unshaped),
        0,
        "'abc' (no ligature) must render identically shaped vs unshaped"
    );
    assert!(shaped.ink() > 0, "'abc' strip must have ink");
}
