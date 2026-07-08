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

use qwertty_term_font::coretext::{Face, PixelFormat};
use qwertty_term_font::grid::AtlasKind;
use qwertty_term_font::metrics::Metrics;
use qwertty_term_font::{CodepointResolver, Collection, FontIndex, Grid, Style};

const SIZE_PX: f64 = 16.0;

/// Build a grid whose primary is the embedded JetBrains Mono, with discovery
/// fallback enabled.
fn grid() -> Grid {
    let primary = Face::load_embedded(SIZE_PX).expect("load embedded JetBrains Mono");
    let metrics = Metrics::calc(primary.face_metrics());
    let resolver = CodepointResolver::new(Collection::new(primary));
    Grid::new(resolver, metrics).expect("build grid")
}

/// Build a grid matching the real default-font-parity chain: primary
/// (embedded JetBrains Mono variable) + the explicit embedded nerd-symbols
/// fallback slot (`Collection::new_with_default_fallbacks`), with discovery
/// **disabled**. This is the shape that proves the nerd-symbols slot resolves
/// PUA codepoints on its own, without ever consulting system discovery.
fn default_chain_grid_no_discovery() -> Grid {
    let primary = Face::load_embedded(SIZE_PX).expect("load embedded JetBrains Mono");
    let metrics = Metrics::calc(primary.face_metrics());
    let collection = Collection::new_with_default_fallbacks(primary, SIZE_PX)
        .expect("build default collection with nerd-symbols fallback");
    let resolver = CodepointResolver::without_discovery(collection);
    Grid::new(resolver, metrics).expect("build grid")
}

/// Build a grid over the real default chain (`new_with_default_fallbacks`,
/// which pre-seeds the macOS Apple Color Emoji fallback) WITH discovery enabled.
/// This is the exact shape the app uses for the no-`font-family` default, and
/// the one that exercises the emoji-discovery-parity fix (Item 1).
fn default_chain_grid() -> Grid {
    let primary = Face::load_embedded(SIZE_PX).expect("load embedded JetBrains Mono");
    let metrics = Metrics::calc(primary.face_metrics());
    let collection =
        Collection::new_with_default_fallbacks(primary, SIZE_PX).expect("build default collection");
    let resolver = CodepointResolver::new(collection);
    Grid::new(resolver, metrics).expect("build grid")
}

/// Emoji-presentation codepoints (🦀 U+1F980, 🥋 U+1F94B, ✅ U+2705) resolve to
/// **Apple Color Emoji** — the OS-native emoji font — via the pre-seeded
/// collection fallback, NOT a user-installed third-party emoji font (e.g. Noto
/// Color Emoji, which has ~10x the glyph count and would otherwise win the
/// discovery Score tiebreak). This is the Item-1 field-bug parity assertion.
///
/// Apple Color Emoji ships with macOS, so this asserts hard (no skip-if-missing).
/// Matches the ground truth from `ghostty +show-face --cp=0x1F980` on the same
/// machine (upstream mechanism: `SharedGridSet.zig:335-354`).
#[test]
fn emoji_resolves_to_apple_color_emoji() {
    let mut g = default_chain_grid();
    for cp in [0x1F980u32, 0x1F94B, 0x2705] {
        let idx = g
            .get_index(cp)
            .unwrap_or_else(|| panic!("U+{cp:X} must resolve"));
        let face = g.resolver().collection().get_face(idx).expect("face");
        assert_eq!(
            face.family_name(),
            "Apple Color Emoji",
            "U+{cp:X} must resolve to Apple Color Emoji, got {:?}",
            face.family_name()
        );
        assert!(face.has_color(), "U+{cp:X} emoji face must be a color face");
    }
}

/// Inverse guard: a **text-presentation** symbol (U+270C VICTORY HAND, which is
/// emoji-*capable* but text-presentation by default) must NOT resolve to the
/// color Apple Color Emoji fallback. The pre-seeded emoji fallback is a color
/// face marked `fallback = true`, so upstream's presentation asymmetry rejects
/// it for a default-text request; resolution falls through to a text font.
/// Ground truth: `ghostty +show-face --cp=0x270C` picks a text Nerd Font on this
/// machine, never Apple Color Emoji.
#[test]
fn text_presentation_symbol_avoids_color_emoji_fallback() {
    let mut g = default_chain_grid();
    // Default presentation (no VS16): must be a TEXT face.
    if let Some(idx) = g.get_index(0x270C) {
        let face = g.resolver().collection().get_face(idx).expect("face");
        assert_ne!(
            face.family_name(),
            "Apple Color Emoji",
            "text-presentation U+270C must not resolve to the color emoji fallback"
        );
        assert!(
            !face.has_color(),
            "text-presentation U+270C resolved to a color face ({:?})",
            face.family_name()
        );
    } else {
        // No text font on this machine has U+270C: acceptable, but Apple Color
        // Emoji must still not have satisfied it (it didn't, since idx is None).
    }
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

// ============================================================================
// Default-font-parity: explicit nerd-symbols fallback slot (no discovery).
// ============================================================================

/// U+E725 (a Nerd Fonts devicon-range glyph, not a sprite-dispatched
/// codepoint and NOT present in JetBrains Mono itself — confirmed via
/// `fontTools` cmap inspection of both vendored files) must resolve to the
/// embedded nerd-symbols fallback face, with discovery entirely disabled.
/// This is the parity claim: real Ghostty's default chain (`SharedGridSet`)
/// adds `symbols_nerd_font` as a static fallback ahead of system discovery,
/// so a PUA nerd glyph resolves from the *bundled* font, never the system.
///
/// (Note: U+E0A0, the classic Nerd Fonts "branch" glyph mentioned as an
/// example in the task brief, turns out to already be present in JetBrains
/// Mono's own cmap in this vendored variable build, so it resolves from the
/// *primary* face (slot 0) rather than the fallback — correct per upstream's
/// precedence rules [user/primary faces beat fallback faces], but it doesn't
/// exercise the fallback slot, hence U+E725 here instead.)
#[test]
fn nerd_pua_devicon_resolves_via_embedded_symbols_without_discovery() {
    let mut g = default_chain_grid_no_discovery();
    let idx = g
        .get_index(0xE725)
        .expect("U+E725 resolves via the embedded nerd-symbols fallback");

    match idx {
        FontIndex::Face { style, slot } => {
            assert_eq!(style, Style::Regular);
            assert!(
                slot >= 1,
                "expected the nerd-symbols fallback slot (slot > 0), got slot {slot}"
            );
        }
        FontIndex::Sprite => panic!("U+E725 is not a sprite codepoint, got Sprite"),
    }

    // It must actually rasterize (grayscale outline glyph).
    let glyph = g
        .render_codepoint(0xE725)
        .expect("render succeeds")
        .expect("U+E725 renders");
    assert_eq!(glyph.atlas, AtlasKind::Grayscale);
    assert!(
        glyph.width > 0 && glyph.height > 0,
        "U+E725 rasterized empty"
    );
}

/// U+F015 (a classic Nerd/Font-Awesome "home" glyph, also not sprite-
/// dispatched) resolves the same way: embedded nerd-symbols fallback, no
/// discovery required.
#[test]
fn nerd_pua_home_resolves_via_embedded_symbols_without_discovery() {
    let mut g = default_chain_grid_no_discovery();
    let idx = g
        .get_index(0xF015)
        .expect("U+F015 resolves via the embedded nerd-symbols fallback");
    match idx {
        FontIndex::Face { style, slot } => {
            assert_eq!(style, Style::Regular);
            assert!(slot >= 1, "expected a fallback slot, got slot {slot}");
        }
        FontIndex::Sprite => panic!("U+F015 is not a sprite codepoint, got Sprite"),
    }
}

/// Sprite precedence: U+E0B0 (POWERLINE RIGHT ARROW) is *both* a
/// qwertty-term-sprite-dispatched codepoint AND present in the nerd-symbols font.
/// The resolver must still pick the sprite (step 3 of `getIndex` runs before
/// any font/collection lookup), exactly mirroring upstream's
/// `CodepointResolver.getIndex` ordering — the explicit nerd-symbols fallback
/// slot must NOT shadow the procedural sprite renderer for codepoints both
/// cover.
#[test]
fn sprite_takes_precedence_over_embedded_symbols_fallback() {
    let mut g = default_chain_grid_no_discovery();
    let idx = g.get_index(0xE0B0).expect("U+E0B0 resolves");
    assert_eq!(
        idx,
        FontIndex::Sprite,
        "box-drawing/powerline-core codepoints must resolve to the sprite \
         renderer even when the embedded nerd-symbols font also has the glyph"
    );

    // Same check for a box-drawing codepoint (U+2500), which the symbols font
    // very likely also covers.
    let idx_box = g.get_index(0x2500).expect("U+2500 resolves");
    assert_eq!(idx_box, FontIndex::Sprite, "box drawing must be a sprite");
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
