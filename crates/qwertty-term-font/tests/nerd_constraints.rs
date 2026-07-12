//! Nerd Fonts constraint sizing (Item 3).
//!
//! Upstream sizes/positions Nerd Font PUA icons via a codegen'd per-codepoint
//! constraint table (`src/font/nerd_font_attributes.zig` → `getConstraint`),
//! consumed in the render path (`renderer/generic.zig:3189` sets
//! `RenderOptions.constraint`, applied by `Glyph.zig`'s `constrain`). We had only
//! the emoji constraint, so PUA icons rendered at natural size
//! (oversized/misaligned powerline + devicon glyphs vs real Ghostty).
//!
//! This ports the constraint TABLE (generated `nerd_font_constraints.rs`, via
//! `cargo run -p xtask -- gen-nerd-constraints`) and the constraint APPLICATION
//! math (`crate::constraint`), wired into the grid's codepoint render path so a
//! PUA icon is scaled/aligned to fit its cell(s) per the table. The exact
//! constraint math is validated byte-for-byte against upstream's own oracle in
//! `constraint.rs` unit tests; here we assert the end-to-end grid behavior.
//!
//! Representative codepoints: E725/F015/EA61 are real font-routed PUA icons;
//! E0B0 (powerline right arrow) is a SPRITE and must stay one (the constraint
//! table lists it, but sprite dispatch runs first, so it never reaches the
//! font-glyph constrain path).
//!
//! FiraCode Nerd Font Mono (the maintainer's configured, nerd-patched family)
//! carries these PUA glyphs in its own cmap, so the constraint applies to a
//! nerd-patched *primary*. Skips if it isn't installed.
#![cfg(all(target_os = "macos", not(feature = "freetype")))]

use qwertty_term_font::coretext::Face;
use qwertty_term_font::grid::Grid;
use qwertty_term_font::metrics::Metrics;
use qwertty_term_font::{CodepointResolver, Collection, FontIndex, Style};

const SIZE_PX: f64 = 32.0;

/// A grid over FiraCode Nerd Font Mono as the primary, or `None` (skip).
fn firacode_grid() -> Option<Grid> {
    let face = match Face::load_by_name("FiraCode Nerd Font Mono", SIZE_PX) {
        Ok(f) if f.family_name().to_lowercase().contains("fira") => f,
        _ => {
            eprintln!("SKIP: FiraCode Nerd Font Mono not installed");
            return None;
        }
    };
    let metrics = Metrics::calc(face.face_metrics());
    let collection =
        Collection::new_with_default_fallbacks(face, SIZE_PX).expect("default collection");
    let resolver = CodepointResolver::without_discovery(collection);
    Some(Grid::new(resolver, metrics).expect("grid"))
}

/// Natural (unconstrained) ink bbox of `cp`'s glyph in the resolved face, for a
/// before/after comparison. Returns `(width, height, bearing_x, bearing_y)`.
fn natural_bbox(grid: &mut Grid, cp: u32) -> Option<(u32, u32, i32, i32)> {
    let idx = grid.get_index(cp)?;
    let FontIndex::Face { .. } = idx else {
        return None;
    };
    let face = grid.resolver().collection().get_face(idx)?;
    let ch = char::from_u32(cp)?;
    let gid = face.glyph_index(ch)?;
    let bmp = face.rasterize(gid).ok()?;
    Some((bmp.width, bmp.height, bmp.bearing_x, bmp.bearing_y))
}

/// Every constrained PUA icon fits within its cell(s): its rasterized height
/// stays within the cell height (allowing the standard 1px sub-pixel ceil slack
/// upstream also has) and its width within two cells. Before the constraint,
/// oversized icons rendered at natural size and spilled well past the cell.
/// Covers a devicon (E725), Font-Awesome home (F015), codicon lightbulb (EA61),
/// git-branch powerline (E0A0), and a stretch-mode flame (E0C4).
#[test]
fn pua_icons_fit_within_cell() {
    let Some(mut grid) = firacode_grid() else {
        return;
    };
    let cell_h = grid.metrics().cell_height;
    let cell_w = grid.metrics().cell_width;

    for cp in [0xE725u32, 0xF015, 0xEA61, 0xE0A0, 0xE0C4] {
        assert!(
            qwertty_term_font::nerd_font_constraints::get_constraint(cp).is_some(),
            "U+{cp:X} should have a Nerd Fonts constraint"
        );

        let idx = grid
            .get_index(cp)
            .unwrap_or_else(|| panic!("U+{cp:X} must resolve"));
        // These are all font-routed PUA icons, not sprites.
        assert!(
            matches!(idx, FontIndex::Face { .. }),
            "U+{cp:X} should resolve to a face glyph, got {idx:?}"
        );

        let g = grid
            .render_codepoint_styled(cp, Style::Regular)
            .expect("render ok")
            .unwrap_or_else(|| panic!("U+{cp:X} renders"));
        assert!(g.width > 0 && g.height > 0, "U+{cp:X} rendered empty");

        // +1 px sub-pixel ceil slack: the constraint scales the ink to <= the
        // cell/icon height, but the pixel canvas is ceil(size + frac), which can
        // be one pixel taller than the exact target (upstream has the same).
        assert!(
            g.height <= cell_h + 1,
            "U+{cp:X} constrained height {} exceeds cell height {cell_h} (+1 slack)",
            g.height
        );
        assert!(
            g.width <= 2 * cell_w + 1,
            "U+{cp:X} constrained width {} exceeds two cells ({})",
            g.width,
            2 * cell_w
        );
    }
}

/// The constraint measurably changes an oversized STRETCH icon's rendered bbox:
/// E0C4 (a `stretch`-mode flame) is stretched much wider than its natural ink
/// box (it fills the cell span). This is the before/after ink-bbox evidence
/// that the constraint is applied to a real font-routed PUA glyph at the grid
/// boundary. (`fit_cover1` icons at this size shrink only sub-pixel and round
/// back to the same canvas, so the visible-change evidence uses stretch icons;
/// the exact `fit_cover1` math is pinned in `constraint.rs`'s upstream oracle
/// tests.)
#[test]
fn oversized_icons_change_bbox() {
    let Some(mut grid) = firacode_grid() else {
        return;
    };

    for cp in [0xE0C4u32, 0xE0C5, 0xE0C0] {
        // Sprites are excluded; these stretch icons are font-routed.
        let idx = match grid.get_index(cp) {
            Some(i @ FontIndex::Face { .. }) => i,
            _ => continue,
        };
        let nat = natural_bbox(&mut grid, cp).expect("natural bbox");
        let _ = idx;
        let g = grid
            .render_codepoint_styled(cp, Style::Regular)
            .expect("render")
            .expect("renders");
        assert!(
            g.width > nat.0,
            "U+{cp:X} (stretch) should widen: natural w={}, constrained w={}",
            nat.0,
            g.width
        );
    }
}

/// E0B0 (POWERLINE RIGHT ARROW) is a SPRITE codepoint and must resolve to the
/// sprite renderer, NOT a font glyph — even though the constraint table lists it
/// (sprite dispatch runs first). This guards the "E0B0 stays a sprite" claim.
#[test]
fn powerline_e0b0_stays_sprite() {
    let Some(mut grid) = firacode_grid() else {
        return;
    };
    let idx = grid.get_index(0xE0B0).expect("U+E0B0 resolves");
    assert_eq!(
        idx,
        FontIndex::Sprite,
        "U+E0B0 must be a sprite (procedural), not a font-routed constrained glyph"
    );
}

/// Before/after ink comparison at the raster level: E0C4 (a stretch-mode flame)
/// rendered with vs without its constraint produces a different bbox. Same face,
/// same glyph id — only the constraint differs — so any bbox difference is the
/// constraint's doing. This is the direct "the constraint is applied" evidence
/// at the grid boundary.
#[test]
fn constraint_changes_render_vs_natural() {
    let Some(mut grid) = firacode_grid() else {
        return;
    };
    let cp = 0xE0C4u32;
    let idx = match grid.get_index(cp) {
        Some(i @ FontIndex::Face { .. }) => i,
        _ => {
            eprintln!("SKIP: U+{cp:X} not a face glyph in this FiraCode build");
            return;
        }
    };
    let face = grid.resolver().collection().get_face(idx).expect("face");
    let gid = face
        .glyph_index(char::from_u32(cp).unwrap())
        .expect("cmap glyph");

    // Unconstrained via the plain render_glyph (DefaultColorEmoji → no
    // constraint for a text glyph).
    let plain = grid.render_glyph(idx, gid).expect("plain render");
    // Constrained via the Nerd path.
    let constrained = grid.render_glyph_nerd(idx, gid, cp).expect("nerd render");

    assert!(
        plain.width != constrained.width
            || plain.height != constrained.height
            || plain.offset_x != constrained.offset_x
            || plain.offset_y != constrained.offset_y,
        "constrained render ({}x{} off=({},{})) equals plain ({}x{} off=({},{})) \
         — the Nerd constraint did not change the glyph",
        constrained.width,
        constrained.height,
        constrained.offset_x,
        constrained.offset_y,
        plain.width,
        plain.height,
        plain.offset_x,
        plain.offset_y,
    );
}
