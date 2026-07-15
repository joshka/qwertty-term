//! Tab-navigation action + the AppKit-free modifier bitset used at the key seam.
//!
//! The former hardcoded `(key, mods) -> TabAction` table has been retired: tab
//! chords now resolve through the ported `Binding.zig`
//! [`Set`](qwertty_term_input::binding::Set) (upstream `default_set()` + the
//! user's `keybind` config), dispatched by `crate::app::Controller::perform_keybind_chord`.
//! This module keeps just the action enum the tab handler consumes and
//! [`TabMods`], the four-modifier bitset the view builds from
//! `NSEvent.modifierFlags` and passes to the keybind lookup.
//!
//! `goto_tab` / `last_tab` / `next` / `previous` runtime semantics (next/previous
//! wrap, `goto_tab N` is 1-based and clamps to the last tab, `last_tab` selects
//! the last) live in [`crate::app`] where the native tab group is.

/// A tab-navigation action a binding maps to. Executed against the active
/// window's native tab group by [`crate::app::Controller`].
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

/// The modifier state a chord matches, reduced to the four modifiers keybinds
/// use. AppKit-free so it needs no event loop; the view translates
/// `NSEvent.modifierFlags` into this, and `crate::keybind` maps it onto the
/// ported `Mods` for the `Set` lookup.
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
