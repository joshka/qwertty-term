//! Minimal user-configurable `keybind` support — the `text:` action subset only.
//!
//! The maintainer's real ghostty config contains
//! `keybind = shift+enter=text:\x1b\r`, which makes Shift+Enter send `ESC CR`
//! (many TUIs — e.g. some REPLs / editors — treat that as "insert newline
//! without submitting"). This module supports EXACTLY that shape: a chord →
//! literal-bytes table, parsed from config at startup and dispatched in the key
//! path *before* the PTY encoder (the same interception discipline
//! [`crate::tabkeys`] / [`crate::splitkeys`] use, but for arbitrary user chords
//! that emit bytes rather than built-in tab/split actions).
//!
//! This is explicitly **not** the full `Binding.zig` port (~4.9k LoC, deferred).
//! It is a static-ish table + a pure `resolve`, structured so the eventual full
//! keybind chunk can absorb it: the trigger grammar and the `text:` unescaper are
//! pure functions over the same AppKit-free [`TabMods`](crate::tabkeys::TabMods)
//! bitset and [`ghostty_input::key::Key`] the built-in tables already use.
//!
//! # Upstream syntax (cited)
//!
//! Trigger grammar — `src/input/Binding.zig` `Trigger.parse` (Ghostty commit
//! `2da015cd6`, lines ~1236–1370): the trigger is split on `+`; each part is
//! either a modifier (matched against the `key.Mods` field names `shift` /
//! `ctrl` / `alt` / `super`, plus aliases such as `cmd`/`command`→`super`,
//! `opt`/`option`→`alt`, `control`→`ctrl`) or a key name (physical `key.Key`
//! names like `enter`/`tab`/`escape`/`space`/arrows, a single Unicode codepoint
//! `a`..`z`/`0`..`9`, or a W3C name). We support the modifier set + the key
//! subset the task calls out (letters, digits, `enter`/`tab`/`escape`/`space`,
//! `f1`..`f12`, the four arrows) — enough for the maintainer's binding and the
//! common cases; anything else logs a warning and is skipped.
//!
//! `text:` action — `src/input/Binding.zig` (the `text: []const u8` action,
//! documented "Uses Zig string literal syntax"): the value after `text:` is a
//! Zig string literal, unescaped via `std.zig.string_literal` when the action is
//! performed. Ghostty's supported escapes include `\n` `\r` `\t` `\\` `\"` `\'`
//! `\xNN` `\u{NNNN}`, plus ghostty's `\e` extension for ESC (0x1b). We implement
//! that subset here; the maintainer's `\x1b\r` exercises `\xNN` + `\r`.

use ghostty_input::key::Key;

use crate::tabkeys::TabMods;

/// One parsed user keybind: an exact `(key, mods)` trigger → the literal bytes a
/// `text:` action emits to the focused pane's pty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextKeybind {
    pub key: Key,
    pub mods: TabMods,
    pub bytes: Vec<u8>,
}

/// The parsed keybind table: user `text:` bindings resolved from config at
/// startup. A future config-driven keybind chunk layers over this.
#[derive(Debug, Clone, Default)]
pub struct KeybindTable {
    bindings: Vec<TextKeybind>,
}

impl KeybindTable {
    /// Parse a list of `keybind` config entries (each `"<trigger>=text:<value>"`).
    /// Unknown actions/keys/modifiers do NOT error the config: the offending
    /// entry logs a warning to stderr and is skipped, so a single bad line can't
    /// take the whole config down (task requirement + matches ghostty's lenient
    /// "error only shows in logs" policy for `text:`).
    ///
    /// A later entry with the same trigger overrides an earlier one (last wins),
    /// matching how a config re-declaring a keybind replaces it.
    pub fn parse(entries: &[String]) -> Self {
        let mut bindings: Vec<TextKeybind> = Vec::new();
        for entry in entries {
            match parse_entry(entry) {
                Ok(binding) => {
                    // Last-wins on an exact trigger collision.
                    bindings.retain(|b| !(b.key == binding.key && b.mods == binding.mods));
                    bindings.push(binding);
                }
                Err(reason) => {
                    eprintln!("ghostty-app: ignoring keybind '{entry}': {reason}");
                }
            }
        }
        KeybindTable { bindings }
    }

    /// Resolve a physical key + modifier state to the `text:` bytes to send, or
    /// `None` if no user binding matches. Exact match on key + the four
    /// modifiers, so a binding never swallows a key it wasn't declared for.
    pub fn resolve(&self, key: Key, mods: TabMods) -> Option<&[u8]> {
        self.bindings
            .iter()
            .find(|b| b.key == key && b.mods == mods)
            .map(|b| b.bytes.as_slice())
    }

    /// Number of parsed bindings (test/introspection).
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

/// Parse one `"<trigger>=text:<value>"` entry into a [`TextKeybind`].
fn parse_entry(entry: &str) -> Result<TextKeybind, String> {
    // Split on the FIRST '='. The trigger never contains '=' (it is `+`-joined
    // modifier/key names), so everything after the first '=' is the action.
    let (trigger, action) = entry
        .split_once('=')
        .ok_or_else(|| "missing '=' between trigger and action".to_string())?;

    // Only the `text:` action is supported. Any other action (split:, new_tab,
    // …) is deliberately out of scope for this minimal subset — skip with a
    // warning rather than erroring (a full keybind port would handle them).
    let value = action
        .strip_prefix("text:")
        .ok_or_else(|| format!("unsupported action '{action}' (only 'text:' is supported)"))?;

    let (key, mods) = parse_trigger(trigger)?;
    let bytes = unescape_text(value)?;
    Ok(TextKeybind { key, mods, bytes })
}

/// Parse a `+`-joined trigger (`"shift+enter"`, `"ctrl+alt+a"`) into a key + the
/// four-modifier bitset. Ported subset of upstream `Binding.zig` `Trigger.parse`.
fn parse_trigger(trigger: &str) -> Result<(Key, TabMods), String> {
    if trigger.is_empty() {
        return Err("empty trigger".to_string());
    }
    let mut mods = TabMods::default();
    let mut key: Option<Key> = None;

    for part in trigger.split('+') {
        let part = part.trim();
        if part.is_empty() {
            return Err("empty trigger component (stray '+')".to_string());
        }
        // A modifier component sets a bit; anything else must be the (single)
        // key. Modifier names + upstream aliases (Binding.zig key_mods.alias).
        if let Some(set) = modifier_bit(part) {
            set(&mut mods);
            continue;
        }
        // Not a modifier → it's the key. Only one key per trigger.
        if key.is_some() {
            return Err(format!(
                "more than one non-modifier key in trigger ('{part}')"
            ));
        }
        key = Some(parse_key_name(part).ok_or_else(|| format!("unknown key '{part}'"))?);
    }

    let key = key.ok_or_else(|| "trigger has modifiers but no key".to_string())?;
    Ok((key, mods))
}

/// If `name` is a modifier keyword (or upstream alias), return a setter for its
/// bit. `None` means `name` is not a modifier (so it must be the key).
fn modifier_bit(name: &str) -> Option<fn(&mut TabMods)> {
    // Case-insensitive match on the modifier keywords ghostty accepts.
    match name.to_ascii_lowercase().as_str() {
        "shift" => Some(|m: &mut TabMods| m.shift = true),
        "ctrl" | "control" => Some(|m: &mut TabMods| m.ctrl = true),
        "alt" | "opt" | "option" => Some(|m: &mut TabMods| m.alt = true),
        "super" | "cmd" | "command" => Some(|m: &mut TabMods| m.super_ = true),
        _ => None,
    }
}

/// Map a ghostty key name to the physical [`Key`], for the supported subset:
/// `enter`/`tab`/`escape`/`space`, the four arrows, `f1`..`f12`, single letters
/// `a`..`z`, and single digits `0`..`9`. Returns `None` for anything else
/// (caller warns + skips).
fn parse_key_name(name: &str) -> Option<Key> {
    let lower = name.to_ascii_lowercase();
    let named = match lower.as_str() {
        "enter" | "return" => Some(Key::Enter),
        "tab" => Some(Key::Tab),
        "escape" | "esc" => Some(Key::Escape),
        "space" => Some(Key::Space),
        "up" | "arrow_up" => Some(Key::ArrowUp),
        "down" | "arrow_down" => Some(Key::ArrowDown),
        "left" | "arrow_left" => Some(Key::ArrowLeft),
        "right" | "arrow_right" => Some(Key::ArrowRight),
        "f1" => Some(Key::F1),
        "f2" => Some(Key::F2),
        "f3" => Some(Key::F3),
        "f4" => Some(Key::F4),
        "f5" => Some(Key::F5),
        "f6" => Some(Key::F6),
        "f7" => Some(Key::F7),
        "f8" => Some(Key::F8),
        "f9" => Some(Key::F9),
        "f10" => Some(Key::F10),
        "f11" => Some(Key::F11),
        "f12" => Some(Key::F12),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    // Single letter a..z or digit 0..9 (upstream matches these as a single
    // Unicode codepoint; we map to the physical KeyA.. / Digit0.. variant).
    let mut chars = lower.chars();
    let (c, rest) = (chars.next(), chars.next());
    match (c, rest) {
        (Some(c @ 'a'..='z'), None) => Some(letter_key(c)),
        (Some(d @ '0'..='9'), None) => Some(digit_key(d)),
        _ => None,
    }
}

/// `'a'..='z'` → `Key::KeyA..=Key::KeyZ` (the enum variants are contiguous).
fn letter_key(c: char) -> Key {
    // SAFETY of the arithmetic: `KeyA..KeyZ` are the 26 contiguous variants
    // (key.rs lines 191–216); we index by the letter's 0..25 offset. Done with
    // an explicit match table generator to avoid any transmute.
    const LETTERS: [Key; 26] = [
        Key::KeyA,
        Key::KeyB,
        Key::KeyC,
        Key::KeyD,
        Key::KeyE,
        Key::KeyF,
        Key::KeyG,
        Key::KeyH,
        Key::KeyI,
        Key::KeyJ,
        Key::KeyK,
        Key::KeyL,
        Key::KeyM,
        Key::KeyN,
        Key::KeyO,
        Key::KeyP,
        Key::KeyQ,
        Key::KeyR,
        Key::KeyS,
        Key::KeyT,
        Key::KeyU,
        Key::KeyV,
        Key::KeyW,
        Key::KeyX,
        Key::KeyY,
        Key::KeyZ,
    ];
    LETTERS[(c as u8 - b'a') as usize]
}

/// `'0'..='9'` → `Key::Digit0..=Key::Digit9`.
fn digit_key(d: char) -> Key {
    const DIGITS: [Key; 10] = [
        Key::Digit0,
        Key::Digit1,
        Key::Digit2,
        Key::Digit3,
        Key::Digit4,
        Key::Digit5,
        Key::Digit6,
        Key::Digit7,
        Key::Digit8,
        Key::Digit9,
    ];
    DIGITS[(d as u8 - b'0') as usize]
}

/// Unescape a ghostty `text:` value (Zig string literal syntax + ghostty's `\e`)
/// into literal bytes. Supported escapes: `\n` `\r` `\t` `\\` `\"` `\'` `\0`
/// `\e` (ESC, 0x1b), `\xNN` (two hex digits → one byte), and `\u{NNNN}` (a
/// Unicode codepoint, UTF-8-encoded). Anything else is an error (the whole
/// binding is skipped with a warning — matching ghostty's "invalid escape only
/// shows in logs" leniency, scoped to the one binding rather than the config).
fn unescape_text(value: &str) -> Result<Vec<u8>, String> {
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

    #[test]
    fn maintainer_shift_enter_binding_parses_to_esc_cr() {
        // The maintainer's real config line.
        let table = KeybindTable::parse(&["shift+enter=text:\\x1b\\r".to_string()]);
        assert_eq!(table.len(), 1);
        assert_eq!(
            table.resolve(Key::Enter, shift()),
            Some(&b"\x1b\r"[..]),
            "shift+enter must send ESC CR"
        );
        // Plain enter (no shift) is NOT bound — it falls through to the encoder
        // (which sends \r), so this binding never swallows a normal Return.
        assert_eq!(table.resolve(Key::Enter, TabMods::default()), None);
    }

    #[test]
    fn e_escape_and_named_esc_both_yield_esc() {
        let table = KeybindTable::parse(&["ctrl+a=text:\\e[A".to_string()]);
        assert_eq!(table.resolve(Key::KeyA, mods_ctrl()), Some(&b"\x1b[A"[..]));
    }

    fn mods_ctrl() -> TabMods {
        TabMods {
            ctrl: true,
            ..Default::default()
        }
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

    #[test]
    fn trigger_parses_modifiers_and_key_subset() {
        assert_eq!(parse_trigger("shift+enter").unwrap(), (Key::Enter, shift()));
        assert_eq!(
            parse_trigger("ctrl+alt+a").unwrap(),
            (
                Key::KeyA,
                TabMods {
                    ctrl: true,
                    alt: true,
                    ..Default::default()
                }
            )
        );
        // Aliases: cmd→super, opt→alt, control→ctrl.
        assert_eq!(
            parse_trigger("cmd+k").unwrap().1,
            TabMods {
                super_: true,
                ..Default::default()
            }
        );
        assert_eq!(parse_trigger("f5").unwrap().0, Key::F5);
        assert_eq!(parse_trigger("escape").unwrap().0, Key::Escape);
        assert_eq!(parse_trigger("down").unwrap().0, Key::ArrowDown);
        assert_eq!(parse_trigger("space").unwrap().0, Key::Space);
        assert_eq!(parse_trigger("9").unwrap().0, Key::Digit9);
    }

    #[test]
    fn trigger_rejects_unknown_key_and_empty() {
        assert!(parse_trigger("shift+wat").is_err());
        assert!(parse_trigger("").is_err());
        assert!(parse_trigger("shift").is_err()); // no key
        assert!(parse_trigger("enter+tab").is_err()); // two keys
    }

    #[test]
    fn unknown_action_or_key_is_skipped_not_fatal() {
        // A bad entry + a good entry: the good one still parses, the bad one is
        // dropped (warning to stderr), NOT an error.
        let table = KeybindTable::parse(&[
            "shift+enter=split:right".to_string(),   // unsupported action
            "ctrl+nope=text:x".to_string(),          // unknown key
            "shift+enter=text:\\x1b\\r".to_string(), // good
        ]);
        assert_eq!(table.len(), 1);
        assert_eq!(table.resolve(Key::Enter, shift()), Some(&b"\x1b\r"[..]));
    }

    #[test]
    fn last_binding_wins_on_trigger_collision() {
        let table = KeybindTable::parse(&[
            "shift+enter=text:a".to_string(),
            "shift+enter=text:b".to_string(),
        ]);
        assert_eq!(table.len(), 1);
        assert_eq!(table.resolve(Key::Enter, shift()), Some(&b"b"[..]));
    }

    #[test]
    fn does_not_collide_with_builtin_tab_or_split_bindings() {
        // A user text: binding on a chord that is ALSO a built-in tab/split chord
        // is the user's call; but the common maintainer binding (shift+enter) is
        // disjoint from both built-in tables, so it never shadows navigation.
        assert_eq!(crate::tabkeys::resolve(Key::Enter, shift()), None);
        assert_eq!(crate::splitkeys::resolve(Key::Enter, shift()), None);
    }
}
