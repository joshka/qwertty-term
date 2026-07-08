//! Text-run shaping on rustybuzz.
//!
//! Reduced port of Ghostty's `src/font/shaper/harfbuzz.zig` +
//! `src/font/shaper/run.zig` (commit `2da015cd6`), per decision 1
//! (rustybuzz-first). See `docs/analysis/font-shaping.md` for the cluster→cell
//! mapping semantics this reproduces and what the single-font reduced cut
//! keeps (style-boundary segmentation) vs defers (font-fallback / bad-ligature
//! / emoji-presentation splits, complex-script cluster reordering).
//!
//! # Cluster → cell mapping
//!
//! Upstream pushes each codepoint into the shaping buffer with its index as
//! the buffer cluster (cluster level `characters`) and keeps a side table of
//! the original terminal cell-X per codepoint. After shaping it walks the
//! output glyphs, resetting a running "cell offset" each time the cluster
//! advances and computing per-glyph pixel offsets from accumulated advances.
//!
//! The reduced cut supplies the **cell-X directly as the buffer cluster** (it
//! has no multi-codepoint graphemes in scope, so cluster == cell X is exact,
//! and the side-table indirection is unnecessary). rustybuzz keeps the minimum
//! cluster per output glyph under `Characters` level, exactly as HarfBuzz does,
//! so `info.cluster` is the glyph's cell X. Positions come back in font design
//! units, scaled here to pixels by `px_per_em / units_per_em` and rounded
//! round-half-up (the analog of upstream's `(v + ½) >> 6` on 26.6 fixed
//! point).

use rustybuzz::{BufferClusterLevel, UnicodeBuffer};

use crate::coretext::Face;

/// One shaped cell: the placement of a single output glyph on the grid.
///
/// Mirrors upstream `font.shape.Cell` (`shape.zig:41`). `cell_x` is the cell
/// column within the run; `glyph_index` is the shaper's **output glyph id**
/// (already the final glyph, not a codepoint), suitable for rasterization via
/// the face; `x_offset`/`y_offset` are pixel nudges applied at render time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapedCell {
    /// Cell column of this glyph within the run.
    pub cell_x: u16,
    /// Horizontal pixel offset to apply when rendering.
    pub x_offset: i16,
    /// Vertical pixel offset to apply when rendering.
    pub y_offset: i16,
    /// Output glyph id for this cell.
    pub glyph_index: u32,
    /// Advance width of this glyph in whole pixels. Not present upstream on the
    /// `Cell` (the terminal tracks cell widths itself), but the reduced cut
    /// exposes it so the caller can verify wide-glyph geometry (advance ≈ 2
    /// cells) without a terminal.
    pub x_advance: u16,
}

/// A rustybuzz-backed shaper for one face's runs.
///
/// Holds a `rustybuzz::Face` built over the primary face's bytes (decision 1:
/// "rustybuzz::Face over the same font bytes"). Shaping a run reuses the
/// buffer via `take`/return, matching rustybuzz's ownership model.
pub struct Shaper<'a> {
    face: rustybuzz::Face<'a>,
    /// Pixels per em for scaling design-unit positions to pixels.
    px_per_em: f64,
    /// Reusable buffer (rustybuzz consumes and returns it on each shape call).
    buf: Option<UnicodeBuffer>,
}

impl<'a> Shaper<'a> {
    /// Build a shaper over `face`, which must have source bytes available
    /// (`Face::source_bytes`). Returns `None` only for faces whose backing bytes
    /// couldn't be obtained (e.g. a synthesized system face with no file URL);
    /// the caller then renders unshaped per-codepoint. The shaper borrows the
    /// face's bytes for `'a`, so `face` must outlive it (it always does — the
    /// shaper is built, used, and dropped within a single cell-shaping call).
    ///
    /// For a `.ttc` collection the face's recorded [`Face::face_index`] selects
    /// the correct subface. If `face` carries a `wght` variation instance (a
    /// bold face materialized from a variable font, see [`Face::wght`]), the
    /// same variation is applied to the rustybuzz face so shaped glyph ids match
    /// the instance CoreText rasterizes. Without this, the shaper would shape
    /// against the default (`wght=400`) instance while the raster is bold — the
    /// ids happen to align for JetBrains Mono's non-ligature glyphs, but
    /// applying the variation keeps positioning (advances) correct for the bold
    /// instance too.
    pub fn new(face: &'a Face) -> Option<Shaper<'a>> {
        let bytes = face.source_bytes()?;
        let mut shaper = Shaper::from_bytes(bytes, face.face_index(), face.size_px())?;
        if let Some(wght) = face.wght() {
            shaper.face.set_variations(&[rustybuzz::Variation {
                tag: ttf_parser::Tag::from_bytes(b"wght"),
                value: wght,
            }]);
        }
        Some(shaper)
    }

    /// Build a shaper directly from font bytes, a face index (for `.ttc`
    /// collections), and a pixels-per-em size.
    ///
    /// The bytes must outlive the shaper (borrowed for `'a`). This is the
    /// primitive [`Shaper::new`] builds on; it is public so callers can shape
    /// from a byte buffer they own (e.g. a fallback font not yet wired into the
    /// reduced `Collection`).
    pub fn from_bytes(bytes: &'a [u8], face_index: u32, px_per_em: f64) -> Option<Shaper<'a>> {
        let rb = rustybuzz::Face::from_slice(bytes, face_index)?;
        Some(Shaper {
            face: rb,
            px_per_em,
            buf: Some(UnicodeBuffer::new()),
        })
    }

    /// Pixels per font unit, for scaling shaped positions.
    fn px_per_unit(&self) -> f64 {
        self.px_per_em / f64::from(self.face.units_per_em())
    }

    /// Shape `text` as a single run, one char per cell starting at cell 0.
    ///
    /// This is the common monospace case: each `char` maps to one cell column
    /// (its index in `text`), so ASCII maps 1:1. For a wide char the single
    /// output glyph's advance will be ~2 cells; the caller is responsible for
    /// treating the following cell as covered (the terminal's spacer_tail).
    pub fn shape_run(&mut self, text: &str) -> Vec<ShapedCell> {
        self.shape_run_with_clusters(text.chars().enumerate().map(|(i, c)| (c, i as u32)))
    }

    /// Shape an explicit `(char, cell_x)` sequence as one run.
    ///
    /// Exposes the cluster == cell-X mapping directly for callers that lay out
    /// cells themselves (e.g. after wide-char cell assignment). Mirrors the
    /// upstream contract that a run never crosses a row and each codepoint
    /// carries its cell X as its cluster.
    pub fn shape_run_with_clusters(
        &mut self,
        chars: impl IntoIterator<Item = (char, u32)>,
    ) -> Vec<ShapedCell> {
        let mut buf = self.buf.take().unwrap_or_default();
        buf.clear();
        // Cluster level `characters` == upstream's `hb_buffer_set_cluster_level`
        // to `CHARACTERS` (`harfbuzz.zig:270`): report the minimum cluster per
        // output glyph so a ligature keeps its first codepoint's cell X.
        buf.set_cluster_level(BufferClusterLevel::Characters);
        for (ch, cluster) in chars {
            buf.add(ch, cluster);
        }
        // Let rustybuzz infer direction/script/language (LTR for our scope).
        buf.guess_segment_properties();

        let glyphs = rustybuzz::shape(&self.face, &[], buf);

        let px = self.px_per_unit();
        let infos = glyphs.glyph_infos();
        let positions = glyph_positions_of(&glyphs);

        // Accumulated pen position and the current cell's starting pen X, in
        // whole pixels. Mirrors `run_offset` / `cell_offset` in
        // `harfbuzz.zig`, reduced to the common-case reset (reset the cell
        // origin whenever the cluster advances — the full ligature/mark
        // heuristic guard is a deferred completeness pass).
        let mut run_x: f64 = 0.0;
        let mut cell_origin_x: f64 = 0.0;
        let mut cur_cluster: Option<u32> = None;

        let mut cells = Vec::with_capacity(infos.len());
        for (info, pos) in infos.iter().zip(positions.iter()) {
            // `info.cluster` is the cell X we supplied (cluster == cell X).
            let cluster = info.cluster;
            if cur_cluster != Some(cluster) {
                cell_origin_x = run_x;
                cur_cluster = Some(cluster);
            }

            let x_offset = run_x - cell_origin_x + f64::from(pos.x_offset) * px;
            let y_offset = f64::from(pos.y_offset) * px;
            let x_advance = f64::from(pos.x_advance) * px;

            cells.push(ShapedCell {
                cell_x: cluster as u16,
                x_offset: round_half_up(x_offset) as i16,
                y_offset: round_half_up(y_offset) as i16,
                glyph_index: info.glyph_id,
                x_advance: round_half_up(x_advance).max(0) as u16,
            });

            // Advances apply to the next glyph (upstream `run_offset.x +=`).
            run_x += x_advance;
        }

        // Return the buffer for reuse.
        self.buf = Some(glyphs.clear());
        cells
    }
}

/// Round half away from zero to the nearest integer (matches upstream's
/// round-half-up on 26.6 positions).
fn round_half_up(v: f64) -> i32 {
    v.round() as i32
}

/// Borrow glyph positions from a shaped buffer.
fn glyph_positions_of(glyphs: &rustybuzz::GlyphBuffer) -> &[rustybuzz::GlyphPosition] {
    glyphs.glyph_positions()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// An embedded JetBrains Mono face; keep it alive in the test scope, then
    /// build the shaper over it (the shaper borrows the face's bytes).
    fn face() -> Face {
        Face::load_embedded(16.0).expect("load embedded")
    }

    /// Latin 1:1 case (analog of upstream's Latin cluster-mapping test): N
    /// ASCII chars produce N glyphs, one per cell, with zero x_offset and
    /// strictly increasing cell X.
    #[test]
    fn ascii_maps_one_to_one() {
        let f = face();
        let mut s = Shaper::new(&f).expect("embedded face shaper");
        let cells = s.shape_run("hello");
        assert_eq!(cells.len(), 5, "expected 5 glyphs for 'hello'");
        for (i, c) in cells.iter().enumerate() {
            assert_eq!(c.cell_x as usize, i, "cell {i} misplaced");
            assert_eq!(c.x_offset, 0, "ASCII should have no x_offset");
            assert!(c.glyph_index > 0, "cell {i} got notdef");
            assert!(c.x_advance > 0, "cell {i} zero advance");
        }
    }

    /// Wide-cell mapping semantics: a caller that lays out a wide glyph at cell
    /// 0 (leaving cell 1 as the spacer, i.e. no char pushed for it) gets a
    /// single glyph at cell X 0. This is the cluster→cell property the reduced
    /// shaper guarantees; "occupies 2 cells" is the caller's grid decision
    /// (upstream marks cell 1 `spacer_tail`), not a font advance ratio.
    ///
    /// The embedded JetBrains Mono has no CJK glyph, so we exercise the mapping
    /// with an ASCII stand-in whose cell X assignment models a wide char: one
    /// char, assigned cell 0, with cell 1 intentionally skipped by the caller.
    /// The real CJK-glyph-into-atlas path is covered end-to-end in the
    /// `first_pixels_font_substrate` integration test using a CJK system font.
    #[test]
    fn wide_char_maps_to_single_cell() {
        let f = face();
        let mut s = Shaper::new(&f).expect("embedded face shaper");
        // One char occupying cell 0; the caller would mark cell 1 as a spacer.
        let cells = s.shape_run_with_clusters([('M', 0)]);
        assert_eq!(cells.len(), 1, "one glyph");
        assert_eq!(cells[0].cell_x, 0, "wide glyph sits at its leading cell");
        assert!(cells[0].glyph_index > 0);
    }

    /// An em dash shapes to a single glyph in a single cell.
    #[test]
    fn em_dash_single_glyph() {
        let f = face();
        let mut s = Shaper::new(&f).expect("embedded face shaper");
        let cells = s.shape_run("—"); // U+2014
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].cell_x, 0);
        assert!(cells[0].glyph_index > 0);
    }
}
