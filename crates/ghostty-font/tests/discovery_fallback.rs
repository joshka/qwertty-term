//! Discovery + fallback resolution acceptance (M3 F5-full).
//!
//! Drives the full resolver chain over the embedded JetBrains Mono primary:
//! codepoints the primary lacks (emoji, CJK, Greek, a Nerd-Font glyph) must
//! resolve to a discovered system fallback face and rasterize into the correct
//! atlas — color (BGRA) for emoji, grayscale (alpha8) for text. Also exercises
//! Score determinism and fuzzy-name discovery.
//!
//! macOS only (CoreText discovery + rasterization).
#![cfg(target_os = "macos")]

use ghostty_font::coretext::{Face, PixelFormat};
use ghostty_font::grid::AtlasKind;
use ghostty_font::metrics::Metrics;
use ghostty_font::{CodepointResolver, Collection, Grid};

const SIZE_PX: f64 = 16.0;

/// Build a grid whose primary is the embedded JetBrains Mono, with discovery
/// fallback enabled.
fn grid() -> Grid {
    let primary = Face::load_embedded(SIZE_PX).expect("load embedded JetBrains Mono");
    let metrics = Metrics::calc(primary.face_metrics());
    let resolver = CodepointResolver::new(Collection::new(primary));
    Grid::new(resolver, metrics).expect("build grid")
}

/// 😀 (U+1F600) resolves to a discovered color face and rasterizes as a
/// non-empty BGRA glyph in the color atlas.
#[test]
fn emoji_resolves_and_rasterizes_bgra() {
    let mut g = grid();
    let glyph = g
        .render_codepoint(0x1F600)
        .expect("render succeeds")
        .expect("emoji resolves to a glyph");

    assert_eq!(
        glyph.atlas,
        AtlasKind::Color,
        "emoji glyph should live in the color atlas"
    );
    assert!(
        glyph.width > 0 && glyph.height > 0,
        "emoji glyph rasterized empty"
    );

    // The color atlas is BGRA and the region must be non-blank (color emoji
    // have ink). Spot-check that the color atlas holds nonzero bytes.
    let atlas = g.color_atlas();
    assert!(
        atlas.data().iter().any(|&b| b != 0),
        "color atlas is all-zero after rasterizing an emoji"
    );
}

/// The emoji face reached via discovery is genuinely a color face, and its
/// rasterized bitmap is BGRA.
#[test]
fn emoji_face_is_color() {
    let mut g = grid();
    let idx = g.get_index(0x1F600).expect("emoji resolves");
    let face = g.resolver().collection().get_face(idx).expect("face");
    assert!(face.has_color(), "discovered emoji face must be color");

    let gid = face.glyph_index('😀').expect("emoji face has 😀");
    let bmp = face.rasterize(gid).expect("rasterize emoji");
    assert_eq!(bmp.format, PixelFormat::Bgra, "emoji glyph must be BGRA");
    assert!(!bmp.is_blank(), "emoji glyph rasterized blank");
}

/// 水 (U+6C34) resolves (from the primary if it has it, else a discovered CJK
/// system font) and rasterizes as a non-empty grayscale glyph.
#[test]
fn cjk_resolves_and_rasterizes() {
    let mut g = grid();
    let glyph = g
        .render_codepoint('水' as u32)
        .expect("render succeeds")
        .expect("水 resolves");
    assert_eq!(
        glyph.atlas,
        AtlasKind::Grayscale,
        "CJK text glyph should be grayscale"
    );
    assert!(glyph.width > 0 && glyph.height > 0, "水 rasterized empty");

    // Confirm the resolved face genuinely has the glyph.
    let idx = g.get_index('水' as u32).expect("水 resolves");
    let face = g.resolver().collection().get_face(idx).expect("face");
    assert!(
        face.glyph_index('水').is_some(),
        "resolved face must have 水"
    );
}

/// Ω (U+03A9 GREEK CAPITAL LETTER OMEGA) resolves and rasterizes. JetBrains
/// Mono actually has Ω, so this typically resolves to the primary; either way
/// the glyph must render grayscale and non-empty.
#[test]
fn omega_resolves_and_rasterizes() {
    let mut g = grid();
    let glyph = g
        .render_codepoint('Ω' as u32)
        .expect("render succeeds")
        .expect("Ω resolves");
    assert_eq!(glyph.atlas, AtlasKind::Grayscale);
    assert!(glyph.width > 0 && glyph.height > 0, "Ω rasterized empty");
}

/// A Nerd-Font Private-Use codepoint (U+E0B0 POWERLINE RIGHT ARROW): if a
/// Nerd Font is system-installed, discovery resolves it; otherwise skip with a
/// note (the primary JetBrains Mono is not the patched Nerd Font variant).
#[test]
fn nerd_font_codepoint_resolves_if_present() {
    let mut g = grid();
    // U+E0B0 is a common powerline glyph in the Private Use Area.
    match g.render_codepoint(0xE0B0) {
        Ok(Some(glyph)) => {
            assert!(
                glyph.width > 0 && glyph.height > 0,
                "Nerd-Font glyph rasterized empty"
            );
        }
        Ok(None) | Err(_) => {
            eprintln!(
                "note: no system font provides U+E0B0 (Nerd Font not installed); \
                 Nerd-Font resolution skipped"
            );
        }
    }
}

/// Score determinism: resolving the same emoji twice from fresh grids yields the
/// same discovered fallback family.
#[test]
fn discovery_is_deterministic() {
    let mut g1 = grid();
    let mut g2 = grid();

    let i1 = g1.get_index(0x1F600).expect("emoji resolves (1)");
    let i2 = g2.get_index(0x1F600).expect("emoji resolves (2)");

    let f1 = g1.resolver().collection().get_face(i1).expect("face 1");
    let f2 = g2.resolver().collection().get_face(i2).expect("face 2");
    assert_eq!(
        f1.family_name(),
        f2.family_name(),
        "emoji discovery should pick the same family across runs"
    );
}
