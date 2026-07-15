//! Scrollback-search action.
//!
//! The former hardcoded `(key, mods) -> SearchAction` table has been retired:
//! search chords now resolve through the ported `Binding.zig`
//! [`Set`](qwertty_term_input::binding::Set) (upstream `default_set()` + the
//! user's `keybind` config), dispatched by
//! `crate::app::Controller::perform_keybind_chord`. The macOS defaults are
//! upstream's: `cmd+f` → `start_search`, `cmd+shift+f` / `escape` →
//! `end_search`, `cmd+g` / `cmd+shift+g` → next / previous.
//!
//! `escape` self-gates in `perform_keybind_chord`: `end_search` only fires while
//! the focused pane's search bar is open, so a plain Escape still reaches the PTY
//! encoder when not searching. Enter / Shift+Enter as next / previous is a
//! search-bar convention handled by the overlay's text-field delegate (the field
//! is first responder while typing), not a keybind.

/// A scrollback-search action a binding maps to. Executed against the focused
/// pane by [`crate::app::Controller`].
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
