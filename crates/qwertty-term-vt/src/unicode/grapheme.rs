//! Grapheme cluster segmentation and width (port of ghostty `src/unicode/grapheme.zig`
//! and the rule kernel it precomputes, uucode `src/x/grapheme.zig`
//! `computeGraphemeBreakNoControl`).

use super::properties;
use super::props::GraphemeBreakClass as G;

/// Cross-call state for [`grapheme_break`] (mirror of uucode's `BreakState`, u3).
///
/// Tracks the three context-sensitive UAX #29 rules: GB12/13 regional-indicator
/// pairing parity, GB11 emoji ZWJ sequences, and GB9c Indic conjunct breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum BreakState {
    #[default]
    Default = 0,
    RegionalIndicator = 1,
    ExtendedPictographic = 2,
    IndicConjunctBreakConsonant = 3,
    IndicConjunctBreakLinker = 4,
}

impl BreakState {
    const COUNT: usize = 5;

    const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Default,
            1 => Self::RegionalIndicator,
            2 => Self::ExtendedPictographic,
            3 => Self::IndicConjunctBreakConsonant,
            4 => Self::IndicConjunctBreakLinker,
            _ => unreachable!(),
        }
    }
}

/// Determines if there is a grapheme break between two codepoints. This must be
/// called sequentially, maintaining the state between calls (the `cp2` of one call
/// is the `cp1` of the next).
///
/// This function does NOT work with control characters. Control characters, line
/// feeds, and carriage returns are expected to be filtered out before calling this
/// function, because it is tuned for the terminal (which handles controls upstream).
///
/// Port of ghostty `src/unicode/grapheme.zig` `graphemeBreak`: two property lookups
/// plus one index into an 8 KiB transition table precomputed at compile time over
/// every `(state, class1, class2)` triple.
pub fn grapheme_break(cp1: u32, cp2: u32, state: &mut BreakState) -> bool {
    let gb1 = properties(cp1).grapheme_break as usize;
    let gb2 = properties(cp2).grapheme_break as usize;
    let value = TRANSITIONS[*state as usize | (gb1 << 3) | (gb2 << 8)];
    *state = BreakState::from_u8(value >> 1);
    value & 1 != 0
}

/// Width change requested by a codepoint that continues a grapheme cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphemeWidthEffect {
    /// Do not append the codepoint to the cluster and leave break state as it was
    /// before seeing it (invalid emoji variation selector — the terminal does not
    /// store it in the cell).
    Ignore,
    /// Append the codepoint but leave the current cluster width unchanged.
    NoChange,
    /// Make the cluster occupy two terminal cells.
    Wide,
    /// Make the cluster occupy one terminal cell.
    Narrow,
}

/// Returns the width effect of appending `cp` after `prev` within a grapheme.
///
/// This is the shared width-decision kernel for the streaming terminal printer and
/// for [`grapheme_width`]. It assumes [`grapheme_break`] has already said there is
/// no break between `prev` and `cp`; it does not perform segmentation.
///
/// The [`Ignore`](GraphemeWidthEffect::Ignore) result is important for invalid
/// emoji variation selectors: callers must also restore their grapheme break state
/// and leave `prev` unchanged when they see it.
#[inline]
pub fn grapheme_width_effect(prev: u32, cp: u32) -> GraphemeWidthEffect {
    // Emoji variation selectors modify the width of a valid base: VS16 makes the
    // grapheme wide and VS15 makes it narrow — but only when prev forms a valid
    // variation sequence per emoji-variation-sequences.txt.
    if cp == 0xFE0F || cp == 0xFE0E {
        if !properties(prev).emoji_vs_base {
            return GraphemeWidthEffect::Ignore;
        }
        return match cp {
            0xFE0F => GraphemeWidthEffect::Wide,
            _ => GraphemeWidthEffect::Narrow,
        };
    }

    // If a codepoint contributes to the width of a grapheme, the whole grapheme is
    // at least width 2 because the first codepoint must be at least width 1 to
    // start. (Prepend codepoints could effectively mean the first codepoint should
    // be width 0, but ghostty doesn't handle that yet.)
    if !properties(cp).width_zero_in_grapheme {
        return GraphemeWidthEffect::Wide;
    }

    GraphemeWidthEffect::NoChange
}

/// Result of measuring the first grapheme cluster in a codepoint slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphemeWidth {
    /// Number of codepoints consumed from the input slice.
    pub len: usize,
    /// Display width in terminal cells (0..=2).
    pub width: u8,
}

/// Measures the first grapheme cluster in `cps` using the same segmentation and
/// width rules as `Terminal.print` with mode 2027.
///
/// This is not a streaming API: `cps` must contain a complete first grapheme
/// cluster or the logical end of the string. If codepoints arrive in chunks, keep
/// buffering while this consumes all available codepoints and more input may
/// still arrive.
///
/// Values greater than U+10FFFF are accepted so FFI-facing callers can use u32
/// input without trapping: an invalid value consumes one codepoint at width 1 when
/// it starts the slice, and terminates the current cluster when it appears later.
pub fn grapheme_width(cps: &[u32]) -> GraphemeWidth {
    let Some(&first) = cps.first() else {
        return GraphemeWidth { len: 0, width: 0 };
    };
    if invalid_codepoint(first) {
        return GraphemeWidth { len: 1, width: 1 };
    }

    let mut len = 1;
    let mut width = properties(first).width;
    let mut prev = first;
    let mut state = BreakState::Default;

    while len < cps.len() {
        let cp = cps[len];
        // Treat invalid u32 input as a boundary so a valid prefix cluster can
        // still be returned.
        if invalid_codepoint(cp) {
            break;
        }

        let state_before = state;
        if grapheme_break(prev, cp, &mut state) {
            break;
        }

        match grapheme_width_effect(prev, cp) {
            GraphemeWidthEffect::Ignore => state = state_before,
            GraphemeWidthEffect::NoChange => prev = cp,
            GraphemeWidthEffect::Wide => {
                width = 2;
                prev = cp;
            }
            GraphemeWidthEffect::Narrow => {
                width = 1;
                prev = cp;
            }
        }
        len += 1;
    }

    GraphemeWidth { len, width }
}

#[inline]
const fn invalid_codepoint(cp: u32) -> bool {
    cp > 0x10FFFF
}

/// Precomputed lookup table for all permutations of state and grapheme break
/// classes (ghostty `src/unicode/grapheme.zig` `Precompute`, built at Zig comptime;
/// here built by const evaluation of the ported rule kernel).
///
/// Key: `state (3 bits) | gb1 (5 bits) << 3 | gb2 (5 bits) << 8` — 2^13 entries.
/// Value: `is_break (bit 0) | next_state (bits 1..=3)`.
static TRANSITIONS: [u8; 1 << 13] = build_transitions();

const fn build_transitions() -> [u8; 1 << 13] {
    let mut data = [0u8; 1 << 13];
    let mut state = 0;
    while state < BreakState::COUNT {
        let mut gb1 = 0;
        while gb1 < G::COUNT {
            let mut gb2 = 0;
            while gb2 < G::COUNT {
                let (result, next) = compute_break(
                    class_from_index(gb1),
                    class_from_index(gb2),
                    BreakState::from_u8(state as u8),
                );
                data[state | (gb1 << 3) | (gb2 << 8)] = (result as u8) | ((next as u8) << 1);
                gb2 += 1;
            }
            gb1 += 1;
        }
        state += 1;
    }
    data
}

const fn class_from_index(index: usize) -> G {
    match index {
        0 => G::Other,
        1 => G::Prepend,
        2 => G::RegionalIndicator,
        3 => G::SpacingMark,
        4 => G::L,
        5 => G::V,
        6 => G::T,
        7 => G::Lv,
        8 => G::Lvt,
        9 => G::Zwj,
        10 => G::Zwnj,
        11 => G::ExtendedPictographic,
        12 => G::EmojiModifierBase,
        13 => G::EmojiModifier,
        14 => G::IndicConjunctBreakExtend,
        15 => G::IndicConjunctBreakLinker,
        16 => G::IndicConjunctBreakConsonant,
        _ => unreachable!(),
    }
}

/// The tailored UAX #29 rule kernel, a direct port of uucode
/// `src/x/grapheme.zig` `computeGraphemeBreakNoControl` (GB3/4/5 compiled out; see
/// [`GraphemeBreakClass`](super::GraphemeBreakClass) for the tailorings). Total
/// over all `(gb1, gb2, state)` triples: invalid in-flight states are reset to
/// default before the rules run, which is what makes precomputation valid.
const fn compute_break(gb1: G, gb2: G, state: BreakState) -> (bool, BreakState) {
    let mut state = state;

    // Set state back to default when gb1 or gb2 is not expected in sequence.
    match state {
        BreakState::RegionalIndicator => {
            if !matches!(gb1, G::RegionalIndicator) || !matches!(gb2, G::RegionalIndicator) {
                state = BreakState::Default;
            }
        }
        BreakState::ExtendedPictographic => {
            if !is_ext_pict_sequence_member(gb1) || !is_ext_pict_sequence_member(gb2) {
                state = BreakState::Default;
            }
        }
        BreakState::IndicConjunctBreakConsonant | BreakState::IndicConjunctBreakLinker => {
            if !is_indic_sequence_member(gb1) || !is_indic_sequence_member(gb2) {
                state = BreakState::Default;
            }
        }
        BreakState::Default => {}
    }

    // GB3/GB4/GB5 (CR/LF/Control) are compiled out: those classes are collapsed to
    // Other in the property table and handled upstream of segmentation.

    // GB6: L x (L | V | LV | LVT)
    if matches!(gb1, G::L) && matches!(gb2, G::L | G::V | G::Lv | G::Lvt) {
        return (false, state);
    }

    // GB7: (LV | V) x (V | T)
    if matches!(gb1, G::Lv | G::V) && matches!(gb2, G::V | G::T) {
        return (false, state);
    }

    // GB8: (LVT | T) x T
    if matches!(gb1, G::Lvt | G::T) && matches!(gb2, G::T) {
        return (false, state);
    }

    // Handle GB9 (Extend | ZWJ) later, since it can also match the start of GB9c
    // (Indic) and GB11 (Emoji ZWJ).

    // GB9a: SpacingMark
    if matches!(gb2, G::SpacingMark) {
        return (false, state);
    }

    // GB9b: Prepend
    if matches!(gb1, G::Prepend) {
        return (false, state);
    }

    // GB9c: Indic
    if matches!(gb1, G::IndicConjunctBreakConsonant) {
        // Start of sequence. (In normal operation state is default here, but the
        // precomputation iterates all states.)
        if is_indic_extend(gb2) {
            return (false, BreakState::IndicConjunctBreakConsonant);
        } else if matches!(gb2, G::IndicConjunctBreakLinker) {
            // Jump straight to linker state.
            return (false, BreakState::IndicConjunctBreakLinker);
        }
        // else, not an Indic sequence
    } else if matches!(state, BreakState::IndicConjunctBreakConsonant) {
        if matches!(gb2, G::IndicConjunctBreakLinker) {
            // consonant -> linker transition
            return (false, BreakState::IndicConjunctBreakLinker);
        } else if is_indic_extend(gb2) {
            // continue [extend]* sequence
            return (false, state);
        } else {
            // Not a valid Indic sequence.
            state = BreakState::Default;
        }
    } else if matches!(state, BreakState::IndicConjunctBreakLinker) {
        if matches!(gb2, G::IndicConjunctBreakLinker) || is_indic_extend(gb2) {
            // continue [extend linker]* sequence
            return (false, state);
        } else if matches!(gb2, G::IndicConjunctBreakConsonant) {
            // linker -> end of sequence
            return (false, BreakState::Default);
        } else {
            // Not a valid Indic sequence.
            state = BreakState::Default;
        }
    }

    // GB11: Emoji ZWJ sequence and emoji modifier sequence.
    if is_ext_pict(gb1) {
        // Start of sequence (state is default in normal operation, see above).
        if is_extend(gb2) || matches!(gb2, G::Zwj) {
            return (false, BreakState::ExtendedPictographic);
        }

        // UTS #51 ED-13 emoji_modifier_sequence := emoji_modifier_base emoji_modifier.
        if matches!(gb1, G::EmojiModifierBase) && matches!(gb2, G::EmojiModifier) {
            return (false, BreakState::ExtendedPictographic);
        }

        // else, not an emoji ZWJ sequence
    } else if matches!(state, BreakState::ExtendedPictographic) {
        if (is_extend(gb1) || matches!(gb1, G::EmojiModifier))
            && (is_extend(gb2) || matches!(gb2, G::Zwj))
        {
            // continue extend* ZWJ sequence
            return (false, state);
        } else if matches!(gb1, G::Zwj) && is_ext_pict(gb2) {
            // ZWJ -> end of sequence
            return (false, BreakState::Default);
        } else {
            // Not a valid emoji ZWJ sequence.
            state = BreakState::Default;
        }
    }

    // GB12 and GB13: Regional Indicator (pairing parity via state toggle).
    if matches!(gb1, G::RegionalIndicator) && matches!(gb2, G::RegionalIndicator) {
        if matches!(state, BreakState::Default) {
            return (false, BreakState::RegionalIndicator);
        } else {
            return (true, BreakState::Default);
        }
    }

    // GB9: x (Extend | ZWJ)
    if is_extend(gb2) || matches!(gb2, G::Zwj) {
        return (false, state);
    }

    // GB999: otherwise, break everywhere.
    (true, state)
}

/// The classic UAX #29 `Extend` class, minus `EmojiModifier` (UTS #51 tailoring —
/// see the comment block in uucode `src/x/grapheme.zig:686-700`).
const fn is_extend(gb: G) -> bool {
    matches!(
        gb,
        G::Zwnj | G::IndicConjunctBreakExtend | G::IndicConjunctBreakLinker
    )
}

const fn is_ext_pict(gb: G) -> bool {
    matches!(gb, G::ExtendedPictographic | G::EmojiModifierBase)
}

/// GB9c "extend" (InCB Extend includes ZWJ).
const fn is_indic_extend(gb: G) -> bool {
    matches!(gb, G::IndicConjunctBreakExtend | G::Zwj)
}

/// Classes that keep an in-flight extended-pictographic state plausible.
const fn is_ext_pict_sequence_member(gb: G) -> bool {
    matches!(
        gb,
        G::IndicConjunctBreakExtend
            | G::IndicConjunctBreakLinker
            | G::Zwnj
            | G::Zwj
            | G::ExtendedPictographic
            | G::EmojiModifierBase
            | G::EmojiModifier
    )
}

/// Classes that keep an in-flight Indic conjunct state plausible.
const fn is_indic_sequence_member(gb: G) -> bool {
    matches!(
        gb,
        G::IndicConjunctBreakConsonant
            | G::IndicConjunctBreakLinker
            | G::IndicConjunctBreakExtend
            | G::Zwj
    )
}

#[cfg(test)]
mod tests {
    //! Ports of the inline tests in ghostty `src/unicode/grapheme.zig` and
    //! `src/terminal/c/unicode.zig`.

    use super::super::properties;
    use super::*;

    #[test]
    fn grapheme_break_emoji_modifier() {
        // Emoji modifier base and modifier.
        let mut state = BreakState::Default;
        assert!(!grapheme_break(0x261D, 0x1F3FF, &mut state));

        // Non-emoji and emoji modifier.
        let mut state = BreakState::Default;
        assert!(grapheme_break(0x22, 0x1F3FF, &mut state));
    }

    #[test]
    fn long_emoji_zwj_sequences() {
        // 👩‍👩‍👧‍👦 (family: woman, woman, girl, boy), then a break before '_'.
        let cps = [
            0x1F469, 0x200D, 0x1F469, 0x200D, 0x1F467, 0x200D, 0x1F466, '_' as u32,
        ];
        let mut state = BreakState::Default;
        for pair in cps.windows(2) {
            let expect_break = pair[1] == '_' as u32;
            assert_eq!(
                grapheme_break(pair[0], pair[1], &mut state),
                expect_break,
                "pair {pair:X?}"
            );
        }
    }

    #[test]
    fn grapheme_width_variation_selectors() {
        assert_eq!(
            grapheme_width_effect(0x2764, 0xFE0F),
            GraphemeWidthEffect::Wide
        );
        assert_eq!(
            grapheme_width_effect(0x23, 0xFE0E),
            GraphemeWidthEffect::Narrow
        );
        assert_eq!(
            grapheme_width_effect('x' as u32, 0xFE0F),
            GraphemeWidthEffect::Ignore
        );

        let gw = |cps: &[u32]| grapheme_width(cps);
        assert_eq!(gw(&[0x2764, 0xFE0F]), GraphemeWidth { len: 2, width: 2 });
        assert_eq!(gw(&[0x23, 0xFE0F]), GraphemeWidth { len: 2, width: 2 });
        assert_eq!(
            gw(&['x' as u32, 0xFE0F]),
            GraphemeWidth { len: 2, width: 1 }
        );
        assert_eq!(
            gw(&['x' as u32, 0xFE0F, 0xFE0F]),
            GraphemeWidth { len: 3, width: 1 }
        );
        assert_eq!(gw(&[0x23, 0xFE0E]), GraphemeWidth { len: 2, width: 1 });
        assert_eq!(gw(&[0x231A, 0xFE0E]), GraphemeWidth { len: 2, width: 1 });
        assert_eq!(
            gw(&[0x231A, 0xFE0E, 0xFE0F]),
            GraphemeWidth { len: 3, width: 1 }
        );
        assert_eq!(
            gw(&[0x1F3F4, 0x200D, 0x2620, 0xFE0F]),
            GraphemeWidth { len: 4, width: 2 }
        );
    }

    #[test]
    fn grapheme_width_emoji_sequences() {
        assert_eq!(
            grapheme_width(&[0x1F468, 0x200D, 0x1F469, 0x200D, 0x1F467]),
            GraphemeWidth { len: 5, width: 2 }
        );
        assert_eq!(
            grapheme_width(&[0x23, 0xFE0F, 0x20E3]),
            GraphemeWidth { len: 3, width: 2 }
        );
        assert_eq!(
            grapheme_width(&['1' as u32, 0x20E3]),
            GraphemeWidth { len: 2, width: 1 }
        );
        assert_eq!(
            grapheme_width(&[0x1F44B, 0x1F3FF]),
            GraphemeWidth { len: 2, width: 2 }
        );
    }

    #[test]
    fn grapheme_width_spacing_marks_can_widen_narrow_clusters() {
        // Find a codepoint that is width 1, contributes width in a grapheme, and
        // does not break after 'a' — i.e. a spacing mark.
        let mark = (0..0x110000u32).find(|&cp| {
            let props = properties(cp);
            if props.width != 1 || props.width_zero_in_grapheme {
                return false;
            }
            let mut state = BreakState::Default;
            !grapheme_break('a' as u32, cp, &mut state)
        });

        let cp = mark.expect("no spacing mark found");
        assert_eq!(properties(cp).width, 1);
        assert!(!properties(cp).width_zero_in_grapheme);
        assert_eq!(
            grapheme_width(&['a' as u32, cp]),
            GraphemeWidth { len: 2, width: 2 }
        );
    }

    #[test]
    fn grapheme_width_segmentation() {
        assert_eq!(
            grapheme_width(&['a' as u32]),
            GraphemeWidth { len: 1, width: 1 }
        );
        assert_eq!(
            grapheme_width(&['a' as u32, 'b' as u32]),
            GraphemeWidth { len: 1, width: 1 }
        );
        assert_eq!(
            grapheme_width(&[0x1F1E6, 0x1F1E7, 0x1F1E8]),
            GraphemeWidth { len: 2, width: 2 }
        );
        assert_eq!(
            grapheme_width(&[0x1F1E8]),
            GraphemeWidth { len: 1, width: 2 }
        );
        assert_eq!(grapheme_width(&[]), GraphemeWidth { len: 0, width: 0 });
        assert_eq!(
            grapheme_width(&[0x0301, 0x0302]),
            GraphemeWidth { len: 2, width: 0 }
        );
    }

    #[test]
    fn grapheme_width_u32_invalid_codepoints_stand_alone() {
        assert_eq!(
            grapheme_width(&[0x110000, 0x0301]),
            GraphemeWidth { len: 1, width: 1 }
        );
        assert_eq!(
            grapheme_width(&['a' as u32, 0x110000]),
            GraphemeWidth { len: 1, width: 1 }
        );
    }
}
