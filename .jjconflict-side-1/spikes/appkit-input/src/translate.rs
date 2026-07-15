//! Pure translation core: `NSEvent`-derived data -> [`qwertty_term_input`] key
//! event -> encoded PTY bytes.
//!
//! This module is deliberately AppKit-free so it can be unit-tested without a
//! running event loop. The `view` module (macOS-only) extracts the raw fields
//! from a real `NSEvent` and feeds them in here. This mirrors upstream's split:
//! `NSEvent+Extension.swift::ghosttyKeyEvent` builds a plain C struct, then
//! `apprt/embedded.zig` turns it into an `input.KeyEvent` and hands it to the
//! shared encoder. We are the Rust equivalent of that second half.

use qwertty_term_input::key::{Action, Key, KeyEvent};
use qwertty_term_input::key_encode::{self, KittyFlags, Options as EncodeOptions};
use qwertty_term_input::key_mods::{Mods, OptionAsAlt};

use crate::keymap::key_from_macos_keycode;

/// The subset of an `NSEvent` (keyDown/keyUp) we need to build a key event,
/// already reduced to plain Rust types. Populated from AppKit in `view.rs`.
#[derive(Debug, Clone, Default)]
pub struct RawKeyEvent {
    /// `NSEvent.keyCode` — the physical, layout-independent macOS virtual
    /// keycode.
    pub keycode: u16,
    /// Whether this is a repeat (`NSEvent.isARepeat`). Governs
    /// press-vs-repeat.
    pub is_repeat: bool,
    /// Whether this is a key-up (`keyUp:`) rather than key-down.
    pub is_up: bool,

    /// Modifier flags, already decomposed. These are the *raw* device
    /// modifiers (before any option-as-alt filtering).
    pub shift: bool,
    pub ctrl: bool,
    /// The physical Option key state (macOS `.option`). Whether this becomes
    /// an `alt` mod for encoding depends on [`Config::option_as_alt`].
    pub option: bool,
    /// The Command key (macOS `.command`) -> ghostty `super`.
    pub command: bool,
    pub caps_lock: bool,

    /// Right-side variants, from the `NX_DEVICER*KEYMASK` bits upstream reads
    /// in `Ghostty.ghosttyMods`. Used only to set the `sides` field so
    /// option-as-alt's Left/Right discrimination works.
    pub shift_right: bool,
    pub ctrl_right: bool,
    pub option_right: bool,
    pub command_right: bool,

    /// The UTF-8 text AppKit produced for this event (`event.characters`,
    /// after upstream's control-char / PUA filtering in `ghosttyCharacters`).
    /// Empty if none / suppressed.
    pub text: String,

    /// The unshifted codepoint (`characters(byApplyingModifiers: [])`),
    /// 0 if none. Matches `key_ev.unshifted_codepoint` upstream.
    pub unshifted_codepoint: u32,
}

/// Spike-level config knobs the R5 host would source from `Config`.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// `macos-option-as-alt`. When `True`/`Left`/`Right`, a held Option key is
    /// treated as Alt (stripped from text translation, ESC-prefixed / kitty
    /// `alt` bit set) instead of composing an accented character.
    pub option_as_alt: OptionAsAlt,
    /// Kitty keyboard protocol flags currently active for the screen. When all
    /// zero, `qwertty_term_input::key_encode::encode` dispatches to the *legacy*
    /// path — which in this port is a narrow stub that does NOT echo plain
    /// printable text nor apply alt/ESC-prefix (see that crate's `key_encode`
    /// module docs and `docs/analysis/appkit-input.md` § "Encoder maturity").
    ///
    /// The spike defaults to **disambiguate-only** (the minimal kitty mode most
    /// terminals negotiate first): it exercises the fully-ported kitty encoder
    /// while still passing *plain* printable keys through as their literal byte
    /// (e.g. `a` -> `[0x61]`), which is the realistic behavior to prove.
    /// `KittyFlags::ALL` would instead force every key — including plain `a` —
    /// into full `CSI u` form (`report_all`), which is correct but not the
    /// common path.
    pub kitty_flags: KittyFlags,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            option_as_alt: OptionAsAlt::False,
            kitty_flags: KittyFlags {
                disambiguate: true,
                ..KittyFlags::DISABLED
            },
        }
    }
}

/// Build the raw (device) [`Mods`] from a [`RawKeyEvent`], BEFORE option-as-alt
/// filtering. Option always maps to `alt` here; the caller decides via
/// [`Mods::translation`] whether that survives for text translation. This
/// mirrors `Ghostty.ghosttyMods`, which unconditionally maps `.option` ->
/// `GHOSTTY_MODS_ALT` and only later filters via
/// `ghostty_surface_key_translation_mods`.
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
/// treated as Alt (i.e. bypass IME/accent composition). This is the decision
/// AppKit makes in `keyDown` via `ghostty_surface_key_translation_mods`: when
/// option-as-alt is active, the Option bit is *removed* from the modifiers used
/// to translate characters, so macOS produces the ASCII letter (which the
/// encoder then ESC-prefixes) rather than the accented dead-key character.
pub fn option_is_alt(raw: &RawKeyEvent, cfg: &Config) -> bool {
    if !raw.option {
        return false;
    }
    // `translation` returns the mods to use for TEXT translation. If it drops
    // `alt`, Option is being treated as Alt (bypass composition). We invert:
    // Option-as-Alt is active iff the translated mods have `alt == false`
    // while the raw mods had `alt == true`.
    let raw_mods = mods_from_raw(raw);
    let translated = raw_mods.translation(cfg.option_as_alt);
    raw_mods.alt && !translated.alt
}

/// Build a [`KeyEvent`] from raw AppKit data + config.
///
/// `text` on the resulting event is the printable UTF-8 (already filtered).
/// When Option is being treated as Alt, we clear the accented text and instead
/// rely on the encoder's `alt` handling (ESC-prefix in legacy, `alt` bit in
/// kitty) — matching the effect of upstream sending the ASCII-translated
/// `translationEvent.ghosttyCharacters` down with the `alt` mod set.
pub fn build_key_event(raw: &RawKeyEvent, cfg: &Config) -> KeyEvent {
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

    // Text handling: if Option is acting as Alt, the accent-composed text is
    // suppressed; the encoder will emit the base key ESC-prefixed / with the
    // alt bit. Otherwise, pass the composed text through.
    let text = if opt_alt {
        String::new()
    } else {
        raw.text.clone()
    };

    // Consumed mods heuristic, matching upstream `ghosttyKeyEvent`: control and
    // command never contribute to text translation; everything else did.
    let mut consumed = mods;
    consumed.ctrl = false;
    consumed.super_ = false;
    if opt_alt {
        // Option was NOT consumed for text (it's acting as Alt).
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

/// Encoder options for a given config. Public so the harness/logging can build
/// the same options the view uses.
pub fn encode_options(cfg: &Config) -> EncodeOptions {
    EncodeOptions {
        kitty_flags: cfg.kitty_flags,
        macos_option_as_alt: cfg.option_as_alt,
        ..EncodeOptions::default()
    }
}

/// Full path: raw AppKit event -> encoded PTY bytes.
///
/// Returns the bytes that would be written to the PTY. Empty means "nothing to
/// send" (e.g. a bare modifier, a Cmd-key that should be a menu binding, or a
/// release under the legacy path).
pub fn encode_raw(raw: &RawKeyEvent, cfg: &Config) -> Vec<u8> {
    // Command/super keys must NOT reach the terminal encoder as text: on macOS
    // they are menu key-equivalents / bindings. Upstream routes them through
    // `performKeyEquivalent` and only re-injects into `keyDown` if unbound.
    // The spike models the common case: a Cmd-modified key is swallowed (menu
    // territory) and produces no PTY bytes.
    if raw.command {
        return Vec::new();
    }

    let event = build_key_event(raw, cfg);
    key_encode::encode(&event, &encode_options(cfg))
}

/// Convenience: does this raw event denote a bare modifier key (no keycode we
/// map to a character/navigation key)? Bare modifiers produce no encoded bytes.
pub fn is_bare_modifier(raw: &RawKeyEvent) -> bool {
    matches!(key_from_macos_keycode(raw.keycode), Key::Unidentified) && raw.text.is_empty()
}
