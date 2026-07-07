//! Golden-value tests for the color math ported into `ghostty.metal`.
//!
//! Plan decision 6 (`docs/plans/m3-first-pixels.md`): "The color math
//! (linearize/unlinearize, sRGB<->P3, WCAG contrast, luminance-based alpha
//! remap) must be numerically exact — golden-value unit tests on the helper
//! functions."
//!
//! MSL runs on the GPU, so it can't be unit-tested directly from `cargo
//! test` (that's what [`super::smoke`] covers — compiling it). Instead this
//! module **reimplements the same formulas in Rust** (test-only, not shared
//! code with the shader — a deliberate mirror, not a dependency) and pins
//! golden values from documented constants: the WCAG 2.0 contrast-ratio
//! definition, the sRGB piecewise transfer function's own published
//! breakpoints, and the fixed points of `linearize`/`unlinearize` (0.0, 1.0,
//! and the sRGB<->linear breakpoint itself). If a future edit to
//! `ghostty.metal`'s formulas drifts from these constants, either this
//! module must be updated with an explicit justification, or the drift is a
//! bug — that's the point of pinning upstream's documented values here
//! rather than whatever the current implementation happens to produce.

/// Mirror of `ghostty.metal`'s scalar `float linearize(float v)`: the sRGB
/// electro-optical transfer function (EOTF), converting gamma-encoded sRGB
/// to linear light. Breakpoint and coefficients per the sRGB spec (IEC
/// 61966-2-1) as upstream states in its own comment reference.
fn linearize(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Mirror of `ghostty.metal`'s scalar `float unlinearize(float v)`: the sRGB
/// opto-electronic transfer function (OETF), converting linear light back to
/// gamma-encoded sRGB. Exact inverse of [`linearize`] except at the
/// breakpoint, where the two piecewise branches are each other's inverse by
/// construction of the sRGB spec.
fn unlinearize(v: f64) -> f64 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        v.powf(1.0 / 2.4) * 1.055 - 0.055
    }
}

/// Mirror of `ghostty.metal`'s `float luminance(float3 color)`: relative
/// luminance per the WCAG 2.0 formula
/// (<https://www.w3.org/TR/2008/REC-WCAG20-20081211/#relativeluminancedef>),
/// which upstream also cites directly above `contrast_ratio`. Takes
/// **linear** RGB, matching the shader's documented precondition.
fn luminance(r: f64, g: f64, b: f64) -> f64 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Mirror of `ghostty.metal`'s `float contrast_ratio(float3, float3)`: WCAG
/// 2.0 contrast ratio
/// (<https://www.w3.org/TR/2008/REC-WCAG20-20081211/#contrast-ratiodef>),
/// `(L1 + 0.05) / (L2 + 0.05)` with `L1` the lighter of the two relative
/// luminances.
fn contrast_ratio(c1: (f64, f64, f64), c2: (f64, f64, f64)) -> f64 {
    let l1 = luminance(c1.0, c1.1, c1.2);
    let l2 = luminance(c2.0, c2.1, c2.2);
    (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
}

/// Mirror of the linear-blending weight-correction remap inside
/// `cell_text_fragment`'s `ATLAS_GRAYSCALE` branch (`use_linear_correction`):
/// given gamma-encoded foreground/background luminances and a linear
/// coverage alpha `a`, compute the "desired" gamma-space blend luminance and
/// re-derive the alpha that would produce it under linear interpolation from
/// `bg_l` to `fg_l`. This is the "luminance-based alpha remap" the R3 task
/// calls out for a documented explanation — see
/// `docs/analysis/renderer-r3.md` for the prose walkthrough; this function
/// pins its numeric behavior.
fn linear_correction_alpha(fg_l: f64, bg_l: f64, a: f64) -> f64 {
    if (fg_l - bg_l).abs() <= 0.001 {
        return a;
    }
    let blend_l = linearize(unlinearize(fg_l) * a + unlinearize(bg_l) * (1.0 - a));
    ((blend_l - bg_l) / (fg_l - bg_l)).clamp(0.0, 1.0)
}

/// Tolerance for float golden-value comparisons: tight enough to catch a
/// wrong formula (e.g. a swapped exponent or breakpoint), loose enough to
/// tolerate `f64` vs the shader's `f32` arithmetic and libm transcendental
/// differences across platforms.
const EPS: f64 = 1e-9;

fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < EPS,
        "{what}: expected {expected}, got {actual} (diff {})",
        (actual - expected).abs()
    );
}

#[test]
fn linearize_fixed_points() {
    // v=0 and v=1 are fixed points of the sRGB transfer function in both
    // directions by construction (0 -> 0, 1 -> 1 exactly).
    assert_close(linearize(0.0), 0.0, "linearize(0.0)");
    assert_close(linearize(1.0), 1.0, "linearize(1.0)");
}

#[test]
fn linearize_midpoint_golden_value() {
    // linearize(0.5) golden value: ((0.5 + 0.055) / 1.055)^2.4. Pinned to
    // 15 significant digits computed independently (Python `**`); this is
    // the textbook "sRGB 50% gray -> ~21.4% linear" fact used throughout
    // color-management literature.
    assert_close(linearize(0.5), 0.214_041_140_482_232_55, "linearize(0.5)");
}

#[test]
fn linearize_unlinearize_are_inverses_away_from_breakpoint() {
    // Round-tripping through both halves of the shader's split (each has
    // its own dedicated scalar overload for exactly this reason: contrast
    // and luminance math needs to move between spaces without drift).
    for v in [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
        assert_close(unlinearize(linearize(v)), v, "unlinearize(linearize(v))");
        assert_close(linearize(unlinearize(v)), v, "linearize(unlinearize(v))");
    }
}

#[test]
fn linearize_breakpoint_is_continuous() {
    // The sRGB spec's two documented breakpoint constants (0.04045 on the
    // gamma side, 0.0031308 on the linear side) are independently-rounded
    // decimal literals, not exact images of one another under the formula —
    // a well-known quirk of the spec (evaluating the "higher" branch at
    // 0.04045 gives ~0.0031308050, off from 0.0031308 by ~5e-9). Pin that
    // known small discontinuity explicitly with a looser tolerance, rather
    // than asserting a false exact match.
    const BREAKPOINT_EPS: f64 = 1e-7;
    assert!(
        (linearize(0.04045) - 0.0031308).abs() < BREAKPOINT_EPS,
        "linearize(0.04045) should be within {BREAKPOINT_EPS} of the spec's 0.0031308 constant, \
         got {}",
        linearize(0.04045)
    );
    assert!(
        (unlinearize(0.0031308) - 0.04045).abs() < BREAKPOINT_EPS,
        "unlinearize(0.0031308) should be within {BREAKPOINT_EPS} of the spec's 0.04045 constant, \
         got {}",
        unlinearize(0.0031308)
    );
}

#[test]
fn luminance_of_primaries_and_white() {
    // WCAG relative-luminance coefficients are exactly the Rec. 709 (sRGB)
    // luma coefficients; the three primaries' luminances are the
    // coefficients themselves and white sums to 1.0 by construction.
    assert_close(luminance(1.0, 0.0, 0.0), 0.2126, "luminance(red)");
    assert_close(luminance(0.0, 1.0, 0.0), 0.7152, "luminance(green)");
    assert_close(luminance(0.0, 0.0, 1.0), 0.0722, "luminance(blue)");
    assert_close(luminance(1.0, 1.0, 1.0), 1.0, "luminance(white)");
    assert_close(luminance(0.0, 0.0, 0.0), 0.0, "luminance(black)");
}

#[test]
fn contrast_ratio_black_white_is_21() {
    // The canonical WCAG boundary value: black-on-white (or vice versa) is
    // the maximum possible contrast ratio, (1.0 + 0.05) / (0.0 + 0.05) =
    // 21.0 exactly. This is the number WCAG AAA-level text-contrast
    // requirements are measured against (upstream's `min_contrast` uniform
    // is compared directly to this function's output).
    assert_close(
        contrast_ratio((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)),
        21.0,
        "contrast_ratio(black, white)",
    );
    assert_close(
        contrast_ratio((1.0, 1.0, 1.0), (0.0, 0.0, 0.0)),
        21.0,
        "contrast_ratio(white, black) is symmetric",
    );
}

#[test]
fn contrast_ratio_identical_colors_is_1() {
    // Minimum possible contrast ratio: identical luminances give
    // (L + 0.05) / (L + 0.05) = 1.0 for any L.
    assert_close(
        contrast_ratio((0.5, 0.5, 0.5), (0.5, 0.5, 0.5)),
        1.0,
        "contrast_ratio(gray, gray)",
    );
    assert_close(
        contrast_ratio((0.0, 0.0, 0.0), (0.0, 0.0, 0.0)),
        1.0,
        "contrast_ratio(black, black)",
    );
}

#[test]
fn contrast_ratio_wcag_aa_boundary_case() {
    // WCAG 2.0 AA normal-text minimum is 4.5:1. Ghostty's default
    // `minimum-contrast` config option is documented against this
    // threshold family; pin a concrete gray-on-white pair that sits
    // essentially at the AA boundary as a sanity check on the formula
    // shape (not just the black/white extreme).
    //
    // Linear gray g solving (1.0 + 0.05) / (g + 0.05) = 4.5
    // => g = (1.05 / 4.5) - 0.05.
    let g = (1.05 / 4.5) - 0.05;
    assert_close(
        contrast_ratio((g, g, g), (1.0, 1.0, 1.0)),
        4.5,
        "contrast_ratio at the WCAG AA 4.5:1 boundary",
    );
}

#[test]
fn linear_correction_alpha_no_op_when_luminances_are_close() {
    // Upstream: "we don't apply correction when the bg and fg luminances
    // are within 0.001 of each other" — an explicit dead-band to avoid the
    // remap's division blowing up near fg_l == bg_l.
    assert_close(
        linear_correction_alpha(0.5, 0.5005, 0.3),
        0.3,
        "no-op inside the 0.001 dead-band",
    );
    assert_close(
        linear_correction_alpha(0.5, 0.5, 0.7),
        0.7,
        "no-op at exact equality",
    );
}

#[test]
fn linear_correction_alpha_boundaries_are_preserved() {
    // At full coverage (a=1) the blend is pure foreground; at zero
    // coverage (a=0) it's pure background — the remap must reproduce these
    // regardless of the luminance gap, since blend_l collapses to fg_l or
    // bg_l exactly at the endpoints.
    assert_close(
        linear_correction_alpha(0.9, 0.1, 1.0),
        1.0,
        "a=1 (full fg coverage) stays 1",
    );
    assert_close(
        linear_correction_alpha(0.9, 0.1, 0.0),
        0.0,
        "a=0 (full bg coverage) stays 0",
    );
}

#[test]
fn linear_correction_alpha_black_text_on_white_bg_darkens_faster() {
    // The whole point of the weight-correction remap: gamma-incorrect
    // (naive sRGB-space) blending makes light-on-dark and dark-on-light
    // text look "thinner" or "thicker" asymmetrically compared to
    // physically-correct linear blending, because sRGB gamma is
    // perceptually-biased-but-not-linear. For black text (fg_l=0) on a
    // white background (bg_l=1) at 50% linear coverage, the corrected
    // alpha must be **greater** than the naive 0.5 — perceptually-uniform
    // 50% gray requires more than 50% coverage of black paint over white,
    // since sRGB gamma encoding expands mid-tones. This golden value pins
    // the direction and magnitude of that correction.
    let corrected = linear_correction_alpha(0.0, 1.0, 0.5);
    assert!(
        corrected > 0.5,
        "expected corrected alpha > naive 0.5, got {corrected}"
    );
    assert_close(
        corrected,
        0.785_958_859_517_767_6,
        "corrected alpha golden value",
    );
}
