//! Integration test net for the sprite rasterizer.
//!
//! Upstream has no inline unit tests to port (its coverage is golden-PNG
//! fixtures held elsewhere), so this builds a fresh net from properties we can
//! assert without a reference image:
//!
//! * **smoke** — every range's representative codepoints produce a non-empty,
//!   in-bounds bitmap at several odd/even cell sizes;
//! * **seam** — a box-drawing line drawn in one cell lines up pixel-for-pixel
//!   with the same line in the adjacent cell (the [`Fraction`] property);
//! * **coverage** — every codepoint the crate claims to handle actually renders,
//!   and codepoints in the gaps do not;
//! * **determinism** — the same input yields byte-identical output.

use qwertty_term_sprite::{Metrics, Sprite, has_codepoint, render};

/// A spread of cell sizes: even/odd width crossed with even/odd height, plus a
/// few small sizes to stress the greedy sizing paths.
const SIZES: &[(u32, u32)] = &[
    (18, 30),
    (12, 20),
    (11, 19),
    (9, 15),
    (8, 16),
    (10, 21),
    (7, 13),
];

fn metrics(w: u32, h: u32) -> Metrics {
    Metrics::simple(w, h)
}

/// One representative codepoint per handled range (and a couple of members
/// within larger ranges), used by the smoke test.
const REPRESENTATIVES: &[u32] = &[
    0x2500, 0x2501, 0x2503, 0x250c, 0x256d, 0x2571, 0x2573, 0x257f, // box
    0x2580, 0x2588, 0x2591, 0x2592, 0x2596, 0x259f, // block
    0x25e2, 0x25e4, 0x25f8, 0x25ff, // geometric
    0x2800, 0x2801, 0x28ff, // braille
    0xe0b0, 0xe0b1, 0xe0b4, 0xe0b6, 0xe0bc, 0xe0d2, 0xe0d4, // powerline
    0x0f5d0, 0x0f5d6, 0x0f5ee, 0x0f60d, // branch
    0x1cc1b, 0x1cc21, 0x1cc30, 0x1cc35, // supplement
    0x1cd00, 0x1cde5, // octants
    0x1ce00, 0x1ce0b, 0x1ce16, 0x1ce51, 0x1ce90, 0x1ceaf, // supplement
    0x1fb00, 0x1fb3c, 0x1fb41, 0x1fb68, 0x1fb70, 0x1fb76, 0x1fb7c, // legacy
    0x1fb98, 0x1fb9a, 0x1fba0, 0x1fbaf, 0x1fbbd, 0x1fbce, 0x1fbd0, 0x1fbe0, 0x1fbe8, // legacy
];

fn sprite_codepoints() -> Vec<u32> {
    [
        Sprite::Underline,
        Sprite::UnderlineDouble,
        Sprite::UnderlineDotted,
        Sprite::UnderlineDashed,
        Sprite::UnderlineCurly,
        Sprite::Strikethrough,
        Sprite::Overline,
        Sprite::CursorRect,
        Sprite::CursorHollowRect,
        Sprite::CursorBar,
        Sprite::CursorUnderline,
    ]
    .iter()
    .map(|s| s.codepoint())
    .collect()
}

#[test]
fn smoke_representatives_render_in_bounds() {
    for &(w, h) in SIZES {
        let m = metrics(w, h);
        // Bitmaps may extend up to a quarter cell of padding on each side.
        let max_w = w + 2 * (w / 4);
        let max_h = h + 2 * (h / 4);
        for &cp in REPRESENTATIVES {
            let g = render(cp, &m).unwrap_or_else(|| panic!("cp {cp:#x} not a sprite"));
            assert_eq!(
                g.alpha.len(),
                (g.width * g.height) as usize,
                "cp {cp:#x} at {w}x{h}: alpha len mismatch"
            );
            assert!(
                g.width <= max_w && g.height <= max_h,
                "cp {cp:#x} at {w}x{h}: bitmap {}x{} exceeds bounds {max_w}x{max_h}",
                g.width,
                g.height
            );
        }
    }
}

#[test]
fn smoke_specials_render_in_bounds() {
    for &(w, h) in SIZES {
        let m = metrics(w, h);
        let max_w = w + 2 * (w / 4);
        // Cursors may use cursor_height; decorations extend into vertical
        // padding. Allow a generous vertical bound.
        let max_h = h + 2 * (h / 4);
        for cp in sprite_codepoints() {
            let g = render(cp, &m).unwrap_or_else(|| panic!("sprite {cp:#x} not handled"));
            assert_eq!(g.alpha.len(), (g.width * g.height) as usize);
            assert!(
                g.width <= max_w && g.height <= max_h,
                "sprite {cp:#x} at {w}x{h}: bitmap {}x{} exceeds {max_w}x{max_h}",
                g.width,
                g.height
            );
        }
    }
}

/// Non-empty glyphs should actually put ink down. A handful of representatives
/// that are guaranteed non-blank across all sizes.
#[test]
fn representatives_are_non_blank() {
    let non_blank = [0x2500u32, 0x2588, 0x2580, 0x2591, 0x2801, 0xe0b0, 0x1fb00];
    for &(w, h) in SIZES {
        let m = metrics(w, h);
        for &cp in &non_blank {
            let g = render(cp, &m).unwrap();
            assert!(g.width > 0 && g.height > 0, "cp {cp:#x} blank at {w}x{h}");
            assert!(
                g.alpha.iter().any(|&a| a != 0),
                "cp {cp:#x} all-transparent at {w}x{h}"
            );
        }
    }
}

/// The seam property: a horizontal line (U+2500) rendered into the full cell
/// buffer must occupy the same vertical band regardless of horizontal position,
/// and a vertical line (U+2502) the same horizontal band — so tiling cells
/// produces continuous, unbroken lines. We verify this by reconstructing the
/// full padded coverage row/column band and checking it is identical whether we
/// treat the cell as a left or right neighbour.
///
/// More directly: the `Fraction::min`/`max` asymmetry means the middle band's
/// pixel bounds depend only on cell size, never on which side we compute from.
/// We assert the horizontal line's filled rows are centered and contiguous, and
/// that two horizontally-adjacent cells share the exact same filled rows (a
/// continuous line across the seam).
#[test]
fn seam_horizontal_line_is_continuous() {
    for &(w, h) in SIZES {
        let m = metrics(w, h);
        let g = render(0x2500, &m).unwrap();
        // Collect the set of rows that have any ink, and assert every such row
        // is fully inked across the whole width (a solid horizontal bar). Two
        // adjacent cells drawing the same glyph therefore meet seamlessly.
        let mut inked_rows = Vec::new();
        for y in 0..g.height {
            let row = &g.alpha[(y * g.width) as usize..((y + 1) * g.width) as usize];
            if row.iter().any(|&a| a != 0) {
                inked_rows.push(y);
                assert!(
                    row.iter().all(|&a| a != 0),
                    "cp 2500 at {w}x{h}: row {y} has a gap (not a continuous line)"
                );
            }
        }
        assert!(!inked_rows.is_empty(), "cp 2500 at {w}x{h}: no line drawn");
        // Rows must be contiguous (a single band, no split).
        for pair in inked_rows.windows(2) {
            assert_eq!(
                pair[1],
                pair[0] + 1,
                "cp 2500 at {w}x{h}: inked rows not contiguous"
            );
        }
    }
}

#[test]
fn seam_vertical_line_is_continuous() {
    for &(w, h) in SIZES {
        let m = metrics(w, h);
        let g = render(0x2502, &m).unwrap();
        let mut inked_cols = Vec::new();
        for x in 0..g.width {
            let mut any = false;
            let mut all = true;
            for y in 0..g.height {
                let a = g.alpha[(y * g.width + x) as usize];
                any |= a != 0;
                all &= a != 0;
            }
            if any {
                inked_cols.push(x);
                assert!(all, "cp 2502 at {w}x{h}: column {x} has a gap");
            }
        }
        assert!(!inked_cols.is_empty(), "cp 2502 at {w}x{h}: no line");
        for pair in inked_cols.windows(2) {
            assert_eq!(
                pair[1],
                pair[0] + 1,
                "cp 2502 at {w}x{h}: columns not contiguous"
            );
        }
    }
}

/// The core seam identity that makes adjacent cells line up: a fraction used as
/// a **min** (left/top) edge equals `size` minus the *complementary* fraction
/// used as a **max** (right/bottom) edge. This is exactly why a stroke ending at
/// a fraction on cell N's right meets the mirrored stroke on cell N+1's left with
/// no gap or overlap, at every size. It is the property the whole subsystem's
/// seam-freedom rests on, so we pin it for all fractions and a wide size sweep.
#[test]
fn fraction_min_is_complement_of_max() {
    use qwertty_term_sprite::Fraction as F;
    // (fraction, complement) pairs: complement(f) = 1 - f.
    let pairs = [
        (F::Zero, F::One),
        (F::OneEighth, F::SevenEighths),
        (F::OneQuarter, F::ThreeQuarters),
        (F::OneThird, F::TwoThirds),
        (F::ThreeEighths, F::FiveEighths),
        (F::Half, F::Half),
    ];
    for size in 1u32..=128 {
        for &(f, comp) in &pairs {
            assert_eq!(
                f.min(size),
                size as i32 - comp.max(size),
                "size {size}: min/max complement identity broke for {f:?}/{comp:?}"
            );
            assert_eq!(
                comp.min(size),
                size as i32 - f.max(size),
                "size {size}: min/max complement identity broke for {comp:?}/{f:?}"
            );
        }
    }
}

/// Every codepoint the crate claims via `has_codepoint` must render, and a
/// sampling of gap codepoints must not be claimed.
#[test]
fn dispatch_coverage_is_consistent() {
    let m = metrics(12, 24);

    // Everything claimed renders to Some.
    // Sweep all handled Unicode ranges plus the sprite pseudo-range.
    let mut claimed = 0u32;
    for cp in 0x2500u32..=0x1fbef {
        if has_codepoint(cp) {
            claimed += 1;
            assert!(render(cp, &m).is_some(), "claimed {cp:#x} failed to render");
        }
    }
    for cp in sprite_codepoints() {
        assert!(has_codepoint(cp));
        assert!(render(cp, &m).is_some());
    }
    // Sanity: we should be claiming a large number of codepoints.
    assert!(
        claimed > 1000,
        "only {claimed} codepoints claimed, expected >1000"
    );

    // Gaps that upstream does NOT handle must be rejected.
    for cp in [
        0x24ff,
        0x2600,
        0x25a0,
        0x25e1,
        0x25e6,
        0x2900,
        0xe0c0,
        0xe0d3,
        0x1fbf0,
        0x1cc00,
        0x1cd00 - 1,
    ] {
        assert!(!has_codepoint(cp), "{cp:#x} should not be claimed");
        assert!(render(cp, &m).is_none(), "{cp:#x} should not render");
    }
}

/// Same input, identical bytes — twice.
#[test]
fn rendering_is_deterministic() {
    let m = metrics(13, 27);
    for &cp in REPRESENTATIVES {
        let a = render(cp, &m).unwrap();
        let b = render(cp, &m).unwrap();
        assert_eq!(a, b, "cp {cp:#x} not deterministic");
    }
    for cp in sprite_codepoints() {
        let a = render(cp, &m).unwrap();
        let b = render(cp, &m).unwrap();
        assert_eq!(a, b, "sprite {cp:#x} not deterministic");
    }
}

/// Rendering every handled codepoint at a couple of sizes must never panic and
/// must always produce a consistent bitmap. This is the broad fuzz-lite net.
#[test]
fn render_all_handled_codepoints() {
    for &(w, h) in &[(9u32, 15u32), (12, 24)] {
        let m = metrics(w, h);
        let mut count = 0u32;
        for cp in 0x2500u32..=0x1fbef {
            if let Some(g) = render(cp, &m) {
                assert_eq!(g.alpha.len(), (g.width * g.height) as usize, "{cp:#x}");
                count += 1;
            }
        }
        assert!(count > 1000, "rendered only {count} at {w}x{h}");
    }
}
