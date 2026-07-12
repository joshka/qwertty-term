//! User-configurable `keybind` dispatch, backed by the real `Binding.zig` port.
//!
//! The maintainer's real ghostty config contains
//! `keybind = shift+enter=text:\x1b\r`, which makes Shift+Enter send `ESC CR`
//! (many TUIs treat that as "insert newline without submitting"). This module
//! wires the user's `keybind` config entries into
//! [`qwertty_term_input::binding::Set`] — the ported `Binding.zig` trigger/action
//! system — and dispatches the `text:` action in the key path *before* the PTY
//! encoder (the same interception discipline [`crate::tabkeys`] /
//! [`crate::splitkeys`] use, but for arbitrary user chords that emit bytes).
//!
//! This replaces the previous bespoke `text:`-only table: trigger parsing, the
//! action model, and last-wins overwrite now come from the shared port
//! (`Set::parse_and_put`), so every trigger shape and action the port supports
//! parses here. For now only `text:` is *dispatched* at this seam; the other
//! actions (tab/split/search/window/…) are dispatched in later keybind slices as
//! the four bespoke tables are collapsed into this one `Set`.
//!
//! # Upstream syntax (cited)
//!
//! Trigger grammar + `text:` action — `src/input/Binding.zig` (Ghostty commit
//! `2da015cd6`); see `docs/analysis/keybinds.md`. The `text:` value is Zig
//! string-literal syntax, unescaped when the action is performed; supported
//! escapes are `\n` `\r` `\t` `\\` `\"` `\'` `\0` `\xNN` `\u{NNNN}` plus
//! ghostty's `\e` extension for ESC (0x1b). The unescape happens at dispatch
//! (the port stores the raw `text:` value verbatim), via [`unescape_text`].

use qwertty_term_input::binding::{Action, Set, Trigger, TriggerKey};
use qwertty_term_input::key::Key;
use qwertty_term_input::key_mods::Mods;

use crate::tabkeys::TabMods;

/// Build the keybind [`Set`]: the ported upstream default keymap
/// ([`default_set`](qwertty_term_input::binding::default_set), macOS's 93 binds)
/// with the user's `config.keybind` entries layered on top. Each user entry is a
/// full `"<trigger>=<action>"` string parsed by the ported `Binding.zig` system
/// (`Set::parse_and_put`); an invalid entry logs a warning and is skipped, so a
/// single bad line never takes the whole config down (house rule + ghostty's
/// lenient policy). A user entry re-declaring a default trigger overrides it
/// (last-wins), which the `Set` handles.
pub fn build_set(entries: &[String]) -> Set {
    let mut set = qwertty_term_input::binding::default_set();
    for entry in entries {
        if let Err(reason) = set.parse_and_put(entry) {
            eprintln!("qwertty-term: ignoring keybind '{entry}': {reason:?}");
        }
    }
    set
}

/// Resolve a physical key + modifier state to its bound [`Action`] (cloned), or
/// `None` if unbound. Probes the physical trigger then the key's Unicode
/// codepoint, mirroring the first two probes of [`Set::get_event`]. Used by the
/// chord-dispatch path (`performKeyEquivalent:`); the byte-producing `text:`
/// counterpart is [`resolve_text_bytes`].
pub fn resolve_action(set: &Set, key: Key, mods: TabMods) -> Option<Action> {
    lookup_action(set, key, to_mods(mods)).cloned()
}

/// Resolve a physical key + modifier state to the bytes a `text:` binding emits,
/// or `None` if no `text:` binding matches. Probes the physical trigger first,
/// then the key's Unicode codepoint (mirroring the first two probes of
/// [`Set::get_event`]) — a `text:` binding may be declared either way
/// (`shift+enter` parses to physical `enter`; `ctrl+a` parses to unicode `a`).
///
/// Only `text:` is dispatched here for now; any other bound action falls through
/// (returns `None`), preserving today's behaviour until the remaining actions
/// are wired.
pub fn resolve_text_bytes(set: &Set, key: Key, mods: TabMods) -> Option<Vec<u8>> {
    match lookup_action(set, key, to_mods(mods))? {
        Action::Text(value) => unescape_text(value).ok(),
        _ => None,
    }
}

/// The `text:` value stored by the port is the raw, still-escaped string; look
/// it up by trigger and hand back a reference to it.
fn lookup_action(set: &Set, key: Key, mods: Mods) -> Option<&Action> {
    if let Some(bound) = set.get(Trigger {
        key: TriggerKey::Physical(key),
        mods,
    }) {
        return Some(&bound.action);
    }
    let cp = key.codepoint()?;
    set.get(Trigger {
        key: TriggerKey::Unicode(cp),
        mods,
    })
    .map(|bound| &bound.action)
}

/// The four AppKit-free bindable modifiers, already in `mods.binding()` form (no
/// locks/sides), as the `Set`'s forward map expects.
fn to_mods(mods: TabMods) -> Mods {
    Mods {
        shift: mods.shift,
        ctrl: mods.ctrl,
        alt: mods.alt,
        super_: mods.super_,
        ..Mods::default()
    }
}

/// Decode a `text:` value's Zig string-literal escapes to the bytes to send.
/// Supports `\n` `\r` `\t` `\\` `\"` `\'` `\0` `\xNN` `\u{NNNN}` and ghostty's
/// `\e` (ESC). Returns an error string for a malformed escape (the caller skips
/// sending on error).
pub fn unescape_text(value: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            // Ordinary character: push its UTF-8 bytes.
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        // Escape sequence.
        let esc = chars
            .next()
            .ok_or_else(|| "trailing '\\' in text value".to_string())?;
        match esc {
            'n' => out.push(b'\n'),
            'r' => out.push(b'\r'),
            't' => out.push(b'\t'),
            '\\' => out.push(b'\\'),
            '"' => out.push(b'"'),
            '\'' => out.push(b'\''),
            '0' => out.push(0),
            'e' => out.push(0x1b), // ghostty extension: ESC
            'x' => {
                // Exactly two hex digits → one byte.
                let hi = chars
                    .next()
                    .ok_or_else(|| "\\x needs two hex digits".to_string())?;
                let lo = chars
                    .next()
                    .ok_or_else(|| "\\x needs two hex digits".to_string())?;
                let byte = u8::from_str_radix(&format!("{hi}{lo}"), 16)
                    .map_err(|_| format!("invalid \\x hex '{hi}{lo}'"))?;
                out.push(byte);
            }
            'u' => {
                // \u{NNNN} → the codepoint's UTF-8 bytes.
                if chars.next() != Some('{') {
                    return Err("\\u must be followed by '{'".to_string());
                }
                let mut hex = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(h) => hex.push(h),
                        None => return Err("unterminated \\u{...}".to_string()),
                    }
                }
                let cp = u32::from_str_radix(&hex, 16)
                    .map_err(|_| format!("invalid \\u hex '{hex}'"))?;
                let ch = char::from_u32(cp)
                    .ok_or_else(|| format!("\\u{{{hex}}} is not a valid codepoint"))?;
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            other => return Err(format!("unsupported escape '\\{other}'")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shift() -> TabMods {
        TabMods {
            shift: true,
            ..Default::default()
        }
    }

    fn ctrl() -> TabMods {
        TabMods {
            ctrl: true,
            ..Default::default()
        }
    }

    #[test]
    fn maintainer_shift_enter_binding_sends_esc_cr() {
        // The maintainer's real config line.
        let set = build_set(&["shift+enter=text:\\x1b\\r".to_string()]);
        assert_eq!(
            resolve_text_bytes(&set, Key::Enter, shift()),
            Some(b"\x1b\r".to_vec()),
            "shift+enter must send ESC CR"
        );
        // Plain enter (no shift) is NOT bound — it falls through to the encoder
        // (which sends \r), so this binding never swallows a normal Return.
        assert_eq!(
            resolve_text_bytes(&set, Key::Enter, TabMods::default()),
            None
        );
    }

    #[test]
    fn unicode_trigger_binding_resolves_via_codepoint() {
        // `ctrl+a` parses to a *unicode* trigger ('a'); dispatch must find it via
        // the key's codepoint probe, not the physical one.
        let set = build_set(&["ctrl+a=text:\\e[A".to_string()]);
        assert_eq!(
            resolve_text_bytes(&set, Key::KeyA, ctrl()),
            Some(b"\x1b[A".to_vec())
        );
    }

    #[test]
    fn unknown_action_or_trigger_is_skipped_not_fatal() {
        // A bad entry + a good entry: the good one still binds, the bad ones are
        // dropped (warning to stderr), NOT an error.
        let set = build_set(&[
            "shift+enter=totally_bogus_action".to_string(), // unknown action
            "=text:x".to_string(),                          // empty trigger
            "shift+enter=text:\\x1b\\r".to_string(),        // good
        ]);
        assert_eq!(
            resolve_text_bytes(&set, Key::Enter, shift()),
            Some(b"\x1b\r".to_vec())
        );
    }

    #[test]
    fn last_binding_wins_on_trigger_collision() {
        let set = build_set(&[
            "shift+enter=text:a".to_string(),
            "shift+enter=text:b".to_string(),
        ]);
        assert_eq!(
            resolve_text_bytes(&set, Key::Enter, shift()),
            Some(b"b".to_vec())
        );
    }

    #[test]
    fn non_text_action_falls_through_at_this_seam() {
        // A bound non-`text:` action parses fine but is not dispatched here yet
        // (returns None → falls through), matching pre-collapse behaviour.
        let set = build_set(&["shift+enter=new_tab".to_string()]);
        assert_eq!(resolve_text_bytes(&set, Key::Enter, shift()), None);
    }

    #[test]
    fn unescape_supports_the_documented_subset() {
        assert_eq!(unescape_text("\\x1b\\r").unwrap(), b"\x1b\r");
        assert_eq!(unescape_text("\\n\\t\\\\").unwrap(), b"\n\t\\");
        assert_eq!(unescape_text("\\e").unwrap(), vec![0x1b]);
        assert_eq!(unescape_text("\\0").unwrap(), vec![0]);
        assert_eq!(unescape_text("plain").unwrap(), b"plain");
        // \u{...} → UTF-8.
        assert_eq!(unescape_text("\\u{1b}").unwrap(), vec![0x1b]);
        assert_eq!(unescape_text("\\u{e9}").unwrap(), "é".as_bytes());
    }

    #[test]
    fn unescape_rejects_bad_sequences() {
        assert!(unescape_text("\\").is_err());
        assert!(unescape_text("\\q").is_err());
        assert!(unescape_text("\\xZZ").is_err());
        assert!(unescape_text("\\u1b").is_err());
    }
}
