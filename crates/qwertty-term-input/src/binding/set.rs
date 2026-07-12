//! The binding set: the runtime trigger→action table. Port of the dispatch
//! core of `Binding.Set` in `input/Binding.zig` (upstream `2da015cd6`,
//! lines 2045-2695).
//!
//! Scope so far: the forward map with **case-folded, `mods.binding()`-
//! normalized** lookup, `put` (with overwrite), the 5-probe [`Set::get_event`],
//! `remove`, the **reverse action→trigger map** ([`Set::get_trigger`]) for GUI
//! menu accelerators, and [`Set::parse_and_put`] — full config-string application
//! including `>`-separated sequences (leaders), `chain=`, and `unbind`. What
//! remains (see `docs/analysis/keybinds.md`, issue #24) is the app-side runtime
//! *dispatch* of sequences (the leader-key stack + timeout) and chained actions.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::key::KeyEvent;

use super::Action;
use super::BindError;
use super::flags::Flags;
use super::parser::{Binding, ParseItem, Parser};
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

/// The value stored for a trigger in the forward map. Port of `Set.Value`
/// (Binding.zig:2103-2105). A trigger either completes a binding (`Leaf` /
/// `LeafChained`) or is a leader into a nested [`Set`] for a key sequence
/// (`Leader`).
#[derive(Debug, Clone)]
enum Value {
    /// A completed binding: one action + flags.
    Leaf(Bound),
    /// A completed binding whose trigger runs several actions in order
    /// (`chain=`). The flags are shared across the chain.
    LeafChained { actions: Vec<Action>, flags: Flags },
    /// A leader in a sequence: follow the nested set for the next trigger.
    Leader(Box<Set>),
}

/// The trigger→action binding table. Port of `Binding.Set` (Binding.zig:2045).
///
/// The `reverse` map (action→trigger) supports GUI menu accelerators: given an
/// action, find a trigger to display as its shortcut. It is an insertion-ordered
/// `Vec` acting as a one-entry-per-action map (rather than a `HashMap`) because
/// [`Action`] carries `f32` payloads and so is not `Hash`/`Eq`; lookups use
/// `PartialEq`. This matches upstream's `Set.reverse` semantics
/// (Binding.zig:2053-2077): **performable** bindings are never tracked (so
/// toolkits don't register them as menu shortcuts), and only the most recently
/// added trigger per action is kept.
///
/// `chain_parent` records the trigger path to the most recently added leaf so a
/// following `chain=<action>` (see [`Set::parse_and_put`]) can append to it. It
/// is a plain path (root→leaf) rather than upstream's raw pointers
/// (`Set.ChainParent`, Binding.zig:2091-2095), which don't translate to safe
/// Rust; `append_chain` re-walks the path.
#[derive(Debug, Clone, Default)]
pub struct Set {
    forward: HashMap<SetKey, Value>,
    reverse: Vec<(Action, Trigger)>,
    chain_parent: Option<Vec<Trigger>>,
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
        // matching upstream). An existing leader/chained value is simply replaced
        // by the insert below (its `Box`/`Vec` is dropped).
        if track_reverse && matches!(self.forward.get(&key), Some(Value::Leaf(_))) {
            self.reverse.retain(|(_, t)| *t != trigger);
        }

        self.forward.insert(
            key,
            Value::Leaf(Bound {
                action: action.clone(),
                flags,
            }),
        );

        if track_reverse {
            // reverse.put(action, trigger): upsert keyed by action.
            self.reverse.retain(|(a, _)| *a != action);
            self.reverse.push((action, trigger));
        }

        // The last-added leaf is now the chain parent.
        self.chain_parent = Some(vec![trigger]);
    }

    /// Look up an exact trigger (folded, binding-normalized), returning its leaf
    /// binding. A leader (sequence prefix) or chained leaf yields `None` here;
    /// use [`Set::get_leader`] to follow a sequence. Port of `Set.get`.
    pub fn get(&self, trigger: Trigger) -> Option<&Bound> {
        match self.forward.get(&SetKey(trigger)) {
            Some(Value::Leaf(b)) => Some(b),
            _ => None,
        }
    }

    /// Follow a leader trigger to its nested set (the next level of a key
    /// sequence), or `None` if the trigger is unbound or a leaf.
    pub fn get_leader(&self, trigger: Trigger) -> Option<&Set> {
        match self.forward.get(&SetKey(trigger)) {
            Some(Value::Leader(s)) => Some(s.as_ref()),
            _ => None,
        }
    }

    /// If `trigger` maps to a chained leaf (built with `chain=`), return its
    /// ordered actions and their shared flags. This is the accessor a dispatcher
    /// uses to run every action in the chain.
    pub fn get_chained(&self, trigger: Trigger) -> Option<(&[Action], Flags)> {
        match self.forward.get(&SetKey(trigger)) {
            Some(Value::LeafChained { actions, flags }) => Some((actions, *flags)),
            _ => None,
        }
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

    /// Remove a trigger's entry. Returns the leaf binding if the trigger held
    /// one (a leader or chained leaf is dropped and returns `None`). Port of
    /// `Set.removeExact` (Binding.zig:2702-2730) including reverse-map fixup;
    /// like upstream, removal always clears the chain parent.
    pub fn remove(&mut self, trigger: Trigger) -> Option<Bound> {
        self.chain_parent = None;
        match self.forward.remove(&SetKey(trigger)) {
            Some(Value::Leaf(b)) => {
                self.fixup_reverse_for_action(&b.action, trigger);
                Some(b)
            }
            // Leaders and chained leaves are never in the reverse map; their
            // owned data (`Box<Set>` / `Vec`) is freed on drop here.
            Some(Value::LeafChained { .. }) | Some(Value::Leader(_)) | None => None,
        }
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
        // like upstream — the forward map is unordered). Only plain leaves are
        // reverse-map candidates (leaders/chained leaves never are).
        let other = self
            .forward
            .iter()
            .find(|(_, v)| matches!(v, Value::Leaf(b) if &b.action == action))
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

    /// Parse a full binding string and apply it to the set. Handles single
    /// bindings, `>`-separated sequences (leaders), `chain=<action>`, and
    /// `unbind`. Port of `Set.parseAndPut` (Binding.zig:2302-2494).
    ///
    /// The whole input is validated before any mutation (so a parse error never
    /// leaves a partially-modified set), matching upstream.
    pub fn parse_and_put(&mut self, input: &str) -> Result<(), BindError> {
        // Validate fully first.
        {
            let mut validate = Parser::init(input)?;
            while validate.next()?.is_some() {}
        }

        // Collect the (now known-valid) elements.
        let mut it = Parser::init(input)?;
        let mut leaders: Vec<Trigger> = Vec::new();
        loop {
            match it.next()? {
                Some(ParseItem::Leader(t)) => leaders.push(t),
                Some(ParseItem::Binding(b)) => {
                    if matches!(b.action, Action::Unbind) {
                        self.unbind_path(&leaders, b.trigger);
                        self.chain_parent = None;
                    } else {
                        let final_trigger = b.trigger;
                        self.put_path(&leaders, b);
                        // The chain parent is the full path to the new leaf.
                        leaders.push(final_trigger);
                        self.chain_parent = Some(leaders);
                    }
                    return Ok(());
                }
                Some(ParseItem::Chain(action)) => {
                    return self.append_chain(action);
                }
                None => return Ok(()),
            }
        }
    }

    /// Descend (creating leaders as needed) and put `binding` as a leaf in the
    /// deepest set. A leader step over an existing leaf/chained value replaces it
    /// with a fresh leader (matching upstream's "remove and fall through").
    fn put_path(&mut self, leaders: &[Trigger], binding: Binding) {
        let mut cur: &mut Set = self;
        for &lt in leaders {
            cur = cur.ensure_leader(lt);
        }
        cur.put(binding);
    }

    /// Ensure `trigger` maps to a leader in this set and return the nested set.
    fn ensure_leader(&mut self, trigger: Trigger) -> &mut Set {
        let key = SetKey(trigger);
        if !matches!(self.forward.get(&key), Some(Value::Leader(_))) {
            // Drop any existing leaf/chained here (with reverse fixup) first.
            self.remove(trigger);
            self.forward
                .insert(key, Value::Leader(Box::new(Set::new())));
        }
        match self.forward.get_mut(&key) {
            Some(Value::Leader(s)) => s.as_mut(),
            _ => unreachable!("just ensured a leader"),
        }
    }

    /// Remove the binding at `leaders > final_trigger`, pruning any leader sets
    /// that become empty. Navigates only existing leaders: if the path isn't
    /// present (or a segment is a leaf, not a leader) it is a no-op — which is
    /// how upstream's "restore the previous value" behaviour manifests without an
    /// explicit save/restore (we never destroyed the old value).
    fn unbind_path(&mut self, leaders: &[Trigger], final_trigger: Trigger) {
        self.unbind_recurse(leaders, final_trigger);
    }

    /// Returns true if `self` became empty (so the caller should prune the leader
    /// that owns it).
    fn unbind_recurse(&mut self, leaders: &[Trigger], final_trigger: Trigger) -> bool {
        match leaders.split_first() {
            None => {
                self.remove(final_trigger);
                self.forward.is_empty()
            }
            Some((&head, rest)) => {
                let emptied = match self.forward.get_mut(&SetKey(head)) {
                    Some(Value::Leader(s)) => s.unbind_recurse(rest, final_trigger),
                    // Not a leader → nothing to unbind along this path.
                    _ => return false,
                };
                if emptied {
                    self.remove(head);
                }
                self.forward.is_empty()
            }
        }
    }

    /// Append a chained action to the most recently added leaf. Port of
    /// `Set.appendChain` (Binding.zig:2586-2638). Converts a `Leaf` to a
    /// `LeafChained` (dropping its reverse-map entry, since chained actions are
    /// never menu accelerators), or appends to an existing `LeafChained`.
    fn append_chain(&mut self, action: Action) -> Result<(), BindError> {
        let path = self.chain_parent.clone().ok_or(BindError::InvalidFormat)?;
        let Some((leaf_trigger, leader_path)) = path.split_last() else {
            return Err(BindError::InvalidFormat);
        };

        // Walk to the set that holds the leaf.
        let mut cur: &mut Set = self;
        for &lt in leader_path {
            cur = match cur.forward.get_mut(&SetKey(lt)) {
                Some(Value::Leader(s)) => s.as_mut(),
                _ => return Err(BindError::InvalidFormat),
            };
        }

        let key = SetKey(*leaf_trigger);
        match cur.forward.get_mut(&key) {
            Some(Value::LeafChained { actions, .. }) => {
                actions.push(action);
                Ok(())
            }
            Some(Value::Leaf(_)) => {
                let Some(Value::Leaf(leaf)) = cur.forward.remove(&key) else {
                    unreachable!("matched Leaf");
                };
                let old_action = leaf.action.clone();
                cur.forward.insert(
                    key,
                    Value::LeafChained {
                        actions: vec![leaf.action, action],
                        flags: leaf.flags,
                    },
                );
                // The now-chained leaf leaves the reverse map.
                cur.fixup_reverse_for_action(&old_action, *leaf_trigger);
                Ok(())
            }
            // Leader or missing entry: no valid chain parent.
            _ => Err(BindError::InvalidFormat),
        }
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

    // ---- parse_and_put: sequences, chains, unbind ----

    fn trig(raw: &str) -> Trigger {
        bind(&format!("{raw}=ignore")).trigger
    }

    #[test]
    fn parse_and_put_single() {
        let mut set = Set::new();
        set.parse_and_put("ctrl+a=new_tab").unwrap();
        assert_eq!(set.get(trig("ctrl+a")).unwrap().action, Action::NewTab);
    }

    /// Port of `parse: sequences` storage (Binding.zig test 3863): a leader set
    /// holds the next trigger; the leader itself is not a leaf.
    #[test]
    fn parse_and_put_sequence() {
        let mut set = Set::new();
        set.parse_and_put("a>b=new_tab").unwrap();
        assert!(set.get(trig("a")).is_none()); // 'a' is a leader, not a leaf
        let child = set.get_leader(trig("a")).expect("a is a leader");
        assert_eq!(child.get(trig("b")).unwrap().action, Action::NewTab);
    }

    /// Siblings share a leader prefix (Binding.zig test 3728).
    #[test]
    fn sequence_siblings() {
        let mut set = Set::new();
        set.parse_and_put("a>b=new_tab").unwrap();
        set.parse_and_put("a>c=new_window").unwrap();
        let child = set.get_leader(trig("a")).unwrap();
        assert_eq!(child.get(trig("b")).unwrap().action, Action::NewTab);
        assert_eq!(child.get(trig("c")).unwrap().action, Action::NewWindow);
    }

    /// `a>b=unbind` when only `b` exists removes `a` entirely (Binding.zig 3969).
    #[test]
    fn unbind_sequence_prunes_empty_leader() {
        let mut set = Set::new();
        set.parse_and_put("a>b=new_tab").unwrap();
        set.parse_and_put("a>b=unbind").unwrap();
        assert!(set.get_leader(trig("a")).is_none());
        assert!(set.get(trig("a")).is_none());
        assert!(set.is_empty());
    }

    /// A surviving sibling keeps the leader alive (Binding.zig test 3728).
    #[test]
    fn unbind_sequence_sibling_survives() {
        let mut set = Set::new();
        set.parse_and_put("a>b=new_tab").unwrap();
        set.parse_and_put("a>c=new_window").unwrap();
        set.parse_and_put("a>b=unbind").unwrap();
        let child = set.get_leader(trig("a")).expect("a survives");
        assert!(child.get(trig("b")).is_none());
        assert_eq!(child.get(trig("c")).unwrap().action, Action::NewWindow);
    }

    /// Unbinding a nonexistent sequence is a no-op (Binding.zig test 3985).
    #[test]
    fn unbind_nonexistent_is_noop() {
        let mut set = Set::new();
        set.parse_and_put("x=new_tab").unwrap();
        set.parse_and_put("a>b=unbind").unwrap(); // no panic, no change
        assert_eq!(set.get(trig("x")).unwrap().action, Action::NewTab);
    }

    /// A leader step over an existing leaf replaces it (Binding.zig test 3944).
    #[test]
    fn overwrite_leaf_with_leader() {
        let mut set = Set::new();
        set.parse_and_put("a=new_tab").unwrap();
        set.parse_and_put("a>b=new_window").unwrap();
        assert!(set.get(trig("a")).is_none()); // was a leaf, now a leader
        let child = set.get_leader(trig("a")).unwrap();
        assert_eq!(child.get(trig("b")).unwrap().action, Action::NewWindow);
    }

    /// `chain=` converts the last leaf to a multi-action chained leaf and drops
    /// its reverse-map entry (Binding.zig tests 4144-4169).
    #[test]
    fn chain_converts_leaf() {
        let mut set = Set::new();
        set.parse_and_put("ctrl+a=new_tab").unwrap();
        set.parse_and_put("chain=new_window").unwrap();
        // No longer a plain leaf, and no menu-accelerator reverse entry.
        assert!(set.get(trig("ctrl+a")).is_none());
        assert_eq!(set.get_trigger(&Action::NewTab), None);
        // Stored as a chained leaf with both actions, in order.
        let (actions, _) = set.get_chained(trig("ctrl+a")).expect("chained leaf");
        assert_eq!(actions, &[Action::NewTab, Action::NewWindow]);
    }

    /// The original leaf's flags carry to the chained leaf (Binding.zig 4205).
    #[test]
    fn chain_preserves_flags() {
        let mut set = Set::new();
        set.parse_and_put("performable:ctrl+a=new_tab").unwrap();
        set.parse_and_put("chain=new_window").unwrap();
        let (_, flags) = set.get_chained(trig("ctrl+a")).expect("chained leaf");
        assert!(flags.performable);
    }

    /// A chain with no prior binding is an error (Binding.zig test 4171).
    #[test]
    fn chain_without_parent_errors() {
        let mut set = Set::new();
        assert_eq!(
            set.parse_and_put("chain=new_tab"),
            Err(BindError::InvalidFormat)
        );
    }

    /// An invalid input never mutates the set (validate-before-mutate).
    #[test]
    fn invalid_input_does_not_mutate() {
        let mut set = Set::new();
        set.parse_and_put("ctrl+a=new_tab").unwrap();
        assert!(set.parse_and_put("ctrl+a>=new_tab").is_err()); // empty seq segment
        assert_eq!(set.get(trig("ctrl+a")).unwrap().action, Action::NewTab);
    }
}
