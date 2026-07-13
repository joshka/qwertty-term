//! [`PageListSearch`] — search the whole `PageList` in reverse (history). Port of
//! `src/terminal/search/pagelist.zig` (ghostty commit `2da015cd6`).
//!
//! Searches in reverse order (most-recent first) from a starting node. Assumes nodes do not
//! change contents; for the mutable active area, pair this with
//! [`ActiveSearch`](super::ActiveSearch), starting from the node it returns.
//!
//! Concurrent access to a `PageList` is not allowed, so the caller must hold necessary locks.
//! Each method documents whether it touches the `PageList`: `next` does not (safe without a
//! lock), `feed` does (needs a lock).

// `next()` named for parity with Zig; deliberately not an `Iterator` impl. `init`/`feed`/
// `deinit` are Phase-2 public API (`ScreenSearch`); only tests reach them today.
#![allow(clippy::should_implement_trait, dead_code)]

use crate::highlight::Flattened;
use crate::pagelist::{Node, PageList, Pin};

use super::sliding_window::{Direction, SlidingWindow};

/// Searches for a term in a [`PageList`], in reverse from a starting node. Port of
/// `PageListSearch`. The tracked pin is untracked on [`PageListSearch::deinit`].
pub struct PageListSearch {
    /// The sliding window of page contents/nodes to search.
    window: SlidingWindow,

    /// Tracked pin at our current position, so pruning can't invalidate our progress.
    pin: *mut Pin,
}

impl PageListSearch {
    /// Initialize the search. The needle is copied. Feeds the start page immediately. Port of
    /// `PageListSearch.init`.
    ///
    /// Accesses the `PageList`/node, so the caller must ensure that is safe.
    ///
    /// # Safety
    /// `start` must be a live node vended by `list`.
    pub(crate) fn init(needle: &[u8], list: &mut PageList, start: *mut Node) -> PageListSearch {
        // Track a pin in the start node; a tracked pin is moved somewhere safe if the
        // PageList prunes pages, keeping our references valid.
        let start_page = unsafe { &(*start).data };
        let pin = list.track_pin(Pin::with(
            start,
            start_page.size.rows - 1,
            start_page.size.cols - 1,
        ));

        let mut window = SlidingWindow::init(Direction::Reverse, needle);
        // Always feed the initial page (we have the lock anyway); this lets `pin` point at
        // our current node and `feed` work properly.
        window.append(start);

        PageListSearch { window, pin }
    }

    /// Initialize a whole-history search starting from the bottom-most node (the
    /// active area), searching in reverse toward the top of scrollback. This is
    /// the safe entry point for a caller that just wants "search everything" and
    /// does not track its own `*mut Node` — it resolves the start node from
    /// `list` internally. Not present upstream (whose `ScreenSearch` threads the
    /// active-search node in), but a thin convenience over [`PageListSearch::init`]
    /// for the app-side synchronous driver.
    pub fn from_end(needle: &[u8], list: &mut PageList) -> PageListSearch {
        let start = list.last_node();
        PageListSearch::init(needle, list, start)
    }

    /// Untrack the pin. Modifies the `PageList`, so the caller must ensure that is safe. Port
    /// of `PageListSearch.deinit`.
    pub fn deinit(self, list: &mut PageList) {
        list.untrack_pin(self.pin);
    }

    /// Return the next match in the loaded nodes, or `None` when the window needs more data
    /// (call [`PageListSearch::feed`]). Does NOT access the `PageList` (safe without a lock).
    /// Port of `PageListSearch.next`.
    pub fn next(&mut self) -> Option<Flattened> {
        self.window.next()
    }

    /// Feed more data from the pagelist — enough to cover at least one match (needle length)
    /// if it exists. Does not perform the search. Accesses nodes, so the caller must hold
    /// necessary locks. Returns `false` when there is no more data to feed (the whole list
    /// has been searched). Port of `PageListSearch.feed`.
    pub fn feed(&mut self) -> bool {
        // If our pin is garbage, wherever we were next was reused; treat as end of list.
        if unsafe { (*self.pin).garbage } {
            return false;
        }

        let needle_len = self.window.needle().len();
        let mut rem: usize = needle_len;

        // Start at our previous node and continue adding until we have enough data.
        let mut node = unsafe { (*(*self.pin).node).prev };
        while !node.is_null() {
            let added = self.window.append(node);
            rem = rem.saturating_sub(added);

            // Move our tracked pin to the new node, resetting both coordinates
            // to that node's actual bottom-right cell. A preceding history page
            // may have fewer rows/cols (e.g. after a split), so retaining the
            // old coordinates would leave the pin outside the new page and trip
            // the next PageList integrity check. Port of upstream 5d8eb78b7.
            unsafe {
                let size = (*node).data.size;
                (*self.pin).node = node;
                (*self.pin).y = size.rows - 1;
                (*self.pin).x = size.cols - 1;
            }

            if rem == 0 {
                break;
            }
            node = unsafe { (*node).prev };
        }

        // True if we fed any data.
        rem < needle_len
    }
}

#[cfg(test)]
mod tests;
