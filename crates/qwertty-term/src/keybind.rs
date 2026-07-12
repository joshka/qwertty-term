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

/// Resolve a physical key + modifier state to the bytes a byte-emitting binding
/// sends to the pty (`text:` / `esc:` / `csi:`), or `None` for any other (or no)
/// binding. Probes the physical trigger first, then the key's Unicode codepoint
/// (mirroring the first two probes of [`Set::get_event`]) — a byte binding may be
/// declared either way (`shift+enter` parses to physical `enter`; `ctrl+a` parses
/// to unicode `a`).
///
/// The three byte actions, per `Surface.performBindingAction` (Binding.zig /
/// Surface.zig): `text:` sends its Zig-string-literal value decoded via
/// [`unescape_text`]; `esc:` sends `ESC` + the raw value; `csi:` sends `ESC [` +
/// the raw value (the value is not string-literal-unescaped for `esc`/`csi`).
/// This is what makes e.g. the default `alt+left`=`esc:b` / `alt+right`=`esc:f`
/// word-motion bindings work. Non-byte actions fall through (`None`) — chord
/// actions dispatch via [`crate::app::Controller::perform_keybind_chord`], and
/// menu-covered chords via the menu.
pub fn resolve_text_bytes(set: &Set, key: Key, mods: TabMods) -> Option<Vec<u8>> {
    action_bytes(lookup_action(set, key, to_mods(mods))?)
}

/// The pty bytes a byte-emitting action (`text:` / `esc:` / `csi:`) sends, or
/// `None` for any other action. `text:` decodes its Zig-string-literal value via
/// [`unescape_text`]; `esc:` is `ESC` + the raw value; `csi:` is `ESC [` + the
/// raw value. Shared by the single-key seam ([`resolve_text_bytes`]) and the
/// leader-sequence dispatch.
pub fn action_bytes(action: &Action) -> Option<Vec<u8>> {
    match action {
        Action::Text(value) => unescape_text(value).ok(),
        Action::Esc(value) => {
            let mut bytes = vec![0x1b];
            bytes.extend_from_slice(value.as_bytes());
            Some(bytes)
        }
        Action::Csi(value) => {
            let mut bytes = vec![0x1b, b'['];
            bytes.extend_from_slice(value.as_bytes());
            Some(bytes)
        }
        _ => None,
    }
}

/// The outcome of feeding one key to an in-progress leader sequence (see
/// [`sequence_step`]).
#[derive(Debug, Clone, PartialEq)]
pub enum SeqStep {
    /// The key completed a binding; dispatch this action and end the sequence.
    Leaf(Action),
    /// The key is a further leader; the sequence continues one level deeper. The
    /// carried [`Trigger`] is the one that matched (pushed onto the path).
    Descend(Trigger),
    /// The key matches nothing at this level; abort the sequence.
    NoMatch,
}

/// Feed one key to a leader sequence. `path` is the leader triggers pressed so
/// far (empty ⇒ we're matching against the root, i.e. testing whether `key` is
/// itself a leader or a top-level leaf). Navigates the `Set`'s
/// `Leader`/`Leaf` storage: descends each `path` leader from the root, then looks
/// up `(key, mods)` (physical trigger then the key's codepoint) in that level —
/// a `Leaf` completes, a `Leader` descends, anything else is [`SeqStep::NoMatch`].
///
/// Runtime port of the leader-key half of Ghostty's `getEvent` sequence
/// handling; the idle-timeout flush is deferred (a follow-up).
pub fn sequence_step(set: &Set, path: &[Trigger], key: Key, mods: TabMods) -> SeqStep {
    // Descend to the current level along the leader path.
    let mut level = set;
    for t in path {
        match level.get_leader(*t) {
            Some(child) => level = child,
            None => return SeqStep::NoMatch, // stale path (config changed)
        }
    }
    let m = to_mods(mods);
    for trigger in candidate_triggers(key, m) {
        if let Some(bound) = level.get(trigger) {
            return SeqStep::Leaf(bound.action.clone());
        }
        if level.get_leader(trigger).is_some() {
            return SeqStep::Descend(trigger);
        }
    }
    SeqStep::NoMatch
}

/// The physical then codepoint triggers to probe for a `(key, mods)`, matching
/// the first two probes of [`Set::get_event`].
fn candidate_triggers(key: Key, mods: Mods) -> impl Iterator<Item = Trigger> {
    let physical = Some(Trigger {
        key: TriggerKey::Physical(key),
        mods,
    });
    let unicode = key.codepoint().map(|cp| Trigger {
        key: TriggerKey::Unicode(cp),
        mods,
    });
    physical.into_iter().chain(unicode)
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

    fn alt() -> TabMods {
        TabMods {
            alt: true,
            ..Default::default()
        }
    }

    fn ctrl_alt() -> TabMods {
        TabMods {
            ctrl: true,
            alt: true,
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
    fn esc_action_sends_esc_plus_value() {
        // The default `alt+left`=`esc:b` word-motion binding: ESC + "b".
        let set = build_set(&["alt+left=esc:b".to_string()]);
        assert_eq!(
            resolve_text_bytes(&set, Key::ArrowLeft, alt()),
            Some(b"\x1bb".to_vec())
        );
    }

    #[test]
    fn csi_action_sends_esc_bracket_plus_value() {
        let set = build_set(&["ctrl+alt+a=csi:1;2A".to_string()]);
        assert_eq!(
            resolve_text_bytes(&set, Key::KeyA, ctrl_alt()),
            Some(b"\x1b[1;2A".to_vec())
        );
    }

    #[test]
    fn leader_sequence_steps_descend_then_complete() {
        let set = build_set(&["ctrl+a>c=text:zz".to_string()]);

        // `ctrl+a` at the root is a leader → descend (carrying its trigger).
        let leader = match sequence_step(&set, &[], Key::KeyA, ctrl()) {
            SeqStep::Descend(t) => t,
            other => panic!("expected Descend, got {other:?}"),
        };

        // Then `c` completes the sequence → its leaf action.
        assert_eq!(
            sequence_step(&set, &[leader], Key::KeyC, TabMods::default()),
            SeqStep::Leaf(Action::Text("zz".to_string()))
        );

        // An unrelated key after the leader aborts (NoMatch).
        assert_eq!(
            sequence_step(&set, &[leader], Key::KeyX, TabMods::default()),
            SeqStep::NoMatch
        );

        // `c` at the *root* (no leader pressed) is not a sequence key here.
        assert_eq!(
            sequence_step(&set, &[], Key::KeyC, TabMods::default()),
            SeqStep::NoMatch
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
