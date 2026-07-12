//! The binding set: the runtime trigger→action table. Port of the dispatch
//! core of `Binding.Set` in `input/Binding.zig` (upstream `2da015cd6`,
//! lines 2045-2695).
//!
//! Scope so far: the forward map with **case-folded, `mods.binding()`-
//! normalized** lookup, `put` (with overwrite), the 5-probe [`Set::get_event`],
//! `remove`, and the **reverse action→trigger map** ([`Set::get_trigger`]) used
//! for GUI menu accelerators. Deferred to later slices (see
//! `docs/analysis/keybinds.md` and issue #24): sequences/leaders + chains +
//! `unbind` (`parse_and_put`).

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
///
/// The `reverse` map (action→trigger) supports GUI menu accelerators: given an
/// action, find a trigger to display as its shortcut. It is an insertion-ordered
/// `Vec` acting as a one-entry-per-action map (rather than a `HashMap`) because
/// [`Action`] carries `f32` payloads and so is not `Hash`/`Eq`; lookups use
/// `PartialEq`. This matches upstream's `Set.reverse` semantics
/// (Binding.zig:2053-2077): **performable** bindings are never tracked (so
/// toolkits don't register them as menu shortcuts), and only the most recently
/// added trigger per action is kept.
#[derive(Debug, Clone, Default)]
pub struct Set {
    forward: HashMap<SetKey, Bound>,
    reverse: Vec<(Action, Trigger)>,
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
    ///
    /// Reverse-map maintenance mirrors `Set.putFlags` (Binding.zig:2508-2573):
    /// tracked only when the binding is not `performable`; overwriting a trigger
    /// drops the reverse entry that pointed at it, then the new action→trigger
    /// mapping replaces any prior entry for the same action.
    pub fn put(&mut self, binding: Binding) {
        let trigger = binding.trigger;
        let action = binding.action;
        let flags = binding.flags;
        let track_reverse = !flags.performable;
        let key = SetKey(trigger);

        // On overwrite of an existing leaf, drop the old action's reverse entry
        // that pointed at this trigger (gated on the new binding being tracked,
        // matching upstream).
        if track_reverse && self.forward.contains_key(&key) {
            self.reverse.retain(|(_, t)| *t != trigger);
        }

        self.forward.insert(
            key,
            Bound {
                action: action.clone(),
                flags,
            },
        );

        if track_reverse {
            // reverse.put(action, trigger): upsert keyed by action.
            self.reverse.retain(|(a, _)| *a != action);
            self.reverse.push((action, trigger));
        }
    }

    /// Look up an exact trigger (folded, binding-normalized). Port of `Set.get`.
    pub fn get(&self, trigger: Trigger) -> Option<&Bound> {
        self.forward.get(&SetKey(trigger))
    }

    /// Get a trigger bound to the given action, for GUI menu accelerators. An
    /// action may have several triggers; this returns the tracked (most recent,
    /// non-performable) one. Port of `Set.getTrigger` (Binding.zig:2647-2649).
    pub fn get_trigger(&self, action: &Action) -> Option<Trigger> {
        self.reverse
            .iter()
            .find(|(a, _)| a == action)
            .map(|(_, t)| *t)
    }

    /// Remove a trigger's binding, returning it if present. Port of the leaf
    /// path of `Set.removeExact` (Binding.zig:2702-2730) including reverse-map
    /// fixup.
    pub fn remove(&mut self, trigger: Trigger) -> Option<Bound> {
        let removed = self.forward.remove(&SetKey(trigger))?;
        self.fixup_reverse_for_action(&removed.action, trigger);
        Some(removed)
    }

    /// Fix up the reverse mapping after an action's binding is removed. If the
    /// reverse entry for `action` still points at `old`, repoint it to any other
    /// binding with the same action, or drop it if none remain. Port of
    /// `Set.fixupReverseForAction` (Binding.zig:2749-2782).
    fn fixup_reverse_for_action(&mut self, action: &Action, old: Trigger) {
        let Some(idx) = self.reverse.iter().position(|(a, _)| a == action) else {
            return;
        };
        // If the reverse map already points elsewhere, nothing to do.
        if self.reverse[idx].1 != old {
            return;
        }
        // Find another trigger mapping to the same action ("whatever" order,
        // like upstream — the forward map is unordered).
        let other = self
            .forward
            .iter()
            .find(|(_, b)| &b.action == action)
            .map(|(k, _)| k.0);
        match other {
            Some(t) => self.reverse[idx].1 = t,
            None => {
                self.reverse.remove(idx);
            }
        }
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

    #[test]
    fn reverse_get_trigger() {
        let mut set = Set::new();
        set.put(bind("ctrl+t=new_tab"));
        assert_eq!(
            set.get_trigger(&Action::NewTab),
            Some(bind("ctrl+t=ignore").trigger)
        );
        // Unknown action → None.
        assert_eq!(set.get_trigger(&Action::Quit), None);
    }

    /// Port of the intent of `performable exclusion` (Binding.zig test 4065):
    /// performable bindings never enter the reverse map.
    #[test]
    fn reverse_excludes_performable() {
        let mut set = Set::new();
        set.put(bind("performable:cmd+c=copy_to_clipboard"));
        // Forward lookup works…
        assert!(set.get(bind("cmd+c=ignore").trigger).is_some());
        // …but there is no menu-accelerator reverse entry.
        assert_eq!(
            set.get_trigger(&Action::CopyToClipboard(
                super::super::action::CopyToClipboard::Mixed
            )),
            None
        );
    }

    /// Overriding a trigger with a different action removes the old action's
    /// reverse entry (Binding.zig test 4098).
    #[test]
    fn reverse_override_updates() {
        let mut set = Set::new();
        set.put(bind("ctrl+x=new_tab"));
        set.put(bind("ctrl+x=new_window"));
        assert_eq!(set.get_trigger(&Action::NewTab), None);
        assert_eq!(
            set.get_trigger(&Action::NewWindow),
            Some(bind("ctrl+x=ignore").trigger)
        );
    }

    /// Removing a trigger repoints the reverse map to another trigger with the
    /// same action, or drops it if none remain (Binding.zig test 4037).
    #[test]
    fn reverse_remove_repoints_then_drops() {
        let mut set = Set::new();
        set.put(bind("ctrl+t=new_tab"));
        set.put(bind("cmd+t=new_tab")); // second trigger, same action
        // Reverse points at the most-recent (cmd+t).
        assert_eq!(
            set.get_trigger(&Action::NewTab),
            Some(bind("cmd+t=ignore").trigger)
        );
        // Remove cmd+t → reverse repoints to the remaining ctrl+t.
        set.remove(bind("cmd+t=ignore").trigger);
        assert_eq!(
            set.get_trigger(&Action::NewTab),
            Some(bind("ctrl+t=ignore").trigger)
        );
        // Remove the last one → reverse entry gone.
        set.remove(bind("ctrl+t=ignore").trigger);
        assert_eq!(set.get_trigger(&Action::NewTab), None);
    }
}
