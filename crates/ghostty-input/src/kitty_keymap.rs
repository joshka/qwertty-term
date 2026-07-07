//! Kitty keyboard protocol functional-key table (port of `input/kitty.zig`).
//!
//! This is a small, flat table mapping [`Key`] values to the kitty keyboard
//! protocol's functional-key codes, per:
//! <https://sw.kovidgoyal.net/kitty/keyboard-protocol/#functional-key-definitions>
//!
//! The exact table is ported from Foot:
//! <https://codeberg.org/dnkl/foot/src/branch/master/kitty-keymap.h>
//!
//! Entries are listed in the same order as the Kitty spec's table above, so
//! it's easy to compare against the upstream source when updating this list.

use crate::key::Key;

/// A single entry in the kitty keymap data. There are only ~100 entries so
/// the recommendation is to just use a linear search to find the entry for a
/// given key.
///
/// Port of `kitty.Entry`. The Zig struct's `final` field (the CSI final
/// byte) is renamed to `final_byte` here: `final` is a reserved keyword in
/// the Rust 2018+ edition (reserved for future use), so it cannot be used as
/// a plain field name without raw-identifier syntax (`r#final`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub key: Key,
    /// The Zig field is `u21`; Rust has no 21-bit integer type, so this is a
    /// `u32` with the valid range documented as `0..=0x1FFFFF` (any Unicode
    /// scalar value's codepoint).
    pub code: u32,
    /// The CSI final byte (e.g. `u`, `~`, `A`). Port of the Zig `final` field
    /// (renamed; see struct doc comment).
    pub final_byte: u8,
    pub modifier: bool,
}

/// The full list of entries for the current platform. Port of `kitty.entries`
/// (the Zig source builds this from `raw_entries` via a comptime `for` loop;
/// here we just write the structured form directly since Rust const-eval
/// doesn't need the raw-tuple indirection Zig used for "easy human
/// management").
///
/// Ported from Zig's `raw_entries` table, entry for entry, in the same
/// order.
pub const ENTRIES: &[Entry] = &[
    Entry {
        key: Key::Escape,
        code: 27,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Enter,
        code: 13,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Tab,
        code: 9,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Backspace,
        code: 127,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Insert,
        code: 2,
        final_byte: b'~',
        modifier: false,
    },
    Entry {
        key: Key::Delete,
        code: 3,
        final_byte: b'~',
        modifier: false,
    },
    Entry {
        key: Key::ArrowLeft,
        code: 1,
        final_byte: b'D',
        modifier: false,
    },
    Entry {
        key: Key::ArrowRight,
        code: 1,
        final_byte: b'C',
        modifier: false,
    },
    Entry {
        key: Key::ArrowUp,
        code: 1,
        final_byte: b'A',
        modifier: false,
    },
    Entry {
        key: Key::ArrowDown,
        code: 1,
        final_byte: b'B',
        modifier: false,
    },
    Entry {
        key: Key::PageUp,
        code: 5,
        final_byte: b'~',
        modifier: false,
    },
    Entry {
        key: Key::PageDown,
        code: 6,
        final_byte: b'~',
        modifier: false,
    },
    Entry {
        key: Key::Home,
        code: 1,
        final_byte: b'H',
        modifier: false,
    },
    Entry {
        key: Key::End,
        code: 1,
        final_byte: b'F',
        modifier: false,
    },
    Entry {
        key: Key::CapsLock,
        code: 57358,
        final_byte: b'u',
        modifier: true,
    },
    Entry {
        key: Key::ScrollLock,
        code: 57359,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumLock,
        code: 57360,
        final_byte: b'u',
        modifier: true,
    },
    Entry {
        key: Key::PrintScreen,
        code: 57361,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Pause,
        code: 57362,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::F1,
        code: 1,
        final_byte: b'P',
        modifier: false,
    },
    Entry {
        key: Key::F2,
        code: 1,
        final_byte: b'Q',
        modifier: false,
    },
    Entry {
        key: Key::F3,
        code: 13,
        final_byte: b'~',
        modifier: false,
    },
    Entry {
        key: Key::F4,
        code: 1,
        final_byte: b'S',
        modifier: false,
    },
    Entry {
        key: Key::F5,
        code: 15,
        final_byte: b'~',
        modifier: false,
    },
    Entry {
        key: Key::F6,
        code: 17,
        final_byte: b'~',
        modifier: false,
    },
    Entry {
        key: Key::F7,
        code: 18,
        final_byte: b'~',
        modifier: false,
    },
    Entry {
        key: Key::F8,
        code: 19,
        final_byte: b'~',
        modifier: false,
    },
    Entry {
        key: Key::F9,
        code: 20,
        final_byte: b'~',
        modifier: false,
    },
    Entry {
        key: Key::F10,
        code: 21,
        final_byte: b'~',
        modifier: false,
    },
    Entry {
        key: Key::F11,
        code: 23,
        final_byte: b'~',
        modifier: false,
    },
    Entry {
        key: Key::F12,
        code: 24,
        final_byte: b'~',
        modifier: false,
    },
    Entry {
        key: Key::F13,
        code: 57376,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::F14,
        code: 57377,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::F15,
        code: 57378,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::F16,
        code: 57379,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::F17,
        code: 57380,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::F18,
        code: 57381,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::F19,
        code: 57382,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::F20,
        code: 57383,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::F21,
        code: 57384,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::F22,
        code: 57385,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::F23,
        code: 57386,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::F24,
        code: 57387,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::F25,
        code: 57388,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Numpad0,
        code: 57399,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Numpad1,
        code: 57400,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Numpad2,
        code: 57401,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Numpad3,
        code: 57402,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Numpad4,
        code: 57403,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Numpad5,
        code: 57404,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Numpad6,
        code: 57405,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Numpad7,
        code: 57406,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Numpad8,
        code: 57407,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::Numpad9,
        code: 57408,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadDecimal,
        code: 57409,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadDivide,
        code: 57410,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadMultiply,
        code: 57411,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadSubtract,
        code: 57412,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadAdd,
        code: 57413,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadEnter,
        code: 57414,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadEqual,
        code: 57415,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadSeparator,
        code: 57416,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadLeft,
        code: 57417,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadRight,
        code: 57418,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadUp,
        code: 57419,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadDown,
        code: 57420,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadPageUp,
        code: 57421,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadPageDown,
        code: 57422,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadHome,
        code: 57423,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadEnd,
        code: 57424,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadInsert,
        code: 57425,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadDelete,
        code: 57426,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::NumpadBegin,
        code: 57427,
        final_byte: b'u',
        modifier: false,
    },
    Entry {
        key: Key::ShiftLeft,
        code: 57441,
        final_byte: b'u',
        modifier: true,
    },
    Entry {
        key: Key::ShiftRight,
        code: 57447,
        final_byte: b'u',
        modifier: true,
    },
    Entry {
        key: Key::ControlLeft,
        code: 57442,
        final_byte: b'u',
        modifier: true,
    },
    Entry {
        key: Key::ControlRight,
        code: 57448,
        final_byte: b'u',
        modifier: true,
    },
    Entry {
        key: Key::MetaLeft,
        code: 57444,
        final_byte: b'u',
        modifier: true,
    },
    Entry {
        key: Key::MetaRight,
        code: 57450,
        final_byte: b'u',
        modifier: true,
    },
    Entry {
        key: Key::AltLeft,
        code: 57443,
        final_byte: b'u',
        modifier: true,
    },
    Entry {
        key: Key::AltRight,
        code: 57449,
        final_byte: b'u',
        modifier: true,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    // Adapted from the Zig anonymous `test { _ = entries; }`, which exists
    // only to force comptime evaluation of `entries` (Zig comptime blocks
    // are lazily evaluated unless referenced). Rust has no comptime-forcing
    // equivalent (this table is a plain `const` evaluated eagerly), so
    // instead this asserts a real, meaningful property: the exact entry
    // count, matching the upstream Zig `raw_entries` table 1:1.
    #[test]
    fn entries_len_matches_zig_raw_entries() {
        assert_eq!(ENTRIES.len(), 81);
    }

    #[test]
    fn entries_are_well_formed() {
        // Sanity check: every entry's final byte is one of the CSI finals
        // used by the kitty protocol for functional keys.
        for entry in ENTRIES {
            assert!(matches!(
                entry.final_byte,
                b'u' | b'~' | b'A' | b'B' | b'C' | b'D' | b'H' | b'F' | b'P' | b'Q' | b'S'
            ));
            assert!(entry.code > 0);
        }
    }

    #[test]
    fn arrow_up_entry_matches_kitty_spec() {
        let entry = ENTRIES
            .iter()
            .find(|e| e.key == Key::ArrowUp)
            .expect("ArrowUp entry present");
        assert_eq!(entry.code, 1);
        assert_eq!(entry.final_byte, b'A');
        assert!(!entry.modifier);
    }

    #[test]
    fn caps_lock_entry_is_modifier() {
        let entry = ENTRIES
            .iter()
            .find(|e| e.key == Key::CapsLock)
            .expect("CapsLock entry present");
        assert_eq!(entry.code, 57358);
        assert!(entry.modifier);
    }
}
