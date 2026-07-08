//! Column-resize reflow: `resizeCols` + `ReflowCursor` (the hardest part).
//!
//! Port of `PageList.zig:1018-2038`. Reflow is triggered only by column changes;
//! it eagerly rewrites the whole page list into pages of the new width, re-wrapping
//! (shrinking cols) or unwrapping (growing cols) text and remapping tracked pins.

use super::iter::Direction;
use super::ops::IncreaseCapacity;
use super::pin::Pin;
use super::resize::ResizeCursor;
use super::{Node, PageList};
use crate::page::size::CellCountInt;
use crate::page::{
    Capacity, Cell as PageCell, ContentTag, Page, ReflowManagedError, Row, SemanticPrompt, Wide,
};
use crate::point::{Point, Tag};

/// The kitty placeholder codepoint (U+10EEEE), always tracked. Mirrors the page layer.
const KITTY_PLACEHOLDER: u32 = 0x10EEEE;

/// Result of writing a single cell during reflow.
enum WriteResult {
    Success,
    Repeat,
    SkipNext,
}

/// Error from writing a cell: fatal or a capacity dimension to grow.
enum WriteError {
    Managed(ReflowManagedError),
}

impl PageList {
    /// Resize columns with reflow. Port of `resizeCols`.
    pub(crate) fn resize_cols(&mut self, cols: CellCountInt, cursor: Option<ResizeCursor>) {
        debug_assert_ne!(cols, self.cols);

        // preserved_cursor setup.
        struct Preserved {
            tracked_pin: *mut Pin,
            untrack: bool,
            remaining_rows: usize,
            wrapped_rows: usize,
        }

        // Resolve the cursor pin (if requested). An unresolvable cursor pin simply
        // yields `None` and the reflow proceeds without preserved-cursor growth
        // (matching Zig, where `pin(...) orelse break :cursor null` falls through).
        let preserved: Option<Preserved> = if let Some(c) = cursor {
            let resolved = match c.pin {
                Some(pin) => Some(unsafe { *pin }),
                None => self.pin(Point::active(c.x, c.y as u32)),
            };
            let p = match resolved {
                Some(p) => p,
                None => {
                    // Fall through with no preserved cursor.
                    return self.resize_cols_reflow(cols, None);
                }
            };
            let active_pin = self.pin(Point::active(0, 0));
            let wrapped = {
                let mut wrapped = 0usize;
                let skip = if let Some(ap) = active_pin {
                    unsafe { p.before(ap) }
                } else {
                    false
                };
                if !skip {
                    let mut it = unsafe { p.row_iterator(Direction::LeftUp, active_pin) };
                    while let Some(next) = unsafe { it.next() } {
                        if unsafe { (*next.row_and_cell().0).wrap_continuation() } {
                            wrapped += 1;
                        }
                    }
                }
                wrapped
            };
            let tracked_pin = match c.pin {
                Some(pin) => pin,
                None => self.track_pin(p),
            };
            Some(Preserved {
                tracked_pin,
                untrack: c.pin.is_none(),
                remaining_rows: (self.rows as usize).saturating_sub(c.y as usize + 1),
                wrapped_rows: wrapped,
            })
        } else {
            None
        };

        // Run the reflow, tracking the preserved cursor pin (if any) so it is
        // remapped into the new layout.
        let cursor_pin = preserved.as_ref().map(|c| c.tracked_pin);
        self.resize_cols_reflow(cols, cursor_pin);

        // Preserved-cursor growth.
        if let Some(c) = preserved {
            'cursor: {
                let active_pt = match self.point_from_pin(Tag::Active, unsafe { *c.tracked_pin }) {
                    Some(pt) => pt,
                    None => break 'cursor,
                };
                let active_pin = self.pin(Point::active(0, 0));
                let wrapped = {
                    let mut wrapped = 0usize;
                    let mut row_it =
                        unsafe { (*c.tracked_pin).row_iterator(Direction::LeftUp, active_pin) };
                    while let Some(next) = unsafe { row_it.next() } {
                        if unsafe { (*next.row_and_cell().0).wrap_continuation() } {
                            wrapped += 1;
                        }
                    }
                    wrapped
                };
                let current = (self.rows as usize).saturating_sub(active_pt.coord().y as usize + 1);
                let mut req_rows = c.remaining_rows;
                req_rows = req_rows.saturating_sub(wrapped.saturating_sub(c.wrapped_rows));
                req_rows = req_rows.saturating_sub(current);
                while req_rows > 0 {
                    self.grow();
                    req_rows -= 1;
                }
            }
            if c.untrack {
                self.untrack_pin(c.tracked_pin);
            }
        }
    }

    /// The core reflow body shared by both the cursor and no-cursor paths of
    /// `resize_cols`: sets `cols`, creates the first rewritten page, orphans the old
    /// list, drives a [`ReflowCursor`] over every old row (remapping `cursor_pin`),
    /// grows to at least the active row count, and fixes the viewport if unwrapping
    /// landed the pin in the active area. Port of `resizeCols` (`:1084-1178`).
    fn resize_cols_reflow(&mut self, cols: CellCountInt, cursor_pin: Option<*mut Pin>) {
        self.cols = cols;

        // Create the first rewritten node.
        let first_rewritten = {
            let page = unsafe { &(*self.pages.first).data };
            let cap = match page.capacity.adjust_cols(cols) {
                Ok(c) => c,
                Err(_) => {
                    let mut cap = page.capacity;
                    cap.cols = cols;
                    cap.rows = page.size.rows.min(cap.rows);
                    cap
                }
            };
            let node = self.create_page(cap);
            unsafe { (*node).data.size.rows = 1 };
            node
        };

        // Row iterator over the OLD list before we orphan it.
        let mut it = self.row_iterator(Direction::RightDown, Point::screen(0, 0), None);

        // Orphan old pages: new node becomes the only page.
        self.pages.first = first_rewritten;
        self.pages.last = first_rewritten;

        // Reflow all rows, destroying each source page once its last row is consumed.
        {
            let mut rc = ReflowCursor::init(first_rewritten);
            while let Some(row) = unsafe { it.next() } {
                rc.reflow_row(self, row, cursor_pin);
                unsafe {
                    if row.y == (*row.node).data.size.rows - 1 {
                        self.destroy_node(row.node);
                    }
                }
            }
            self.total_rows = rc.total_rows;
        }

        // Grow to at least the active row count.
        let mut node = self.pages.first;
        let mut total = 0usize;
        let mut enough = false;
        while !node.is_null() {
            total += unsafe { (*node).data.size.rows } as usize;
            if total >= self.rows as usize {
                enough = true;
                break;
            }
            node = unsafe { (*node).next };
        }
        if !enough {
            for _ in total..self.rows as usize {
                self.grow();
            }
        }

        // Fix viewport if unwrapping landed the pin in active.
        if self.viewport == super::Viewport::Pin
            && self.pin_is_active(unsafe { *self.viewport_pin })
        {
            self.viewport = super::Viewport::Active;
        }
    }
}

/// The reflow state machine. Port of `ReflowCursor`.
struct ReflowCursor {
    x: CellCountInt,
    y: CellCountInt,
    pending_wrap: bool,
    node: *mut Node,
    page_row: *mut Row,
    page_cell: *mut PageCell,
    new_rows: usize,
    total_rows: usize,
}

impl ReflowCursor {
    fn init(node: *mut Node) -> ReflowCursor {
        unsafe {
            let page = &mut (*node).data;
            let row = page.get_row(0);
            let cell = page.get_cells(row).cast::<PageCell>();
            ReflowCursor {
                x: 0,
                y: 0,
                pending_wrap: false,
                node,
                page_row: row,
                page_cell: cell,
                new_rows: 0,
                total_rows: page.size.rows as usize,
            }
        }
    }

    #[inline]
    fn page(&self) -> *mut Page {
        unsafe { &mut (*self.node).data }
    }

    /// Reflow one source row into this cursor. Port of `reflowRow`.
    fn reflow_row(&mut self, list: &mut PageList, row: Pin, cursor_pin: Option<*mut Pin>) {
        unsafe {
            let src_node = row.node;
            let src_page: *const Page = &(*src_node).data;
            let src_row = row.row_and_cell().0;
            let src_y = row.y;
            let src_cols = (*src_page).size.cols as usize;
            let cells_base = (*src_row).cells().ptr((*src_page).memory() as *mut u8);

            // Compute cols_len: trim trailing blanks for non-wrapped rows.
            let mut cols_len = (*src_page).size.cols;
            if !(*src_row).wrap() {
                while cols_len > 0 {
                    if !(*cells_base.add(cols_len as usize - 1)).is_empty() {
                        break;
                    }
                    cols_len -= 1;
                }
                if cols_len == 0 && (*src_row).semantic_prompt() != SemanticPrompt::None {
                    cols_len = 1;
                }
            }

            // Tracked pin adjustments in the trailing-blank region.
            let dst_cols = (*self.page()).size.cols;
            for &p in &list.tracked_pins {
                if (*p).node != src_node || (*p).y != src_y {
                    continue;
                }
                if let Some(cp) = cursor_pin
                    && p == cp
                {
                    continue;
                }
                if (*p).x >= cols_len {
                    (*p).x = (*p).x.min(dst_cols - 1 - self.x);
                }
                cols_len = cols_len.max((*p).x + 1);
            }
            // Cursor pin blank preservation.
            if let Some(cp) = cursor_pin
                && (*cp).node == src_node
                && (*cp).y == src_y
            {
                cols_len = cols_len.max((*cp).x + 1);
            }

            // Defer blank rows.
            if cols_len == 0 {
                if !(*src_row).wrap_continuation() {
                    self.new_rows += 1;
                }
                return;
            }

            // Capacity to inherit for new pages.
            let cap = match (*src_page).capacity.adjust_cols((*self.page()).size.cols) {
                Ok(c) => c,
                Err(_) => {
                    let mut cap = (*src_page).capacity;
                    cap.cols = (*self.page()).size.cols;
                    cap.rows = (*src_page).size.rows.min(Capacity::std().rows);
                    cap
                }
            };

            // Flush deferred blank rows.
            while self.new_rows > 0 {
                self.cursor_scroll_or_new_page(list, cap);
                self.new_rows -= 1;
            }

            self.copy_row_metadata(src_row);

            let mut x: usize = 0;
            while x < cols_len as usize {
                if self.pending_wrap {
                    (*self.page_row).set_wrap(true);
                    self.cursor_scroll_or_new_page(list, cap);
                    self.copy_row_metadata(src_row);
                    (*self.page_row).set_wrap_continuation(true);
                }

                // Remap pins at current source x.
                for &p in &list.tracked_pins {
                    if (*p).node == src_node && (*p).y == src_y && (*p).x as usize == x {
                        (*p).node = self.node;
                        (*p).x = self.x;
                        (*p).y = self.y;
                    }
                }

                let src_cell = cells_base.add(x);
                match self.write_cell(list, src_cell, src_page) {
                    Ok(WriteResult::Success) => x += 1,
                    Ok(WriteResult::SkipNext) => {
                        for &p in &list.tracked_pins {
                            if (*p).node == src_node && (*p).y == src_y && (*p).x as usize == x + 1
                            {
                                (*p).node = self.node;
                                (*p).x = self.x;
                                (*p).y = self.y;
                            }
                        }
                        x += 2;
                    }
                    Ok(WriteResult::Repeat) => {}
                    Err(WriteError::Managed(dim)) => {
                        if self.y == 0 {
                            // Can't split; degrade.
                            x += 1;
                            self.cursor_forward();
                        } else {
                            let _ = dim;
                            self.move_last_row_to_new_page(list, cap);
                            // retry same x
                        }
                    }
                }
                let _ = src_cols;
            }

            if !(*src_row).wrap() {
                self.new_rows += 1;
            }
        }
    }

    /// Write a single cell (unmanaged bits + managed copy). Port of `writeCell`.
    unsafe fn write_cell(
        &mut self,
        list: &mut PageList,
        cell: *const PageCell,
        src_page: *const Page,
    ) -> Result<WriteResult, WriteError> {
        unsafe {
            let dst_cols = (*self.page()).size.cols;

            // Basic unmanaged bits.
            match (*cell).content_tag() {
                ContentTag::Codepoint | ContentTag::CodepointGrapheme => match (*cell).wide() {
                    Wide::Narrow => *self.page_cell = *cell,
                    Wide::Wide => {
                        if dst_cols > 1 {
                            if self.x == dst_cols - 1 {
                                let mut sh = PageCell::init(0);
                                sh.set_wide(Wide::SpacerHead);
                                *self.page_cell = sh;
                                self.cursor_forward();
                                return Ok(WriteResult::Repeat);
                            } else {
                                *self.page_cell = *cell;
                            }
                        } else {
                            (*self.page_cell).set_codepoint(0);
                            (*self.page_cell).set_wide(Wide::Narrow);
                            self.cursor_forward();
                            return Ok(WriteResult::SkipNext);
                        }
                    }
                    Wide::SpacerTail => {
                        if dst_cols > 1 {
                            *self.page_cell = *cell;
                        } else {
                            return Ok(WriteResult::Success);
                        }
                    }
                    Wide::SpacerHead => {
                        return Ok(WriteResult::Success);
                    }
                },
                ContentTag::BgColorPalette | ContentTag::BgColorRgb => {
                    *self.page_cell = *cell;
                    self.cursor_forward();
                    return Ok(WriteResult::Success);
                }
            }

            // Reset managed markers before managed copy.
            (*self.page_cell).set_content_tag(ContentTag::Codepoint);
            (*self.page_cell).set_hyperlink(false);
            (*self.page_cell).set_style_id(crate::page::style_default_id());

            if (*cell).codepoint() == KITTY_PLACEHOLDER {
                (*self.page_row).set_kitty_virtual_placeholder(true);
            }

            // Managed copy (grapheme/hyperlink/style).
            match (*self.page()).reflow_copy_managed(src_page, cell, self.page_row, self.page_cell)
            {
                Ok(()) => {
                    self.cursor_forward();
                    Ok(WriteResult::Success)
                }
                Err(dim) => {
                    // Grow the page and retry the whole cell.
                    let adj = match dim {
                        ReflowManagedError::Styles => Some(IncreaseCapacity::Styles),
                        ReflowManagedError::GraphemeBytes => Some(IncreaseCapacity::GraphemeBytes),
                        ReflowManagedError::HyperlinkBytes => {
                            Some(IncreaseCapacity::HyperlinkBytes)
                        }
                        ReflowManagedError::StringBytes => Some(IncreaseCapacity::StringBytes),
                        ReflowManagedError::Rehash => None,
                    };
                    if self.increase_capacity(list, adj).is_err() {
                        return Err(WriteError::Managed(dim));
                    }
                    // Reborrow src_page and cell would be stale? No: src_page is a
                    // separate page (not the one we grew). The dst cursor is reinit.
                    // Signal caller to retry (Repeat re-runs the same src x).
                    Ok(WriteResult::Repeat)
                }
            }
        }
    }

    /// Move the current (last) row to a new page when out of capacity mid-row.
    /// Port of `moveLastRowToNewPage`.
    fn move_last_row_to_new_page(&mut self, list: &mut PageList, cap: Capacity) {
        unsafe {
            debug_assert_eq!(self.y, (*self.page()).size.rows - 1);
            debug_assert!(!self.pending_wrap);

            let old_node = self.node;
            let old_x = self.x;
            let old_total = self.total_rows;

            self.cursor_new_page(list, cap);
            debug_assert_ne!(self.node, old_node);
            self.total_rows = old_total;

            self.cursor_absolute(old_x, 0);

            let old_page: *mut Page = &mut (*old_node).data;
            let old_row = (*old_page).get_row((*old_page).size.rows as usize - 1);
            (*self.page())
                .clone_row_from(&*old_page, self.page_row, old_row)
                .expect("clone_row_from in move_last_row_to_new_page");

            for &p in &list.tracked_pins {
                if (*p).node == old_node && (*p).y == (*old_page).size.rows - 1 {
                    (*p).node = self.node;
                    (*p).y = self.y;
                }
            }

            (*old_page).clear_cells(old_row, 0, (*old_page).size.cols as usize);
            (*old_page).size.rows -= 1;
            if (*old_page).size.rows == 0 {
                list.pages.remove(old_node);
                list.destroy_node(old_node);
            }
        }
    }

    /// Increase capacity of the current page, reinit cursor. Port of ReflowCursor
    /// `increaseCapacity`.
    unsafe fn increase_capacity(
        &mut self,
        list: &mut PageList,
        adj: Option<IncreaseCapacity>,
    ) -> Result<(), ()> {
        let old_x = self.x;
        let old_y = self.y;
        let old_total = self.total_rows;
        unsafe {
            (*self.page()).pause_integrity_checks(true);
            let node = list.increase_capacity(self.node, adj);
            // Note: the old page was destroyed by increase_capacity; can't unpause it.
            let node = node?;
            *self = ReflowCursor::init(node);
            self.cursor_absolute(old_x, old_y);
            self.total_rows = old_total;
        }
        Ok(())
    }

    fn bottom(&self) -> bool {
        unsafe { self.y == (*self.page()).capacity.rows - 1 }
    }

    fn cursor_forward(&mut self) {
        unsafe {
            if self.x == (*self.page()).size.cols - 1 {
                self.pending_wrap = true;
            } else {
                self.page_cell = self.page_cell.add(1);
                self.x += 1;
            }
        }
    }

    fn cursor_scroll(&mut self) {
        unsafe {
            debug_assert_eq!(self.y, (*self.page()).size.rows - 1);
            debug_assert!((*self.page()).size.rows < (*self.page()).capacity.rows);
            (*self.page()).size.rows += 1;
            self.page_row = self.page_row.add(1);
            self.page_cell = (*self.page()).get_cells(self.page_row).cast::<PageCell>();
            self.pending_wrap = false;
            self.x = 0;
            self.y += 1;
        }
    }

    fn cursor_new_page(&mut self, list: &mut PageList, cap: Capacity) {
        unsafe {
            let new_rows = self.new_rows;
            let node = list.create_page(cap);
            (*node).data.size.rows = 1;
            list.pages.insert_after(self.node, node);
            *self = ReflowCursor::init(node);
            self.new_rows = new_rows;
        }
    }

    fn cursor_scroll_or_new_page(&mut self, list: &mut PageList, cap: Capacity) {
        let new_total = self.total_rows + 1;
        if self.bottom() {
            self.cursor_new_page(list, cap);
        } else {
            self.cursor_scroll();
        }
        self.total_rows = new_total;
    }

    fn cursor_absolute(&mut self, x: CellCountInt, y: CellCountInt) {
        unsafe {
            debug_assert!(x < (*self.page()).size.cols);
            debug_assert!(y < (*self.page()).size.rows);
            use std::cmp::Ordering;
            let row = match y.cmp(&self.y) {
                Ordering::Equal => self.page_row,
                Ordering::Less => self.page_row.sub((self.y - y) as usize),
                Ordering::Greater => self.page_row.add((y - self.y) as usize),
            };
            self.page_row = row;
            self.page_cell = (*self.page())
                .get_cells(row)
                .cast::<PageCell>()
                .add(x as usize);
            self.pending_wrap = false;
            self.x = x;
            self.y = y;
        }
    }

    fn copy_row_metadata(&mut self, other: *const Row) {
        unsafe {
            (*self.page_row).set_semantic_prompt((*other).semantic_prompt());
        }
    }
}
