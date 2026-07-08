//! Resize-without-reflow, erase, reset, and clone (port of the corresponding
//! PageList methods). The reflow engine lives in `reflow.rs`.

use super::iter::{Chunk, Direction};
use super::pin::Pin;
use super::{MemoryPool, Node, NodeList, PageList, Viewport, min_max_size};
use crate::page::Cell as PageCell;
use crate::page::size::CellCountInt;
use crate::point::{Point, Tag};

/// Resize options. Port of `PageList.Resize`.
#[derive(Debug, Clone, Copy)]
pub struct Resize {
    pub cols: Option<CellCountInt>,
    pub rows: Option<CellCountInt>,
    pub reflow: bool,
    pub cursor: Option<ResizeCursor>,
}

impl Default for Resize {
    fn default() -> Self {
        Resize {
            cols: None,
            rows: None,
            reflow: true,
            cursor: None,
        }
    }
}

/// Split failure. Port of `PageList.SplitError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitError {
    OutOfSpace,
}

/// Cursor info for resize. Port of `Resize.Cursor`.
#[derive(Debug, Clone, Copy)]
pub struct ResizeCursor {
    pub x: CellCountInt,
    pub y: CellCountInt,
    /// Optional pre-existing tracked pin at the cursor.
    pub pin: Option<*mut Pin>,
}

impl PageList {
    /// Resize the page list. Port of `resize`.
    pub fn resize(&mut self, opts: Resize) {
        self.invalidate_viewport_offset();

        if !opts.reflow {
            self.resize_without_reflow(opts);
            self.assert_integrity();
            return;
        }

        let old_min_max = self.min_max_size;
        self.min_max_size = min_max_size(
            opts.cols.unwrap_or(self.cols),
            opts.rows.unwrap_or(self.rows),
        );
        let _ = old_min_max;

        let cols = opts.cols.unwrap_or(self.cols);
        use std::cmp::Ordering;
        match cols.cmp(&self.cols) {
            Ordering::Equal => self.resize_without_reflow(opts),
            Ordering::Greater => {
                self.resize_cols(cols, opts.cursor);
                self.resize_without_reflow(opts);
            }
            Ordering::Less => {
                let mut copy = opts;
                copy.cols = Some(self.cols);
                self.resize_without_reflow(copy);
                self.resize_cols(cols, opts.cursor);
            }
        }

        if self.viewport == Viewport::Pin && self.pin_is_active(unsafe { *self.viewport_pin }) {
            self.viewport = Viewport::Active;
        }
        self.assert_integrity();
    }

    /// Resize without reflow (row/col truncation or padding). Port of
    /// `resizeWithoutReflow`.
    pub(crate) fn resize_without_reflow(&mut self, opts: Resize) {
        if !opts.reflow {
            self.min_max_size = min_max_size(
                opts.cols.unwrap_or(self.cols),
                opts.rows.unwrap_or(self.rows),
            );
        }

        if let Some(cols) = opts.cols {
            use std::cmp::Ordering;
            match cols.cmp(&self.cols) {
                Ordering::Equal => {}
                Ordering::Less => {
                    // Shrink cols: clear beyond-cols cells, set page cols, clamp pins.
                    let mut it =
                        self.page_iterator(Direction::RightDown, Point::screen(0, 0), None);
                    while let Some(chunk) = unsafe { it.next() } {
                        unsafe {
                            let rows = (*chunk.node).data.size.rows as usize;
                            for y in 0..rows {
                                let row = (*chunk.node).data.get_row(y);
                                (*chunk.node).data.clear_cells(
                                    row,
                                    cols as usize,
                                    self.cols as usize,
                                );
                            }
                            (*chunk.node).data.size.cols = cols;
                            (*chunk.node).data.assert_integrity();
                        }
                    }
                    for &p in &self.tracked_pins {
                        unsafe {
                            if (*p).x >= cols {
                                (*p).x = cols - 1;
                            }
                        }
                    }
                    self.cols = cols;
                }
                Ordering::Greater => {
                    let old_cols = self.cols;
                    let mut it =
                        self.page_iterator(Direction::RightDown, Point::screen(0, 0), None);
                    while let Some(chunk) = unsafe { it.next() } {
                        self.cols = old_cols;
                        self.resize_grow_cols(cols, chunk);
                    }
                    self.cols = cols;
                }
            }
        }

        if let Some(rows) = opts.rows {
            use std::cmp::Ordering;
            match rows.cmp(&self.rows) {
                Ordering::Equal => {}
                Ordering::Less => {
                    let trimmed = self.trim_trailing_blank_rows(self.rows - rows);
                    self.total_rows -= trimmed as usize;
                    self.rows = rows;
                }
                Ordering::Greater => {
                    if let Some(cursor) = opts.cursor
                        && cursor.y < self.rows - 1
                    {
                        let delta = rows - self.rows;
                        self.rows = rows;
                        for _ in 0..delta {
                            self.grow();
                        }
                        return;
                    }
                    self.rows = rows;
                    let mut count = 0usize;
                    let mut node = self.pages.first;
                    let mut enough = false;
                    while !node.is_null() {
                        count += unsafe { (*node).data.size.rows } as usize;
                        if count >= rows as usize {
                            enough = true;
                            break;
                        }
                        node = unsafe { (*node).next };
                    }
                    if !enough {
                        for _ in count..rows as usize {
                            self.grow();
                        }
                    }
                    if self.viewport == Viewport::Pin
                        && self.pin_is_active(unsafe { *self.viewport_pin })
                    {
                        self.viewport = Viewport::Active;
                    }
                }
            }
        }
    }

    /// Grow columns without reflow for a single page chunk. Port of
    /// `resizeWithoutReflowGrowCols`.
    fn resize_grow_cols(&mut self, cols: CellCountInt, chunk: Chunk) {
        debug_assert!(cols > self.cols);
        let node = chunk.node;
        self.cols = cols;

        unsafe {
            // Fast path: capacity in the page (unless a stale spacer_head at old last col).
            if (*node).data.capacity.cols >= cols {
                let old_cols = (*node).data.size.cols;
                let mut fast = true;
                for y in 0..(*node).data.size.rows as usize {
                    let row = (*node).data.get_row(y);
                    let cells = (*node).data.get_cells(row);
                    if (*cells)[old_cols as usize - 1].wide() == crate::page::Wide::SpacerHead {
                        fast = false;
                        break;
                    }
                }
                if fast {
                    (*node).data.size.cols = cols;
                    return;
                }
            }
        }

        // Slow path: allocate wider page(s) and copy rows.
        let cap = match (unsafe { &(*node).data }).capacity.adjust_cols(cols) {
            Ok(c) => c,
            Err(_) => {
                let mut cap = unsafe { (*node).data.capacity };
                cap.cols = cols;
                cap.rows = unsafe { (*node).data.size.rows }.min(cap.rows);
                cap
            }
        };

        let prev = unsafe { (*node).prev };
        let mut copied: CellCountInt = 0;

        // Try to fill the previous page's spare capacity first.
        if !prev.is_null() {
            unsafe {
                let prev_has_room = (*prev).data.size.rows < (*prev).data.capacity.rows;
                if prev_has_room {
                    let len = ((*prev).data.capacity.rows - (*prev).data.size.rows)
                        .min((*node).data.size.rows);
                    let mut done = 0;
                    let mut failed = false;
                    for i in 0..len {
                        let dst_y = (*prev).data.size.rows as usize;
                        (*prev).data.size.rows += 1;
                        let dst_row = (*prev).data.get_row(dst_y);
                        let src_row = (*node).data.get_row(i as usize);
                        copied += 1;
                        if (*prev)
                            .data
                            .clone_row_from(&(*node).data, dst_row, src_row)
                            .is_err()
                        {
                            (*prev).data.size.rows -= 1;
                            copied -= 1;
                            failed = true;
                            break;
                        }
                        done += 1;
                    }
                    if !failed {
                        debug_assert_eq!(done, len);
                        // Remap pins that pointed to rows copied to prev.
                        for &p in &self.tracked_pins {
                            if (*p).node != node || (*p).y >= len {
                                continue;
                            }
                            (*p).node = prev;
                            (*p).y += (*prev).data.size.rows - len;
                        }
                    }
                }
            }
        }

        // Split remaining rows into new pages.
        unsafe {
            while copied < (*node).data.size.rows {
                let new_node = self.create_page(cap);
                let len = cap.rows.min((*node).data.size.rows - copied);
                let y_start = copied;
                for i in 0..len {
                    (*new_node).data.size.rows += 1;
                    let dst_row = (*new_node).data.get_row(i as usize);
                    let src_row = (*node).data.get_row((y_start + i) as usize);
                    if (*new_node)
                        .data
                        .clone_row_from(&(*node).data, dst_row, src_row)
                        .is_ok()
                    {
                        copied += 1;
                    } else {
                        (*new_node).data.size.rows -= 1;
                        break;
                    }
                }
                let y_end = copied;
                (*new_node).data.assert_integrity();
                self.pages.insert_before(node, new_node);
                for &p in &self.tracked_pins {
                    if (*p).node != node || (*p).y < y_start || (*p).y >= y_end {
                        continue;
                    }
                    (*p).node = new_node;
                    (*p).y -= y_start;
                }
            }
            debug_assert_eq!(copied, (*node).data.size.rows);

            self.pages.remove(node);
            self.destroy_node(node);
        }
    }

    /// Count trailing blank rows up to `max`. Port of `trailingBlankLines`.
    #[allow(dead_code)]
    pub(crate) fn trailing_blank_lines(&self, max: CellCountInt) -> CellCountInt {
        let mut count = 0;
        let mut node = self.pages.last;
        while !node.is_null() {
            unsafe {
                let len = (*node).data.size.rows as usize;
                for i in 0..len {
                    let rev_i = len - i - 1;
                    let row = (*node).data.get_row(rev_i);
                    let cells = (*node).data.get_cells(row);
                    let slice = std::slice::from_raw_parts(
                        cells.cast::<PageCell>(),
                        (*node).data.size.cols as usize,
                    );
                    if PageCell::has_text_any(slice) {
                        return count;
                    }
                    count += 1;
                    if count >= max {
                        return count;
                    }
                }
                node = (*node).prev;
            }
        }
        count
    }

    /// Trim up to `max` trailing blank rows (respecting pins). Returns the number
    /// trimmed. Port of `trimTrailingBlankRows`.
    pub(crate) fn trim_trailing_blank_rows(&mut self, max: CellCountInt) -> CellCountInt {
        let mut trimmed = 0;
        let bl = self.get_bottom_right(Tag::Screen).unwrap();
        let mut it = unsafe { bl.row_iterator(Direction::LeftUp, None) };
        while let Some(row_pin) = unsafe { it.next() } {
            unsafe {
                let cells = row_pin.cells(super::CellSubset::All);
                if PageCell::has_text_any(&*cells) {
                    return trimmed;
                }
                let mut has_pin = false;
                for &p in &self.tracked_pins {
                    if (*p).node == row_pin.node && (*p).y == row_pin.y {
                        has_pin = true;
                        break;
                    }
                }
                if has_pin {
                    return trimmed;
                }
                (*row_pin.node).data.size.rows -= 1;
                if (*row_pin.node).data.size.rows == 0 {
                    self.erase_page(row_pin.node);
                } else {
                    (*row_pin.node).data.assert_integrity();
                }
                trimmed += 1;
                if trimmed >= max {
                    return trimmed;
                }
            }
        }
        trimmed
    }

    // ---- reset ----

    /// Reset to an empty state, preserving tracked-pin pointer stability. Port of
    /// `reset`.
    pub fn reset(&mut self) {
        self.page_serial_min = self.page_serial;

        // Free all existing pages.
        let mut it = self.pages.first;
        while !it.is_null() {
            let next = unsafe { (*it).next };
            unsafe { self.destroy_node(it) };
            it = next;
        }
        self.pages = NodeList::empty();
        self.page_size = 0;

        let (pages, page_size) =
            super::init_pages(&mut self.pool, &mut self.page_serial, self.cols, self.rows);
        self.pages = pages;
        self.page_size = page_size;
        self.total_rows = self.rows as usize;

        // Move all tracked pins to the first page, mark garbage.
        let first = self.pages.first;
        for &p in &self.tracked_pins {
            unsafe {
                (*p).node = first;
                (*p).x = 0;
                (*p).y = 0;
                (*p).garbage = true;
            }
        }
        unsafe { (*self.viewport_pin).garbage = false };
        self.viewport = Viewport::Active;
        self.assert_integrity();
    }

    // ---- erase ----

    /// Erase all history rows, optionally up to a bottom bound. Port of `eraseHistory`.
    pub fn erase_history(&mut self, bl_pt: Option<Point>) {
        self.erase_rows(Point::history(0, 0), bl_pt);
    }

    /// Erase active rows from the top to `y` inclusive. Port of `eraseActive`.
    pub fn erase_active(&mut self, y: CellCountInt) {
        debug_assert!(y < self.rows);
        self.erase_rows(Point::active(0, 0), Some(Point::active(0, y as u32)));
    }

    /// Physically erase rows in `[tl, bl]`. Port of `eraseRows`.
    fn erase_rows(&mut self, tl_pt: Point, bl_pt: Option<Point>) {
        let mut erased = 0usize;
        let mut it = self.page_iterator(Direction::RightDown, tl_pt, bl_pt);
        while let Some(chunk) = unsafe { it.next() } {
            unsafe {
                if chunk.full_page() {
                    // Special case: erasing the only page.
                    if (*chunk.node).next.is_null() && (*chunk.node).prev.is_null() {
                        erased += (*chunk.node).data.size.rows as usize;
                        (*chunk.node).data.reinit();
                        (*chunk.node).data.size.rows = 0;
                        break;
                    }
                    erased += (*chunk.node).data.size.rows as usize;
                    self.erase_page(chunk.node);
                    continue;
                }

                debug_assert_eq!(chunk.start, 0);
                let scroll_amount = (*chunk.node).data.size.rows - chunk.end;
                for i in 0..scroll_amount as usize {
                    let src = (*chunk.node).data.get_row(i + chunk.end as usize);
                    let dst = (*chunk.node).data.get_row(i);
                    std::ptr::swap(dst, src);
                    (*dst).set_dirty(true);
                }
                for i in scroll_amount as usize..(*chunk.node).data.size.rows as usize {
                    let row = (*chunk.node).data.get_row(i);
                    (*chunk.node)
                        .data
                        .clear_cells(row, 0, (*chunk.node).data.size.cols as usize);
                }
                for &p in &self.tracked_pins {
                    if (*p).node != chunk.node {
                        continue;
                    }
                    if (*p).y >= chunk.end {
                        (*p).y -= chunk.end;
                    } else {
                        (*p).y = 0;
                        (*p).x = 0;
                    }
                }
                (*chunk.node).data.size.rows = scroll_amount;
                erased += chunk.end as usize;
                (*chunk.node).data.assert_integrity();
            }
        }

        self.total_rows -= erased;

        if tl_pt.tag == Tag::Active {
            for _ in 0..erased {
                self.grow();
            }
        }
        self.fixup_viewport(erased);
        self.assert_integrity();
    }

    /// Erase a single page (front/back only). Port of `erasePage`.
    pub(crate) fn erase_page(&mut self, node: *mut Node) {
        unsafe {
            debug_assert!(!(*node).next.is_null() || !(*node).prev.is_null());
            debug_assert!((*node).prev.is_null() || (*node).next.is_null());

            if (*node).prev.is_null() {
                self.page_serial_min = (*(*node).next).serial;
            }

            for &p in &self.tracked_pins {
                if (*p).node != node {
                    continue;
                }
                let target = if !(*node).prev.is_null() {
                    (*node).prev
                } else {
                    (*node).next
                };
                (*p).node = target;
                (*p).y = 0;
                (*p).x = 0;
            }

            self.pages.remove(node);
            self.destroy_node(node);
        }
    }

    /// Fast-path erase of exactly one row, shifting following rows up. Port of `eraseRow`.
    pub fn erase_row(&mut self, pt: Point) {
        let pn = self.pin(pt).unwrap();
        let mut node = pn.node;
        unsafe {
            rotate_once_rows(
                &mut (*node).data,
                pn.y as usize,
                (*node).data.size.rows as usize,
            );

            for &p in &self.tracked_pins {
                if (*p).node == node && (*p).y > pn.y {
                    (*p).y -= 1;
                }
            }
            self.fixup_viewport(1);
            (*node).data.dirty = true;

            while !(*node).next.is_null() {
                let next = (*node).next;
                let last_row = (*node).data.get_row((*node).data.size.rows as usize - 1);
                let next_first = (*next).data.get_row(0);
                (*node)
                    .data
                    .clone_row_from(&(*next).data, last_row, next_first)
                    .expect("clone_row_from in erase_row");

                node = next;
                rotate_once_rows(&mut (*node).data, 0, (*node).data.size.rows as usize);
                (*node).data.dirty = true;

                for &p in &self.tracked_pins {
                    if (*p).node != node {
                        continue;
                    }
                    if (*p).y == 0 {
                        (*p).node = (*node).prev;
                        (*p).y = (*(*node).prev).data.size.rows - 1;
                    } else {
                        (*p).y -= 1;
                    }
                }
            }

            let last_row = (*node).data.get_row((*node).data.size.rows as usize - 1);
            (*node)
                .data
                .clear_cells(last_row, 0, (*node).data.size.cols as usize);
        }
        self.assert_integrity();
    }

    /// Erase one row, shifting only `limit` following rows up (leaving a blank).
    /// Port of `eraseRowBounded`.
    pub fn erase_row_bounded(&mut self, pt: Point, limit: usize) {
        let pn = self.pin(pt).unwrap();
        let mut node = pn.node;
        unsafe {
            // In-page bounded shift.
            if (*node).data.size.rows as usize - pn.y as usize > limit {
                let row = (*node).data.get_row(pn.y as usize);
                (*node)
                    .data
                    .clear_cells(row, 0, (*node).data.size.cols as usize);
                rotate_once_rows(&mut (*node).data, pn.y as usize, pn.y as usize + limit + 1);
                (*node).data.dirty = true;

                if self.viewport == Viewport::Pin {
                    let p = self.viewport_pin;
                    if let Some(v) = self.viewport_offset_cache().as_mut() {
                        let ok = (*p).node == node
                            && (*p).y >= pn.y
                            && (*p).y <= pn.y + limit as CellCountInt
                            && (*p).y != 0;
                        if ok {
                            *v -= 1;
                        }
                    }
                }

                for &p in &self.tracked_pins {
                    if (*p).node == node && (*p).y >= pn.y && (*p).y <= pn.y + limit as CellCountInt
                    {
                        if (*p).y == 0 {
                            (*p).x = 0;
                        } else {
                            (*p).y -= 1;
                        }
                    }
                }
                self.assert_integrity();
                return;
            }

            rotate_once_rows(
                &mut (*node).data,
                pn.y as usize,
                (*node).data.size.rows as usize,
            );
            (*node).data.dirty = true;
            let mut shifted = (*node).data.size.rows as usize - pn.y as usize;

            if self.viewport == Viewport::Pin {
                let p = self.viewport_pin;
                if let Some(v) = self.viewport_offset_cache().as_mut()
                    && (*p).node == node
                    && (*p).y >= pn.y
                    && (*p).y != 0
                {
                    *v -= 1;
                }
            }
            for &p in &self.tracked_pins {
                if (*p).node == node && (*p).y >= pn.y {
                    if (*p).y == 0 {
                        (*p).x = 0;
                    } else {
                        (*p).y -= 1;
                    }
                }
            }

            while !(*node).next.is_null() {
                let next = (*node).next;
                let last_row = (*node).data.get_row((*node).data.size.rows as usize - 1);
                let next_first = (*next).data.get_row(0);
                (*node)
                    .data
                    .clone_row_from(&(*next).data, last_row, next_first)
                    .expect("clone_row_from in erase_row_bounded");

                node = next;

                let shifted_limit = limit - shifted;
                if (*node).data.size.rows as usize > shifted_limit {
                    let row0 = (*node).data.get_row(0);
                    (*node)
                        .data
                        .clear_cells(row0, 0, (*node).data.size.cols as usize);
                    rotate_once_rows(&mut (*node).data, 0, shifted_limit + 1);
                    (*node).data.dirty = true;

                    if self.viewport == Viewport::Pin {
                        let p = self.viewport_pin;
                        if let Some(v) = self.viewport_offset_cache().as_mut()
                            && (*p).node == node
                            && (*p).y as usize <= shifted_limit
                        {
                            *v -= 1;
                        }
                    }
                    for &p in &self.tracked_pins {
                        if (*p).node != node || (*p).y as usize > shifted_limit {
                            continue;
                        }
                        if (*p).y == 0 {
                            (*p).node = (*node).prev;
                            (*p).y = (*(*node).prev).data.size.rows - 1;
                        } else {
                            (*p).y -= 1;
                        }
                    }
                    self.assert_integrity();
                    return;
                }

                rotate_once_rows(&mut (*node).data, 0, (*node).data.size.rows as usize);
                (*node).data.dirty = true;
                shifted += (*node).data.size.rows as usize;

                if self.viewport == Viewport::Pin {
                    let p = self.viewport_pin;
                    if let Some(v) = self.viewport_offset_cache().as_mut()
                        && (*p).node == node
                    {
                        *v -= 1;
                    }
                }
                for &p in &self.tracked_pins {
                    if (*p).node != node {
                        continue;
                    }
                    if (*p).y == 0 {
                        (*p).node = (*node).prev;
                        (*p).y = (*(*node).prev).data.size.rows - 1;
                    } else {
                        (*p).y -= 1;
                    }
                }
            }

            let last_row = (*node).data.get_row((*node).data.size.rows as usize - 1);
            (*node)
                .data
                .clear_cells(last_row, 0, (*node).data.size.cols as usize);
        }
        self.assert_integrity();
    }

    // ---- split / compact ----

    /// Split `p.node` at `p`: rows at and after `p.y` move to a new page. Port of
    /// `split`. Returns Err(OutOfSpace) if the page is a single row.
    pub fn split(&mut self, p: Pin) -> Result<(), SplitError> {
        debug_assert!(self.pin_is_valid(p));
        let original_node = p.node;
        unsafe {
            if (*original_node).data.size.rows <= 1 {
                return Err(SplitError::OutOfSpace);
            }
            if p.y == 0 {
                return Ok(());
            }

            let cap = (*original_node).data.capacity;
            let target = self.create_page(cap);
            let y_start = p.y;
            let y_end = (*original_node).data.size.rows;
            (*target).data.size.rows = y_end - y_start;
            (*target).data.size.cols = (*original_node).data.size.cols;
            if (*target)
                .data
                .clone_from(&(*original_node).data, y_start as usize, y_end as usize)
                .is_err()
            {
                self.destroy_node(target);
                return Err(SplitError::OutOfSpace);
            }

            for &tracked in &self.tracked_pins {
                if (*tracked).node != original_node || (*tracked).y < p.y {
                    continue;
                }
                (*tracked).node = target;
                (*tracked).y -= p.y;
            }

            for y in y_start..y_end {
                let row = (*original_node).data.get_row(y as usize);
                (*original_node)
                    .data
                    .clear_cells(row, 0, (*original_node).data.size.cols as usize);
            }
            (*original_node).data.size.rows -= y_end - y_start;
            self.pages.insert_after(original_node, target);
        }
        self.assert_integrity();
        Ok(())
    }

    /// Compact a page to its minimum required capacity. Returns the new node, or None
    #[allow(dead_code)]
    /// if no compaction occurred. Port of `compact`. Internal (operates on a raw node
    /// handle vended by this list; `Screen` and tests are in-crate).
    pub(crate) fn compact(&mut self, node: *mut Node) -> Option<*mut Node> {
        unsafe {
            debug_assert!((*node).data.size.rows > 0);
            if (*node).data.byte_len() <= super::std_size() {
                return None;
            }
            let req_cap = (*node)
                .data
                .exact_row_capacity(0, (*node).data.size.rows as usize);
            let new_size = crate::page::layout_total_size(req_cap);
            let old_size = (*node).data.byte_len();
            if new_size >= old_size {
                return None;
            }

            let new_node = self.create_page(req_cap);
            (*new_node).data.size = (*node).data.size;
            (*new_node).data.dirty = (*node).data.dirty;
            if (*new_node)
                .data
                .clone_from(&(*node).data, 0, (*node).data.size.rows as usize)
                .is_err()
            {
                self.destroy_node(new_node);
                return None;
            }

            for &p in &self.tracked_pins {
                if (*p).node == node {
                    (*p).node = new_node;
                }
            }

            self.pages.insert_before(node, new_node);
            self.pages.remove(node);
            self.destroy_node(node);
            (*new_node).data.assert_integrity();
            self.assert_integrity();
            Some(new_node)
        }
    }

    // ---- clone ----

    /// Clone the page list from `top` to `bot` inclusive. Port of `clone`. The
    /// returned clone's viewport is active. If `remap` is given, tracked pins in the
    /// range are duplicated into the clone and their old→new mapping recorded.
    pub fn clone(
        &self,
        top: Point,
        bot: Option<Point>,
        mut remap: Option<&mut Vec<(*mut Pin, *mut Pin)>>,
    ) -> PageList {
        let mut pool = MemoryPool::init(super::PAGE_PREHEAT);
        let viewport_pin = pool.create_pin(Pin::default());
        let mut tracked_pins: Vec<*mut Pin> = vec![viewport_pin];

        let mut page_list = NodeList::empty();
        let mut page_serial: u64 = 0;
        let mut total_rows = 0usize;
        let mut page_size = 0usize;

        let mut it = self.page_iterator(Direction::RightDown, top, bot);
        while let Some(chunk) = unsafe { it.next() } {
            unsafe {
                let src_page = &(*chunk.node).data;
                let mut page = crate::page::Page::init(src_page.capacity);
                let byte_len = page.byte_len();
                page.size.rows = chunk.end - chunk.start;
                page.size.cols = src_page.size.cols;
                page.clone_from(src_page, chunk.start as usize, chunk.end as usize)
                    .expect("clone_from in clone");
                page.dirty = src_page.dirty;
                page.assert_integrity();

                let node = pool.create_node(page, page_serial);
                page_serial += 1;
                page_list.append(node);
                total_rows += (*node).data.size.rows as usize;
                page_size += byte_len;

                if let Some(remap) = remap.as_deref_mut() {
                    for &p in &self.tracked_pins {
                        if (*p).node != chunk.node || (*p).y < chunk.start || (*p).y >= chunk.end {
                            continue;
                        }
                        let mut new_p = *p;
                        new_p.node = node;
                        new_p.y -= chunk.start;
                        let new_ptr = pool.create_pin(new_p);
                        remap.push((p, new_ptr));
                        tracked_pins.push(new_ptr);
                    }
                }
            }
        }

        unsafe { *viewport_pin = Pin::at(page_list.first) };

        let mut result = PageList {
            pool,
            pages: page_list,
            page_serial,
            page_serial_min: 0,
            page_size,
            explicit_max_size: self.explicit_max_size,
            min_max_size: self.min_max_size,
            total_rows,
            tracked_pins,
            viewport: Viewport::Active,
            viewport_pin,
            viewport_pin_row_offset: None,
            cols: self.cols,
            rows: self.rows,
        };

        if total_rows < self.rows as usize {
            let len = self.rows as usize - total_rows;
            for _ in 0..len {
                result.grow();
                unsafe {
                    let last = result.pages.last;
                    let row = (*last).data.get_row((*last).data.size.rows as usize - 1);
                    (*last).data.clear_cells(row, 0, result.cols as usize);
                }
            }
            result.total_rows = result.rows as usize;
        }

        result.assert_integrity();
        result
    }
}

/// Rotate the row slice `[start, end)` left by one (Zig `fastmem.rotateOnce`):
/// `[a b c d] -> [b c d a]`. Moves each following row up by one, wrapping the
/// first to the end. Operates on the raw Row structs (cell offsets, not contents).
///
/// # Safety
/// `page` valid; `start <= end <= size.rows`.
unsafe fn rotate_once_rows(page: &mut crate::page::Page, start: usize, end: usize) {
    if end <= start + 1 {
        return;
    }
    unsafe {
        let first = *page.get_row(start);
        for i in start..end - 1 {
            let src = *page.get_row(i + 1);
            *page.get_row(i) = src;
        }
        *page.get_row(end - 1) = first;
    }
}
