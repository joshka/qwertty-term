//! Maps egui input events to `qwertty-term-input` events, and routes key presses
//! through `qwertty_term_input::key_encode` (kitty protocol when active, a narrow
//! legacy placeholder otherwise — see that crate's module docs for the seam
//! design).
//!
//! egui's `Key`/`Modifiers` are coarser than Ghostty's native key model: no
//! left/right modifier sides, no caps-lock/num-lock state, and `Event::Key`
//! carries no UTF-8 text (that arrives via a separate `Event::Text`). This
//! mapping fills in what egui actually provides and leaves the rest at
//! default, which is enough to drive the kitty protocol's disambiguation
//! (correct key codes + modifiers + press/release/repeat) even though the
//! richer alternates/associated-text reporting needs input this frontend
//! doesn't have.

use eframe::egui::{Key as EguiKey, Modifiers as EguiModifiers, PointerButton};
use qwertty_term_input::key::{Action, Key as InputKey, KeyEvent};
use qwertty_term_input::key_encode::{self, Options as KeyEncodeOptions};
use qwertty_term_input::key_mods::Mods;

pub(super) fn mouse_button_code(button: PointerButton) -> Option<u8> {
    match button {
        PointerButton::Primary => Some(0),
        PointerButton::Middle => Some(1),
        PointerButton::Secondary => Some(2),
        _ => None,
    }
}

/// Encode a key press/repeat into PTY bytes via `qwertty_term_input::key_encode`.
/// Returns `None` if the event produces no output (e.g. a bare modifier key,
/// or a key/mods combination the encoder doesn't map to anything).
pub(super) fn encode_key(
    key: EguiKey,
    modifiers: EguiModifiers,
    repeat: bool,
    opts: &KeyEncodeOptions,
) -> Option<Vec<u8>> {
    let input_key = map_key(key)?;
    let event = KeyEvent {
        action: if repeat {
            Action::Repeat
        } else {
            Action::Press
        },
        key: input_key,
        mods: map_modifiers(modifiers),
        ..KeyEvent::default()
    };
    let bytes = key_encode::encode(&event, opts);
    if bytes.is_empty() { None } else { Some(bytes) }
}

fn map_modifiers(modifiers: EguiModifiers) -> Mods {
    Mods {
        shift: modifiers.shift,
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        // egui's `mac_cmd`/`command` both track the macOS Command key (and
        // fall back to ctrl on other platforms); `super_` here is Ghostty's
        // "super"/Cmd modifier specifically, so `mac_cmd` is the right field
        // — using `command` would double-count ctrl as both `ctrl` and
        // `super_` on non-Mac platforms.
        super_: modifiers.mac_cmd,
        ..Mods::default()
    }
}

/// Map an egui logical [`EguiKey`] to Ghostty's layout-independent
/// [`InputKey`]. Returns `None` for egui keys with no Ghostty equivalent
/// (mostly punctuation Ghostty expects as plain UTF-8 text via `Event::Text`
/// rather than a named key).
fn map_key(key: EguiKey) -> Option<InputKey> {
    use EguiKey as K;
    use InputKey as I;
    Some(match key {
        K::ArrowDown => I::ArrowDown,
        K::ArrowLeft => I::ArrowLeft,
        K::ArrowRight => I::ArrowRight,
        K::ArrowUp => I::ArrowUp,
        K::Escape => I::Escape,
        K::Tab => I::Tab,
        K::Backspace => I::Backspace,
        K::Enter => I::Enter,
        K::Space => I::Space,
        K::Insert => I::Insert,
        K::Delete => I::Delete,
        K::Home => I::Home,
        K::End => I::End,
        K::PageUp => I::PageUp,
        K::PageDown => I::PageDown,
        K::Copy => I::Copy,
        K::Cut => I::Cut,
        K::Paste => I::Paste,
        K::Comma => I::Comma,
        K::Backslash => I::Backslash,
        K::Slash => I::Slash,
        K::OpenBracket => I::BracketLeft,
        K::CloseBracket => I::BracketRight,
        K::Backtick => I::Backquote,
        K::Minus => I::Minus,
        K::Period => I::Period,
        K::Equals => I::Equal,
        K::Semicolon => I::Semicolon,
        K::Quote => I::Quote,
        K::Num0 => I::Digit0,
        K::Num1 => I::Digit1,
        K::Num2 => I::Digit2,
        K::Num3 => I::Digit3,
        K::Num4 => I::Digit4,
        K::Num5 => I::Digit5,
        K::Num6 => I::Digit6,
        K::Num7 => I::Digit7,
        K::Num8 => I::Digit8,
        K::Num9 => I::Digit9,
        K::A => I::KeyA,
        K::B => I::KeyB,
        K::C => I::KeyC,
        K::D => I::KeyD,
        K::E => I::KeyE,
        K::F => I::KeyF,
        K::G => I::KeyG,
        K::H => I::KeyH,
        K::I => I::KeyI,
        K::J => I::KeyJ,
        K::K => I::KeyK,
        K::L => I::KeyL,
        K::M => I::KeyM,
        K::N => I::KeyN,
        K::O => I::KeyO,
        K::P => I::KeyP,
        K::Q => I::KeyQ,
        K::R => I::KeyR,
        K::S => I::KeyS,
        K::T => I::KeyT,
        K::U => I::KeyU,
        K::V => I::KeyV,
        K::W => I::KeyW,
        K::X => I::KeyX,
        K::Y => I::KeyY,
        K::Z => I::KeyZ,
        K::F1 => I::F1,
        K::F2 => I::F2,
        K::F3 => I::F3,
        K::F4 => I::F4,
        K::F5 => I::F5,
        K::F6 => I::F6,
        K::F7 => I::F7,
        K::F8 => I::F8,
        K::F9 => I::F9,
        K::F10 => I::F10,
        K::F11 => I::F11,
        K::F12 => I::F12,
        K::F13 => I::F13,
        K::F14 => I::F14,
        K::F15 => I::F15,
        K::F16 => I::F16,
        K::F17 => I::F17,
        K::F18 => I::F18,
        K::F19 => I::F19,
        K::F20 => I::F20,
        K::F21 => I::F21,
        K::F22 => I::F22,
        K::F23 => I::F23,
        K::F24 => I::F24,
        K::F25 => I::F25,
        // No Ghostty `Key` equivalent (egui-only synthetic punctuation keys,
        // or keys Ghostty expects as plain `Event::Text` instead): handled
        // by the caller falling through to text input.
        _ => return None,
    })
}
