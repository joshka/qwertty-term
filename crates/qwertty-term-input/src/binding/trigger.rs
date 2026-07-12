//! Keybind triggers and their parse grammar. Port of `Trigger` in
//! `input/Binding.zig` (upstream `2da015cd6`, lines 1660-1932) plus the
//! `backwards_compatible_keys` table (1806-1925).
//!
//! A trigger is a key plus a set of modifiers. The key is one of three shapes:
//! a layout-independent physical W3C code, a Unicode codepoint (matched against
//! whatever key produces it), or `catch_all`. The parse grammar's rule *order*
//! is load-bearing — see [`Trigger::parse`].

use crate::key::Key;
use crate::key_mods::{ALIAS, Mods};

use super::BindError;

/// The key half of a [`Trigger`]. Port of `Trigger.Key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriggerKey {
    /// A layout-independent physical key code (W3C `code`). Binds the physical
    /// location regardless of keyboard layout.
    Physical(Key),
    /// Matches any key that produces this Unicode codepoint.
    Unicode(u32),
    /// Matches any key press that is otherwise unbound.
    CatchAll,
}

impl Default for TriggerKey {
    fn default() -> Self {
        // Matches `Trigger.Key`'s default of `.{ .physical = .unidentified }`.
        TriggerKey::Physical(Key::Unidentified)
    }
}

/// A key combination that can be bound to an action. Port of `Trigger`.
///
/// Equality/hashing here is *direct* (`Trigger.equal`, Binding.zig:1976) —
/// mods and key compared as-is. The case-folded, `mods.binding()`-normalized
/// equality that `Set` uses for lookup is a separate concern that will live on
/// the `Set` type; parsed triggers are expected to already be in
/// binding-normalized form for that lookup to work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Trigger {
    pub key: TriggerKey,
    pub mods: Mods,
}

impl Trigger {
    /// True when the key is still at its unset default (`physical =
    /// unidentified`). Port of `Trigger.isKeyUnset`.
    pub fn is_key_unset(&self) -> bool {
        matches!(self.key, TriggerKey::Physical(Key::Unidentified))
    }

    /// Parse a single trigger (no `>` sequences — those are split earlier).
    ///
    /// The grammar splits on `+` and tries each part **in this exact order**;
    /// the order is the spec (Binding.zig:1706-1803):
    ///
    /// 1. modifier field name (`shift`, `ctrl`, `alt`, `super`, `caps_lock`,
    ///    `num_lock`); duplicate → error
    /// 2. modifier alias (`cmd`/`command`→super, `opt`/`option`→alt,
    ///    `control`→ctrl); duplicate (incl. alias-vs-canonical) → error
    /// 3. otherwise it's the key; a second key → error
    /// 4. empty part → literal `+` (Unicode `'+'`)
    /// 5. Ghostty key enum name (`key_a`, `arrow_up`, …), excluding
    ///    `unidentified` → physical
    /// 6. single Unicode codepoint → unicode (this is why bare `a` is unicode,
    ///    not `key_a`)
    /// 7. W3C key name (`KeyA`) → physical
    /// 8. `catch_all`
    /// 9. backwards-compatible ≤1.1.x name (incl. literal `physical:` variants)
    /// 10. otherwise → error
    ///
    /// Note: like upstream, this does not require the key to be set — `ctrl+`
    /// parses to a keyless trigger with `mods.ctrl`.
    pub fn parse(input: &str) -> Result<Trigger, BindError> {
        if input.is_empty() {
            return Err(BindError::InvalidFormat);
        }

        let mut result = Trigger::default();
        let mut rem = input;
        while !rem.is_empty() {
            let (part, next) = match rem.find('+') {
                Some(idx) => (&rem[..idx], &rem[idx + 1..]),
                None => (rem, ""),
            };
            rem = next;

            // Rules 1 & 2: modifier field name or alias (with duplicate check).
            match try_set_mod(&mut result.mods, part) {
                Some(Ok(())) => continue,
                Some(Err(e)) => return Err(e),
                None => {}
            }

            // Rule 3: anything from here is the key; only one key allowed.
            if !result.is_key_unset() {
                return Err(BindError::InvalidFormat);
            }

            // Rule 4: empty part is a literal `+`.
            if part.is_empty() {
                result.key = TriggerKey::Unicode('+' as u32);
                continue;
            }

            // Rule 5: Ghostty key enum name. `from_name` matches `unidentified`
            // too, so exclude it (upstream skips that field).
            match Key::from_name(part) {
                Some(k) if k != Key::Unidentified => {
                    result.key = TriggerKey::Physical(k);
                    continue;
                }
                _ => {}
            }

            // Rule 6: exactly one Unicode codepoint.
            {
                let mut chars = part.chars();
                if let (Some(cp), None) = (chars.next(), chars.next()) {
                    result.key = TriggerKey::Unicode(cp as u32);
                    continue;
                }
            }

            // Rule 7: W3C key name.
            if let Some(k) = Key::from_w3c(part) {
                result.key = TriggerKey::Physical(k);
                continue;
            }

            // Rule 8: catch_all.
            if part == "catch_all" {
                result.key = TriggerKey::CatchAll;
                continue;
            }

            // Rule 9: backwards-compatible ≤1.1.x key names.
            if let Some(tk) = compat_lookup(part) {
                result.key = tk;
                continue;
            }

            // Rule 10: unrecognized.
            return Err(BindError::InvalidFormat);
        }

        Ok(result)
    }
}

/// Try to interpret `name` as a modifier (canonical field name or alias) and
/// set it on `mods`. Returns `None` if `name` is not a modifier, `Some(Err)` if
/// the modifier was already set (duplicate), `Some(Ok)` on success.
///
/// Mirrors upstream's two `inline for` passes: first over `Mods`'s bool fields
/// (`shift`/`ctrl`/`alt`/`super`/`caps_lock`/`num_lock`), then over
/// `key_mods.alias` (which only aliases the four standard mods).
fn try_set_mod(mods: &mut Mods, name: &str) -> Option<Result<(), BindError>> {
    let field: &mut bool = match name {
        "shift" => &mut mods.shift,
        "ctrl" => &mut mods.ctrl,
        "alt" => &mut mods.alt,
        "super" => &mut mods.super_,
        "caps_lock" => &mut mods.caps_lock,
        "num_lock" => &mut mods.num_lock,
        _ => {
            use crate::key_mods::Mod;
            let m = ALIAS.iter().find(|(n, _)| *n == name).map(|(_, m)| *m)?;
            match m {
                Mod::Shift => &mut mods.shift,
                Mod::Ctrl => &mut mods.ctrl,
                Mod::Alt => &mut mods.alt,
                Mod::Super => &mut mods.super_,
            }
        }
    };
    if *field {
        return Some(Err(BindError::InvalidFormat));
    }
    *field = true;
    Some(Ok(()))
}

/// Backwards-compatible ≤1.1.x key names. Port of `backwards_compatible_keys`
/// (Binding.zig:1806-1925). `physical:`-prefixed entries are the only surviving
/// use of a literal `physical:` prefix — it is no longer a general grammar
/// prefix.
fn compat_lookup(name: &str) -> Option<TriggerKey> {
    use Key::*;
    let uni = |c: char| TriggerKey::Unicode(c as u32);
    let phys = |k: Key| TriggerKey::Physical(k);
    Some(match name {
        "zero" => uni('0'),
        "one" => uni('1'),
        "two" => uni('2'),
        "three" => uni('3'),
        "four" => uni('4'),
        "five" => uni('5'),
        "six" => uni('6'),
        "seven" => uni('7'),
        "eight" => uni('8'),
        "nine" => uni('9'),
        "plus" => uni('+'),
        "apostrophe" => uni('\''),
        "grave_accent" => phys(Backquote),
        "left_bracket" => phys(BracketLeft),
        "right_bracket" => phys(BracketRight),
        "up" => phys(ArrowUp),
        "down" => phys(ArrowDown),
        "left" => phys(ArrowLeft),
        "right" => phys(ArrowRight),
        "kp_0" => phys(Numpad0),
        "kp_1" => phys(Numpad1),
        "kp_2" => phys(Numpad2),
        "kp_3" => phys(Numpad3),
        "kp_4" => phys(Numpad4),
        "kp_5" => phys(Numpad5),
        "kp_6" => phys(Numpad6),
        "kp_7" => phys(Numpad7),
        "kp_8" => phys(Numpad8),
        "kp_9" => phys(Numpad9),
        "kp_add" => phys(NumpadAdd),
        "kp_subtract" => phys(NumpadSubtract),
        "kp_multiply" => phys(NumpadMultiply),
        "kp_divide" => phys(NumpadDivide),
        "kp_decimal" => phys(NumpadDecimal),
        "kp_enter" => phys(NumpadEnter),
        "kp_equal" => phys(NumpadEqual),
        "kp_separator" => phys(NumpadSeparator),
        "kp_left" => phys(NumpadLeft),
        "kp_right" => phys(NumpadRight),
        "kp_up" => phys(NumpadUp),
        "kp_down" => phys(NumpadDown),
        "kp_page_up" => phys(NumpadPageUp),
        "kp_page_down" => phys(NumpadPageDown),
        "kp_home" => phys(NumpadHome),
        "kp_end" => phys(NumpadEnd),
        "kp_insert" => phys(NumpadInsert),
        "kp_delete" => phys(NumpadDelete),
        "kp_begin" => phys(NumpadBegin),
        "left_shift" => phys(ShiftLeft),
        "right_shift" => phys(ShiftRight),
        "left_control" => phys(ControlLeft),
        "right_control" => phys(ControlRight),
        "left_alt" => phys(AltLeft),
        "right_alt" => phys(AltRight),
        "left_super" => phys(MetaLeft),
        "right_super" => phys(MetaRight),

        // Physical variants.
        "physical:zero" => phys(Digit0),
        "physical:one" => phys(Digit1),
        "physical:two" => phys(Digit2),
        "physical:three" => phys(Digit3),
        "physical:four" => phys(Digit4),
        "physical:five" => phys(Digit5),
        "physical:six" => phys(Digit6),
        "physical:seven" => phys(Digit7),
        "physical:eight" => phys(Digit8),
        "physical:nine" => phys(Digit9),
        "physical:apostrophe" => phys(Quote),
        "physical:grave_accent" => phys(Backquote),
        "physical:left_bracket" => phys(BracketLeft),
        "physical:right_bracket" => phys(BracketRight),
        "physical:up" => phys(ArrowUp),
        "physical:down" => phys(ArrowDown),
        "physical:left" => phys(ArrowLeft),
        "physical:right" => phys(ArrowRight),
        "physical:kp_0" => phys(Numpad0),
        "physical:kp_1" => phys(Numpad1),
        "physical:kp_2" => phys(Numpad2),
        "physical:kp_3" => phys(Numpad3),
        "physical:kp_4" => phys(Numpad4),
        "physical:kp_5" => phys(Numpad5),
        "physical:kp_6" => phys(Numpad6),
        "physical:kp_7" => phys(Numpad7),
        "physical:kp_8" => phys(Numpad8),
        "physical:kp_9" => phys(Numpad9),
        "physical:kp_add" => phys(NumpadAdd),
        "physical:kp_subtract" => phys(NumpadSubtract),
        "physical:kp_multiply" => phys(NumpadMultiply),
        "physical:kp_divide" => phys(NumpadDivide),
        "physical:kp_decimal" => phys(NumpadDecimal),
        "physical:kp_enter" => phys(NumpadEnter),
        "physical:kp_equal" => phys(NumpadEqual),
        "physical:kp_separator" => phys(NumpadSeparator),
        "physical:kp_left" => phys(NumpadLeft),
        "physical:kp_right" => phys(NumpadRight),
        "physical:kp_up" => phys(NumpadUp),
        "physical:kp_down" => phys(NumpadDown),
        "physical:kp_page_up" => phys(NumpadPageUp),
        "physical:kp_page_down" => phys(NumpadPageDown),
        "physical:kp_home" => phys(NumpadHome),
        "physical:kp_end" => phys(NumpadEnd),
        "physical:kp_insert" => phys(NumpadInsert),
        "physical:kp_delete" => phys(NumpadDelete),
        "physical:kp_begin" => phys(NumpadBegin),
        "physical:left_shift" => phys(ShiftLeft),
        "physical:right_shift" => phys(ShiftRight),
        "physical:left_control" => phys(ControlLeft),
        "physical:right_control" => phys(ControlRight),
        "physical:left_alt" => phys(AltLeft),
        "physical:right_alt" => phys(AltRight),
        "physical:left_super" => phys(MetaLeft),
        "physical:right_super" => phys(MetaRight),

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(f: impl FnOnce(&mut Mods)) -> Mods {
        let mut m = Mods::default();
        f(&mut m);
        m
    }

    /// Port of Binding.zig `parse: triggers` (2845).
    #[test]
    fn parse_triggers() {
        // Single char is unicode.
        assert_eq!(
            Trigger::parse("a").unwrap(),
            Trigger {
                key: TriggerKey::Unicode('a' as u32),
                mods: Mods::default()
            }
        );

        // Single mod.
        assert_eq!(
            Trigger::parse("shift+a").unwrap(),
            Trigger {
                key: TriggerKey::Unicode('a' as u32),
                mods: mods(|m| m.shift = true),
            }
        );

        // Multiple mods.
        assert_eq!(
            Trigger::parse("shift+ctrl+a").unwrap(),
            Trigger {
                key: TriggerKey::Unicode('a' as u32),
                mods: mods(|m| {
                    m.shift = true;
                    m.ctrl = true;
                }),
            }
        );

        // Key can come before mod.
        assert_eq!(
            Trigger::parse("a+shift").unwrap(),
            Trigger {
                key: TriggerKey::Unicode('a' as u32),
                mods: mods(|m| m.shift = true),
            }
        );

        // Ghostty enum name is physical.
        assert_eq!(
            Trigger::parse("key_a").unwrap(),
            Trigger {
                key: TriggerKey::Physical(Key::KeyA),
                mods: Mods::default()
            }
        );

        // Non-ASCII single codepoint is unicode.
        assert_eq!(
            Trigger::parse("ö").unwrap(),
            Trigger {
                key: TriggerKey::Unicode('ö' as u32),
                mods: Mods::default()
            }
        );

        // Errors.
        assert!(Trigger::parse("foo").is_err());
        assert!(Trigger::parse("").is_err());
        assert!(Trigger::parse("shift+shift").is_err()); // duplicate mod
        assert!(Trigger::parse("a+b").is_err()); // two keys
    }

    /// Port of `parse: w3c key names` (2949): case-sensitive W3C names.
    #[test]
    fn parse_w3c_names() {
        assert_eq!(
            Trigger::parse("KeyA").unwrap().key,
            TriggerKey::Physical(Key::KeyA)
        );
        // `Keya` is 4 chars (not one codepoint) and not a valid W3C CamelCase
        // for KeyA, so it fails.
        assert!(Trigger::parse("Keya").is_err());
    }

    /// Port of `parse: catch_all` (2965).
    #[test]
    fn parse_catch_all() {
        assert_eq!(
            Trigger::parse("catch_all").unwrap(),
            Trigger {
                key: TriggerKey::CatchAll,
                mods: Mods::default()
            }
        );
        assert_eq!(
            Trigger::parse("ctrl+catch_all").unwrap(),
            Trigger {
                key: TriggerKey::CatchAll,
                mods: mods(|m| m.ctrl = true)
            }
        );
    }

    /// Port of `parse: plus sign` (2990).
    #[test]
    fn parse_plus_sign() {
        assert_eq!(
            Trigger::parse("+").unwrap().key,
            TriggerKey::Unicode('+' as u32)
        );
        assert_eq!(
            Trigger::parse("ctrl++").unwrap(),
            Trigger {
                key: TriggerKey::Unicode('+' as u32),
                mods: mods(|m| m.ctrl = true)
            }
        );
        // `++` is a double key (empty part twice) → error.
        assert!(Trigger::parse("++").is_err());
    }

    /// Port of `parse: modifier aliases` (3264).
    #[test]
    fn parse_modifier_aliases() {
        type ModSetter = fn(&mut Mods);
        let cases: [(&str, ModSetter); 5] = [
            ("cmd+a", |m| m.super_ = true),
            ("command+a", |m| m.super_ = true),
            ("opt+a", |m| m.alt = true),
            ("option+a", |m| m.alt = true),
            ("control+a", |m| m.ctrl = true),
        ];
        for (spelling, set) in cases {
            assert_eq!(
                Trigger::parse(spelling).unwrap(),
                Trigger {
                    key: TriggerKey::Unicode('a' as u32),
                    mods: mods(set)
                },
                "spelling {spelling}"
            );
        }
        // Alias duplicating its canonical form is a duplicate error.
        assert!(Trigger::parse("ctrl+control+a").is_err());
    }

    /// Port of `parse: backwards compatibility with <= 1.1.x` (3080).
    #[test]
    fn parse_backwards_compatible() {
        // Bare `zero` is unicode '0'; `physical:zero` is physical digit_0.
        assert_eq!(
            Trigger::parse("zero").unwrap().key,
            TriggerKey::Unicode('0' as u32)
        );
        assert_eq!(
            Trigger::parse("physical:zero").unwrap().key,
            TriggerKey::Physical(Key::Digit0)
        );
        assert_eq!(
            Trigger::parse("up").unwrap().key,
            TriggerKey::Physical(Key::ArrowUp)
        );
        assert_eq!(
            Trigger::parse("kp_enter").unwrap().key,
            TriggerKey::Physical(Key::NumpadEnter)
        );
        assert_eq!(
            Trigger::parse("left_super").unwrap().key,
            TriggerKey::Physical(Key::MetaLeft)
        );
        // `zero+one` is two keys → error.
        assert!(Trigger::parse("zero+one").is_err());
    }

    /// A trailing `+` leaves the key unset (faithful to upstream's
    /// `while (rem.len > 0)` loop).
    #[test]
    fn trailing_plus_leaves_key_unset() {
        let t = Trigger::parse("ctrl+").unwrap();
        assert!(t.is_key_unset());
        assert!(t.mods.ctrl);
    }
}
