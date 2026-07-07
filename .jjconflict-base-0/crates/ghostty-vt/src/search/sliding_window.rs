//! The sliding-window matcher. Port of `src/terminal/search/sliding_window.zig`
//! (ghostty commit `2da015cd6`).
//!
//! Searches page nodes via a sliding window that maintains the invariant that data isn't
//! pruned until (1) we've searched it and (2) we've accounted for cross-page overlaps to fit
//! the needle. The window is initialized empty; pages are appended in search order (forward
//! linked-list order for a forward search, reverse for a reverse search). Appends grow the
//! window; it only prunes on a completed search via [`SlidingWindow::next`].
//!
//! The window does not own the pages — it copies their text at append time and holds raw
//! node pointers to build results. If any fed page becomes invalid, the caller must clear the
//! window and start over.

// `next()` is named for parity with the Zig source; deliberately not an `Iterator` impl.
#![allow(clippy::should_implement_trait)]
// Public API consumed by the not-yet-ported `ScreenSearch`/`Thread` (Phase 2); until those
// land, only the inline tests reach some of it.
#![allow(dead_code)]

use std::collections::VecDeque;

use crate::highlight::{self, Flattened};
use crate::page::size::CellCountInt;
use crate::page::{Cell, ContentTag, Wide};
use crate::pagelist::Node;
use crate::point::Coordinate;

/// The search direction. Port of `SlidingWindow.Direction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Append pages in forward linked-list order.
    Forward,
    /// Append pages in reverse order (most-recent first). More desirable for scrollback
    /// search so recent data is found first.
    Reverse,
}

/// Per-node metadata: which node produced a byte range and the byte→cell coordinate map.
/// Port of the anonymous `Meta` struct (`sliding_window.zig:89-97`).
struct Meta {
    node: *mut Node,
    serial: u64,
    /// Maps each encoded byte to its `(x, y)` cell within the page.
    cell_map: Vec<Coordinate>,
}

/// Searches page nodes via a sliding window. Port of `SlidingWindow`.
pub struct SlidingWindow {
    /// Circular buffer of encoded page text. Zig uses `CircBuf(u8)`; the Rust equivalent is
    /// a `VecDeque<u8>` whose `as_slices()` gives the same two-slice wrapped view.
    data: VecDeque<u8>,

    /// Per-node metadata, oldest-first. Zig `CircBuf(Meta)` → `VecDeque<Meta>`.
    meta: VecDeque<Meta>,

    /// Scratch chunk list reused across `next()` so results need no fresh allocation.
    chunk_buf: Vec<highlight::Chunk>,

    /// Offset into `data` where the next search begins (handles a partially consumed
    /// `meta[0]`). `sliding_window.zig:61-64`.
    data_offset: usize,

    /// The needle, owned. Duped at init; reversed for a reverse search.
    needle: Vec<u8>,

    /// The search direction.
    direction: Direction,

    /// Scratch for the within-buffer overlap search; length is always `needle.len * 2`.
    overlap_buf: Vec<u8>,
}

impl SlidingWindow {
    /// Initialize an empty window for `needle`. Port of `SlidingWindow.init`.
    ///
    /// Zig threads `Allocator.Error`; the Rust model allocates infallibly, so this returns
    /// the value directly.
    pub fn init(direction: Direction, needle_unowned: &[u8]) -> SlidingWindow {
        let mut needle = needle_unowned.to_vec();
        if direction == Direction::Reverse {
            needle.reverse();
        }
        let overlap_buf = vec![0u8; needle.len() * 2];
        SlidingWindow {
            data: VecDeque::new(),
            meta: VecDeque::new(),
            chunk_buf: Vec::new(),
            data_offset: 0,
            needle,
            direction,
            overlap_buf,
        }
    }

    /// The needle this window is searching (in append-order byte layout).
    pub fn needle(&self) -> &[u8] {
        &self.needle
    }

    /// The search direction.
    pub fn direction(&self) -> Direction {
        self.direction
    }

    /// Total bytes currently in the data buffer.
    pub fn data_len(&self) -> usize {
        self.data.len()
    }

    /// Number of metadata entries (appended pages) currently retained.
    pub fn meta_len(&self) -> usize {
        self.meta.len()
    }

    /// Clear all data but retain allocated capacity. Port of `clearAndRetainCapacity`.
    pub fn clear_and_retain_capacity(&mut self) {
        self.meta.clear();
        self.data.clear();
        self.data_offset = 0;
    }

    /// Search the window for the next occurrence of the needle, pruning as it advances while
    /// keeping enough tail to catch a cross-page match. Port of `SlidingWindow.next`.
    ///
    /// Returns a [`Flattened`] highlight on a match. The chunks reference internal window
    /// memory and are valid only until the next `next()`/`append()`; clone to retain.
    pub fn next(&mut self) -> Option<Flattened> {
        // If we have less data than the needle then we can't possibly match.
        let data_len = self.data.len();
        if data_len < self.needle.len() {
            return None;
        }

        // Two-slice view of `data` from `data_offset`. `VecDeque::as_slices` yields the
        // (front, back) wrapped halves; we drop the first `data_offset` bytes across them.
        let (s0, s1) = self.data.as_slices();
        let (slice0, slice1): (&[u8], &[u8]) = if self.data_offset <= s0.len() {
            (&s0[self.data_offset..], s1)
        } else {
            (&s1[self.data_offset - s0.len()..], &[])
        };

        // Search the first slice.
        if let Some(idx) = index_of_ignore_case(slice0, &self.needle) {
            return Some(self.highlight(idx, self.needle.len()));
        }

        // Search the overlap between the two slices (circular-buffer wrap).
        if !slice0.is_empty() && !slice1.is_empty() {
            let prefix: &[u8] = {
                let len = slice0.len().min(self.needle.len() - 1);
                &slice0[slice0.len() - len..]
            };
            let suffix: &[u8] = {
                let len = slice1.len().min(self.needle.len() - 1);
                &slice1[..len]
            };
            let overlap_len = prefix.len() + suffix.len();
            debug_assert!(overlap_len <= self.overlap_buf.len());
            self.overlap_buf[..prefix.len()].copy_from_slice(prefix);
            self.overlap_buf[prefix.len()..overlap_len].copy_from_slice(suffix);

            if let Some(idx) = index_of_ignore_case(&self.overlap_buf[..overlap_len], &self.needle)
            {
                // Map the overlap index back into the data buffer.
                return Some(self.highlight(slice0.len() - prefix.len() + idx, self.needle.len()));
            }
        }

        // Search the last slice.
        if let Some(idx) = index_of_ignore_case(slice1, &self.needle) {
            return Some(self.highlight(slice0.len() + idx, self.needle.len()));
        }

        // Special case a 1-length needle to delete the entire buffer.
        if self.needle.len() == 1 {
            self.clear_and_retain_capacity();
            self.assert_integrity();
            return None;
        }

        // No match. Keep `needle.len - 1` bytes to handle a future overlap.
        self.prune_no_match();
        None
    }

    /// The no-match prune path of `next()` (`sliding_window.zig:235-282`).
    fn prune_no_match(&mut self) {
        // Walk meta in reverse, retaining just enough trailing metas to hold needle.len-1
        // bytes. `prune_count` metas from the front become deletable.
        let need_total = self.needle.len() - 1;
        let mut saved: usize = 0;
        // `keep_from` is the index (from front) of the first meta we retain.
        let mut keep_from: Option<usize> = None;
        for (rev_i, meta) in self.meta.iter().enumerate().rev() {
            let needed = need_total - saved;
            if meta.cell_map.len() >= needed {
                // Retain up to this meta; set data_offset within it.
                self.data_offset = meta.cell_map.len() - needed;
                keep_from = Some(rev_i);
                break;
            }
            saved += meta.cell_map.len();
        }

        let Some(keep_from) = keep_from else {
            // Never accumulated enough → nothing to prune.
            debug_assert!(saved < need_total);
            return;
        };

        let prune_count = keep_from;
        if prune_count == 0 {
            // We need to keep up to the first meta to retain our window.
            return;
        }

        // Delete all metas up to (not including) `keep_from`.
        let mut prune_data_len: usize = 0;
        for _ in 0..prune_count {
            let meta = self.meta.pop_front().unwrap();
            prune_data_len += meta.cell_map.len();
        }
        self.data.drain(..prune_data_len);

        // Data offset moves to needle.len - 1 from the end to handle the overlap case.
        self.data_offset = self.data.len() - self.needle.len() + 1;
        self.assert_integrity();
    }

    /// Return a flattened highlight for a match at `start_offset` (relative to `data_offset`)
    /// of `len` bytes, pruning fully-consumed leading data. Port of `SlidingWindow.highlight`.
    fn highlight(&mut self, start_offset: usize, len: usize) -> Flattened {
        let start = start_offset + self.data_offset;
        let end = start + len - 1;
        debug_assert!(start < self.data.len());
        debug_assert!(start + len <= self.data.len());

        self.chunk_buf.clear();
        let mut top_x: CellCountInt = 0;
        let mut bot_x: CellCountInt = 0;

        // Find the meta holding the match start. `br` carries the state needed to continue
        // searching for the end if the match spans metas. `prune` is what we can drop.
        struct BrState {
            /// Index (from front) of the next meta to examine for the end.
            next_idx: usize,
            /// Total cell_map bytes consumed up to and including the start meta.
            consumed: usize,
        }
        let mut br: Option<BrState> = None;
        let mut prune_meta: usize = 0;
        let mut prune_data: usize = 0;

        {
            let mut meta_consumed: usize = 0;
            let mut found = false;
            for (i, meta) in self.meta.iter().enumerate() {
                let prior_meta_consumed = meta_consumed;
                meta_consumed += meta.cell_map.len();

                let meta_i = start - prior_meta_consumed;
                // This meta doesn't contain the start; it's before the match, prunable.
                if meta_i >= meta.cell_map.len() {
                    continue;
                }

                let end_i = end - prior_meta_consumed;
                if end_i < meta.cell_map.len() {
                    // Entire highlight within this meta (fast path).
                    let start_map = meta.cell_map[meta_i];
                    let end_map = meta.cell_map[end_i];
                    top_x = start_map.x;
                    bot_x = end_map.x;
                    self.chunk_buf.push(highlight::Chunk {
                        node: meta.node,
                        serial: meta.serial,
                        start: start_map.y as CellCountInt,
                        end: end_map.y as CellCountInt + 1,
                    });
                    prune_meta = i;
                    prune_data = prior_meta_consumed;
                    br = None;
                } else {
                    // Start meta only; consume this whole node from the start offset.
                    let map = meta.cell_map[meta_i];
                    top_x = map.x;
                    let page = unsafe { &(*meta.node).data };
                    self.chunk_buf.push(highlight::Chunk {
                        node: meta.node,
                        serial: meta.serial,
                        start: map.y as CellCountInt,
                        end: page.size.rows,
                    });
                    prune_meta = i;
                    prune_data = prior_meta_consumed;
                    br = Some(BrState {
                        next_idx: i + 1,
                        consumed: meta_consumed,
                    });
                }
                found = true;
                break;
            }
            // Precondition: the start index is within the data buffer.
            assert!(found, "sliding window highlight: start out of bounds");
        }

        // Search for our end if the match spans metas.
        if let Some(br) = br {
            let mut meta_consumed = br.consumed;
            let mut found = false;
            for meta in self.meta.iter().skip(br.next_idx) {
                let meta_i = end - meta_consumed;
                if meta_i >= meta.cell_map.len() {
                    // Full middle page.
                    let page = unsafe { &(*meta.node).data };
                    self.chunk_buf.push(highlight::Chunk {
                        node: meta.node,
                        serial: meta.serial,
                        start: 0,
                        end: page.size.rows,
                    });
                    meta_consumed += meta.cell_map.len();
                    continue;
                }
                // Found the end.
                let map = meta.cell_map[meta_i];
                bot_x = map.x;
                self.chunk_buf.push(highlight::Chunk {
                    node: meta.node,
                    serial: meta.serial,
                    start: 0,
                    end: map.y as CellCountInt + 1,
                });
                found = true;
                break;
            }
            assert!(found, "sliding window highlight: end out of bounds");
        }

        // Advance data_offset past the match (+1 so we don't re-return it).
        self.data_offset = start - prune_data + 1;

        // Prune fully-consumed leading metas/data.
        if prune_meta > 0 {
            let mut meta_consumed: usize = 0;
            for _ in 0..prune_meta {
                let meta = self.meta.pop_front().unwrap();
                meta_consumed += meta.cell_map.len();
            }
            debug_assert!(prune_data > 0);
            debug_assert_eq!(meta_consumed, prune_data);
            self.data.drain(..prune_data);
        }

        // Reverse-direction geometry fixup.
        if self.direction == Direction::Reverse {
            let n = self.chunk_buf.len();
            if n > 1 {
                self.chunk_buf.reverse();
                // Forward traversal uses the suffix of the first page and prefix of the last;
                // reverse order inverts this, so re-invert the first/last chunk geometry.
                let first_rows = unsafe { (*self.chunk_buf[0].node).data.size.rows };
                self.chunk_buf[0].start = self.chunk_buf[0].end - 1;
                self.chunk_buf[0].end = first_rows;
                self.chunk_buf[n - 1].end = self.chunk_buf[n - 1].start + 1;
                self.chunk_buf[n - 1].start = 0;
            } else {
                // Single chunk: y values are in reverse order; swap to top-to-bottom.
                let start_y = self.chunk_buf[0].start;
                self.chunk_buf[0].start = self.chunk_buf[0].end - 1;
                self.chunk_buf[0].end = start_y + 1;
            }
            // X values also swap since top/bottom swapped for the nodes.
            std::mem::swap(&mut top_x, &mut bot_x);
        }

        self.assert_integrity();
        Flattened {
            chunks: std::mem::take(&mut self.chunk_buf),
            top_x,
            bot_x,
        }
    }

    /// Add a node to the window (always grows; pruning happens in `next()`). Port of
    /// `SlidingWindow.append`. Returns the number of content bytes added.
    ///
    /// # Safety
    /// `node` must be a live node vended by the same `PageList`.
    pub(crate) fn append(&mut self, node: *mut Node) -> usize {
        let (mut written, mut cell_map) = encode_page_plain(node);

        // If the node's last row isn't soft-wrapped, add a trailing newline so a needle
        // can't match across an explicit line break.
        let page = unsafe { &(*node).data };
        let last_row = page.get_row(page.size.rows as usize - 1);
        let wrapped = unsafe { (*last_row).wrap() };
        if !wrapped {
            written.push(b'\n');
            let last = cell_map
                .last()
                .copied()
                .unwrap_or(Coordinate { x: 0, y: 0 });
            cell_map.push(last);
        }

        // Empty encode (whitespace-only page) → nothing to add.
        if written.is_empty() {
            self.assert_integrity();
            return 0;
        }

        // Reverse direction → reverse bytes and coordinate map.
        if self.direction == Direction::Reverse {
            written.reverse();
            cell_map.reverse();
        }

        let added = written.len();
        debug_assert_eq!(cell_map.len(), written.len());
        self.data.extend(written.iter().copied());
        self.meta.push_back(Meta {
            node,
            serial: unsafe { (*node).serial },
            cell_map,
        });

        self.assert_integrity();
        added
    }

    /// Swap the needle for one of the same length. Test-only, mirrors `testChangeNeedle`.
    #[cfg(test)]
    pub(crate) fn test_change_needle(&mut self, new: &[u8]) {
        assert_eq!(new.len(), self.needle.len());
        self.needle = new.to_vec();
    }

    /// The two-slice (front, back) lengths of the data buffer, mirroring the Zig tests'
    /// `data.getPtrSlice(0, data.len())` → `slices[0].len` / `slices[1].len` assertions.
    #[cfg(test)]
    pub(crate) fn data_slice_lens(&self) -> (usize, usize) {
        let (a, b) = self.data.as_slices();
        (a.len(), b.len())
    }

    /// Verify data/metadata consistency. Port of `assertIntegrity` (debug-only).
    fn assert_integrity(&self) {
        if !cfg!(debug_assertions) {
            return;
        }
        let data_len: usize = self.meta.iter().map(|m| m.cell_map.len()).sum();
        debug_assert_eq!(data_len, self.data.len());
        debug_assert!(self.data.is_empty() || self.data_offset < self.data.len());
    }
}

/// Case-insensitive-ASCII substring search. Port of `std.ascii.indexOfIgnoreCase`.
///
/// Returns the byte index of the first occurrence of `needle` in `haystack` where ASCII
/// letters match regardless of case and all other bytes match exactly.
fn index_of_ignore_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    let first = needle[0];
    for i in 0..=(haystack.len() - needle.len()) {
        if haystack[i].eq_ignore_ascii_case(&first)
            && haystack[i..i + needle.len()]
                .iter()
                .zip(needle.iter())
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            return Some(i);
        }
    }
    None
}

/// Encode a page node to plaintext (`unwrap = true`) with a per-byte coordinate map.
///
/// This is the **plain + unwrap** subset of `PageFormatter.formatWithState` + its `point_map`
/// accounting (`formatter.zig:797-1360`). The full `formatter.rs` port deferred `point_map`
/// and exposes only whole-`Screen` rendering, so search carries its own per-node encoder.
/// The plain path has no headers/styles/hyperlinks — only blank-cell runs (spaces), codepoint
/// bytes, and deferred blank-row newlines, each mapped to its cell coordinate.
///
/// Returns `(bytes, cell_map)` where `cell_map[i]` is the `(x, y)` of `bytes[i]`. Trailing
/// blank rows are dropped (never flushed), matching the formatter (trim-trailing semantics).
///
/// # Safety
/// `node` must be a live node vended by a `PageList`.
fn encode_page_plain(node: *mut Node) -> (Vec<u8>, Vec<Coordinate>) {
    let page = unsafe { &(*node).data };
    let cols = page.size.cols as usize;
    let rows = page.size.rows as usize;

    let mut out: Vec<u8> = Vec::new();
    let mut map: Vec<Coordinate> = Vec::new();

    let mut blank_rows: usize = 0;
    let mut blank_cells: usize = 0;

    for y in 0..rows {
        let row = page.get_row(y);
        let cells: &[Cell] = unsafe {
            let slice = page.get_cells(row);
            &*slice
        };

        // Blank row: defer (accumulate) it.
        if !Cell::has_text_any(cells) {
            blank_rows += 1;
            continue;
        }

        // Flush deferred blank rows as newlines. The first newline inherits the prior
        // coordinate's x; the rest reference their own (prior) blank row with x = 0.
        if blank_rows > 0 {
            let start = map.last().copied().unwrap_or(Coordinate { x: 0, y: 0 });
            map.push(Coordinate {
                x: start.x,
                y: start.y,
            });
            out.push(b'\n');
            for y_offset in 1..blank_rows {
                map.push(Coordinate {
                    x: 0,
                    y: start.y + y_offset as u32,
                });
                out.push(b'\n');
            }
            blank_rows = 0;
        }

        let wrap = unsafe { (*row).wrap() };
        let wrap_cont = unsafe { (*row).wrap_continuation() };

        // Newline accounting after this row unless we unwrap a wrapped row.
        if !wrap {
            blank_rows += 1;
        }
        // Reset blank-cell run unless we continue a wrap.
        if !wrap_cont {
            blank_cells = 0;
        }

        for (x, cell) in cells.iter().enumerate() {
            // Skip spacers.
            match cell.wide() {
                Wide::Narrow | Wide::Wide => {}
                Wide::SpacerHead | Wide::SpacerTail => continue,
            }

            // Blank cell (no text, or trailing space): defer. Plain path always trims
            // trailing spaces (the formatter's `trim` is on for the plain preset).
            if !cell.has_text() || cell.codepoint() == u32::from(b' ') {
                blank_cells += 1;
                continue;
            }

            // Flush deferred blank cells as spaces, mapping each to its coordinate by
            // walking back from `(x, y)` (can cross rows on wrap continuation).
            if blank_cells > 0 {
                let mut remaining = blank_cells;
                let mut bx = x as CellCountInt;
                let mut by = y as u32;
                while remaining > 0 {
                    if bx > 0 {
                        bx -= 1;
                    } else if by > 0 {
                        by -= 1;
                        bx = cols as CellCountInt - 1;
                    } else {
                        bx = 0;
                        by = 0;
                    }
                    map.push(Coordinate { x: bx, y: by });
                    out.push(b' ');
                    remaining -= 1;
                }
                blank_cells = 0;
            }

            // Emit the cell's codepoint(s) as UTF-8, mapping every byte to (x, y).
            let cx = x as CellCountInt;
            let cy = y as u32;
            match cell.content_tag() {
                ContentTag::Codepoint | ContentTag::CodepointGrapheme => {
                    push_codepoint(cell.codepoint(), &mut out, &mut map, cx, cy);
                    if cell.content_tag() == ContentTag::CodepointGrapheme {
                        // SAFETY: cell belongs to page.
                        if let Some(slice) = unsafe { page.lookup_grapheme(cell as *const Cell) } {
                            for &cp in unsafe { &*slice } {
                                push_codepoint(cp, &mut out, &mut map, cx, cy);
                            }
                        }
                    }
                }
                // Bg-color-only cells: a space mapped to this cell.
                ContentTag::BgColorPalette | ContentTag::BgColorRgb => {
                    map.push(Coordinate { x: cx, y: cy });
                    out.push(b' ');
                }
            }
        }
    }

    (out, map)
}

/// Push a codepoint's UTF-8 bytes to `out`, mapping each byte to `(x, y)`.
fn push_codepoint(cp: u32, out: &mut Vec<u8>, map: &mut Vec<Coordinate>, x: CellCountInt, y: u32) {
    let Some(c) = char::from_u32(cp) else {
        return;
    };
    let mut buf = [0u8; 4];
    let s = c.encode_utf8(&mut buf);
    for &b in s.as_bytes() {
        out.push(b);
        map.push(Coordinate { x, y });
    }
}

#[cfg(test)]
mod tests;
