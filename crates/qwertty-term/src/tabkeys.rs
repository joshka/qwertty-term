//! Built-in tab-navigation keybinds — the tab subset of real Ghostty's default
//! keybind block, hardcoded as a small static table.
//!
//! This is deliberately *not* the full `Binding.zig` keybind system (4.9k LoC,
//! explicitly deferred). It is the narrowest thing that makes tab navigation
//! match real Ghostty's macOS defaults for a maintainer who daily-drives the
//! app: a static `(key, mods) -> TabAction` table plus a pure `resolve`.
//!
//! Ported from upstream `src/config/Config.zig` (Ghostty commit `2da015cd6`),
//! the default keybind block:
//!
//! - `ctrl+tab` → next tab, `ctrl+shift+tab` → previous tab
//!   (lines 6556–6566, "Tabs common to all platforms").
//! - `cmd+1`..`cmd+8` → goto tab N, `cmd+9` → last tab
//!   (lines 6780–6847). Upstream registers BOTH the physical `digit_N` key
//!   and the unicode `N` key so non-US layouts (AZERTY) work; we key off the
//!   physical [`Key::Digit1`..`Digit9`] which is layout-independent, covering
//!   both. On macOS the modifier is `super` (Cmd); elsewhere `alt`.
//! - `cmd+shift+]` / `cmd+shift+[` → next / previous tab. These are *not* in
//!   upstream's default table (upstream's `cmd+shift+[`/`]` are unbound on
//!   macOS by default — the bracket bindings there are `ctrl+super+[`/`]` for
//!   *splits*, lines 6636–6645, gated to non-Darwin). We add the bracket pair
//!   as a maintainer-requested convenience alias for next/previous tab; the
//!   AppKit tab bar also offers `cmd+shift+[`/`]` natively, so this keeps the
//!   two consistent.
//!
//! `goto_tab` / `last_tab` / `next` / `previous` runtime semantics are ported
//! from `macos/Sources/Features/Terminal/TerminalController.swift::onGotoTab`
//! (lines ~1500–1546): next/previous **wrap** (cyclic), `goto_tab N` is 1-based
//! and **clamps** to the last tab (`min(N-1, count-1)`), `last_tab` selects the
//! last tab. Those live in [`crate::app`] where the tab group is; this module is
//! just the pure key→action lookup.
//!
//! Structured so a future config-driven keybind chunk can absorb it: the table
//! is data ([`DEFAULT_TAB_BINDINGS`]), the modifiers are a small [`TabMods`]
//! bitset independent of AppKit, and [`resolve`] is a pure function. Swapping in
//! a user-configurable `Set` later means replacing the table lookup, not the
//! call sites.

use qwertty_term_input::key::Key;

/// A tab-navigation action a built-in binding maps to. Executed against the
/// active window's native tab group by [`crate::app::Controller`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabAction {
    /// Select the next tab, wrapping from the last to the first.
    NextTab,
    /// Select the previous tab, wrapping from the first to the last.
    PreviousTab,
    /// Select the Nth tab (1-based). Clamps to the last tab if N exceeds the
    /// tab count (upstream `min(N-1, count-1)`).
    GotoTab(usize),
    /// Select the last tab.
    LastTab,
}

/// The modifier state a tab binding matches, reduced to the four modifiers the
/// tab chords use. AppKit-free so the table + [`resolve`] unit-test without an
/// event loop; the view translates `NSEvent.modifierFlags` into this.
///
/// Note `super_` is Cmd on macOS. The `caps_lock`/`num_lock`/side distinctions
/// that the PTY encoder cares about are irrelevant to these chords, so they are
/// intentionally dropped here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TabMods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub super_: bool,
}

impl TabMods {
    const fn new(shift: bool, ctrl: bool, alt: bool, super_: bool) -> Self {
        TabMods {
            shift,
            ctrl,
            alt,
            super_,
        }
    }
}

/// One entry in the built-in table: an exact `(key, mods)` trigger → action.
#[derive(Debug, Clone, Copy)]
pub struct TabKeyBinding {
    pub key: Key,
    pub mods: TabMods,
    pub action: TabAction,
}

/// The built-in tab keybind table. A future config-driven keybind chunk should
/// treat this as the *default* set it layers user overrides on top of.
///
/// Ordering is match-priority-irrelevant: [`resolve`] requires an exact `(key,
/// mods)` match, and no two entries share the same trigger.
pub static DEFAULT_TAB_BINDINGS: &[TabKeyBinding] = &[
    // --- ctrl+tab / ctrl+shift+tab (Config.zig 6556-6566) ---
    TabKeyBinding {
        key: Key::Tab,
        mods: TabMods::new(false, true, false, false), // ctrl
        action: TabAction::NextTab,
    },
    TabKeyBinding {
        key: Key::Tab,
        mods: TabMods::new(true, true, false, false), // ctrl+shift
        action: TabAction::PreviousTab,
    },
    // --- cmd+shift+] / cmd+shift+[ → next / previous (maintainer alias; see
    // module docs — matches the AppKit tab bar's native bracket equivalents) ---
    TabKeyBinding {
        key: Key::BracketRight,
        mods: TabMods::new(true, false, false, true), // cmd+shift
        action: TabAction::NextTab,
    },
    TabKeyBinding {
        key: Key::BracketLeft,
        mods: TabMods::new(true, false, false, true), // cmd+shift
        action: TabAction::PreviousTab,
    },
    // --- cmd+1..cmd+8 → goto tab N (Config.zig 6780-6834; physical digit keys
    // so non-US layouts work) ---
    TabKeyBinding {
        key: Key::Digit1,
        mods: TabMods::new(false, false, false, true),
        action: TabAction::GotoTab(1),
    },
    TabKeyBinding {
        key: Key::Digit2,
        mods: TabMods::new(false, false, false, true),
        action: TabAction::GotoTab(2),
    },
    TabKeyBinding {
        key: Key::Digit3,
        mods: TabMods::new(false, false, false, true),
        action: TabAction::GotoTab(3),
    },
    TabKeyBinding {
        key: Key::Digit4,
        mods: TabMods::new(false, false, false, true),
        action: TabAction::GotoTab(4),
    },
    TabKeyBinding {
        key: Key::Digit5,
        mods: TabMods::new(false, false, false, true),
        action: TabAction::GotoTab(5),
    },
    TabKeyBinding {
        key: Key::Digit6,
        mods: TabMods::new(false, false, false, true),
        action: TabAction::GotoTab(6),
    },
    TabKeyBinding {
        key: Key::Digit7,
        mods: TabMods::new(false, false, false, true),
        action: TabAction::GotoTab(7),
    },
    TabKeyBinding {
        key: Key::Digit8,
        mods: TabMods::new(false, false, false, true),
        action: TabAction::GotoTab(8),
    },
    // --- cmd+9 → last tab (Config.zig 6835-6846) ---
    TabKeyBinding {
        key: Key::Digit9,
        mods: TabMods::new(false, false, false, true),
        action: TabAction::LastTab,
    },
];

/// Resolve a physical key + modifier state to a built-in tab action, or `None`
/// if the chord is not a tab binding.
///
/// The match is *exact* on both key and the four modifiers, so plain Tab
/// (`mods` all false), Shift+Tab (shift only), and Ctrl+I (a different key,
/// `Key::KeyI`) never resolve here and fall through to the PTY encoder
/// untouched — the correctness point the callers depend on.
pub fn resolve(key: Key, mods: TabMods) -> Option<TabAction> {
    DEFAULT_TAB_BINDINGS
        .iter()
        .find(|b| b.key == key && b.mods == mods)
        .map(|b| b.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl() -> TabMods {
        TabMods::new(false, true, false, false)
    }
    fn ctrl_shift() -> TabMods {
        TabMods::new(true, true, false, false)
    }
    fn cmd() -> TabMods {
        TabMods::new(false, false, false, true)
    }
    fn cmd_shift() -> TabMods {
        TabMods::new(true, false, false, true)
    }

    #[test]
    fn ctrl_tab_is_next_and_ctrl_shift_tab_is_previous() {
        assert_eq!(resolve(Key::Tab, ctrl()), Some(TabAction::NextTab));
        assert_eq!(
            resolve(Key::Tab, ctrl_shift()),
            Some(TabAction::PreviousTab)
        );
    }

    #[test]
    fn cmd_digits_goto_tab_and_cmd_nine_is_last() {
        assert_eq!(resolve(Key::Digit1, cmd()), Some(TabAction::GotoTab(1)));
        assert_eq!(resolve(Key::Digit3, cmd()), Some(TabAction::GotoTab(3)));
        assert_eq!(resolve(Key::Digit8, cmd()), Some(TabAction::GotoTab(8)));
        assert_eq!(resolve(Key::Digit9, cmd()), Some(TabAction::LastTab));
    }

    #[test]
    fn cmd_shift_brackets_cycle_tabs() {
        assert_eq!(
            resolve(Key::BracketRight, cmd_shift()),
            Some(TabAction::NextTab)
        );
        assert_eq!(
            resolve(Key::BracketLeft, cmd_shift()),
            Some(TabAction::PreviousTab)
        );
    }

    /// The critical correctness point: keys that must reach the PTY encoder
    /// unaffected never resolve to a tab action.
    #[test]
    fn plain_tab_shift_tab_and_ctrl_i_are_not_tab_bindings() {
        // Plain Tab (no mods) → pty '\t'.
        assert_eq!(resolve(Key::Tab, TabMods::default()), None);
        // Shift+Tab (CSI Z) — shift only, no ctrl.
        assert_eq!(
            resolve(Key::Tab, TabMods::new(true, false, false, false)),
            None
        );
        // Ctrl+I is a different physical key (KeyI), same byte as Tab but must
        // not be swallowed.
        assert_eq!(resolve(Key::KeyI, ctrl()), None);
    }

    #[test]
    fn unrelated_cmd_chords_do_not_resolve() {
        // cmd+t (new tab, menu territory) is not a tab-nav binding here.
        assert_eq!(resolve(Key::KeyT, cmd()), None);
        // cmd+0 (font reset) is not goto-tab-0.
        assert_eq!(resolve(Key::Digit0, cmd()), None);
        // ctrl+alt+tab (extra modifier) does not match ctrl+tab.
        assert_eq!(
            resolve(Key::Tab, TabMods::new(false, true, true, false)),
            None
        );
    }

    #[test]
    fn every_binding_resolves_to_itself() {
        for b in DEFAULT_TAB_BINDINGS {
            assert_eq!(
                resolve(b.key, b.mods),
                Some(b.action),
                "binding {b:?} did not round-trip"
            );
        }
    }

    #[test]
    fn table_has_no_duplicate_triggers() {
        for (i, a) in DEFAULT_TAB_BINDINGS.iter().enumerate() {
            for b in &DEFAULT_TAB_BINDINGS[i + 1..] {
                assert!(
                    !(a.key == b.key && a.mods == b.mods),
                    "duplicate trigger: {a:?} and {b:?}"
                );
            }
        }
    }
}
