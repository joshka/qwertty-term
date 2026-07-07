//! Exhaustive cross-checks of the ported unicode tables/FSM against the Rust
//! ecosystem oracles `unicode-width` and `unicode-segmentation` (dev-dependencies
//! only; both implement Unicode 17.0.0, the same UCD version as our generated
//! tables, so every divergence below is semantic, not version skew).
//!
//! Ghostty's semantics intentionally diverge from general-text semantics in
//! terminal-specific places. Every divergence must be classified by an allowlist
//! rule with a documented reason; anything unclassified fails the test.

use ghostty_vt::unicode::{
    self, BreakState, GraphemeBreakClass, codepoint_width, grapheme_break, properties,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

#[test]
fn oracle_unicode_versions_match_tables() {
    assert_eq!(unicode::UNICODE_VERSION, "17.0.0");
    assert_eq!(unicode_width::UNICODE_VERSION, (17, 0, 0));
    assert_eq!(unicode_segmentation::UNICODE_VERSION, (17, 0, 0));
}

/// Classifies an intentional width divergence from `unicode-width`. Returns the
/// reason, or None if the divergence is unexpected (test failure).
///
/// `ours` is ghostty's terminal width (0..=2); `oracle` is
/// `UnicodeWidthChar::width` (None for control characters).
fn width_divergence_reason(cp: u32, ours: u8, oracle: Option<usize>) -> Option<&'static str> {
    let gb = properties(cp).grapheme_break;
    match (ours, oracle) {
        // unicode-width defines control characters as None ("width unknown");
        // ghostty encodes 0: the terminal executes controls, they never occupy a
        // cell (uucode wcwidth: gc=Cc -> 0).
        (0, None) if cp < 0x20 || (0x7F..=0x9F).contains(&cp) => {
            Some("C0/C1 control: terminal executes controls, width 0")
        }
        // U+2028 LINE SEPARATOR / U+2029 PARAGRAPH SEPARATOR: mandatory breaks
        // never advance the cursor (uucode wcwidth: gc Zl/Zp -> 0); oracle says 1.
        (0, Some(1)) if cp == 0x2028 || cp == 0x2029 => {
            Some("line/paragraph separator: width 0 in a terminal")
        }
        // Lone regional indicators render as a letter-in-a-box occupying two cells
        // (UTS #51 C3; uucode wcwidth tailoring); oracle says 1 (EAW=Neutral).
        (2, Some(1)) if gb == GraphemeBreakClass::RegionalIndicator => {
            Some("regional indicator: terminal renders letter-in-box, width 2")
        }
        // Prepend characters (Arabic number signs, Indic rephas, ...) keep their
        // standalone width so they don't vanish when ghostty zeroes them
        // (ghostty src/build/uucode_config.zig computeWidth exception); the oracle
        // zeroes the Cf ones.
        (1, Some(0)) if gb == GraphemeBreakClass::Prepend => {
            Some("Prepend characters keep standalone width in ghostty")
        }
        // Spacing marks (gc=Mc) that UAX #29 classifies as Extend/Linker for
        // canonical-equivalence reasons (e.g. U+09BE, U+302E, U+16FF0):
        // unicode-width zeroes everything Grapheme_Extend; ghostty keeps their
        // positive advance width (UAX #44: Mc have positive advance; defective
        // sequences render on a NBSP base), incl. width 2 for the EAW=W ones.
        (1..=2, Some(0))
            if matches!(
                gb,
                GraphemeBreakClass::IndicConjunctBreakExtend
                    | GraphemeBreakClass::IndicConjunctBreakLinker
            ) =>
        {
            Some("spacing mark classified Extend/Linker: ghostty keeps its advance width")
        }
        // Kirat Rai vowels (GCB=V, U+16D63/U+16D67..U+16D6A): ghostty zeroes all
        // GCB=V/T like Hangul jamo (they only occur inside clusters whose other
        // codepoints carry the width); the oracle only zeroes the Hangul ones.
        (0, Some(1)) if gb == GraphemeBreakClass::V => {
            Some("GCB=V vowel: zero width in grapheme, like Hangul jamo")
        }
        // U+115F HANGUL CHOSEONG FILLER: Default_Ignorable_Code_Point -> 0 in
        // uucode's wcwidth (checked before EAW=W); the oracle keeps EAW width 2.
        (0, Some(2)) if cp == 0x115F => Some("Hangul choseong filler: default-ignorable, width 0"),
        // U+00AD SOFT HYPHEN: rendered as a visible hyphen in terminals (Unicode
        // FAQ; matches ecosystem wcwidth); unicode-width says 0.
        (1, Some(0)) if cp == 0x00AD => Some("soft hyphen: visible hyphen in terminals"),
        // U+2D7F TIFINAGH CONSONANT JOINER: gc=Mn -> 0 in ghostty; unicode-width
        // tailors it to 1.
        (0, Some(1)) if cp == 0x2D7F => Some("Tifinagh consonant joiner: gc=Mn, width 0"),
        // U+2E3A/U+2E3B TWO/THREE-EM DASH: uucode explicitly sizes them (3 clamped
        // to 2 by ghostty); unicode-width says 1.
        (2, Some(1)) if cp == 0x2E3A || cp == 0x2E3B => {
            Some("two/three-em dash: renders across two cells (clamped)")
        }
        // U+A8FA DEVANAGARI CARET: zero-advance editorial mark in unicode-width's
        // tailoring; gc=Po/EAW=N -> 1 in ghostty (and in ghostty's Zig tables).
        (1, Some(0)) if cp == 0xA8FA => Some("Devanagari caret: ghostty follows EAW, width 1"),
        // U+17A4 / U+17D8: unicode-width tailors these Khmer signs to widths 2 and
        // 3 (ligature-like rendering); ghostty follows EAW -> 1.
        (1, Some(2 | 3)) if cp == 0x17A4 || cp == 0x17D8 => {
            Some("Khmer sign: unicode-width multi-cell tailoring, ghostty follows EAW")
        }
        _ => None,
    }
}

#[test]
fn width_matches_unicode_width_exhaustive() {
    let mut checked = 0u32;
    let mut diverged = 0u32;
    let mut unexpected: Vec<String> = Vec::new();

    for cp in 0..=0x10FFFFu32 {
        let Some(c) = char::from_u32(cp) else {
            // Surrogates: not representable as char; ghostty's table gives them
            // width 0 (gc=Cs). Assert that directly since the oracle can't.
            assert_eq!(codepoint_width(cp), 0, "surrogate U+{cp:04X} must be 0");
            continue;
        };

        let ours = codepoint_width(cp);
        let oracle = c.width();
        checked += 1;
        if oracle == Some(ours as usize) {
            continue;
        }

        diverged += 1;
        if width_divergence_reason(cp, ours, oracle).is_none() {
            unexpected.push(format!("U+{cp:04X}: ours={ours} oracle={oracle:?}"));
        }
    }

    // Pin the divergence volume so silent drift is caught: 188 known divergences
    // as of UCD 17.0.0 / unicode-width 0.2.2 (see width_divergence_reason).
    assert!(
        unexpected.is_empty(),
        "unexpected width divergences ({} of {checked} checked, {diverged} total diverged):\n{}",
        unexpected.len(),
        unexpected[..unexpected.len().min(40)].join("\n"),
    );
    assert_eq!(diverged, 188, "width divergence count drifted");
}

/// UAX #29 `Grapheme_Cluster_Break ∈ {Control, CR, LF}` membership. Our runtime
/// table deliberately collapses these classes into `Other`, so the original set is
/// carried as a generated test fixture (see `xtask gen-unicode`).
fn grapheme_control_set() -> Vec<(u32, u32)> {
    include_str!("data/grapheme_control.txt")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| {
            let (lo, hi) = line.split_once("..").expect("range");
            (
                u32::from_str_radix(lo, 16).unwrap(),
                u32::from_str_radix(hi, 16).unwrap(),
            )
        })
        .collect()
}

/// Oracle break decision: is there a grapheme cluster boundary between the two
/// chars of the pair per unicode-segmentation (extended clusters)?
fn oracle_breaks(a: char, b: char) -> bool {
    let s: String = [a, b].iter().collect();
    s.graphemes(true).count() > 1
}

#[test]
fn breaks_match_unicode_segmentation_exhaustive() {
    let control = grapheme_control_set();
    let is_control = |cp: u32| control.iter().any(|&(lo, hi)| (lo..=hi).contains(&cp));

    // Classifies an intentional break divergence from stock UAX #29. Returns the
    // reason, or None if the divergence is unexpected (test failure).
    let break_divergence_reason = |cp1: u32, cp2: u32| -> Option<&'static str> {
        let gb1 = properties(cp1).grapheme_break;
        let gb2 = properties(cp2).grapheme_break;

        // UTS #51 emoji-modifier tailoring (uucode src/x/grapheme.zig `isExtend`
        // comment): stock UAX #29 treats Emoji_Modifier as Extend, gluing it to
        // anything; ghostty only lets it continue a cluster after an emoji
        // modifier base (or within an emoji sequence). "a" + skin tone breaks.
        if gb2 == GraphemeBreakClass::EmojiModifier && gb1 != GraphemeBreakClass::EmojiModifierBase
        {
            return Some("emoji modifier only continues a cluster after a modifier base");
        }

        // Control/CR/LF are collapsed to Other in ghostty's table: the terminal
        // strips controls before segmentation ever runs (ghostty grapheme.zig doc
        // comment), so GB4/GB5 are compiled out and e.g. Control x Extend no
        // longer breaks.
        if is_control(cp1) || is_control(cp2) {
            return Some("control/CR/LF are filtered before segmentation in the terminal");
        }

        None
    };

    let mut checked = 0u64;
    let mut diverged = 0u64;
    let mut unexpected: Vec<String> = Vec::new();

    // Anchors exercise every stateless rule against every codepoint from both
    // sides: Other ('a'), Extend (U+0301), Extended_Pictographic (U+1F469),
    // Hangul L (U+1100), ZWJ (U+200D); the self-pair hits RI x RI, T x T, etc.
    let anchors = ['a', '\u{0301}', '\u{1F469}', '\u{1100}', '\u{200D}'];

    for cp in 0..=0x10FFFFu32 {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };

        let mut pairs: Vec<(char, char)> = Vec::with_capacity(anchors.len() * 2 + 1);
        for a in anchors {
            pairs.push((a, c));
            pairs.push((c, a));
        }
        pairs.push((c, c));

        for (x, y) in pairs {
            let mut state = BreakState::default();
            let ours = grapheme_break(x as u32, y as u32, &mut state);
            let oracle = oracle_breaks(x, y);
            checked += 1;
            if ours == oracle {
                continue;
            }
            diverged += 1;
            if break_divergence_reason(x as u32, y as u32).is_none() {
                unexpected.push(format!(
                    "U+{:04X} x U+{:04X}: ours={ours} oracle={oracle} (classes {:?} {:?})",
                    x as u32,
                    y as u32,
                    properties(x as u32).grapheme_break,
                    properties(y as u32).grapheme_break,
                ));
            }
        }
    }

    assert!(
        unexpected.is_empty(),
        "unexpected break divergences ({} of {checked} pairs, {diverged} total diverged):\n{}",
        unexpected.len(),
        unexpected[..unexpected.len().min(40)].join("\n"),
    );
}

/// Streaming multi-codepoint sequences: run the stateful FSM over whole strings
/// and compare every boundary against unicode-segmentation.
#[test]
fn streaming_sequences_match_unicode_segmentation() {
    let corpus: &[&str] = &[
        "hello",
        "👩‍👩‍👧‍👦 family",
        "🇨🇭🇺🇸🇩🇪 flags",
        "👋🏿 wave",
        "क्‍ष conjunct", // consonant + virama (linker) + ZWJ + consonant
        "நி tamil",
        "각각 hangul",
        "a\u{0301}\u{0302}bc", // stacked combining marks
        "#\u{FE0F}\u{20E3} keycap",
        "x\u{200D}y", // stray ZWJ
        "🏴‍☠️ pirate",
        "🇦🇧🇨 odd regional indicators",
    ];

    for s in corpus {
        let cps: Vec<u32> = s.chars().map(|c| c as u32).collect();
        // Our boundaries: entry i is the break decision between cps[i] and cps[i+1].
        let mut ours = Vec::new();
        let mut state = BreakState::default();
        for w in cps.windows(2) {
            ours.push(grapheme_break(w[0], w[1], &mut state));
        }
        // Oracle boundaries from grapheme cluster char offsets.
        let mut boundaries = std::collections::HashSet::new();
        let mut char_index = 0;
        for g in s.graphemes(true) {
            char_index += g.chars().count();
            boundaries.insert(char_index);
        }
        let oracle: Vec<bool> = (1..cps.len()).map(|i| boundaries.contains(&i)).collect();
        assert_eq!(ours, oracle, "boundary mismatch in {s:?}");
    }
}
