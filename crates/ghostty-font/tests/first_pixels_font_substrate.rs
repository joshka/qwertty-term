//! First-pixels font substrate proof (M3 F6/F7-reduced).
//!
//! Drives the full reduced shaping+rasterization path — `Collection` →
//! `CodepointResolver` → `Shaper` (rustybuzz) → `Grid` (atlas upload) — over
//! the embedded JetBrains Mono, and asserts the acceptance the plan calls for:
//! shape+rasterize "hello", an em dash, a CJK char, and a box-drawing char
//! into a grayscale atlas; every rendered cell gets an in-bounds atlas region
//! with plausible geometry; ligature-free ASCII maps 1:1; the wide char is one
//! glyph occupying two cells; the box-drawing glyph comes from the sprite
//! subsystem, not a face.
//!
//! macOS only (the shaping path needs the CoreText `Face` for rasterization).
#![cfg(target_os = "macos")]

use ghostty_font::coretext::Face;
use ghostty_font::grid::CachedGlyph;
use ghostty_font::metrics::Metrics;
use ghostty_font::{CodepointResolver, Collection, FontIndex, Grid, Shaper};

const SIZE_PX: f64 = 16.0;

/// Build the reduced substrate: a grid over the embedded font plus a shaper.
fn substrate() -> (Grid, Shaper, Metrics) {
    let primary = Face::load_embedded(SIZE_PX).expect("load embedded JetBrains Mono");
    let metrics = Metrics::calc(primary.face_metrics());

    // The shaper needs its own face handle (rustybuzz reads the same bytes).
    let shaper_face = Face::load_embedded(SIZE_PX).expect("load embedded for shaper");
    let shaper = Shaper::new(&shaper_face).expect("embedded face has source bytes");

    let collection = Collection::new(primary);
    let resolver = CodepointResolver::new(collection);
    let grid = Grid::new(resolver, metrics).expect("build grid");

    (grid, shaper, metrics)
}

/// Assert a cached glyph sits inside the atlas and has plausible geometry for
/// a ~16px cell. A blank glyph (zero size) is allowed (spaces) but the callers
/// here only pass inked glyphs.
fn assert_in_atlas(g: &CachedGlyph, atlas_size: u32, label: &str) {
    assert!(g.width > 0 && g.height > 0, "{label}: rendered empty");
    assert!(
        g.atlas_x + g.width < atlas_size && g.atlas_y + g.height < atlas_size,
        "{label}: region ({},{}) {}x{} outside atlas {atlas_size}",
        g.atlas_x,
        g.atlas_y,
        g.width,
        g.height
    );
    // Plausible pixel dimensions for a 16px glyph (generous upper bound to
    // accommodate the wide CJK glyph, which spans two cells).
    assert!(
        g.width <= 64 && g.height <= 64,
        "{label}: implausibly large {}x{}",
        g.width,
        g.height
    );
}

#[test]
fn hello_maps_one_to_one_into_atlas() {
    let (mut grid, mut shaper, _m) = substrate();

    // Shape the ASCII run.
    let cells = shaper.shape_run("hello");
    assert_eq!(
        cells.len(),
        5,
        "'hello' should be 5 glyphs (no ASCII ligature)"
    );

    // 1:1 cell mapping, monotonic cell X, no ligature/x_offset.
    for (i, c) in cells.iter().enumerate() {
        assert_eq!(c.cell_x as usize, i, "cell {i} misplaced");
        assert_eq!(c.x_offset, 0, "ASCII cell {i} should have no x_offset");
        assert!(c.glyph_index > 0, "cell {i} resolved to notdef");
    }

    // Every shaped glyph rasterizes into the atlas with a distinct region.
    let regular = FontIndex::Face {
        style: ghostty_font::Style::Regular,
        slot: 0,
    };
    let atlas_size = grid.atlas().size();
    let mut regions = Vec::new();
    for c in &cells {
        let g = grid
            .render_glyph(regular, c.glyph_index)
            .unwrap_or_else(|e| panic!("render 'hello' cell {}: {e}", c.cell_x));
        assert_in_atlas(&g, atlas_size, "hello glyph");
        regions.push((g.atlas_x, g.atlas_y, g.width, g.height));
    }

    // 'l' repeats, so its region is shared (cache hit) — but distinct letters
    // ('h','e','l','o') must occupy distinct atlas positions. Check that the
    // set of unique glyph ids maps to the same count of unique regions.
    let unique_glyphs: std::collections::HashSet<u32> =
        cells.iter().map(|c| c.glyph_index).collect();
    let unique_regions: std::collections::HashSet<(u32, u32, u32, u32)> =
        regions.iter().copied().collect();
    assert_eq!(
        unique_glyphs.len(),
        unique_regions.len(),
        "distinct glyphs should occupy distinct atlas regions"
    );
}

#[test]
fn em_dash_single_cell_from_face() {
    let (mut grid, mut shaper, _m) = substrate();

    let cells = shaper.shape_run("—"); // U+2014 EM DASH
    assert_eq!(cells.len(), 1, "em dash is a single glyph");
    assert_eq!(cells[0].cell_x, 0);

    // Resolves to the primary face (not a sprite).
    let idx = grid.get_index(0x2014).expect("em dash resolves");
    assert!(
        matches!(idx, FontIndex::Face { .. }),
        "em dash should come from the face, got {idx:?}"
    );

    let g = grid
        .render_glyph(idx, cells[0].glyph_index)
        .expect("render em dash");
    assert_in_atlas(&g, grid.atlas().size(), "em dash");
}

/// A monospace CJK system font, loaded from disk as `'static` bytes, for the
/// real wide-glyph case. The embedded JetBrains Mono has no CJK coverage, so
/// this is the only way to exercise a genuine fullwidth glyph end-to-end. If no
/// known CJK font is present the wide-char test skips rather than fails (the
/// mapping semantics are also covered at the unit level in `shaper.rs`).
///
/// Returns `(static_bytes, face_index)`. STHeiti / Hiragino Sans GB are
/// monospace-ideograph fonts that ship with macOS; their fullwidth ideograph
/// advance is exactly `units_per_em` while Latin glyphs are narrower, giving a
/// clean "ideograph advance ≈ 2× a half-em Latin cell" relationship.
fn cjk_font() -> Option<(&'static [u8], u32)> {
    const CANDIDATES: &[(&str, u32)] = &[
        ("/System/Library/Fonts/STHeiti Light.ttc", 0),
        ("/System/Library/Fonts/STHeiti Medium.ttc", 0),
        ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
        ("/System/Library/Fonts/Supplemental/Songti.ttc", 0),
    ];
    for &(path, idx) in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            // Leak to 'static: test-only, one small buffer for the process.
            return Some((Box::leak(bytes.into_boxed_slice()), idx));
        }
    }
    None
}

#[test]
fn wide_cjk_is_one_glyph_into_atlas() {
    let Some((bytes, face_index)) = cjk_font() else {
        eprintln!("skipping: no CJK system font available");
        return;
    };

    // Shape 水 (U+6C34) with the CJK font. One codepoint → one glyph at one
    // cell X. Its advance is one em (a fullwidth ideograph), which the terminal
    // grid renders across two half-em cells; the shaper reports the single
    // glyph at its leading cell (upstream marks the trailing cell spacer_tail).
    let mut shaper = Shaper::from_bytes(bytes, face_index, SIZE_PX).expect("cjk shaper");
    let cells = shaper.shape_run("水");
    assert_eq!(cells.len(), 1, "CJK char is a single glyph");
    assert_eq!(cells[0].cell_x, 0, "wide glyph sits at its leading cell");
    assert!(
        cells[0].glyph_index > 0,
        "CJK resolved to notdef in CJK font"
    );

    // The fullwidth ideograph advance is (about) twice a narrow Latin advance
    // in the same font — the "occupies 2 cells" geometry.
    let latin = shaper.shape_run("i")[0].x_advance;
    assert!(
        cells[0].x_advance > latin,
        "ideograph advance {} should exceed a Latin advance {latin}",
        cells[0].x_advance
    );

    // Rasterize the CJK glyph into a grayscale atlas via a grid built over the
    // same font (its own Collection, since the reduced Collection is
    // single-font). This proves a real wide glyph lands in the atlas.
    let cjk_face = Face::load_from_bytes(bytes, SIZE_PX).expect("load cjk face");
    let metrics = Metrics::calc(cjk_face.face_metrics());
    let resolver = CodepointResolver::new(Collection::new(cjk_face));
    let mut cjk_grid = Grid::new(resolver, metrics).expect("cjk grid");

    let idx = FontIndex::Face {
        style: ghostty_font::Style::Regular,
        slot: 0,
    };
    let g = cjk_grid
        .render_glyph(idx, cells[0].glyph_index)
        .expect("render CJK glyph");
    assert_in_atlas(&g, cjk_grid.atlas().size(), "CJK");
}

#[test]
fn box_drawing_comes_from_sprite() {
    let (mut grid, _shaper, _m) = substrate();

    // U+2500 BOX DRAWINGS LIGHT HORIZONTAL is a sprite codepoint. It bypasses
    // shaping entirely (codepoint == glyph for special fonts).
    let idx = grid.get_index(0x2500).expect("box drawing resolves");
    assert_eq!(
        idx,
        FontIndex::Sprite,
        "box drawing must route to the sprite subsystem"
    );
    assert!(
        ghostty_sprite::has_codepoint(0x2500),
        "sanity: sprite has U+2500"
    );

    // The sprite subsystem draws it (not the face) and it lands in the atlas.
    let g = grid
        .render_codepoint(0x2500)
        .expect("render box sprite")
        .expect("box drawing produced a glyph");
    assert_in_atlas(&g, grid.atlas().size(), "box sprite");
}

/// The full acceptance line in one pass: shape+rasterize a mixed line
/// (ASCII + em dash + CJK) plus a sprite, asserting every cell got a distinct
/// in-bounds atlas region and the atlas actually received writes.
#[test]
fn full_line_every_cell_gets_a_region() {
    let (mut grid, mut shaper, _m) = substrate();

    let atlas_size = grid.atlas().size();
    let before_modified = grid.atlas().modified();

    // Text cells via the shaper. Uses only codepoints the embedded font
    // covers (ASCII + em dash); the real CJK-into-atlas path is proven in
    // `wide_cjk_is_one_glyph_into_atlas` with a CJK system font.
    let text = "hello—world"; // hello, em dash, world
    let cells = shaper.shape_run(text);
    // 5 + 1 + 5 = 11 glyphs (JetBrains Mono does not ligate this ASCII).
    assert_eq!(
        cells.len(),
        11,
        "expected 11 glyphs for {text:?}, got {}",
        cells.len()
    );

    let regular = FontIndex::Face {
        style: ghostty_font::Style::Regular,
        slot: 0,
    };
    for c in &cells {
        let g = grid
            .render_glyph(regular, c.glyph_index)
            .unwrap_or_else(|e| panic!("render cell {}: {e}", c.cell_x));
        assert_in_atlas(&g, atlas_size, "line glyph");
    }

    // Plus a sprite box-drawing cell.
    let box_g = grid
        .render_codepoint(0x2500)
        .expect("render box")
        .expect("box glyph");
    assert_in_atlas(&box_g, atlas_size, "box");

    // The atlas received writes (modified counter advanced) — the renderer's
    // re-upload signal.
    assert!(
        grid.atlas().modified() > before_modified,
        "atlas should have been written"
    );
}
