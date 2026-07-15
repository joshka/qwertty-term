//! Minimal macOS native (Carbon `kVK_*`) keycode -> [`qwertty_term_input::key::Key`]
//! map.
//!
//! Upstream does NOT do this mapping in Swift: `NSEvent+Extension.swift`
//! (`ghosttyKeyEvent`) passes the raw `event.keyCode` straight through the C API
//! as `key_ev.keycode`, and libghostty resolves it on the Zig side in
//! `apprt/embedded.zig` (`KeyEvent.core`) by scanning `input.keycodes.entries`
//! for the entry whose `.native` column matches, else `.unidentified`.
//!
//! Our `qwertty-term-input` crate is freestanding and (deliberately) does not port
//! that `keycodes.zig` native-keycode table, so a real AppKit host has to supply
//! the physical `Key` itself. This module is that map, transcribed from the
//! macOS (`native_idx = 4`) column of upstream `src/input/keycodes.zig`
//! `raw_entries` (Ghostty commit `38e49a23`). It is deliberately partial: only
//! the keys exercised by the spike verification matrix plus the common ASCII /
//! navigation set. R5's real host must complete it (or port `keycodes.zig`
//! wholesale into `qwertty-term-input`).

use qwertty_term_input::key::Key;

/// Map a macOS native virtual keycode (`NSEvent.keyCode`, i.e. Carbon
/// `kVK_*`) to a layout-independent [`Key`]. Returns [`Key::Unidentified`]
/// for keycodes not in this partial table, mirroring upstream's `else
/// .unidentified` fallthrough.
pub fn key_from_macos_keycode(keycode: u16) -> Key {
    // Values are the `kVK_ANSI_*` / `kVK_*` constants (the macOS column of
    // `keycodes.zig`). Grouped to match that file's ordering.
    match keycode {
        // Letters (kVK_ANSI_A ..)
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

        // Digits (kVK_ANSI_0 ..)
        0x1D => Key::Digit0,
        0x12 => Key::Digit1,
        0x13 => Key::Digit2,
        0x14 => Key::Digit3,
        0x15 => Key::Digit4,
        0x17 => Key::Digit5,
        0x16 => Key::Digit6,
        0x1A => Key::Digit7,
        0x1C => Key::Digit8,
        0x19 => Key::Digit9,

        // Punctuation
        0x18 => Key::Equal,
        0x1B => Key::Minus,
        0x21 => Key::BracketLeft,
        0x1E => Key::BracketRight,
        0x2A => Key::Backslash,
        0x29 => Key::Semicolon,
        0x27 => Key::Quote,
        0x32 => Key::Backquote,
        0x2B => Key::Comma,
        0x2F => Key::Period,
        0x2C => Key::Slash,

        // Whitespace / control
        0x24 => Key::Enter,
        0x30 => Key::Tab,
        0x31 => Key::Space,
        0x33 => Key::Backspace,
        0x35 => Key::Escape,
        0x75 => Key::Delete,
        0x73 => Key::Home,
        0x77 => Key::End,
        0x74 => Key::PageUp,
        0x79 => Key::PageDown,

        // Arrows
        0x7B => Key::ArrowLeft,
        0x7C => Key::ArrowRight,
        0x7D => Key::ArrowDown,
        0x7E => Key::ArrowUp,

        _ => Key::Unidentified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_verification_matrix_keys() {
        assert_eq!(key_from_macos_keycode(0x00), Key::KeyA);
        assert_eq!(key_from_macos_keycode(0x08), Key::KeyC);
        assert_eq!(key_from_macos_keycode(0x0E), Key::KeyE);
        assert_eq!(key_from_macos_keycode(0x09), Key::KeyV);
    }

    #[test]
    fn unknown_keycode_is_unidentified() {
        // 0xFF is not a real macOS keycode.
        assert_eq!(key_from_macos_keycode(0xFF), Key::Unidentified);
    }
}
