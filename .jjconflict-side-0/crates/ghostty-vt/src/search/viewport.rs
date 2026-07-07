//! [`ViewportSearch`] — search within the viewport with change detection. Port of
//! `src/terminal/search/viewport.zig` (ghostty commit `2da015cd6`).
//!
//! The viewport is the part of the search the user actively sees, so it is more efficient to
//! re-search just the viewport than to store all-screen results. A node-pointer `Fingerprint`
//! detects when the viewport moves so `update` only re-searches when necessary. Note this
//! searches all pages the viewport covers, so it can include extra matches outside the
//! viewport if they live in the same page.

// `next()` named for parity with Zig; deliberately not an `Iterator` impl. Some methods are
// Phase-2 public API (`ScreenSearch`/`Thread`); only tests reach them today.
#![allow(clippy::should_implement_trait, dead_code)]

use crate::highlight::Flattened;
use crate::pagelist::{Direction as PageDirection, Node, PageList};
use crate::point::Tag;

use super::sliding_window::{Direction, SlidingWindow};

/// Searches for a substring within the viewport of a [`PageList`]. Port of `ViewportSearch`.
pub struct ViewportSearch {
    window: SlidingWindow,
    fingerprint: Option<Fingerprint>,

    /// If `None`, active dirty tracking is disabled and we always re-search when the viewport
    /// overlaps the active area. If `Some`, we only re-search when the active area is dirty;
    /// dirty marking is up to the caller.
    pub active_dirty: Option<bool>,
}

impl ViewportSearch {
    /// Initialize a viewport search for `needle`. Port of `ViewportSearch.init`.
    ///
    /// Forward search: the viewport is small so results are instant and this skips reversal.
    pub fn init(needle: &[u8]) -> ViewportSearch {
        ViewportSearch {
            window: SlidingWindow::init(Direction::Forward, needle),
            fingerprint: None,
            active_dirty: None,
        }
    }

    /// Reset the fingerprint and window so the next `update` always re-searches. Port of
    /// `ViewportSearch.reset`.
    pub fn reset(&mut self) {
        self.fingerprint = None;
        self.window.clear_and_retain_capacity();
    }

    /// The needle this search is using. Port of `ViewportSearch.needle`.
    pub fn needle(&self) -> &[u8] {
        debug_assert!(self.window.direction() == Direction::Forward);
        self.window.needle()
    }

    /// Update the window to reflect the current viewport, doing nothing if unchanged. Returns
    /// `true` if a re-search is needed, `false` if the viewport is unchanged. Port of
    /// `ViewportSearch.update`.
    ///
    /// # Safety
    /// `list` must be safe to read throughout this call.
    pub fn update(&mut self, list: &mut PageList) -> bool {
        let fingerprint = Fingerprint::init(list);

        if let Some(old) = &self.fingerprint
            && old.eql(&fingerprint)
        {
            // Determine if we must check active-area overlap.
            let check_active = match self.active_dirty {
                None => true,
                Some(false) => false,
                Some(true) => {
                    self.active_dirty = Some(false);
                    true
                }
            };

            let mut overlaps_active = false;
            if check_active {
                // If the viewport contains the active area (mutable), always re-search.
                let active_tl = list.get_top_left(Tag::Active);
                let active_br = list.get_bottom_right(Tag::Active).unwrap();
                for &node in &old.nodes {
                    if node == active_tl.node || node == active_br.node {
                        overlaps_active = true;
                        break;
                    }
                }
            }

            if !overlaps_active {
                return false;
            }
        }

        self.fingerprint = Some(fingerprint);

        // If the active area was marked dirty, we always unset it since we're re-searching.
        if self.active_dirty.is_some() {
            self.active_dirty = Some(false);
        }

        self.window.clear_and_retain_capacity();

        let nodes = &self.fingerprint.as_ref().unwrap().nodes;

        // Leading soft-wrap overlap (prior pages) to cover needle.len - 1 bytes.
        let mut node = unsafe { (*nodes[0]).prev };
        let mut added: usize = 0;
        while !node.is_null() {
            let page = unsafe { &(*node).data };
            let last_row = page.get_row(page.size.rows as usize - 1);
            if !unsafe { (*last_row).wrap() } {
                break;
            }
            added += self.window.append(node);
            if added >= self.window.needle().len() - 1 {
                break;
            }
            node = unsafe { (*node).prev };
        }

        // The viewport nodes themselves.
        for &node in nodes {
            self.window.append(node);
        }

        // Trailing soft-wrap overlap (following pages).
        let end = *nodes.last().unwrap();
        let end_page = unsafe { &(*end).data };
        let end_last_row = end_page.get_row(end_page.size.rows as usize - 1);
        if unsafe { (*end_last_row).wrap() } {
            let mut node = unsafe { (*end).next };
            added = 0;
            while !node.is_null() {
                added += self.window.append(node);
                if added >= self.window.needle().len() - 1 {
                    break;
                }
                let page = unsafe { &(*node).data };
                let last_row = page.get_row(page.size.rows as usize - 1);
                if !unsafe { (*last_row).wrap() } {
                    break;
                }
                node = unsafe { (*node).next };
            }
        }

        true
    }

    /// Find the next match in the viewport, or `None` when exhausted. Port of
    /// `ViewportSearch.next`.
    pub fn next(&mut self) -> Option<Flattened> {
        self.window.next()
    }
}

/// Viewport fingerprint: the ordered node pointers the viewport spans. Only pointer identity
/// is safe to compare (cached page contents may be invalid). Port of the `Fingerprint`
/// struct.
struct Fingerprint {
    nodes: Vec<*mut Node>,
}

impl Fingerprint {
    fn init(pages: &mut PageList) -> Fingerprint {
        let tl = pages.get_top_left(Tag::Viewport);
        let br = pages.get_bottom_right(Tag::Viewport).unwrap();
        let mut nodes = Vec::new();
        // SAFETY: tl/br are live viewport pins for `pages`.
        let mut it = unsafe { tl.page_iterator(PageDirection::RightDown, Some(br)) };
        while let Some(chunk) = unsafe { it.next() } {
            nodes.push(chunk.node);
        }
        Fingerprint { nodes }
    }

    fn eql(&self, other: &Fingerprint) -> bool {
        self.nodes == other.nodes
    }
}

#[cfg(test)]
mod tests;
