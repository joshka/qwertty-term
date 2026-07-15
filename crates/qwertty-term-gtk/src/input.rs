//! GDK key event → PTY bytes translation (the platform-free half of keyboard
//! input, factored out so it unit-tests without a running GTK event loop).
//!
//! This is the Linux analog of the macOS `crate::input::translate` path in the
//! `qwertty-term` app crate: a raw platform key event is turned into a
//! [`qwertty_term_input::key::KeyEvent`] and handed to the shared
//! [`key_encode::encode`] encoder, whose bytes are what we write to the pty.
//! Only the event *source* differs (GDK keyval/keycode/`ModifierType` here vs.
//! `NSEvent` on macOS); the encoder is identical.
//!
//! Mirrors upstream Ghostty's `Surface.keyEvent`
//! (`src/apprt/gtk/class/surface.zig:1240`, pin `2da015cd6` / ancestor
//! `38e49a2`), reduced to the **direct key path** it runs even for plain keys:
//!
//! - `keyval` → UTF-8 text via `gdk.keyvalToUnicode` (`surface.zig:1352`), with
//!   control codepoints (`< 0x20`) excluded from the text so the encoder
//!   handles them (`surface.zig:1413-1424`).
//! - `keyval`/`keycode` → the physical [`Key`] (`surface.zig:1356-1382`).
//! - `ModifierType` → [`Mods`] (`surface.zig:1385`).
//! - The assembled [`KeyEvent`] is encoded (`surface.zig:1428`).
//!
//! Non-goals for this chunk (left as seams, see the module-level TODOs):
//!
//! - Full IME / dead-key / compose handling via `GtkIMMulticontext`
//!   (`surface.zig:1246-1334`). We do the direct keyval→text path only.
//! - Layout-accurate physical-key mapping via the XKB hardware `keycode` and
//!   the W3C keycodes table (`surface.zig:1356`); we derive [`Key`] from the
//!   keyval, which is correct for ASCII + the common named keys this chunk
//!   targets but not for every layout.
//! - Mouse / selection.

use qwertty_term_input::key::{Action, Key, KeyEvent};
use qwertty_term_input::key_encode::{self, Options as EncodeOptions};
use qwertty_term_input::key_mods::{Mods, Side};

// GDK `ModifierType` bit values (stable ABI; see `gdk/gdkenums.h`). Redefined
// here so this module (and its tests) stay free of a GTK dependency — the GTK
// handler passes `ModifierType::bits()`.
const GDK_SHIFT_MASK: u32 = 1 << 0;
const GDK_LOCK_MASK: u32 = 1 << 1; // Caps Lock
const GDK_CONTROL_MASK: u32 = 1 << 2;
const GDK_ALT_MASK: u32 = 1 << 3; // a.k.a. MOD1
const GDK_SUPER_MASK: u32 = 1 << 26;

// Common GDK/X11 keysym values we translate directly (see
// `gdk/gdkkeysyms.h`). Only the set this chunk targets (ASCII printables are
// handled generically via the Latin-1 / direct-Unicode ranges below).
const KEY_BACKSPACE: u32 = 0xFF08;
const KEY_TAB: u32 = 0xFF09;
const KEY_ISO_LEFT_TAB: u32 = 0xFE20; // Shift+Tab
const KEY_RETURN: u32 = 0xFF0D;
const KEY_KP_ENTER: u32 = 0xFF8D;
const KEY_ESCAPE: u32 = 0xFF1B;
const KEY_HOME: u32 = 0xFF50;
const KEY_LEFT: u32 = 0xFF51;
const KEY_UP: u32 = 0xFF52;
const KEY_RIGHT: u32 = 0xFF53;
const KEY_DOWN: u32 = 0xFF54;
const KEY_PAGE_UP: u32 = 0xFF55;
const KEY_PAGE_DOWN: u32 = 0xFF56;
const KEY_END: u32 = 0xFF57;
const KEY_INSERT: u32 = 0xFF63;
const KEY_DELETE: u32 = 0xFFFF;
const KEY_SHIFT_L: u32 = 0xFFE1;
const KEY_SHIFT_R: u32 = 0xFFE2;
const KEY_CONTROL_L: u32 = 0xFFE3;
const KEY_CONTROL_R: u32 = 0xFFE4;
const KEY_CAPS_LOCK: u32 = 0xFFE5;
const KEY_ALT_L: u32 = 0xFFE9;
const KEY_ALT_R: u32 = 0xFFEA;
const KEY_SUPER_L: u32 = 0xFFEB;
const KEY_SUPER_R: u32 = 0xFFEC;

/// Convert a GDK keyval to its Unicode scalar, or `None` if it is not a
/// character-producing key. Reduced port of `gdk_keyval_to_unicode`: covers the
/// ASCII (`0x20..=0x7E`) and Latin-1 (`0xA0..=0xFF`) ranges — where the keysym
/// equals the codepoint — plus the "direct Unicode" keysym range
/// (`0x0100_0000..=0x0110_FFFF`). Named/functional keysyms return `None`.
///
/// This intentionally does not cover the full keysym→Unicode table (Greek,
/// Cyrillic, etc.); those arrive as part of the later IME/layout chunk. Control
/// characters (e.g. the `\r` for Return) are represented by their named keysym,
/// not this path, so they are handled by [`key_from_keyval`] + the encoder.
pub fn keyval_to_unicode(keyval: u32) -> Option<u32> {
    match keyval {
        0x20..=0x7E | 0xA0..=0xFF => Some(keyval),
        0x0100_0000..=0x0110_FFFF => Some(keyval - 0x0100_0000),
        _ => None,
    }
}

/// Map a GDK keyval to the physical [`Key`]. Named keys (Enter, arrows, …) and
/// modifiers match explicit keysyms; printable keys are resolved through the
/// keyval's (lowercased) Unicode via [`Key::from_ascii`], which yields the
/// layout-independent, shift-independent key (e.g. both `a` and `A` → `KeyA`).
///
/// Reduced analog of upstream's keycode→W3C-key lookup
/// (`surface.zig:1356-1382`): we key off the keyval rather than the XKB
/// hardware `keycode`, which is correct for ASCII + the named keys this chunk
/// targets. Returns [`Key::Unidentified`] for anything unmapped — the encoder
/// still forwards any accompanying UTF-8 text, so unmapped printables (e.g.
/// shifted punctuation) still type.
pub fn key_from_keyval(keyval: u32) -> Key {
    match keyval {
        KEY_BACKSPACE => Key::Backspace,
        KEY_TAB | KEY_ISO_LEFT_TAB => Key::Tab,
        KEY_RETURN => Key::Enter,
        KEY_KP_ENTER => Key::NumpadEnter,
        KEY_ESCAPE => Key::Escape,
        KEY_HOME => Key::Home,
        KEY_LEFT => Key::ArrowLeft,
        KEY_UP => Key::ArrowUp,
        KEY_RIGHT => Key::ArrowRight,
        KEY_DOWN => Key::ArrowDown,
        KEY_PAGE_UP => Key::PageUp,
        KEY_PAGE_DOWN => Key::PageDown,
        KEY_END => Key::End,
        KEY_INSERT => Key::Insert,
        KEY_DELETE => Key::Delete,
        KEY_SHIFT_L => Key::ShiftLeft,
        KEY_SHIFT_R => Key::ShiftRight,
        KEY_CONTROL_L => Key::ControlLeft,
        KEY_CONTROL_R => Key::ControlRight,
        KEY_CAPS_LOCK => Key::CapsLock,
        KEY_ALT_L => Key::AltLeft,
        KEY_ALT_R => Key::AltRight,
        KEY_SUPER_L => Key::MetaLeft,
        KEY_SUPER_R => Key::MetaRight,
        _ => keyval_to_unicode(keyval)
            .and_then(|cp| u8::try_from(cp).ok())
            .map(|b| b.to_ascii_lowercase())
            .and_then(Key::from_ascii)
            .unwrap_or(Key::Unidentified),
    }
}

/// Build [`Mods`] from a GDK `ModifierType` bitfield. Analog of
/// `gtk_key.eventMods` (`surface.zig:1385`), reduced to the base modifier bits
/// (no left/right side disambiguation, which GDK's `state` mask doesn't carry).
pub fn mods_from_gdk_state(state: u32) -> Mods {
    Mods {
        shift: state & GDK_SHIFT_MASK != 0,
        ctrl: state & GDK_CONTROL_MASK != 0,
        alt: state & GDK_ALT_MASK != 0,
        super_: state & GDK_SUPER_MASK != 0,
        caps_lock: state & GDK_LOCK_MASK != 0,
        num_lock: false,
        sides: Side::default(),
    }
}

/// Assemble a [`KeyEvent`] from GDK inputs. `action` distinguishes
/// press/repeat/release; `keycode` is accepted for parity with upstream (and a
/// future XKB physical-key path) but not yet consulted.
///
/// Port of the event assembly in `surface.zig:1428-1436`.
pub fn build_key_event(action: Action, keyval: u32, _keycode: u32, state: u32) -> KeyEvent {
    let key = key_from_keyval(keyval);
    let mods = mods_from_gdk_state(state);

    // UTF-8 text from the keyval's Unicode, excluding control codepoints
    // (< 0x20): those are named keys the encoder handles itself (Enter, Tab,
    // …). Mirrors `surface.zig:1413-1424`.
    let utf8 = match keyval_to_unicode(keyval) {
        Some(cp) if cp >= 0x20 => char::from_u32(cp).map(String::from).unwrap_or_default(),
        _ => String::new(),
    };

    // The unshifted codepoint is the character the physical key produces with
    // no modifiers — i.e. the base-layout key. For mapped printable keys that
    // is `Key::codepoint()` (e.g. `KeyA` → 'a'); otherwise fall back to the
    // event's own codepoint. Feeds the encoder's ctrl-sequence / caps handling
    // (`key_encode::ctrl_seq`).
    let unshifted_codepoint = key
        .codepoint()
        .or_else(|| keyval_to_unicode(keyval))
        .unwrap_or(0);

    // Consumed-mods heuristic matching the macOS path: control and super never
    // contribute to text translation, so they are never "consumed".
    let mut consumed_mods = mods;
    consumed_mods.ctrl = false;
    consumed_mods.super_ = false;

    KeyEvent {
        action,
        key,
        mods,
        consumed_mods,
        composing: false,
        utf8,
        unshifted_codepoint,
    }
}

/// The full translation used by the GTK key handler: GDK
/// keyval + hardware keycode + `ModifierType` bits → the exact PTY bytes to
/// write, using the given live encoder [`EncodeOptions`] (terminal mode state:
/// DECCKM, kitty flags, …). Returns `None` when the event encodes to nothing
/// (a bare modifier, a release under the legacy encoder, etc.) so the caller
/// can skip the pty write.
///
/// This is the single translation function shared by the interactive GTK
/// handler and the headless round-trip test, so the test exercises the real
/// encode path with no GTK event injection.
pub fn gdk_key_to_bytes(
    action: Action,
    keyval: u32,
    keycode: u32,
    state: u32,
    opts: &EncodeOptions,
) -> Option<Vec<u8>> {
    let event = build_key_event(action, keyval, keycode, state);
    let bytes = key_encode::encode(&event, opts);
    if bytes.is_empty() { None } else { Some(bytes) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(keyval: u32) -> Option<Vec<u8>> {
        gdk_key_to_bytes(Action::Press, keyval, 0, 0, &EncodeOptions::default())
    }

    fn press_mods(keyval: u32, state: u32) -> Option<Vec<u8>> {
        gdk_key_to_bytes(Action::Press, keyval, 0, state, &EncodeOptions::default())
    }

    #[test]
    fn printable_ascii_types_itself() {
        assert_eq!(press(0x68), Some(b"h".to_vec())); // 'h'
        assert_eq!(press(0x20), Some(b" ".to_vec())); // space
        assert_eq!(press(0x41), Some(b"A".to_vec())); // shifted 'A' keyval
    }

    #[test]
    fn hello_sequence() {
        let mut out = Vec::new();
        for &kv in &[0x68u32, 0x65, 0x6c, 0x6c, 0x6f, KEY_RETURN] {
            if let Some(b) = press(kv) {
                out.extend_from_slice(&b);
            }
        }
        assert_eq!(out, b"hello\r");
    }

    #[test]
    fn named_keys() {
        assert_eq!(press(KEY_RETURN), Some(b"\r".to_vec()));
        assert_eq!(press(KEY_TAB), Some(b"\t".to_vec()));
        assert_eq!(press(KEY_BACKSPACE), Some(b"\x7f".to_vec()));
        assert_eq!(press(KEY_ESCAPE), Some(b"\x1b".to_vec()));
    }

    #[test]
    fn arrows_default_cursor_mode() {
        assert_eq!(press(KEY_UP), Some(b"\x1b[A".to_vec()));
        assert_eq!(press(KEY_DOWN), Some(b"\x1b[B".to_vec()));
        assert_eq!(press(KEY_RIGHT), Some(b"\x1b[C".to_vec()));
        assert_eq!(press(KEY_LEFT), Some(b"\x1b[D".to_vec()));
    }

    #[test]
    fn ctrl_c_is_etx() {
        // Ctrl+C → 0x03. GDK delivers keyval 'c' with the control bit set.
        assert_eq!(press_mods(0x63, GDK_CONTROL_MASK), Some(vec![0x03]));
    }

    #[test]
    fn ctrl_d_is_eot() {
        assert_eq!(press_mods(0x64, GDK_CONTROL_MASK), Some(vec![0x04]));
    }

    #[test]
    fn bare_modifier_encodes_nothing() {
        assert_eq!(press_mods(KEY_CONTROL_L, GDK_CONTROL_MASK), None);
        assert_eq!(press_mods(KEY_SHIFT_L, GDK_SHIFT_MASK), None);
    }

    #[test]
    fn delete_sends_csi() {
        assert_eq!(press(KEY_DELETE), Some(b"\x1b[3~".to_vec()));
    }

    #[test]
    fn key_mapping_is_layout_independent() {
        assert_eq!(key_from_keyval(0x61), Key::KeyA); // 'a'
        assert_eq!(key_from_keyval(0x41), Key::KeyA); // 'A'
        assert_eq!(key_from_keyval(KEY_UP), Key::ArrowUp);
    }

    /// End-to-end proof of the keyboard loop with the REAL translation
    /// function, no GTK event injection: script GDK keyvals for `h e l l o
    /// Enter` through [`gdk_key_to_bytes`], write the encoded bytes to a pty
    /// master, and read the line-discipline echo back — asserting it carries
    /// "hello". This exercises exactly the encode→pty path the GTK key handler
    /// runs; only the event source (a scripted keyval list vs. a live
    /// `EventControllerKey`) differs.
    ///
    /// Uses a raw `qwertty_term_termio::pty` pair (cooked mode, ECHO on by
    /// default) rather than spawning a shell, so the echo is deterministic and
    /// doesn't depend on any program being present.
    #[test]
    fn keyboard_echo_round_trip() {
        use qwertty_term_termio::pty::{Pty, Winsize};
        use std::io::{Read, Write};
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        let pty = Pty::open(Winsize::default()).expect("openpty");
        let (master, slave) = pty.into_parts();
        // One master handle to read the echo, one to write keystrokes.
        let master_read = master.try_clone().expect("clone master");
        let mut writer = std::fs::File::from(master);

        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let reader_buf = Arc::clone(&buf);
        let reader = std::thread::spawn(move || {
            let mut f = std::fs::File::from(master_read);
            let mut chunk = [0u8; 1024];
            loop {
                match f.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => reader_buf.lock().unwrap().extend_from_slice(&chunk[..n]),
                }
            }
        });

        // Script the keyvals for "hello" + Enter and feed them through the same
        // translation the GTK handler uses.
        let opts = EncodeOptions::default();
        let keyvals = [0x68u32, 0x65, 0x6c, 0x6c, 0x6f, KEY_RETURN]; // h e l l o ⏎
        for kv in keyvals {
            if let Some(bytes) = gdk_key_to_bytes(Action::Press, kv, 0, 0, &opts) {
                writer.write_all(&bytes).expect("write to pty");
            }
        }
        writer.flush().ok();

        // Poll the echo buffer until "hello" shows up (or time out).
        let deadline = Instant::now() + Duration::from_secs(5);
        let found = loop {
            if buf.lock().unwrap().windows(5).any(|w| w == b"hello") {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        let echo = String::from_utf8_lossy(&buf.lock().unwrap()).into_owned();
        // Dropping the slave hangs up the master so the reader thread exits.
        drop(slave);
        let _ = reader.join();

        assert!(
            found,
            "pty did not echo 'hello' back (got {echo:?}); the encode→pty loop is broken",
        );
    }
}
