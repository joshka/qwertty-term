//! Built-in scrollback-search keybinds — the search subset of real Ghostty's
//! default keybind block, hardcoded as a small static table (mirrors the shape
//! of [`crate::tabkeys`]).
//!
//! This is deliberately *not* the full `Binding.zig` keybind system (deferred).
//! It is the narrowest thing that makes Cmd+F search match real Ghostty's macOS
//! defaults: a static `(key, mods) -> SearchAction` table plus a pure `resolve`.
//!
//! Ported from upstream `src/config/Config.zig` (Ghostty commit `2da015cd6`),
//! the macOS default search keybind block (verified present at this commit —
//! lines ~7068–7104):
//!
//! - `super+f` → `start_search` (open the search bar).
//! - `super+shift+f` → `end_search`; `escape` → `end_search`.
//! - `super+g` → `navigate_search .next`; `super+shift+g` → `.previous`.
//! - (`super+e` → `search_selection` is upstream too, but "search the current
//!   selection" is out of scope for slice 1 and omitted.)
//!
//! Enter / Shift+Enter as next / previous is a search-bar convention handled by
//! the overlay's text-field delegate (the field is first responder while
//! typing), not this table — those keys must reach the field as text otherwise.
//!
//! `escape` resolves here *only while the search bar is open*; the caller
//! (`TerminalView::performKeyEquivalent:`) gates the escape binding on search
//! being active so a plain Escape still reaches the PTY encoder when not
//! searching. `resolve` itself is pure and unconditional; the caller applies
//! that gate.

use qwertty_term_input::key::Key;

use crate::tabkeys::TabMods;

/// A scrollback-search action a built-in binding maps to. Executed against the
/// focused pane by [`crate::app::Controller`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchAction {
    /// Open (or focus) the search bar on the focused pane.
    Start,
    /// Close the search bar and return focus to the terminal.
    End,
    /// Move to the next match (wrapping) and scroll it into view.
    Next,
    /// Move to the previous match (wrapping) and scroll it into view.
    Previous,
}

/// One entry in the built-in table: an exact `(key, mods)` trigger → action.
#[derive(Debug, Clone, Copy)]
pub struct SearchKeyBinding {
    pub key: Key,
    pub mods: TabMods,
    pub action: SearchAction,
}

const fn mods(shift: bool, ctrl: bool, alt: bool, super_: bool) -> TabMods {
    TabMods {
        shift,
        ctrl,
        alt,
        super_,
    }
}

/// The built-in search keybind table. A future config-driven keybind chunk
/// should treat this as the *default* set it layers user overrides on top of.
pub static DEFAULT_SEARCH_BINDINGS: &[SearchKeyBinding] = &[
    // super+f → start search.
    SearchKeyBinding {
        key: Key::KeyF,
        mods: mods(false, false, false, true),
        action: SearchAction::Start,
    },
    // super+shift+f → end search.
    SearchKeyBinding {
        key: Key::KeyF,
        mods: mods(true, false, false, true),
        action: SearchAction::End,
    },
    // escape → end search (gated on search being active by the caller).
    SearchKeyBinding {
        key: Key::Escape,
        mods: mods(false, false, false, false),
        action: SearchAction::End,
    },
    // super+g → next match.
    SearchKeyBinding {
        key: Key::KeyG,
        mods: mods(false, false, false, true),
        action: SearchAction::Next,
    },
    // super+shift+g → previous match.
    SearchKeyBinding {
        key: Key::KeyG,
        mods: mods(true, false, false, true),
        action: SearchAction::Previous,
    },
];

/// Resolve a physical key + modifier state to a built-in search action, or
/// `None` if the chord is not a search binding. Exact match on both key and the
/// four modifiers.
pub fn resolve(key: Key, mods: TabMods) -> Option<SearchAction> {
    DEFAULT_SEARCH_BINDINGS
        .iter()
        .find(|b| b.key == key && b.mods == mods)
        .map(|b| b.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd() -> TabMods {
        mods(false, false, false, true)
    }
    fn cmd_shift() -> TabMods {
        mods(true, false, false, true)
    }

    #[test]
    fn cmd_f_starts_search() {
        assert_eq!(resolve(Key::KeyF, cmd()), Some(SearchAction::Start));
    }

    #[test]
    fn cmd_shift_f_and_escape_end_search() {
        assert_eq!(resolve(Key::KeyF, cmd_shift()), Some(SearchAction::End));
        assert_eq!(
            resolve(Key::Escape, TabMods::default()),
            Some(SearchAction::End)
        );
    }

    #[test]
    fn cmd_g_navigates() {
        assert_eq!(resolve(Key::KeyG, cmd()), Some(SearchAction::Next));
        assert_eq!(
            resolve(Key::KeyG, cmd_shift()),
            Some(SearchAction::Previous)
        );
    }

    #[test]
    fn unrelated_chords_do_not_resolve() {
        // Plain f (no cmd) must fall through to the encoder so typing 'f' works.
        assert_eq!(resolve(Key::KeyF, TabMods::default()), None);
        // cmd+a is not a search chord.
        assert_eq!(resolve(Key::KeyA, cmd()), None);
    }

    #[test]
    fn does_not_collide_with_tab_or_split_bindings() {
        // None of the search chords are tab or split chords.
        for b in DEFAULT_SEARCH_BINDINGS {
            assert_eq!(
                crate::tabkeys::resolve(b.key, b.mods),
                None,
                "search chord {:?} collides with a tab binding",
                b.key
            );
            assert_eq!(
                crate::splitkeys::resolve(b.key, b.mods),
                None,
                "search chord {:?} collides with a split binding",
                b.key
            );
        }
    }
}
