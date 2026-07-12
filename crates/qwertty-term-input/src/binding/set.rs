//! The binding set: the runtime trigger→action table. Port of the dispatch
//! core of `Binding.Set` in `input/Binding.zig` (upstream `2da015cd6`,
//! lines 2045-2695).
//!
//! Scope of this slice: the forward map with **case-folded, `mods.binding()`-
//! normalized** lookup, `put` (with overwrite), the 5-probe [`Set::get_event`],
//! and `remove`. Deferred to later slices (see `docs/analysis/keybinds.md` and
//! issue #24): sequences/leaders + chains + `unbind` (`parse_and_put`), and the
//! reverse action→trigger map used for GUI menu accelerators.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::key::KeyEvent;

use super::Action;
use super::flags::Flags;
use super::parser::Binding;
use super::trigger::{Trigger, TriggerKey};

/// The value stored for a bound trigger: an action plus its flags. Port of
/// `Set.Leaf` (the non-sequence, non-chained value).
#[derive(Debug, Clone, PartialEq)]
pub struct Bound {
    pub action: Action,
    pub flags: Flags,
}

/// The case folding of a codepoint used for trigger comparison. Port of
/// `Trigger.foldedCodepoint` (Binding.zig:1957-1972).
///
/// ASCII letters fast-path to their lowercase form; other codepoints use
/// `char::to_lowercase` when that yields exactly one codepoint, otherwise fall
/// back to the codepoint unchanged (matching upstream's "if more codepoints are
/// produced then we return the codepoint as-is" behavior — a zero-dependency
/// stand-in for `uucode`'s full case folding). The three-element shape mirrors
/// upstream so a future full-folding implementation is a drop-in.
fn folded_codepoint(cp: u32) -> [u32; 3] {
    if let Some(c) = char::from_u32(cp) {
        if c.is_ascii_alphabetic() {
            return [(c as u8).to_ascii_lowercase() as u32, 0, 0];
        }
        let mut it = c.to_lowercase();
        if let (Some(first), None) = (it.next(), it.next()) {
            return [first as u32, 0, 0];
        }
    }
    [cp, 0, 0]
}

/// A [`Trigger`] wrapped with the folded, binding-normalized `Hash`/`Eq` used
/// by the set's forward map. Port of `Set.Context` / `Trigger.bindingSetEqual`
/// (= `foldedEqual`, Binding.zig:2005) and `hashIncremental` (1942-1954).
///
/// Equality compares `mods` directly and the key with unicode codepoints
/// case-folded (so `ctrl+A` == `ctrl+a`); the hash uses the folded codepoint
/// and `mods.binding()`. `a == b` implies `hash(a) == hash(b)` because equal
/// mods have equal `binding()` and folded-equal codepoints hash identically.
#[derive(Debug, Clone, Copy)]
struct SetKey(Trigger);

impl PartialEq for SetKey {
    fn eq(&self, other: &Self) -> bool {
        // foldedEqual: mods compared directly (Binding.zig:1989-2003).
        if self.0.mods != other.0.mods {
            return false;
        }
        match (self.0.key, other.0.key) {
            (TriggerKey::Physical(a), TriggerKey::Physical(b)) => a == b,
            (TriggerKey::Unicode(a), TriggerKey::Unicode(b)) => {
                folded_codepoint(a) == folded_codepoint(b)
            }
            (TriggerKey::CatchAll, TriggerKey::CatchAll) => true,
            _ => false,
        }
    }
}

impl Eq for SetKey {}

impl Hash for SetKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Match hashIncremental: tag, then folded codepoint / physical key,
        // then mods.binding().
        match self.0.key {
            TriggerKey::Physical(k) => {
                0u8.hash(state);
                k.hash(state);
            }
            TriggerKey::Unicode(cp) => {
                1u8.hash(state);
                folded_codepoint(cp).hash(state);
            }
            TriggerKey::CatchAll => {
                2u8.hash(state);
            }
        }
        self.0.mods.binding().hash(state);
    }
}

/// The trigger→action binding table. Port of the dispatch core of `Binding.Set`.
#[derive(Debug, Clone, Default)]
pub struct Set {
    forward: HashMap<SetKey, Bound>,
}

impl Set {
    /// An empty set.
    pub fn new() -> Self {
        Set::default()
    }

    /// Insert (or overwrite) a single binding. Rebinding a trigger replaces the
    /// previous value — matching `Set.put`'s last-wins overwrite (which is why
    /// e.g. the default `ctrl+shift+w` registered twice ends up as the second
    /// binding). Folded equality means `ctrl+A` and `ctrl+a` are the same key.
    pub fn put(&mut self, binding: Binding) {
        self.forward.insert(
            SetKey(binding.trigger),
            Bound {
                action: binding.action,
                flags: binding.flags,
            },
        );
    }

    /// Look up an exact trigger (folded, binding-normalized). Port of `Set.get`.
    pub fn get(&self, trigger: Trigger) -> Option<&Bound> {
        self.forward.get(&SetKey(trigger))
    }

    /// Remove a trigger's binding, returning it if present. Port of the leaf
    /// path of `Set.remove` (no reverse-map maintenance yet).
    pub fn remove(&mut self, trigger: Trigger) -> Option<Bound> {
        self.forward.remove(&SetKey(trigger))
    }

    /// Number of bindings in the set.
    pub fn len(&self) -> usize {
        self.forward.len()
    }

    /// True if the set has no bindings.
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    /// Resolve a key event to its bound action, using the same probe order as
    /// `Set.getEvent` (Binding.zig:2657-2695):
    ///
    /// 1. physical key with `event.mods.binding()`
    /// 2. the single Unicode codepoint of `event.utf8`, if it is exactly one
    /// 3. `event.unshifted_codepoint`, if non-zero
    /// 4. `catch_all` with the event mods
    /// 5. `catch_all` with empty mods (only if the event had mods)
    ///
    /// Physical triggers never match unicode events and vice versa. Consumed /
    /// performable handling is the caller's job — it reads the returned
    /// [`Bound::flags`].
    pub fn get_event(&self, event: &KeyEvent) -> Option<&Bound> {
        let mods = event.mods.binding();
        let mut trigger = Trigger {
            mods,
            key: TriggerKey::Physical(event.key),
        };
        if let Some(v) = self.get(trigger) {
            return Some(v);
        }

        // Exactly one codepoint of utf8.
        if !event.utf8.is_empty() {
            let mut chars = event.utf8.chars();
            if let (Some(cp), None) = (chars.next(), chars.next()) {
                trigger.key = TriggerKey::Unicode(cp as u32);
                if let Some(v) = self.get(trigger) {
                    return Some(v);
                }
            }
        }

        // Fallback to the unshifted codepoint.
        if event.unshifted_codepoint > 0 {
            trigger.key = TriggerKey::Unicode(event.unshifted_codepoint);
            if let Some(v) = self.get(trigger) {
                return Some(v);
            }
        }

        // catch_all with mods, then without.
        trigger.key = TriggerKey::CatchAll;
        if let Some(v) = self.get(trigger) {
            return Some(v);
        }
        if !mods.empty() {
            trigger.mods = crate::key_mods::Mods::default();
            if let Some(v) = self.get(trigger) {
                return Some(v);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::Key;
    use crate::key_mods::Mods;

    fn bind(raw: &str) -> Binding {
        Binding::parse(raw).unwrap()
    }

    fn event(key: Key, mods: Mods, utf8: &str, unshifted: u32) -> KeyEvent {
        KeyEvent {
            key,
            mods,
            utf8: utf8.to_string(),
            unshifted_codepoint: unshifted,
            ..KeyEvent::default()
        }
    }

    fn ctrl() -> Mods {
        Mods {
            ctrl: true,
            ..Mods::default()
        }
    }

    #[test]
    fn put_and_get() {
        let mut set = Set::new();
        set.put(bind("ctrl+a=ignore"));
        assert_eq!(set.len(), 1);
        let t = Trigger {
            key: TriggerKey::Unicode('a' as u32),
            mods: ctrl(),
        };
        assert_eq!(set.get(t).unwrap().action, Action::Ignore);
        // A miss returns None.
        assert!(
            set.get(Trigger {
                key: TriggerKey::Unicode('b' as u32),
                mods: ctrl()
            })
            .is_none()
        );
    }

    #[test]
    fn overwrite_last_wins() {
        let mut set = Set::new();
        set.put(bind("ctrl+shift+w=close_surface"));
        set.put(bind("ctrl+shift+w=close_tab"));
        assert_eq!(set.len(), 1);
        let t = bind("ctrl+shift+w=close_surface").trigger;
        assert_eq!(
            set.get(t).unwrap().action,
            Action::CloseTab(super::super::action::CloseTabMode::This)
        );
    }

    #[test]
    fn case_folding_both_directions() {
        // Binding uppercase, event lowercase.
        let mut set = Set::new();
        set.put(bind("ctrl+A=ignore"));
        // Physical won't match (unicode binding); the utf8/unshifted path does.
        let ev = event(Key::KeyA, ctrl(), "a", 'a' as u32);
        assert!(set.get_event(&ev).is_some());

        // Binding lowercase, event uppercase codepoint.
        let mut set2 = Set::new();
        set2.put(bind("ctrl+a=ignore"));
        let ev2 = event(Key::KeyA, ctrl(), "A", 'A' as u32);
        assert!(set2.get_event(&ev2).is_some());
    }

    #[test]
    fn physical_and_unicode_are_separate() {
        let mut set = Set::new();
        set.put(bind("key_a=ignore")); // physical
        // A unicode 'a' event should NOT match a physical binding.
        let ev = event(Key::KeyA, Mods::default(), "", 'a' as u32);
        // get_event probes physical(event.key) first → matches the physical bind.
        assert!(set.get_event(&ev).is_some());
        // But a direct unicode lookup must miss.
        assert!(
            set.get(Trigger {
                key: TriggerKey::Unicode('a' as u32),
                mods: Mods::default()
            })
            .is_none()
        );
    }

    #[test]
    fn catch_all_fallback_mods_then_none() {
        let mut set = Set::new();
        set.put(bind("catch_all=ignore"));
        // An event with mods falls back to catch_all (mods), and if that were
        // absent, to catch_all with no mods.
        let ev = event(Key::KeyZ, ctrl(), "z", 'z' as u32);
        assert!(set.get_event(&ev).is_some());
    }

    #[test]
    fn get_event_physical_first() {
        let mut set = Set::new();
        set.put(bind("physical:kp_enter=ignore"));
        let ev = event(Key::NumpadEnter, Mods::default(), "", 0);
        assert!(set.get_event(&ev).is_some());
    }
}
