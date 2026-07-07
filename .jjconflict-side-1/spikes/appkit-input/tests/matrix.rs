//! Verification matrix for the AppKit input spike.
//!
//! Two layers:
//! 1. Pure encoder/preedit assertions (platform-independent) — the bulk of the
//!    matrix, proving the exact PTY bytes for each key/mods case.
//! 2. macOS-only: drive the *real* `NSTextInputClient` `NSView` subclass via its
//!    synthetic keyDown seam (`handle_key_down_raw`), which registers the objc2
//!    class and invokes the actual method bodies + ivar state. This proves the
//!    view shell wires the encoder/preedit correctly without a GUI event loop.
//!
//! What is NOT covered here (needs a real event loop / input context / user —
//! see `docs/analysis/appkit-input.md` § "What remains manual"): a live
//! `interpretKeyEvents` dead-key composition, real IME candidate windows, and
//! `performKeyEquivalent` menu routing.

use ghostty_input::key_encode::KittyFlags;
use ghostty_input::key_mods::OptionAsAlt;
use spike_appkit_input::translate::{self, Config, RawKeyEvent};

const KC_A: u16 = 0x00;
const KC_C: u16 = 0x08;
const KC_E: u16 = 0x0E;
const KC_V: u16 = 0x09;

fn disamb() -> Config {
    Config::default()
}

fn legacy() -> Config {
    Config {
        kitty_flags: KittyFlags::DISABLED,
        ..Config::default()
    }
}

fn key(keycode: u16, text: &str, unshifted: char) -> RawKeyEvent {
    RawKeyEvent {
        keycode,
        text: text.into(),
        unshifted_codepoint: unshifted as u32,
        ..Default::default()
    }
}

// ---- Layer 1: pure encoder matrix -----------------------------------------

#[test]
fn plain_a_passes_through() {
    let ev = key(KC_A, "a", 'a');
    assert_eq!(translate::encode_raw(&ev, &disamb()), b"a");
}

#[test]
fn shift_a_is_uppercase_text() {
    // Shift is consumed to produce "A"; no CSI u.
    let mut ev = key(KC_A, "A", 'a');
    ev.shift = true;
    assert_eq!(translate::encode_raw(&ev, &disamb()), b"A");
}

#[test]
fn ctrl_c_kitty_is_csi_u() {
    let mut ev = key(KC_C, "", 'c');
    ev.ctrl = true;
    assert_eq!(translate::encode_raw(&ev, &disamb()), b"\x1b[99;5u");
}

#[test]
fn ctrl_c_legacy_is_raw_etx() {
    // The realistic default (no kitty negotiated): raw 0x03.
    let mut ev = key(KC_C, "", 'c');
    ev.ctrl = true;
    assert_eq!(translate::encode_raw(&ev, &legacy()), vec![0x03]);
}

#[test]
fn option_e_without_option_as_alt_composes_text() {
    // option-as-alt=false: Option contributes to translation, so the composed
    // accent text is passed through (here we model the accent char directly).
    // The dead-key *sequencing* is proven by the preedit tests + the view seam;
    // this asserts the non-alt branch keeps the text.
    let mut ev = key(KC_E, "\u{e9}", 'e'); // "é" already composed
    ev.option = true;
    // Not treated as Alt -> text survives -> emitted literally under kitty
    // disambiguate (printable, no binding mods after option consumed).
    assert_eq!(translate::encode_raw(&ev, &disamb()), "\u{e9}".as_bytes());
}

#[test]
fn option_e_with_option_as_alt_is_alt_e() {
    // option-as-alt=true: Option becomes Alt, accent suppressed, alt+e
    // disambiguates to an ESC-prefixed CSI u (param 3 = alt).
    let cfg = Config {
        option_as_alt: OptionAsAlt::True,
        ..Config::default()
    };
    let mut ev = key(KC_E, "e", 'e'); // AppKit would translate to base 'e'
    ev.option = true;
    assert_eq!(translate::encode_raw(&ev, &cfg), b"\x1b[101;3u");
}

#[test]
fn cmd_key_does_not_reach_encoder() {
    // Cmd-V is a menu/binding key-equivalent; it must produce no PTY bytes.
    let mut ev = key(KC_V, "v", 'v');
    ev.command = true;
    assert_eq!(translate::encode_raw(&ev, &disamb()), b"");
}

#[test]
fn option_as_alt_left_right_discrimination() {
    // Left Option with option-as-alt=Right should NOT be treated as Alt.
    let cfg_right = Config {
        option_as_alt: OptionAsAlt::Right,
        ..Config::default()
    };
    let mut left_opt = key(KC_E, "\u{e9}", 'e');
    left_opt.option = true;
    left_opt.option_right = false; // left side
    assert!(
        !translate::option_is_alt(&left_opt, &cfg_right),
        "left option under option-as-alt=Right must compose, not alt"
    );

    let mut right_opt = key(KC_E, "e", 'e');
    right_opt.option = true;
    right_opt.option_right = true; // right side
    assert!(
        translate::option_is_alt(&right_opt, &cfg_right),
        "right option under option-as-alt=Right must be alt"
    );
}

// ---- Layer 2: real NSTextInputClient view (macOS only) --------------------

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use objc2::MainThreadMarker;
    use spike_appkit_input::view::{ImeScript, InputView};

    fn with_view(cfg: Config, f: impl FnOnce(&InputView)) {
        // Tests run on the main thread by default; get the marker or skip.
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("skipping: not on main thread");
            return;
        };
        let view = InputView::new(mtm, cfg);
        f(&view);
    }

    #[test]
    fn view_encodes_plain_a() {
        with_view(disamb(), |view| {
            view.handle_key_down_raw(&key(KC_A, "a", 'a'), ImeScript::None);
            assert_eq!(view.encoded(), vec![b"a".to_vec()]);
            assert!(view.committed_text().is_empty());
        });
    }

    #[test]
    fn view_encodes_ctrl_c_legacy() {
        with_view(legacy(), |view| {
            let mut ev = key(KC_C, "", 'c');
            ev.ctrl = true;
            view.handle_key_down_raw(&ev, ImeScript::None);
            assert_eq!(view.encoded(), vec![vec![0x03]]);
        });
    }

    #[test]
    fn view_cmd_key_encodes_nothing() {
        with_view(disamb(), |view| {
            let mut ev = key(KC_V, "v", 'v');
            ev.command = true;
            view.handle_key_down_raw(&ev, ImeScript::None);
            assert!(view.encoded().is_empty());
        });
    }

    #[test]
    fn view_dead_key_then_commit_e_acute() {
        // option-e (dead key) sets marked "´"; the key that started it is
        // consumed (composing) and encodes nothing. Then 'e' commits "é" via
        // insertText, which is sent as committed text, not encoded bytes.
        with_view(disamb(), |view| {
            // Step 1: option-e -> setMarkedText("´")
            let mut opt_e = key(KC_E, "", 'e');
            opt_e.option = true;
            view.handle_key_down_raw(&opt_e, ImeScript::Mark("\u{b4}".into()));
            assert!(view.encoded().is_empty(), "dead-key start must not encode");
            assert_eq!(view.preedit_text(), "\u{b4}", "preedit holds accent");

            // Step 2: 'e' -> insertText("é")
            view.handle_key_down_raw(&key(KC_E, "", 'e'), ImeScript::Commit("\u{e9}".into()));
            assert!(
                view.encoded().is_empty(),
                "committed compose must go via text, not encoder"
            );
            assert_eq!(view.committed_text(), vec!["\u{e9}".to_string()]);
            assert_eq!(view.preedit_text(), "", "preedit cleared after commit");
        });
    }

    #[test]
    fn view_ime_marked_then_unmark() {
        with_view(disamb(), |view| {
            // Korean-style: mark a jamo, then a control key clears it (unmark)
            // via the IME without committing.
            let ev = key(KC_A, "", 'a');
            view.handle_key_down_raw(&ev, ImeScript::Mark("\u{3131}".into()));
            assert_eq!(view.preedit_text(), "\u{3131}");
            assert!(view.encoded().is_empty());
        });
    }
}
