//! Pure key-translation core: `NSEvent`-derived data → [`qwertty_term_input`] key
//! event → encoded PTY bytes.
//!
//! Lifted from the R5 spike (`spikes/appkit-input/src/translate.rs`) into
//! production form. Deliberately AppKit-free so it unit-tests without a running
//! event loop; the macOS view (`crate::input::view`) extracts the raw fields
//! from a real `NSEvent` and feeds them in. This mirrors upstream's split:
//! `NSEvent+Extension.swift::ghosttyKeyEvent` builds a plain C struct, then
//! `apprt/embedded.zig` turns it into an `input.KeyEvent` and hands it to the
//! shared encoder.
//!
//! Difference from the spike: the kitty-protocol flags and the other
//! mode-derived encoder knobs are *not* a static `Config` — they come live from
//! the terminal engine each keystroke (`Engine::key_encode_options`), so a
//! program that flips kitty flags or DECCKM mid-session encodes correctly. Only
//! `macos-option-as-alt` stays a user-config knob (carried in [`InputConfig`]).

use qwertty_term_input::key::{Action, Key, KeyEvent};
use qwertty_term_input::key_encode::{self, Options as EncodeOptions};
use qwertty_term_input::key_mods::{Mods, OptionAsAlt};

use super::keymap::key_from_macos_keycode;

/// The subset of an `NSEvent` (keyDown/keyUp) needed to build a key event,
/// reduced to plain Rust types. Populated from AppKit in `view.rs`.
#[derive(Debug, Clone, Default)]
pub struct RawKeyEvent {
    /// `NSEvent.keyCode` — the physical, layout-independent macOS virtual
    /// keycode.
    pub keycode: u16,
    /// Whether this is a repeat (`NSEvent.isARepeat`).
    pub is_repeat: bool,
    /// Whether this is a key-up (`keyUp:`) rather than key-down.
    pub is_up: bool,

    /// Raw device modifiers (before option-as-alt filtering).
    pub shift: bool,
    pub ctrl: bool,
    /// Physical Option key state. Whether it becomes `alt` for encoding depends
    /// on [`InputConfig::option_as_alt`].
    pub option: bool,
    /// Command (macOS `.command`) → ghostty `super`.
    pub command: bool,
    pub caps_lock: bool,

    /// Right-side variants from the `NX_DEVICER*KEYMASK` bits (upstream
    /// `ghosttyMods`). Set the `sides` field so option-as-alt Left/Right works.
    pub shift_right: bool,
    pub ctrl_right: bool,
    pub option_right: bool,
    pub command_right: bool,

    /// The UTF-8 text AppKit produced (`event.characters`, after upstream's
    /// control-char / PUA filtering in `ghosttyCharacters`). Empty if none.
    pub text: String,

    /// The unshifted codepoint (`characters(byApplyingModifiers: [])`), 0 if
    /// none. Matches `key_ev.unshifted_codepoint` upstream.
    pub unshifted_codepoint: u32,
}

/// User-config input knobs (not terminal state). The kitty/DECCKM/etc. encoder
/// options come live from the engine; only `macos-option-as-alt` lives here.
#[derive(Debug, Clone, Copy)]
pub struct InputConfig {
    /// `macos-option-as-alt`. When `True`/`Left`/`Right`, a held Option key is
    /// treated as Alt (stripped from text translation, ESC-prefixed / kitty
    /// `alt` bit) instead of composing an accented character.
    pub option_as_alt: OptionAsAlt,
}

impl Default for InputConfig {
    fn default() -> Self {
        InputConfig {
            option_as_alt: OptionAsAlt::False,
        }
    }
}

/// Build the raw (device) [`Mods`] from a [`RawKeyEvent`], BEFORE option-as-alt
/// filtering. Option always maps to `alt` here; the caller decides via
/// [`Mods::translation`] whether that survives for text translation. Mirrors
/// `Ghostty.ghosttyMods`.
pub fn mods_from_raw(raw: &RawKeyEvent) -> Mods {
    use qwertty_term_input::key_mods::Side;
    Mods {
        shift: raw.shift,
        ctrl: raw.ctrl,
        alt: raw.option,
        super_: raw.command,
        caps_lock: raw.caps_lock,
        num_lock: false,
        sides: Side {
            shift: side(raw.shift_right),
            ctrl: side(raw.ctrl_right),
            alt: side(raw.option_right),
            super_: side(raw.command_right),
        },
    }
}

fn side(is_right: bool) -> qwertty_term_input::key_mods::ModSide {
    use qwertty_term_input::key_mods::ModSide;
    if is_right {
        ModSide::Right
    } else {
        ModSide::Left
    }
}

/// Whether, under this option-as-alt config, the physical Option key should be
/// treated as Alt (bypassing IME/accent composition). Matches the decision
/// AppKit makes via `ghostty_surface_key_translation_mods`.
pub fn option_is_alt(raw: &RawKeyEvent, cfg: &InputConfig) -> bool {
    if !raw.option {
        return false;
    }
    let raw_mods = mods_from_raw(raw);
    let translated = raw_mods.translation(cfg.option_as_alt);
    raw_mods.alt && !translated.alt
}

/// Build a [`KeyEvent`] from raw AppKit data + config.
pub fn build_key_event(raw: &RawKeyEvent, cfg: &InputConfig) -> KeyEvent {
    let action = if raw.is_up {
        Action::Release
    } else if raw.is_repeat {
        Action::Repeat
    } else {
        Action::Press
    };

    let key = key_from_macos_keycode(raw.keycode);
    let mods = mods_from_raw(raw);
    let opt_alt = option_is_alt(raw, cfg);

    // If Option is acting as Alt, the accent-composed text is suppressed; the
    // encoder emits the base key ESC-prefixed / with the alt bit. Otherwise pass
    // the composed text through.
    let text = if opt_alt {
        String::new()
    } else {
        raw.text.clone()
    };

    // Consumed-mods heuristic, matching upstream `ghosttyKeyEvent`: control and
    // command never contribute to text translation.
    let mut consumed = mods;
    consumed.ctrl = false;
    consumed.super_ = false;
    if opt_alt {
        consumed.alt = false;
    }

    KeyEvent {
        action,
        key,
        mods,
        consumed_mods: consumed,
        composing: false,
        utf8: text,
        unshifted_codepoint: raw.unshifted_codepoint,
    }
}

/// Full path: raw AppKit event + live encoder options → encoded PTY bytes.
///
/// `encode_opts` carries the terminal's live kitty/DECCKM/etc. state
/// (`Engine::key_encode_options`); `cfg` carries the user's option-as-alt knob,
/// applied to `encode_opts.macos_option_as_alt` and to text suppression here.
///
/// Returns the bytes to write to the PTY. Empty means "nothing to send" (a bare
/// modifier, a Cmd-key that is menu/binding territory, or a legacy-path
/// release).
pub fn encode_raw(raw: &RawKeyEvent, cfg: &InputConfig, mut encode_opts: EncodeOptions) -> Vec<u8> {
    // Command/super keys must NOT reach the terminal encoder as text: on macOS
    // they are menu key-equivalents / bindings. The menu (via
    // performKeyEquivalent) gets first crack; anything that reaches keyDown with
    // Command held is swallowed here (menu territory) and produces no PTY bytes.
    if raw.command {
        return Vec::new();
    }

    encode_opts.macos_option_as_alt = cfg.option_as_alt;
    let event = build_key_event(raw, cfg);
    key_encode::encode(&event, &encode_opts)
}

/// Does this raw event denote a bare modifier key (no mapped character/nav key
/// and no text)? Bare modifiers produce no encoded bytes.
pub fn is_bare_modifier(raw: &RawKeyEvent) -> bool {
    matches!(key_from_macos_keycode(raw.keycode), Key::Unidentified) && raw.text.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwertty_term_input::key_encode::KittyFlags;

    fn kitty_disambiguate() -> EncodeOptions {
        EncodeOptions {
            kitty_flags: KittyFlags {
                disambiguate: true,
                ..KittyFlags::DISABLED
            },
            ..EncodeOptions::default()
        }
    }

    fn raw_char(keycode: u16, text: &str, unshifted: char) -> RawKeyEvent {
        RawKeyEvent {
            keycode,
            text: text.to_string(),
            unshifted_codepoint: unshifted as u32,
            ..Default::default()
        }
    }

    #[test]
    fn plain_a_passes_through_as_its_byte() {
        let raw = raw_char(0x00, "a", 'a');
        let bytes = encode_raw(&raw, &InputConfig::default(), kitty_disambiguate());
        assert_eq!(bytes, b"a");
    }

    /// End-to-end proof that a plain (legacy, non-kitty) shell — the state a
    /// freshly spawned `$SHELL` is in — encodes `l`, `s`, and Enter as the exact
    /// bytes the PTY expects. Guards suspect #4 (encoder regression) with the
    /// *default* engine options a real tab uses, not the kitty options the other
    /// tests exercise.
    #[test]
    fn typing_ls_enter_with_default_options_emits_expected_bytes() {
        let cfg = InputConfig::default();
        let opts = EncodeOptions::default();

        // 'l' (macOS keycode 0x25) -> the byte 'l'.
        let l = encode_raw(&raw_char(0x25, "l", 'l'), &cfg, opts);
        assert_eq!(l, b"l", "'l' should encode to its literal byte");

        // 's' (macOS keycode 0x01) -> the byte 's'.
        let s = encode_raw(&raw_char(0x01, "s", 's'), &cfg, opts);
        assert_eq!(s, b"s", "'s' should encode to its literal byte");

        // Enter/Return (macOS keycode 0x24) -> carriage return.
        let enter_raw = RawKeyEvent {
            keycode: 0x24,
            text: "\r".to_string(),
            unshifted_codepoint: '\r' as u32,
            ..Default::default()
        };
        let enter = encode_raw(&enter_raw, &cfg, opts);
        assert_eq!(enter, b"\r", "Enter should encode to CR");
    }

    #[test]
    fn ctrl_c_encodes_kitty_csi_u() {
        let mut raw = raw_char(0x08, "", 'c');
        raw.ctrl = true;
        let bytes = encode_raw(&raw, &InputConfig::default(), kitty_disambiguate());
        assert_eq!(bytes, b"\x1b[99;5u");
    }

    #[test]
    fn command_keys_are_swallowed() {
        let mut raw = raw_char(0x09, "v", 'v');
        raw.command = true;
        let bytes = encode_raw(&raw, &InputConfig::default(), kitty_disambiguate());
        assert!(bytes.is_empty());
    }

    #[test]
    fn option_as_alt_suppresses_composed_text() {
        // option-e with option-as-alt=true → alt+e (not the composed accent).
        let mut raw = raw_char(0x0E, "\u{00b4}", 'e');
        raw.option = true;
        let cfg = InputConfig {
            option_as_alt: OptionAsAlt::True,
        };
        assert!(option_is_alt(&raw, &cfg));
        let ev = build_key_event(&raw, &cfg);
        assert!(ev.utf8.is_empty(), "composed accent text suppressed");
    }

    #[test]
    fn bare_modifier_detection() {
        let bare = RawKeyEvent {
            keycode: 0xFF,
            ..Default::default()
        };
        assert!(is_bare_modifier(&bare));
        let a = raw_char(0x00, "a", 'a');
        assert!(!is_bare_modifier(&a));
    }
}
