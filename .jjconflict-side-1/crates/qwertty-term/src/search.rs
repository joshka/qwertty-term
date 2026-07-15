//! Per-pane scrollback-search state: the needle, the resolved match ranges, and
//! the current-match cursor. Pure data + navigation math, AppKit-free so it
//! unit-tests without a window.
//!
//! The engine ([`crate::engine::Engine::search_all`]) resolves matches to
//! absolute-screen [`ScreenRange`]s in reading order (top→bottom); this struct
//! stores that list and tracks which one is "current" for navigation and the
//! distinct current-match highlight tint. The overlay
//! ([`crate::search_overlay`]) shows the needle + the "N/M" counter derived
//! from here; the render path ([`crate::app`]) tints every match, drawing the
//! current one in a distinct color.
//!
//! Coordinate/scroll conversion (which absolute row a match lives on → a
//! scrollback offset) lives in [`crate::app`] where the surface's grid size and
//! `scrollback_offset` machinery are; this module only owns the match set and
//! the index arithmetic.

use crate::selection::ScreenRange;

/// A pane's live search. `active` distinguishes "bar open, empty needle" (no
/// matches yet, still capturing focus) from "no search".
#[derive(Debug, Clone, Default)]
pub struct SearchState {
    /// Whether the search bar is open on this pane.
    active: bool,
    /// The current needle (what the user has typed).
    needle: String,
    /// Every match over the whole scrollback, in reading order (top→bottom), in
    /// absolute screen coordinates. Rebuilt whenever the needle changes.
    matches: Vec<ScreenRange>,
    /// Index into `matches` of the "current" match (the one navigation centers
    /// and that gets the distinct highlight). `None` when there are no matches.
    current: Option<usize>,
}

impl SearchState {
    /// Open the search bar (idempotent). Does not clear an existing needle/match
    /// set, so re-opening restores the prior query (matching a user re-pressing
    /// Cmd+F).
    pub fn open(&mut self) {
        self.active = true;
    }

    /// Close the search bar and drop all match state.
    pub fn close(&mut self) {
        self.active = false;
        self.needle.clear();
        self.matches.clear();
        self.current = None;
    }

    /// Whether the search bar is open.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The current needle text.
    pub fn needle(&self) -> &str {
        &self.needle
    }

    /// Replace the match set for a new needle. `matches` must be in reading
    /// order. Resets the current match to the first one (if any) — the natural
    /// "jump to the first hit as you type" behavior.
    pub fn set_results(&mut self, needle: String, matches: Vec<ScreenRange>) {
        self.needle = needle;
        self.current = if matches.is_empty() { None } else { Some(0) };
        self.matches = matches;
    }

    /// All match ranges (for the highlight pass).
    pub fn matches(&self) -> &[ScreenRange] {
        &self.matches
    }

    /// The current match's range, if any.
    pub fn current_match(&self) -> Option<ScreenRange> {
        self.current.map(|i| self.matches[i])
    }

    /// The current match's index (0-based), if any.
    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    /// The total number of matches.
    pub fn count(&self) -> usize {
        self.matches.len()
    }

    /// The "N/M" counter label for the overlay: `"3/17"`, or `"0/0"` when there
    /// are no matches, or an empty string when the needle is empty (nothing to
    /// count yet).
    pub fn counter_label(&self) -> String {
        if self.needle.is_empty() {
            return String::new();
        }
        match self.current {
            Some(i) => format!("{}/{}", i + 1, self.matches.len()),
            None => "0/0".to_string(),
        }
    }

    /// Advance to the next match (wrapping), returning the new current match's
    /// range to scroll to, or `None` if there are no matches.
    ///
    /// Named `next`/`previous` for the navigation domain, not the `Iterator`
    /// contract (this mutates a cursor and never terminates — it wraps).
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<ScreenRange> {
        let n = self.matches.len();
        if n == 0 {
            return None;
        }
        let i = match self.current {
            Some(i) => (i + 1) % n,
            None => 0,
        };
        self.current = Some(i);
        Some(self.matches[i])
    }

    /// Step to the previous match (wrapping), returning the new current match's
    /// range to scroll to, or `None` if there are no matches.
    pub fn previous(&mut self) -> Option<ScreenRange> {
        let n = self.matches.len();
        if n == 0 {
            return None;
        }
        let i = match self.current {
            Some(i) => (i + n - 1) % n,
            None => n - 1,
        };
        self.current = Some(i);
        Some(self.matches[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(row: usize) -> ScreenRange {
        ScreenRange {
            top_left: (0, row),
            bottom_right: (3, row),
            rectangle: false,
        }
    }

    #[test]
    fn open_close_lifecycle() {
        let mut s = SearchState::default();
        assert!(!s.is_active());
        s.open();
        assert!(s.is_active());
        s.set_results("foo".into(), vec![range(1)]);
        s.close();
        assert!(!s.is_active());
        assert_eq!(s.needle(), "");
        assert_eq!(s.count(), 0);
        assert!(s.current_match().is_none());
    }

    #[test]
    fn set_results_selects_first_match() {
        let mut s = SearchState::default();
        s.set_results("x".into(), vec![range(2), range(5), range(9)]);
        assert_eq!(s.current_index(), Some(0));
        assert_eq!(s.current_match(), Some(range(2)));
        assert_eq!(s.counter_label(), "1/3");
    }

    #[test]
    fn counter_label_shapes() {
        let mut s = SearchState::default();
        assert_eq!(s.counter_label(), ""); // empty needle
        s.set_results("x".into(), vec![]);
        assert_eq!(s.counter_label(), "0/0"); // needle, no matches
        s.set_results("x".into(), vec![range(1), range(2)]);
        assert_eq!(s.counter_label(), "1/2");
    }

    #[test]
    fn next_and_previous_wrap() {
        let mut s = SearchState::default();
        s.set_results("x".into(), vec![range(1), range(2), range(3)]);
        assert_eq!(s.current_index(), Some(0));
        assert_eq!(s.next(), Some(range(2)));
        assert_eq!(s.next(), Some(range(3)));
        assert_eq!(s.next(), Some(range(1))); // wrap forward
        assert_eq!(s.previous(), Some(range(3))); // wrap backward
    }

    #[test]
    fn navigation_with_no_matches_is_none() {
        let mut s = SearchState::default();
        s.set_results("x".into(), vec![]);
        assert!(s.next().is_none());
        assert!(s.previous().is_none());
        assert!(s.current_match().is_none());
    }
}
