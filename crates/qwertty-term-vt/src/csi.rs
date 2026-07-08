//! CSI command enums. Port of `src/terminal/csi.zig` (55 lines, 0 inline tests).
//!
//! These are small, standalone enums used by the stream/terminal layer to interpret
//! CSI final-byte dispatch parameters (ED, EL, TBC modes, XTWINOPS report styles).
//! None of them own parsing logic here — the stream chunk maps raw CSI params onto
//! these types.

/// Modes for the ED (Erase in Display) CSI command. Port of `csi.zig` `EraseDisplay`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EraseDisplay {
    Below = 0,
    Above = 1,
    Complete = 2,
    Scrollback = 3,

    /// A Kitty extension: move the viewport into the scrollback and then
    /// erase the display.
    ScrollComplete = 22,
}

/// Modes for the EL (Erase in Line) CSI command. Port of `csi.zig` `EraseLine`.
///
/// The Zig type is a non-exhaustive `enum(u8)` (`_`) so that converting an
/// arbitrary user-supplied byte never fails. We model that with a plain `u8`
/// payload on an `Other` variant instead of a non-exhaustive `#[repr(u8)]`
/// enum (Rust has no direct equivalent); [`EraseLine::from_param`] is the
/// infallible constructor mirroring Zig's `@enumFromInt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EraseLine {
    Right,
    Left,
    Complete,
    RightUnlessPendingWrap,
    /// Any value not otherwise recognized (mirrors the Zig `_` catch-all).
    Other(u8),
}

impl EraseLine {
    /// Construct from a raw CSI parameter value. Infallible, matching Zig's
    /// non-exhaustive enum semantics (`@enumFromInt` never fails here since
    /// user input drives it).
    pub const fn from_param(value: u8) -> Self {
        match value {
            0 => Self::Right,
            1 => Self::Left,
            2 => Self::Complete,
            4 => Self::RightUnlessPendingWrap,
            other => Self::Other(other),
        }
    }
}

/// Modes for the TBC (Tab Clear) CSI command. Port of `csi.zig` `TabClear`.
///
/// Also non-exhaustive in the Zig source (see [`EraseLine`] doc for the
/// modeling note).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabClear {
    Current,
    All,
    /// Any value not otherwise recognized (mirrors the Zig `_` catch-all).
    Other(u8),
}

impl TabClear {
    /// Construct from a raw CSI parameter value. Infallible.
    pub const fn from_param(value: u8) -> Self {
        match value {
            0 => Self::Current,
            3 => Self::All,
            other => Self::Other(other),
        }
    }
}

/// Style formats for terminal size reports (XTWINOPS 14/16/18/21).
/// Port of `csi.zig` `SizeReportStyle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeReportStyle {
    /// `CSI 14 t` - text area size in pixels.
    Csi14T,
    /// `CSI 16 t` - character cell size in pixels.
    Csi16T,
    /// `CSI 18 t` - text area size in characters.
    Csi18T,
    /// `CSI 21 t` - window title report (not a size report, but shares the
    /// XTWINOPS dispatch table in the Zig source).
    Csi21T,
}

/// XTWINOPS CSI 22/23 (push/pop window title). Port of `csi.zig` `TitlePushPop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitlePushPop {
    pub op: TitlePushPopOp,
    pub index: u16,
}

/// The push/pop operation. Port of `csi.zig` `TitlePushPop.Op`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlePushPopOp {
    Push,
    Pop,
}

#[cfg(test)]
mod tests {
    use super::*;

    // No inline tests exist in csi.zig (grep '^test ' = 0 hits). These are
    // basic sanity checks for the infallible-conversion behavior we added
    // to model Zig's non-exhaustive enums, since that logic has no Zig
    // inline-test coverage to port 1:1.
    #[test]
    fn erase_line_from_param_known_and_other() {
        assert_eq!(EraseLine::from_param(0), EraseLine::Right);
        assert_eq!(EraseLine::from_param(1), EraseLine::Left);
        assert_eq!(EraseLine::from_param(2), EraseLine::Complete);
        assert_eq!(EraseLine::from_param(4), EraseLine::RightUnlessPendingWrap);
        assert_eq!(EraseLine::from_param(9), EraseLine::Other(9));
    }

    #[test]
    fn tab_clear_from_param_known_and_other() {
        assert_eq!(TabClear::from_param(0), TabClear::Current);
        assert_eq!(TabClear::from_param(3), TabClear::All);
        assert_eq!(TabClear::from_param(7), TabClear::Other(7));
    }
}
