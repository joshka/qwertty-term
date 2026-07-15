//! Byte-backed named faces → real shaping (ligatures).
//!
//! Field gap (Item 2): `Face::load_by_name` used to return `source_bytes = None`,
//! so `Shaper::new` couldn't build a rustybuzz face and the engine fell back to
//! unshaped per-codepoint rendering for named families — FiraCode's signature
//! ligatures (`->`, `=>`, `==`) never formed, unlike real Ghostty.
//!
//! The fix reads the font FILE via CoreText's URL attribute
//! (`kCTFontURLAttribute`) so name-loaded faces carry their bytes. These tests
//! prove (a) the shaper builds for a named family, and (b) a ligature run gets
//! its glyphs substituted (the arrow forms) with the per-cell cluster mapping
//! intact.
//!
//! FiraCode Nerd Font Mono is the maintainer's configured family; if it isn't
//! installed the tests skip (they don't fail — `load_by_name` falls back to the
//! embedded face on a miss, which wouldn't exercise the named path).
//!
//! macOS only.
#![cfg(all(target_os = "macos", not(feature = "freetype")))]

use qwertty_term_font::Shaper;
use qwertty_term_font::coretext::Face;

const FAMILY: &str = "FiraCode Nerd Font Mono";
const SIZE_PX: f64 = 16.0;

/// Load the configured family, or `None` (skip) if it isn't actually installed.
fn firacode() -> Option<Face> {
    match Face::load_by_name(FAMILY, SIZE_PX) {
        Ok(f) if f.family_name().to_lowercase().contains("fira") => Some(f),
        _ => {
            eprintln!("SKIP: {FAMILY} not installed");
            None
        }
    }
}

/// (a) A name-loaded FiraCode face now carries its backing bytes, so
/// `Shaper::new` succeeds — the precondition for any OpenType shaping.
#[test]
fn shaper_builds_for_named_family() {
    let Some(face) = firacode() else { return };
    assert!(
        face.source_bytes().is_some(),
        "name-loaded FiraCode must carry backing bytes (read via the font URL)"
    );
    assert!(
        Shaper::new(&face).is_some(),
        "Shaper::new must succeed for a byte-backed named face"
    );
}

/// (b) Shaping `->` with FiraCode forms the arrow ligature: the shaped glyph ids
/// differ from the plain (unshaped) `-` and `>` glyph ids — proof that the
/// `calt` contextual-alternates substitution ran (which is only possible through
/// the byte-backed rustybuzz shaper). FiraCode implements arrows as a two-glyph
/// "split ligature" that preserves one glyph per cell (keeping monospace
/// advances), so the count stays 2 but the glyphs are the ligature halves, not
/// the literal `-`/`>`.
///
/// The per-cell cluster mapping is asserted too: cell 0 and cell 1 carry the two
/// ligature halves in order (cluster == cell X), matching upstream's
/// per-output-glyph cell assignment (`harfbuzz.zig` appends one cell per glyph
/// with `x = cell_offset.cluster`).
#[test]
fn arrow_ligature_forms_and_maps_per_cell() {
    let Some(face) = firacode() else { return };
    let mut shaper = Shaper::new(&face).expect("shaper builds");

    // Plain (isolated) glyph ids for '-' and '>'.
    let plain_dash = shaper.shape_run("-")[0].glyph_index;
    let plain_gt = shaper.shape_run(">")[0].glyph_index;

    let arrow = shaper.shape_run("->");
    assert_eq!(
        arrow.len(),
        2,
        "FiraCode '->' is a two-glyph split ligature (one glyph per cell)"
    );

    // Per-cell cluster mapping: cell 0 then cell 1, in order.
    assert_eq!(arrow[0].cell_x, 0, "first ligature half sits at cell 0");
    assert_eq!(arrow[1].cell_x, 1, "second ligature half sits at cell 1");

    // The substitution actually happened: at least one shaped glyph differs from
    // its plain form (the arrow's ligature glyphs are distinct from '-'/'>').
    let substituted = arrow[0].glyph_index != plain_dash || arrow[1].glyph_index != plain_gt;
    assert!(
        substituted,
        "'->' did not ligate: shaped glyphs {:?} equal the plain '-'={plain_dash} '>'={plain_gt} \
         (calt substitution did not run — the face is not being shaped)",
        arrow.iter().map(|c| c.glyph_index).collect::<Vec<_>>()
    );

    // The two ligature halves are distinct glyphs (left arrow shaft vs head).
    assert_ne!(
        arrow[0].glyph_index, arrow[1].glyph_index,
        "the two arrow-ligature halves should be distinct glyphs"
    );
}

/// A few more of FiraCode's signature ligatures also substitute, confirming the
/// shaper isn't a one-off: `=>`, `==`, `!=` each differ from their plain forms.
#[test]
fn common_ligatures_substitute() {
    let Some(face) = firacode() else { return };
    let mut shaper = Shaper::new(&face).expect("shaper builds");

    for lig in ["=>", "==", "!="] {
        let mut chars = lig.chars();
        let a = chars.next().unwrap();
        let b = chars.next().unwrap();
        let plain_a = shaper.shape_run(&a.to_string())[0].glyph_index;
        let plain_b = shaper.shape_run(&b.to_string())[0].glyph_index;

        let shaped = shaper.shape_run(lig);
        let substituted = shaped.iter().map(|c| c.glyph_index).ne([plain_a, plain_b]);
        assert!(
            substituted,
            "'{lig}' did not ligate (shaped {:?} == plain [{plain_a}, {plain_b}])",
            shaped.iter().map(|c| c.glyph_index).collect::<Vec<_>>()
        );
    }
}
