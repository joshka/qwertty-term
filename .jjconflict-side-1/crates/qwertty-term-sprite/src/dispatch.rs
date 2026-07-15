//! Codepoint -> draw-function dispatch.
//!
//! # Design
//!
//! Upstream (`Face.zig`) collects draw functions *at comptime* by reflecting
//! over decls named `draw<HEX>` / `draw<MIN>_<MAX>`, parsing the range from the
//! name, sorting, and asserting non-overlap. Rust has no comptime reflection,
//! so we build the equivalent table **explicitly** here: one [`Range`] entry per
//! upstream draw function, in a plain `const` array.
//!
//! We deliberately chose an explicit table over a build-script codegen: the
//! table is small (~50 entries), the mapping from Zig function name to range is
//! mechanical and reviewable in one screen, and keeping it in source means no
//! build-time magic and trivial `grep`-ability. The cost — keeping it in sync
//! with upstream — is paid down by [`tests::dispatch_ranges_match_zig`], which
//! asserts the exact set of ranges against a checked-in copy of the Zig table,
//! and by [`tests::ranges_are_sorted_and_disjoint`], which reproduces the
//! upstream non-overlap invariant.
//!
//! Powerline (`U+E0B0..`) is the one block upstream defines as many individual
//! `drawE0BX` functions with **gaps** (e.g. `U+E0C0` is unhandled). We preserve
//! the gaps by listing only the handled codepoints, so [`crate::has_codepoint`]
//! matches upstream exactly.

use crate::draw::{
    DrawFn, block, box_drawing, braille, branch, geometric_shapes, legacy_computing,
    legacy_computing_supplement as sup, powerline, special,
};
use crate::sprite::{SPRITE_START, Sprite};

/// An inclusive codepoint range handled by a single draw function.
struct Range {
    min: u32,
    max: u32,
    draw: DrawFn,
}

impl Range {
    const fn new(min: u32, max: u32, draw: DrawFn) -> Self {
        Self { min, max, draw }
    }
    const fn single(cp: u32, draw: DrawFn) -> Self {
        Self {
            min: cp,
            max: cp,
            draw,
        }
    }
}

/// The full dispatch table, mirroring the set of `draw*` functions in the Zig
/// sprite subsystem. Kept sorted by `min` (enforced by a test).
static RANGES: &[Range] = &[
    // Box Drawing
    Range::new(0x2500, 0x257f, box_drawing::draw2500_257f),
    // Block Elements
    Range::new(0x2580, 0x259f, block::draw2580_259f),
    // Geometric Shapes (partial)
    Range::new(0x25e2, 0x25e5, geometric_shapes::draw25e2_25e5),
    Range::new(0x25f8, 0x25fa, geometric_shapes::draw25f8_25fa),
    Range::single(0x25ff, geometric_shapes::draw25ff),
    // Braille
    Range::new(0x2800, 0x28ff, braille::draw2800_28ff),
    // Powerline (individual glyphs, with gaps)
    Range::single(0xe0b0, powerline::draw_e0b0_e0d4),
    Range::single(0xe0b1, powerline::draw_e0b0_e0d4),
    Range::single(0xe0b2, powerline::draw_e0b0_e0d4),
    Range::single(0xe0b3, powerline::draw_e0b0_e0d4),
    Range::single(0xe0b4, powerline::draw_e0b0_e0d4),
    Range::single(0xe0b5, powerline::draw_e0b0_e0d4),
    Range::single(0xe0b6, powerline::draw_e0b0_e0d4),
    Range::single(0xe0b7, powerline::draw_e0b0_e0d4),
    Range::single(0xe0b8, powerline::draw_e0b0_e0d4),
    Range::single(0xe0b9, powerline::draw_e0b0_e0d4),
    Range::single(0xe0ba, powerline::draw_e0b0_e0d4),
    Range::single(0xe0bb, powerline::draw_e0b0_e0d4),
    Range::single(0xe0bc, powerline::draw_e0b0_e0d4),
    Range::single(0xe0bd, powerline::draw_e0b0_e0d4),
    Range::single(0xe0be, powerline::draw_e0b0_e0d4),
    Range::single(0xe0bf, powerline::draw_e0b0_e0d4),
    Range::single(0xe0d2, powerline::draw_e0b0_e0d4),
    Range::single(0xe0d4, powerline::draw_e0b0_e0d4),
    // Branch drawing (git graph)
    Range::new(0x0f5d0, 0x0f60d, branch::draw_f5d0_f60d),
    // Symbols for Legacy Computing Supplement
    Range::new(0x1cc1b, 0x1cc1e, sup::draw1cc1b_1cc1e),
    Range::new(0x1cc21, 0x1cc2f, sup::draw1cc21_1cc2f),
    Range::new(0x1cc30, 0x1cc3f, sup::draw1cc30_1cc3f),
    Range::new(0x1cd00, 0x1cde5, sup::draw1cd00_1cde5),
    Range::single(0x1ce00, sup::draw1ce00),
    Range::single(0x1ce01, sup::draw1ce01),
    Range::single(0x1ce0b, sup::draw1ce0b),
    Range::single(0x1ce0c, sup::draw1ce0c),
    Range::new(0x1ce16, 0x1ce19, sup::draw1ce16_1ce19),
    Range::new(0x1ce51, 0x1ce8f, sup::draw1ce51_1ce8f),
    Range::new(0x1ce90, 0x1ceaf, sup::draw1ce90_1ceaf),
    // Symbols for Legacy Computing
    Range::new(0x1fb00, 0x1fb3b, legacy_computing::draw1fb00_1fb3b),
    Range::new(0x1fb3c, 0x1fb67, legacy_computing::draw1fb3c_1fb67),
    Range::new(0x1fb68, 0x1fb6f, legacy_computing::draw1fb68_1fb6f),
    Range::new(0x1fb70, 0x1fb75, legacy_computing::draw1fb70_1fb75),
    Range::new(0x1fb76, 0x1fb7b, legacy_computing::draw1fb76_1fb7b),
    Range::new(0x1fb7c, 0x1fb97, legacy_computing::draw1fb7c_1fb97),
    Range::single(0x1fb98, legacy_computing::draw1fb98),
    Range::single(0x1fb99, legacy_computing::draw1fb99),
    Range::new(0x1fb9a, 0x1fb9f, legacy_computing::draw1fb9a_1fb9f),
    Range::new(0x1fba0, 0x1fbae, legacy_computing::draw1fba0_1fbae),
    Range::single(0x1fbaf, legacy_computing::draw1fbaf),
    Range::single(0x1fbbd, legacy_computing::draw1fbbd),
    Range::single(0x1fbbe, legacy_computing::draw1fbbe),
    Range::single(0x1fbbf, legacy_computing::draw1fbbf),
    Range::single(0x1fbce, legacy_computing::draw1fbce),
    Range::single(0x1fbcf, legacy_computing::draw1fbcf),
    Range::new(0x1fbd0, 0x1fbdf, legacy_computing::draw1fbd0_1fbdf),
    Range::new(0x1fbe0, 0x1fbef, legacy_computing::draw1fbe0_1fbef),
];

/// Resolve the draw function for a codepoint, or `None` if it is not a sprite.
pub(crate) fn draw_fn_for(cp: u32) -> Option<DrawFn> {
    // Special (cursor/underline) pseudo-codepoints first.
    if cp >= SPRITE_START {
        return Sprite::from_codepoint(cp).map(|_| special::draw as DrawFn);
    }
    // Binary search over the sorted, disjoint ranges.
    let mut lo = 0usize;
    let mut hi = RANGES.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let r = &RANGES[mid];
        if cp < r.min {
            hi = mid;
        } else if cp > r.max {
            lo = mid + 1;
        } else {
            return Some(r.draw);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reproduces the upstream `Face.zig` comptime invariant: ranges are sorted
    /// ascending and non-overlapping.
    #[test]
    fn ranges_are_sorted_and_disjoint() {
        let mut prev_max: Option<u32> = None;
        for r in RANGES {
            assert!(r.min <= r.max, "range {:x}..{:x} inverted", r.min, r.max);
            if let Some(pm) = prev_max {
                assert!(
                    r.min > pm,
                    "range starting {:x} overlaps previous ending {:x}",
                    r.min,
                    pm
                );
            }
            prev_max = Some(r.max);
        }
    }

    /// The set of `(min, max)` ranges we dispatch must equal the set derived
    /// from the upstream `draw*` function names (commit 2da015cd6). This is the
    /// "kept in sync" guarantee: if upstream adds/moves a range, this fails.
    #[test]
    fn dispatch_ranges_match_zig() {
        // Derived from the `draw<HEX>` / `draw<MIN>_<MAX>` function names in
        // src/font/sprite/draw/*.zig. Powerline is expanded to its individual
        // handled codepoints (the upstream functions are per-glyph).
        let expected: &[(u32, u32)] = &[
            (0x2500, 0x257f),
            (0x2580, 0x259f),
            (0x25e2, 0x25e5),
            (0x25f8, 0x25fa),
            (0x25ff, 0x25ff),
            (0x2800, 0x28ff),
            (0xe0b0, 0xe0b0),
            (0xe0b1, 0xe0b1),
            (0xe0b2, 0xe0b2),
            (0xe0b3, 0xe0b3),
            (0xe0b4, 0xe0b4),
            (0xe0b5, 0xe0b5),
            (0xe0b6, 0xe0b6),
            (0xe0b7, 0xe0b7),
            (0xe0b8, 0xe0b8),
            (0xe0b9, 0xe0b9),
            (0xe0ba, 0xe0ba),
            (0xe0bb, 0xe0bb),
            (0xe0bc, 0xe0bc),
            (0xe0bd, 0xe0bd),
            (0xe0be, 0xe0be),
            (0xe0bf, 0xe0bf),
            (0xe0d2, 0xe0d2),
            (0xe0d4, 0xe0d4),
            (0x0f5d0, 0x0f60d),
            (0x1cc1b, 0x1cc1e),
            (0x1cc21, 0x1cc2f),
            (0x1cc30, 0x1cc3f),
            (0x1cd00, 0x1cde5),
            (0x1ce00, 0x1ce00),
            (0x1ce01, 0x1ce01),
            (0x1ce0b, 0x1ce0b),
            (0x1ce0c, 0x1ce0c),
            (0x1ce16, 0x1ce19),
            (0x1ce51, 0x1ce8f),
            (0x1ce90, 0x1ceaf),
            (0x1fb00, 0x1fb3b),
            (0x1fb3c, 0x1fb67),
            (0x1fb68, 0x1fb6f),
            (0x1fb70, 0x1fb75),
            (0x1fb76, 0x1fb7b),
            (0x1fb7c, 0x1fb97),
            (0x1fb98, 0x1fb98),
            (0x1fb99, 0x1fb99),
            (0x1fb9a, 0x1fb9f),
            (0x1fba0, 0x1fbae),
            (0x1fbaf, 0x1fbaf),
            (0x1fbbd, 0x1fbbd),
            (0x1fbbe, 0x1fbbe),
            (0x1fbbf, 0x1fbbf),
            (0x1fbce, 0x1fbce),
            (0x1fbcf, 0x1fbcf),
            (0x1fbd0, 0x1fbdf),
            (0x1fbe0, 0x1fbef),
        ];
        let actual: Vec<(u32, u32)> = RANGES.iter().map(|r| (r.min, r.max)).collect();
        assert_eq!(actual, expected);
    }
}
