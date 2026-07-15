//! macOS native (Carbon `kVK_*`) virtual-keycode → [`Key`] table.
//!
//! Upstream does NOT do this mapping in Swift: `NSEvent+Extension.swift`
//! (`ghosttyKeyEvent`) passes the raw `event.keyCode` straight through the C
//! API as `key_ev.keycode`, and libghostty resolves it on the Zig side in
//! `apprt/embedded.zig` (`KeyEvent.core`) by scanning `input.keycodes.entries`
//! for the entry whose `.native` column matches, else `.unidentified`.
//!
//! This crate is freestanding and does not port the full `keycodes.zig`
//! cross-platform table (which carries USB/evdev/XKB/Win columns this crate
//! has no use for). Instead, this module bakes the macOS column of that table
//! into a direct keycode → [`Key`] lookup so a native AppKit host (R5) can
//! supply the physical [`Key`] itself without re-deriving the table.
//!
//! The table is transcribed 1:1 from the `mac` column (`native_idx = 4`) of
//! upstream `src/input/keycodes.zig` `raw_entries` (Ghostty commit
//! `2da015cd6` / `38e49a23`), covering every entry with a real macOS keycode
//! (i.e. `mac != 0xffff`) whose W3C DOM code maps to a [`Key`] variant this
//! crate defines. This supersedes and completes the partial map that lived in
//! `spikes/appkit-input/src/keymap.rs` (finding #3 of `docs/analysis/
//! appkit-input.md`).
//!
//! ## Coverage
//!
//! Full printable (letters, digits, punctuation), navigation (arrows, home/
//! end/page/insert/delete), function keys F1–F20, the full numpad, modifiers
//! (left/right control/shift/alt/meta), lock keys, and the international keys
//! (`IntlBackslash`/`IntlRo`/`IntlYen`) and media volume keys that macOS emits
//! keycodes for. See [`ENTRIES`] for the full list.
//!
//! Two upstream entries — `Lang1`/`Lang2` (macOS `0x68`/`0x66`, Japanese/
//! Korean input-source toggles) — have macOS keycodes but no corresponding
//! [`Key`] variant in this crate (matching `key.zig`, which likewise has no
//! `lang1`/`lang2`), so they are intentionally absent and fall through to
//! [`Key::Unidentified`], exactly as upstream's `code_to_key.get(...) orelse
//! .unidentified` would.

use crate::key::Key;

/// A single macOS-keycode → [`Key`] table entry. Port of the relevant columns
/// of `keycodes.zig`'s `Entry` (only `native` + `key` are kept; the USB HID
/// code and W3C DOM code string are dropped since this crate resolves the
/// physical key by native keycode alone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// The macOS native virtual keycode (`NSEvent.keyCode`, i.e. Carbon
    /// `kVK_*`).
    pub native: u16,
    /// The layout-independent physical key.
    pub key: Key,
}

/// The full macOS keycode table, in the order upstream's `raw_entries` lists
/// them. Consumers should prefer [`key_from_macos_keycode`] for lookups; this
/// slice is exported so hosts that want to iterate the table (e.g. building a
/// reverse map, or asserting coverage) have a single source of truth.
pub const ENTRIES: &[Entry] = &[
    Entry {
        native: 0x00,
        key: Key::KeyA,
    },
    Entry {
        native: 0x0B,
        key: Key::KeyB,
    },
    Entry {
        native: 0x08,
        key: Key::KeyC,
    },
    Entry {
        native: 0x02,
        key: Key::KeyD,
    },
    Entry {
        native: 0x0E,
        key: Key::KeyE,
    },
    Entry {
        native: 0x03,
        key: Key::KeyF,
    },
    Entry {
        native: 0x05,
        key: Key::KeyG,
    },
    Entry {
        native: 0x04,
        key: Key::KeyH,
    },
    Entry {
        native: 0x22,
        key: Key::KeyI,
    },
    Entry {
        native: 0x26,
        key: Key::KeyJ,
    },
    Entry {
        native: 0x28,
        key: Key::KeyK,
    },
    Entry {
        native: 0x25,
        key: Key::KeyL,
    },
    Entry {
        native: 0x2E,
        key: Key::KeyM,
    },
    Entry {
        native: 0x2D,
        key: Key::KeyN,
    },
    Entry {
        native: 0x1F,
        key: Key::KeyO,
    },
    Entry {
        native: 0x23,
        key: Key::KeyP,
    },
    Entry {
        native: 0x0C,
        key: Key::KeyQ,
    },
    Entry {
        native: 0x0F,
        key: Key::KeyR,
    },
    Entry {
        native: 0x01,
        key: Key::KeyS,
    },
    Entry {
        native: 0x11,
        key: Key::KeyT,
    },
    Entry {
        native: 0x20,
        key: Key::KeyU,
    },
    Entry {
        native: 0x09,
        key: Key::KeyV,
    },
    Entry {
        native: 0x0D,
        key: Key::KeyW,
    },
    Entry {
        native: 0x07,
        key: Key::KeyX,
    },
    Entry {
        native: 0x10,
        key: Key::KeyY,
    },
    Entry {
        native: 0x06,
        key: Key::KeyZ,
    },
    Entry {
        native: 0x12,
        key: Key::Digit1,
    },
    Entry {
        native: 0x13,
        key: Key::Digit2,
    },
    Entry {
        native: 0x14,
        key: Key::Digit3,
    },
    Entry {
        native: 0x15,
        key: Key::Digit4,
    },
    Entry {
        native: 0x17,
        key: Key::Digit5,
    },
    Entry {
        native: 0x16,
        key: Key::Digit6,
    },
    Entry {
        native: 0x1A,
        key: Key::Digit7,
    },
    Entry {
        native: 0x1C,
        key: Key::Digit8,
    },
    Entry {
        native: 0x19,
        key: Key::Digit9,
    },
    Entry {
        native: 0x1D,
        key: Key::Digit0,
    },
    Entry {
        native: 0x24,
        key: Key::Enter,
    },
    Entry {
        native: 0x35,
        key: Key::Escape,
    },
    Entry {
        native: 0x33,
        key: Key::Backspace,
    },
    Entry {
        native: 0x30,
        key: Key::Tab,
    },
    Entry {
        native: 0x31,
        key: Key::Space,
    },
    Entry {
        native: 0x1B,
        key: Key::Minus,
    },
    Entry {
        native: 0x18,
        key: Key::Equal,
    },
    Entry {
        native: 0x21,
        key: Key::BracketLeft,
    },
    Entry {
        native: 0x1E,
        key: Key::BracketRight,
    },
    Entry {
        native: 0x2A,
        key: Key::Backslash,
    },
    Entry {
        native: 0x29,
        key: Key::Semicolon,
    },
    Entry {
        native: 0x27,
        key: Key::Quote,
    },
    Entry {
        native: 0x32,
        key: Key::Backquote,
    },
    Entry {
        native: 0x2B,
        key: Key::Comma,
    },
    Entry {
        native: 0x2F,
        key: Key::Period,
    },
    Entry {
        native: 0x2C,
        key: Key::Slash,
    },
    Entry {
        native: 0x39,
        key: Key::CapsLock,
    },
    Entry {
        native: 0x7A,
        key: Key::F1,
    },
    Entry {
        native: 0x78,
        key: Key::F2,
    },
    Entry {
        native: 0x63,
        key: Key::F3,
    },
    Entry {
        native: 0x76,
        key: Key::F4,
    },
    Entry {
        native: 0x60,
        key: Key::F5,
    },
    Entry {
        native: 0x61,
        key: Key::F6,
    },
    Entry {
        native: 0x62,
        key: Key::F7,
    },
    Entry {
        native: 0x64,
        key: Key::F8,
    },
    Entry {
        native: 0x65,
        key: Key::F9,
    },
    Entry {
        native: 0x6D,
        key: Key::F10,
    },
    Entry {
        native: 0x67,
        key: Key::F11,
    },
    Entry {
        native: 0x6F,
        key: Key::F12,
    },
    Entry {
        native: 0x72,
        key: Key::Insert,
    },
    Entry {
        native: 0x73,
        key: Key::Home,
    },
    Entry {
        native: 0x74,
        key: Key::PageUp,
    },
    Entry {
        native: 0x75,
        key: Key::Delete,
    },
    Entry {
        native: 0x77,
        key: Key::End,
    },
    Entry {
        native: 0x79,
        key: Key::PageDown,
    },
    Entry {
        native: 0x7C,
        key: Key::ArrowRight,
    },
    Entry {
        native: 0x7B,
        key: Key::ArrowLeft,
    },
    Entry {
        native: 0x7D,
        key: Key::ArrowDown,
    },
    Entry {
        native: 0x7E,
        key: Key::ArrowUp,
    },
    Entry {
        native: 0x47,
        key: Key::NumLock,
    },
    Entry {
        native: 0x4B,
        key: Key::NumpadDivide,
    },
    Entry {
        native: 0x43,
        key: Key::NumpadMultiply,
    },
    Entry {
        native: 0x4E,
        key: Key::NumpadSubtract,
    },
    Entry {
        native: 0x45,
        key: Key::NumpadAdd,
    },
    Entry {
        native: 0x4C,
        key: Key::NumpadEnter,
    },
    Entry {
        native: 0x53,
        key: Key::Numpad1,
    },
    Entry {
        native: 0x54,
        key: Key::Numpad2,
    },
    Entry {
        native: 0x55,
        key: Key::Numpad3,
    },
    Entry {
        native: 0x56,
        key: Key::Numpad4,
    },
    Entry {
        native: 0x57,
        key: Key::Numpad5,
    },
    Entry {
        native: 0x58,
        key: Key::Numpad6,
    },
    Entry {
        native: 0x59,
        key: Key::Numpad7,
    },
    Entry {
        native: 0x5B,
        key: Key::Numpad8,
    },
    Entry {
        native: 0x5C,
        key: Key::Numpad9,
    },
    Entry {
        native: 0x52,
        key: Key::Numpad0,
    },
    Entry {
        native: 0x41,
        key: Key::NumpadDecimal,
    },
    Entry {
        native: 0x0A,
        key: Key::IntlBackslash,
    },
    Entry {
        native: 0x6E,
        key: Key::ContextMenu,
    },
    Entry {
        native: 0x51,
        key: Key::NumpadEqual,
    },
    Entry {
        native: 0x69,
        key: Key::F13,
    },
    Entry {
        native: 0x6B,
        key: Key::F14,
    },
    Entry {
        native: 0x71,
        key: Key::F15,
    },
    Entry {
        native: 0x6A,
        key: Key::F16,
    },
    Entry {
        native: 0x40,
        key: Key::F17,
    },
    Entry {
        native: 0x4F,
        key: Key::F18,
    },
    Entry {
        native: 0x50,
        key: Key::F19,
    },
    Entry {
        native: 0x5A,
        key: Key::F20,
    },
    Entry {
        native: 0x4A,
        key: Key::AudioVolumeMute,
    },
    Entry {
        native: 0x48,
        key: Key::AudioVolumeUp,
    },
    Entry {
        native: 0x49,
        key: Key::AudioVolumeDown,
    },
    Entry {
        native: 0x5F,
        key: Key::NumpadComma,
    },
    Entry {
        native: 0x5E,
        key: Key::IntlRo,
    },
    Entry {
        native: 0x5D,
        key: Key::IntlYen,
    },
    Entry {
        native: 0x3B,
        key: Key::ControlLeft,
    },
    Entry {
        native: 0x38,
        key: Key::ShiftLeft,
    },
    Entry {
        native: 0x3A,
        key: Key::AltLeft,
    },
    Entry {
        native: 0x37,
        key: Key::MetaLeft,
    },
    Entry {
        native: 0x3E,
        key: Key::ControlRight,
    },
    Entry {
        native: 0x3C,
        key: Key::ShiftRight,
    },
    Entry {
        native: 0x3D,
        key: Key::AltRight,
    },
    Entry {
        native: 0x36,
        key: Key::MetaRight,
    },
];

/// Map a macOS native virtual keycode (`NSEvent.keyCode`, i.e. Carbon
/// `kVK_*`) to a layout-independent [`Key`]. Returns [`Key::Unidentified`]
/// for keycodes not in the table, mirroring upstream's
/// `code_to_key.get(...) orelse .unidentified` fallthrough.
pub fn key_from_macos_keycode(keycode: u16) -> Key {
    match keycode {
        0x00 => Key::KeyA,
        0x0B => Key::KeyB,
        0x08 => Key::KeyC,
        0x02 => Key::KeyD,
        0x0E => Key::KeyE,
        0x03 => Key::KeyF,
        0x05 => Key::KeyG,
        0x04 => Key::KeyH,
        0x22 => Key::KeyI,
        0x26 => Key::KeyJ,
        0x28 => Key::KeyK,
        0x25 => Key::KeyL,
        0x2E => Key::KeyM,
        0x2D => Key::KeyN,
        0x1F => Key::KeyO,
        0x23 => Key::KeyP,
        0x0C => Key::KeyQ,
        0x0F => Key::KeyR,
        0x01 => Key::KeyS,
        0x11 => Key::KeyT,
        0x20 => Key::KeyU,
        0x09 => Key::KeyV,
        0x0D => Key::KeyW,
        0x07 => Key::KeyX,
        0x10 => Key::KeyY,
        0x06 => Key::KeyZ,
        0x12 => Key::Digit1,
        0x13 => Key::Digit2,
        0x14 => Key::Digit3,
        0x15 => Key::Digit4,
        0x17 => Key::Digit5,
        0x16 => Key::Digit6,
        0x1A => Key::Digit7,
        0x1C => Key::Digit8,
        0x19 => Key::Digit9,
        0x1D => Key::Digit0,
        0x24 => Key::Enter,
        0x35 => Key::Escape,
        0x33 => Key::Backspace,
        0x30 => Key::Tab,
        0x31 => Key::Space,
        0x1B => Key::Minus,
        0x18 => Key::Equal,
        0x21 => Key::BracketLeft,
        0x1E => Key::BracketRight,
        0x2A => Key::Backslash,
        0x29 => Key::Semicolon,
        0x27 => Key::Quote,
        0x32 => Key::Backquote,
        0x2B => Key::Comma,
        0x2F => Key::Period,
        0x2C => Key::Slash,
        0x39 => Key::CapsLock,
        0x7A => Key::F1,
        0x78 => Key::F2,
        0x63 => Key::F3,
        0x76 => Key::F4,
        0x60 => Key::F5,
        0x61 => Key::F6,
        0x62 => Key::F7,
        0x64 => Key::F8,
        0x65 => Key::F9,
        0x6D => Key::F10,
        0x67 => Key::F11,
        0x6F => Key::F12,
        0x72 => Key::Insert,
        0x73 => Key::Home,
        0x74 => Key::PageUp,
        0x75 => Key::Delete,
        0x77 => Key::End,
        0x79 => Key::PageDown,
        0x7C => Key::ArrowRight,
        0x7B => Key::ArrowLeft,
        0x7D => Key::ArrowDown,
        0x7E => Key::ArrowUp,
        0x47 => Key::NumLock,
        0x4B => Key::NumpadDivide,
        0x43 => Key::NumpadMultiply,
        0x4E => Key::NumpadSubtract,
        0x45 => Key::NumpadAdd,
        0x4C => Key::NumpadEnter,
        0x53 => Key::Numpad1,
        0x54 => Key::Numpad2,
        0x55 => Key::Numpad3,
        0x56 => Key::Numpad4,
        0x57 => Key::Numpad5,
        0x58 => Key::Numpad6,
        0x59 => Key::Numpad7,
        0x5B => Key::Numpad8,
        0x5C => Key::Numpad9,
        0x52 => Key::Numpad0,
        0x41 => Key::NumpadDecimal,
        0x0A => Key::IntlBackslash,
        0x6E => Key::ContextMenu,
        0x51 => Key::NumpadEqual,
        0x69 => Key::F13,
        0x6B => Key::F14,
        0x71 => Key::F15,
        0x6A => Key::F16,
        0x40 => Key::F17,
        0x4F => Key::F18,
        0x50 => Key::F19,
        0x5A => Key::F20,
        0x4A => Key::AudioVolumeMute,
        0x48 => Key::AudioVolumeUp,
        0x49 => Key::AudioVolumeDown,
        0x5F => Key::NumpadComma,
        0x5E => Key::IntlRo,
        0x5D => Key::IntlYen,
        0x3B => Key::ControlLeft,
        0x38 => Key::ShiftLeft,
        0x3A => Key::AltLeft,
        0x37 => Key::MetaLeft,
        0x3E => Key::ControlRight,
        0x3C => Key::ShiftRight,
        0x3D => Key::AltRight,
        0x36 => Key::MetaRight,
        _ => Key::Unidentified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_letters_and_common_keys() {
        assert_eq!(key_from_macos_keycode(0x00), Key::KeyA);
        assert_eq!(key_from_macos_keycode(0x08), Key::KeyC);
        assert_eq!(key_from_macos_keycode(0x0E), Key::KeyE);
        assert_eq!(key_from_macos_keycode(0x09), Key::KeyV);
        assert_eq!(key_from_macos_keycode(0x24), Key::Enter);
        assert_eq!(key_from_macos_keycode(0x35), Key::Escape);
        assert_eq!(key_from_macos_keycode(0x7E), Key::ArrowUp);
    }

    #[test]
    fn maps_function_and_numpad_keys() {
        assert_eq!(key_from_macos_keycode(0x7A), Key::F1);
        assert_eq!(key_from_macos_keycode(0x5A), Key::F20);
        assert_eq!(key_from_macos_keycode(0x52), Key::Numpad0);
        assert_eq!(key_from_macos_keycode(0x4C), Key::NumpadEnter);
        assert_eq!(key_from_macos_keycode(0x41), Key::NumpadDecimal);
    }

    #[test]
    fn maps_modifiers_left_and_right() {
        assert_eq!(key_from_macos_keycode(0x3B), Key::ControlLeft);
        assert_eq!(key_from_macos_keycode(0x3E), Key::ControlRight);
        assert_eq!(key_from_macos_keycode(0x38), Key::ShiftLeft);
        assert_eq!(key_from_macos_keycode(0x3C), Key::ShiftRight);
        assert_eq!(key_from_macos_keycode(0x3A), Key::AltLeft);
        assert_eq!(key_from_macos_keycode(0x3D), Key::AltRight);
        assert_eq!(key_from_macos_keycode(0x37), Key::MetaLeft);
        assert_eq!(key_from_macos_keycode(0x36), Key::MetaRight);
    }

    #[test]
    fn unknown_keycode_is_unidentified() {
        // 0xFF is not a real macOS keycode; Lang1 (0x68) / Lang2 (0x66) have
        // no Key variant in this crate so they also fall through.
        assert_eq!(key_from_macos_keycode(0xFF), Key::Unidentified);
        assert_eq!(key_from_macos_keycode(0x68), Key::Unidentified);
        assert_eq!(key_from_macos_keycode(0x66), Key::Unidentified);
    }

    /// The `ENTRIES` table and the `key_from_macos_keycode` match arms must
    /// agree exactly (they are the same data expressed two ways).
    #[test]
    fn entries_agree_with_lookup_fn() {
        for e in ENTRIES {
            assert_eq!(
                key_from_macos_keycode(e.native),
                e.key,
                "mismatch for keycode {:#04x}",
                e.native
            );
        }
        // No duplicate keycodes in the table.
        for (i, a) in ENTRIES.iter().enumerate() {
            for b in &ENTRIES[i + 1..] {
                assert_ne!(a.native, b.native, "duplicate keycode {:#04x}", a.native);
            }
        }
    }
}
