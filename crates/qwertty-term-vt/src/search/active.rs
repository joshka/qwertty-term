//! [`ActiveSearch`] — search within the mutable active area. Port of
//! `src/terminal/search/active.zig` (ghostty commit `2da015cd6`).
//!
//! The active area is the only mutable part of a `PageList`, so it must be repeatedly
//! re-searched as its contents change. This specializes in that: copying the active-area
//! text into a forward sliding window on each `update`, then iterating matches with `next`.

// `next()` named for parity with Zig; deliberately not an `Iterator` impl. `update` is
// public API consumed by the Phase-2 `ScreenSearch`; only tests reach it today.
#![allow(clippy::should_implement_trait, dead_code)]

use crate::highlight::Flattened;
use crate::pagelist::{Node, PageList};

use super::sliding_window::{Direction, SlidingWindow};

/// Searches for a substring within the active area of a [`PageList`]. Port of `ActiveSearch`.
pub struct ActiveSearch {
    window: SlidingWindow,
}

impl ActiveSearch {
    /// Initialize an active-area search for `needle`. Port of `ActiveSearch.init`.
    ///
    /// Uses a forward search: the active area is small so results are instant anyway, and
    /// this skips the reversal work.
    pub fn init(needle: &[u8]) -> ActiveSearch {
        ActiveSearch {
            window: SlidingWindow::init(Direction::Forward, needle),
        }
    }

    /// Update the window to reflect the current active area. Port of `ActiveSearch.update`.
    ///
    /// Copies the necessary page text so the caller can drop the PageList lock immediately;
    /// it does not perform the search itself.
    ///
    /// Returns the first (reverse-order) node covered, so a history search can overlap and
    /// continue from there. There CAN be duplicates and this node CAN be mutable, so the
    /// history search should prune anything in the active area. `None` means the active area
    /// covers the entire PageList.
    ///
    /// # Safety
    /// `list` must be safe to read for the duration of this call.
    pub(crate) fn update(&mut self, list: &PageList) -> Option<*mut Node> {
        self.window.clear_and_retain_capacity();

        // An empty needle represents an inactive search and has no overlap or
        // history to load. Port of upstream 5bc6588e4.
        if self.window.needle().is_empty() {
            return None;
        }

        // Add enough pages to cover the active area, walking from the last page backward.
        let mut rem: usize = list.rows() as usize;
        let mut node = list.last_node();
        let mut last_node: Option<*mut Node> = None;
        let mut carry_prev: *mut Node = std::ptr::null_mut();
        while !node.is_null() {
            self.window.append(node);
            last_node = Some(node);

            let node_rows = unsafe { (*node).data.size.rows } as usize;
            if rem <= node_rows {
                // This is the last page containing the active area; step once more to the
                // previous page (the first page of the required overlap).
                carry_prev = unsafe { (*node).prev };
                break;
            }
            rem -= node_rows;
            node = unsafe { (*node).prev };
        }

        // Add enough soft-wrapped overlap to cover needle.len - 1 bytes.
        let mut ov = carry_prev;
        while !ov.is_null() {
            let page = unsafe { &(*ov).data };
            let last_row = page.get_row(page.size.rows as usize - 1);
            if !unsafe { (*last_row).wrap() } {
                break;
            }
            let added = self.window.append(ov);
            if added >= self.window.needle().len() - 1 {
                break;
            }
            ov = unsafe { (*ov).prev };
        }

        last_node
    }

    /// Find the next match in the active area, or `None` when exhausted. Port of
    /// `ActiveSearch.next`.
    pub fn next(&mut self) -> Option<Flattened> {
        self.window.next()
    }
}

#[cfg(test)]
mod tests;
