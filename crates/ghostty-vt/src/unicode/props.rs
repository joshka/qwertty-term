//! Per-codepoint property set (port of ghostty `src/unicode/props.zig`).

/// Grapheme cluster break class, with UAX #29 `Control`/`CR`/`LF` collapsed into
/// [`Other`](Self::Other) — the terminal filters control characters before
/// segmentation runs. Mirror of uucode's `GraphemeBreakNoControl`
/// (`uucode src/x/types_x/grapheme.zig`); discriminants are load-bearing (they index
/// the precomputed break transition table and match the generated `tables.rs`).
///
/// Extensions over stock UAX #29 `Grapheme_Cluster_Break`:
///
/// - `Extend` is split into [`Zwnj`](Self::Zwnj),
///   [`IndicConjunctBreakExtend`](Self::IndicConjunctBreakExtend), and
///   [`IndicConjunctBreakLinker`](Self::IndicConjunctBreakLinker) (for GB9c), and
///   [`EmojiModifier`](Self::EmojiModifier) is carved out of it (UTS #51 tailoring:
///   a skin-tone modifier only continues a cluster after an emoji modifier base).
/// - `Other` is split to carve out [`ExtendedPictographic`](Self::ExtendedPictographic),
///   [`EmojiModifierBase`](Self::EmojiModifierBase), and
///   [`IndicConjunctBreakConsonant`](Self::IndicConjunctBreakConsonant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum GraphemeBreakClass {
    Other = 0,
    Prepend = 1,
    RegionalIndicator = 2,
    SpacingMark = 3,
    L = 4,
    V = 5,
    T = 6,
    Lv = 7,
    Lvt = 8,
    Zwj = 9,
    Zwnj = 10,
    ExtendedPictographic = 11,
    EmojiModifierBase = 12,
    EmojiModifier = 13,
    IndicConjunctBreakExtend = 14,
    IndicConjunctBreakLinker = 15,
    IndicConjunctBreakConsonant = 16,
}

impl GraphemeBreakClass {
    /// Number of classes (used to size the precomputed transition table).
    pub(crate) const COUNT: usize = 17;
}

/// Properties ghostty precomputes per codepoint (ghostty `src/unicode/props.zig`).
///
/// Kept intentionally small: every field addition makes the multi-stage lookup
/// table less compressible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Properties {
    /// Terminal display width in cells, clamped to `0..=2` (three-em dash renders
    /// as two cells). 0 covers controls, surrogates, line/paragraph separators,
    /// default-ignorables, and combining marks.
    pub width: u8,

    /// Whether the codepoint does not contribute to the width of a grapheme
    /// cluster it continues (not consulted for single-codepoint cells).
    pub width_zero_in_grapheme: bool,

    /// Grapheme break property (control classes pre-collapsed; see
    /// [`GraphemeBreakClass`]).
    pub grapheme_break: GraphemeBreakClass,

    /// Whether this codepoint is a valid base for VS15/VS16 emoji variation
    /// sequences (per `emoji-variation-sequences.txt`).
    pub emoji_vs_base: bool,
}

impl Properties {
    /// Const constructor used by the generated `tables.rs`.
    pub const fn new(
        width: u8,
        width_zero_in_grapheme: bool,
        grapheme_break: GraphemeBreakClass,
        emoji_vs_base: bool,
    ) -> Self {
        Self {
            width,
            width_zero_in_grapheme,
            grapheme_break,
            emoji_vs_base,
        }
    }
}
