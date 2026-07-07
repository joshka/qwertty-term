//! Mouse input primitives (port of `input/mouse.zig`).
//!
//! Ported types: [`Action`], [`ButtonState`], [`Button`], [`Momentum`],
//! [`PressureStage`], and [`ScrollMods`].

/// The type of action associated with a mouse event. This is different
/// from [`ButtonState`] because button state is simply the current state
/// of a mouse button but an action is something that triggers via
/// an GUI event and supports more.
///
/// Port of `Action` (`enum(c_int) { press, release, motion }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    Motion,
}

/// The state of a mouse button.
///
/// In Zig this is backed by a `c_int` so it can be used as-is for the
/// embedding API.
///
/// IMPORTANT: Any changes here update `include/ghostty.h`.
///
/// Port of `ButtonState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Release,
    Press,
}

/// Possible mouse buttons. We only track up to 11 because that's the maximum
/// button input that terminal mouse tracking handles without becoming
/// ambiguous.
///
/// It's a bit silly to name numbers like this but given its a restricted
/// set, it feels better than passing around raw numeric literals.
///
/// In Zig this is backed by a `c_int` so it can be used as-is for the
/// embedding API.
///
/// IMPORTANT: Any changes here update `include/ghostty.h`.
///
/// Port of `Button`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Unknown = 0,
    Left = 1,
    Right = 2,
    Middle = 3,
    Four = 4,
    Five = 5,
    Six = 6,
    Seven = 7,
    Eight = 8,
    Nine = 9,
    Ten = 10,
    Eleven = 11,
}

impl Button {
    /// The maximum value in this enum. This can be used to create a densely
    /// packed array, for example.
    ///
    /// Port of `Button.max`, which in Zig is computed at comptime by
    /// scanning `@typeInfo(Self).@"enum".fields` for the largest value.
    /// Since the field values above are hardcoded and known, we hardcode
    /// the equivalent here rather than reimplementing comptime reflection.
    pub const MAX: i32 = Button::Eleven as i32;
}

/// The "momentum" of a mouse scroll event. This matches the macOS events
/// because it is the only reliable source right now of momentum events.
/// This is used to handle "inertial scrolling" (i.e. flicking).
///
/// <https://developer.apple.com/documentation/appkit/nseventphase>
///
/// Port of `Momentum` (`enum(u3)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Momentum {
    #[default]
    None = 0,
    Began = 1,
    Stationary = 2,
    Changed = 3,
    Ended = 4,
    Cancelled = 5,
    MayBegin = 6,
}

impl Momentum {
    /// Convert to the 3-bit representation used by [`ScrollMods::as_u8`].
    pub const fn as_u3(self) -> u8 {
        self as u8
    }
}

/// The pressure stage of a pressure-sensitive input device.
///
/// This currently only supports the stages that macOS supports.
///
/// Port of `PressureStage` (`enum(u2)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PressureStage {
    /// The input device is unpressed.
    #[default]
    None = 0,

    /// The input device is pressed a normal amount. On macOS trackpads,
    /// this is after a "click".
    Normal = 1,

    /// The input device is pressed a deep amount. On macOS trackpads,
    /// this is after a "force click".
    Deep = 2,
}

/// The bitmask for mods for scroll events.
///
/// Port of `ScrollMods`, which in Zig is a `packed struct(u8)` with an
/// explicit `_padding: u4` field to round the struct out to a full byte.
/// We drop the padding field here since it is a Zig packed-struct
/// implementation detail with no semantic meaning; the plain struct below
/// carries the same two meaningful fields. [`ScrollMods::as_u8`] reproduces
/// the intended bit layout (`precision` in bit 0, `momentum` in bits 1..=3,
/// padding bits left as 0) for tests and any callers that need the wire
/// representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollMods {
    /// True if this is a high-precision scroll event. For example, Apple
    /// devices such as Magic Mouse, trackpads, etc. are high-precision
    /// and send very detailed scroll events.
    pub precision: bool,

    /// The momentum phase (if available, supported) of the scroll event.
    /// This is used to handle "inertial scrolling" (i.e. flicking).
    pub momentum: Momentum,
}

impl ScrollMods {
    /// Returns the `u8` bit layout that the Zig `packed struct(u8)` would
    /// produce via `@bitCast`: bit 0 is `precision`, bits 1..=3 are
    /// `momentum`, and bits 4..=7 are the (always-zero) padding.
    pub const fn as_u8(self) -> u8 {
        (self.precision as u8) | (self.momentum.as_u3() << 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of the anonymous `test { ... }` block in `ScrollMods`, which
    // checked `@bitCast(ScrollMods{})` and
    // `@bitCast(ScrollMods{ .precision = true })` against expected `u8`
    // values. We dropped the packed-struct `_padding` field (see the
    // doc comment on `ScrollMods`), so instead of `@bitCast` we exercise
    // the equivalent semantics via `ScrollMods::as_u8`.
    #[test]
    fn scroll_mods_bit_layout() {
        assert_eq!(ScrollMods::default().as_u8(), 0b0);
        assert_eq!(
            ScrollMods {
                precision: true,
                ..Default::default()
            }
            .as_u8(),
            0b0000_0001
        );
    }
}
