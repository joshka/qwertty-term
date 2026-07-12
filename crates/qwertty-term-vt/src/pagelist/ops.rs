//! Core mutating operations: grow, scroll, erase, clone, page create/destroy,
//! capacity increase (port of the corresponding PageList methods).

use super::iter::Direction;
use super::pin::{Overflow, Pin};
use super::{Node, PageList, Viewport, initial_capacity, std_size};
use crate::highlight::Untracked;
use crate::page::size::CellCountInt;
use crate::page::{Capacity, Page, SemanticContent};
use crate::point::{Point, Tag};

/// Which capacity dimension to grow. Port of `IncreaseCapacity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncreaseCapacity {
    Styles,
    GraphemeBytes,
    HyperlinkBytes,
    StringBytes,
}

/// Scrollbar state. Port of `PageList.Scrollbar`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scrollbar {
    pub total: usize,
    pub offset: usize,
    pub len: usize,
}

/// A scroll behavior. Port of `PageList.Scroll`.
#[derive(Debug, Clone, Copy)]
pub enum Scroll {
    Active,
    Top,
    Row(usize),
    DeltaRow(isize),
    DeltaPrompt(isize),
    Pin(Pin),
}

/// A cell reference. Port of `PageList.Cell`.
#[derive(Clone, Copy)]
pub struct Cell {
    pub(crate) node: *mut Node,
    pub(crate) row: *mut crate::page::Row,
    pub(crate) cell: *mut crate::page::Cell,
    pub row_idx: CellCountInt,
    pub col_idx: CellCountInt,
}

impl Cell {
    /// Dirty state. Port of `Cell.isDirty`.
    pub fn is_dirty(&self) -> bool {
        unsafe { (*self.node).data.dirty || (*self.row).dirty() }
    }

    /// The underlying page cell (copy).
    pub fn page_cell(&self) -> crate::page::Cell {
        unsafe { *self.cell }
    }

    /// The absolute screen point of this cell. Port of `Cell.screenPoint`.
    pub fn screen_point(&self) -> Point {
        let mut y = self.row_idx as u32;
        let mut node = self.node;
        unsafe {
            while !(*node).prev.is_null() {
                y += (*(*node).prev).data.size.rows as u32;
                node = (*node).prev;
            }
        }
        Point::screen(self.col_idx, y)
    }
}

impl PageList {
    // ---- page create / destroy ----

    /// Create a new page node (not added to the list; updates byte accounting).
    /// Port of `createPage`.
    pub(crate) fn create_page(&mut self, cap: Capacity) -> *mut Node {
        let page = Page::init(cap);
        let byte_len = page.byte_len();
        let node = self.pool.create_node(page, self.page_serial);
        self.page_serial += 1;
        self.page_size += byte_len;
        // The freshly created page has 0 rows.
        unsafe { (*node).data.size.rows = 0 };
        node
    }

    /// Destroy a node (removed from list already) and update byte accounting.
    /// Port of `destroyNode`.
    ///
    /// # Safety
    /// `node` must be removed from the list and unreferenced.
    pub(crate) unsafe fn destroy_node(&mut self, node: *mut Node) {
        let byte_len = unsafe { (*node).data.byte_len() };
        self.page_size -= byte_len;
        unsafe { self.pool.destroy_node(node) };
    }

    // ---- grow ----

    /// Grow the active area by one row, pruning scrollback if the byte budget is
    /// exceeded. Returns `true` if a new/reused page was added (Zig returns the node;
    /// callers only need whether one was created). Port of `grow`.
    pub fn grow(&mut self) -> bool {
        self.grow_node().is_some()
    }

    /// Grow, returning the new/reused node pointer (internal). Port of `grow`.
    pub(crate) fn grow_node(&mut self) -> Option<*mut Node> {
        let last = self.pages.last;
        // Fast path: capacity in the last page.
        unsafe {
            if (*last).data.capacity.rows > (*last).data.size.rows {
                (*last).data.size.rows += 1;
                (*last).data.assert_integrity();
                self.total_rows += 1;
                self.assert_integrity();
                return None;
            }
        }

        let cap = initial_capacity(self.cols);

        // Prune path.
        if !self.pages.first.is_null()
            && self.pages.first != self.pages.last
            && self.page_size + std_size() > self.max_size()
            && let Some(node) = self.grow_prune(cap, last)
        {
            self.assert_integrity();
            return Some(node);
        }

        // Alloc path.
        let next = self.create_page(cap);
        unsafe {
            self.pages.append(next);
            (*next).data.size.rows = 1;
            (*next).data.assert_integrity();
        }
        self.total_rows += 1;
        self.assert_integrity();
        Some(next)
    }

    /// The prune branch of grow; returns None (fell through) if pruning is impossible.
    fn grow_prune(&mut self, _cap: Capacity, last: *mut Node) -> Option<*mut Node> {
        unsafe {
            let first = self.pages.pop_first();
            debug_assert!(first != last);
            let first_rows = (*first).data.size.rows as usize;
            self.total_rows -= first_rows;

            if self.total_rows + 1 < self.rows as usize {
                // Can't prune; undo.
                self.pages.prepend(first);
                self.total_rows += first_rows;
                return None;
            }

            // Update viewport cache.
            if self.viewport == Viewport::Pin
                && let Some(v) = self.viewport_offset_cache().as_mut()
            {
                if *v < first_rows {
                    self.viewport = Viewport::Top;
                } else {
                    *v -= first_rows;
                }
            }

            // Move tracked pins on the pruned page to the new first page top-left.
            let new_first = self.pages.first;
            for &p in &self.tracked_pins {
                if (*p).node != first {
                    continue;
                }
                (*p).node = new_first;
                (*p).y = 0;
                (*p).x = 0;
                (*p).garbage = true;
            }
            (*self.viewport_pin).garbage = false;

            // Non-standard pages can't be reused.
            if (*first).data.byte_len() > std_size() {
                self.destroy_node(first);
                return None;
            }

            // Reuse the page buffer as the new last page.
            (*first).data.reinit();
            (*first).data.size.rows = 1;
            self.pages.insert_after(last, first);
            self.total_rows += 1;

            self.page_serial_min = (*first).serial + 1;
            (*first).serial = self.page_serial;
            self.page_serial += 1;

            (*first).data.assert_integrity();
            Some(first)
        }
    }

    /// Grow the number of active rows by `n` (test helper). Port of `growRows`.
    pub fn grow_rows(&mut self, n: usize) {
        for _ in 0..n {
            self.grow();
        }
    }

    // ---- increaseCapacity ----

    /// Increase the capacity of `node` in the given dimension (or re-clone with the
    /// same capacity when `adjustment` is None, for rehash). The old node is
    /// destroyed; the new node is returned. Port of `increaseCapacity`.
    ///
    /// # Safety
    /// `node` must be a live node in this list.
    pub(crate) unsafe fn increase_capacity(
        &mut self,
        node: *mut Node,
        adjustment: Option<IncreaseCapacity>,
    ) -> Result<*mut Node, ()> {
        unsafe {
            let mut cap = (*node).data.capacity;
            if let Some(adj) = adjustment {
                // A dimension can be zero for pages with exact capacities
                // (see `compact` / `Page::exact_row_capacity`): a compacted
                // plain-text page has zero styles/grapheme/string/hyperlink
                // capacity. Growth from zero jumps to the standard default
                // for the dimension rather than doubling — `0 * 2 == 0`
                // "succeeds" without growing, breaking the guarantee that we
                // increase by at least one unit and turning caller retry
                // loops into infinite loops (single-retry callers silently
                // drop data). The default is what every standard page starts
                // with, so retrying callers get enough room. Port of
                // upstream `8307349ec`.
                let default = Capacity::new(0, 0);
                let ok = match adj {
                    IncreaseCapacity::Styles => grow_u16(&mut cap.styles, default.styles),
                    IncreaseCapacity::HyperlinkBytes => {
                        grow_u16(&mut cap.hyperlink_bytes, default.hyperlink_bytes)
                    }
                    IncreaseCapacity::GraphemeBytes => {
                        grow_u32(&mut cap.grapheme_bytes, default.grapheme_bytes)
                    }
                    IncreaseCapacity::StringBytes => {
                        grow_u32(&mut cap.string_bytes, default.string_bytes)
                    }
                };
                if !ok {
                    return Err(());
                }
                // If the resulting layout overflows max page size, OutOfSpace.
                if crate::page::layout_total_size(cap) > crate::page::size::MAX_PAGE_SIZE {
                    return Err(());
                }
            }

            let new_node = self.create_page(cap);
            let old_rows = (*node).data.size.rows;
            let old_cols = (*node).data.size.cols;
            (*new_node).data.size.rows = old_rows;
            (*new_node).data.size.cols = old_cols;
            (*new_node)
                .data
                .clone_from(&(*node).data, 0, old_rows as usize)
                .expect("increase_capacity clone should not fail");
            (*new_node).data.dirty = (*node).data.dirty;

            // Fix up tracked pins.
            for &p in &self.tracked_pins {
                if (*p).node == node {
                    (*p).node = new_node;
                }
            }

            self.pages.insert_before(node, new_node);
            self.pages.remove(node);
            self.destroy_node(node);

            (*new_node).data.assert_integrity();
            Ok(new_node)
        }
    }

    // ---- scroll ----

    /// Scroll the viewport. Port of `scroll`.
    pub fn scroll(&mut self, behavior: Scroll) {
        if self.explicit_max_size == 0 {
            self.viewport = Viewport::Active;
            self.assert_integrity();
            return;
        }
        match behavior {
            Scroll::Active => self.viewport = Viewport::Active,
            Scroll::Top => self.viewport = Viewport::Top,
            Scroll::Pin(p) => {
                if self.pin_is_active(p) {
                    self.viewport = Viewport::Active;
                } else if self.pin_is_top(p) {
                    self.viewport = Viewport::Top;
                } else {
                    unsafe { *self.viewport_pin = p };
                    self.viewport = Viewport::Pin;
                    self.invalidate_viewport_offset();
                }
            }
            Scroll::Row(n) => self.scroll_row(n),
            Scroll::DeltaPrompt(n) => self.scroll_prompt(n),
            Scroll::DeltaRow(n) => self.scroll_delta_row(n),
        }
        self.assert_integrity();
    }

    fn scroll_row(&mut self, n: usize) {
        if n == 0 {
            self.viewport = Viewport::Top;
            return;
        }
        if n >= self.total_rows - self.rows as usize {
            self.viewport = Viewport::Active;
            return;
        }
        if self.viewport == Viewport::Pin
            && let Some(v) = self.viewport_offset_cache_copy()
        {
            let delta = n as isize - v as isize;
            self.scroll_delta_row(delta);
            return;
        }
        self.viewport_offset_cache_set(Some(n));
        self.viewport = Viewport::Pin;

        let midpoint = self.total_rows / 2;
        if n < midpoint {
            let mut node = self.pages.first;
            let mut rem = n;
            while !node.is_null() {
                let nrows = unsafe { (*node).data.size.rows } as usize;
                if rem < nrows {
                    unsafe { *self.viewport_pin = Pin::with(node, rem as CellCountInt, 0) };
                    return;
                }
                rem -= nrows;
                node = unsafe { (*node).next };
            }
        } else {
            let mut node = self.pages.last;
            let mut rem = self.total_rows - n;
            while !node.is_null() {
                let nrows = unsafe { (*node).data.size.rows } as usize;
                if rem <= nrows {
                    unsafe {
                        *self.viewport_pin = Pin::with(node, (nrows - rem) as CellCountInt, 0)
                    };
                    return;
                }
                rem -= nrows;
                node = unsafe { (*node).prev };
            }
        }
        self.viewport = Viewport::Active;
    }

    fn scroll_delta_row(&mut self, n: isize) {
        // Fast paths keyed on current viewport.
        match self.viewport {
            Viewport::Top => {
                if n <= 0 {
                    return;
                }
            }
            Viewport::Active => {
                if n >= 0 {
                    return;
                }
            }
            Viewport::Pin => {
                use std::cmp::Ordering;
                match n.cmp(&0) {
                    Ordering::Equal => return,
                    Ordering::Less => {
                        let up = unsafe { (*self.viewport_pin).up_overflow((-n) as usize) };
                        match up {
                            Overflow::Offset(new_pin) => {
                                unsafe { *self.viewport_pin = new_pin };
                                if let Some(v) = self.viewport_offset_cache().as_mut() {
                                    *v -= (-n) as usize;
                                }
                                return;
                            }
                            Overflow::Overflow { .. } => {
                                self.viewport = Viewport::Top;
                                return;
                            }
                        }
                    }
                    Ordering::Greater => {
                        let down = unsafe { (*self.viewport_pin).down_overflow(n as usize) };
                        match down {
                            Overflow::Offset(new_pin) => {
                                if self.pin_is_active(new_pin) {
                                    self.viewport = Viewport::Active;
                                } else {
                                    unsafe { *self.viewport_pin = new_pin };
                                    if let Some(v) = self.viewport_offset_cache().as_mut() {
                                        *v += n as usize;
                                    }
                                }
                                return;
                            }
                            Overflow::Overflow { .. } => {
                                self.viewport = Viewport::Active;
                                return;
                            }
                        }
                    }
                }
            }
        }

        // Slow path.
        let top = self.get_top_left(Tag::Viewport);
        let p = if n < 0 {
            match unsafe { top.up_overflow((-n) as usize) } {
                Overflow::Offset(v) => v,
                Overflow::Overflow { end, .. } => end,
            }
        } else {
            match unsafe { top.down_overflow(n as usize) } {
                Overflow::Offset(v) => v,
                Overflow::Overflow { end, .. } => end,
            }
        };

        if self.pin_is_active(p) {
            self.viewport = Viewport::Active;
            return;
        }
        if self.pin_is_top(p) {
            self.viewport = Viewport::Top;
            return;
        }
        unsafe { *self.viewport_pin = p };
        self.viewport = Viewport::Pin;
        self.invalidate_viewport_offset();
    }

    /// Jump the viewport by `delta` prompts. Port of `scrollPrompt`.
    fn scroll_prompt(&mut self, delta: isize) {
        if delta == 0 {
            return;
        }
        let mut delta_rem = delta.unsigned_abs();

        let start_pin = {
            let tl = self.get_top_left(Tag::Viewport);
            if delta <= 0 {
                match unsafe { tl.up(1) } {
                    Some(p) => p,
                    None => return,
                }
            } else {
                let mut adjusted = match unsafe { tl.down(1) } {
                    Some(p) => p,
                    None => return,
                };
                let tl_prompt = unsafe { (*tl.row_and_cell().0).semantic_prompt() };
                if tl_prompt != crate::page::SemanticPrompt::None {
                    loop {
                        let sp = unsafe { (*adjusted.row_and_cell().0).semantic_prompt() };
                        if sp != crate::page::SemanticPrompt::PromptContinuation {
                            break;
                        }
                        adjusted = match unsafe { adjusted.down(1) } {
                            Some(p) => p,
                            None => break,
                        };
                    }
                }
                adjusted
            }
        };

        let dir = if delta > 0 {
            Direction::RightDown
        } else {
            Direction::LeftUp
        };
        let mut prompt_pin: Option<Pin> = None;
        let mut it = PromptIterator::new(start_pin, dir);
        while let Some(next) = unsafe { it.next() } {
            prompt_pin = Some(next);
            delta_rem -= 1;
            if delta_rem == 0 {
                break;
            }
        }

        if let Some(p) = prompt_pin {
            if self.pin_is_active(p) {
                self.viewport = Viewport::Active;
            } else {
                unsafe { *self.viewport_pin = p };
                self.viewport = Viewport::Pin;
                self.invalidate_viewport_offset();
            }
        }
    }

    /// Clear the screen by scrolling written content into scrollback. Port of
    /// `scrollClear`.
    pub fn scroll_clear(&mut self) {
        let non_empty = {
            let mut page = self.pages.last;
            let mut n = 0usize;

            'outer: loop {
                unsafe {
                    let size_rows = (*page).data.size.rows as usize;
                    for i in 0..size_rows {
                        let rev_i = size_rows - i - 1;
                        let row = (*page).data.get_row(rev_i);
                        let cells = (*page).data.get_cells(row);
                        let cells_ref = std::slice::from_raw_parts(
                            cells.cast::<crate::page::Cell>(),
                            self.cols as usize,
                        );
                        if cells_ref.iter().any(|c| !c.is_empty()) {
                            break 'outer self.rows as usize - n;
                        }
                        n += 1;
                        if n > self.rows as usize {
                            break 'outer 0;
                        }
                    }
                    page = (*page).prev;
                    if page.is_null() {
                        break 'outer 0;
                    }
                }
            }
        };
        for _ in 0..non_empty {
            self.grow();
        }
        self.assert_integrity();
    }

    // ---- getCell / dirty helpers ----

    /// The cell at `pt`, or None if out of bounds. Port of `getCell`.
    pub fn get_cell(&self, pt: Point) -> Option<Cell> {
        let p = self.pin(pt)?;
        let (row, cell) = unsafe { (*p.node).data.get_row_and_cell(p.x as usize, p.y as usize) };
        Some(Cell {
            node: p.node,
            row,
            cell,
            row_idx: p.y,
            col_idx: p.x,
        })
    }

    /// True if the point is dirty (test helper). Port of `isDirty`.
    pub fn is_dirty(&self, pt: Point) -> bool {
        self.get_cell(pt).unwrap().is_dirty()
    }

    /// The scrollbar state. Port of `scrollbar`.
    pub fn scrollbar(&mut self) -> Scrollbar {
        if self.explicit_max_size == 0 {
            return Scrollbar {
                total: self.rows as usize,
                offset: 0,
                len: self.rows as usize,
            };
        }
        Scrollbar {
            total: self.total_rows,
            offset: self.viewport_row_offset(),
            len: self.rows as usize,
        }
    }

    /// Mark the point's row dirty (test helper). Port of `markDirty`.
    pub fn mark_dirty(&mut self, pt: Point) {
        let p = self.pin(pt).unwrap();
        unsafe { (*p.row_and_cell().0).set_dirty(true) };
    }

    /// Clear all dirty bits (test helper). Port of `clearDirty`.
    pub fn clear_dirty(&mut self) {
        let mut node = self.pages.first;
        while !node.is_null() {
            unsafe {
                (*node).data.dirty = false;
                let rows = (*node).data.size.rows as usize;
                for y in 0..rows {
                    (*(*node).data.get_row(y)).set_dirty(false);
                }
                node = (*node).next;
            }
        }
    }

    /// Clear every page's page-level dirty flag, leaving per-row dirty bits
    /// untouched. Used by the renderer snapshot's incremental capture path
    /// after it has consumed the page dirty state (upstream `render.zig`'s
    /// `update` clears each observed page's dirty flag as it goes; this is the
    /// same net effect over the tiny page count).
    pub(crate) fn clear_page_dirty(&mut self) {
        let mut node = self.pages.first;
        while !node.is_null() {
            unsafe {
                (*node).data.dirty = false;
                node = (*node).next;
            }
        }
    }

    // ---- semantic-content highlighting ----

    /// Build an untracked highlight for the semantic content (`.prompt`/`.input`/`.output`)
    /// within the command zone containing the prompt row at `at`. Returns `None` when there is
    /// no content of the requested kind. Port of `highlightSemanticContent`.
    ///
    /// `at` must be a prompt row (asserted). The zone runs from `at` to the last cell of the row
    /// just before the next prompt, or to the end of the screen if there is no further prompt.
    ///
    /// Consumers (not in this chunk): `Screen.selectOutput` and the search/renderer pipeline —
    /// see `docs/analysis/highlight.md`.
    pub fn highlight_semantic_content(
        &self,
        at: Pin,
        content: SemanticContent,
    ) -> Option<Untracked> {
        // Performance note (from Zig): this could be a single forward pass. Semantic-content
        // ops aren't the fast path, so clarity wins.

        // Bound the zone: from `at` to just before the next prompt, else end of screen.
        //
        // NOTE: the ported `PromptIterator` (see below in this file) is the simplified variant
        // used by `scrollPrompt` — it yields only rows whose semantic_prompt == Prompt (skipping
        // continuations) and takes no limit. `highlightSemanticContent` only needs `next()`
        // twice with a null limit (self, then the next distinct prompt), so it is behaviorally
        // equivalent to Zig's `nextRightDown(.right_down, null)` here.
        let end: Pin = {
            let mut it = PromptIterator::new(at, Direction::RightDown);
            // Safety assertion: our starting point is a prompt row, so the first returned
            // prompt is ourselves.
            let first = unsafe { it.next() };
            debug_assert_eq!(first.map(|p| p.y), Some(at.y));

            match unsafe { it.next() } {
                Some(next) => {
                    // End is the last cell of the row just before the next prompt.
                    match unsafe { next.up(1) } {
                        Some(mut prev) => {
                            prev.x = unsafe { (*prev.node).data.size.cols } - 1;
                            prev
                        }
                        // No row above the next prompt: fall through to end-of-screen.
                        None => self.get_bottom_right(Tag::Screen)?,
                    }
                }
                // No further prompt: the zone ends at the end of the screen.
                None => self.get_bottom_right(Tag::Screen)?,
            }
        };

        match content {
            // For the prompt, select all the way up to command output, including input lines.
            SemanticContent::Prompt => {
                let mut result = Untracked {
                    start: at.left(at.x as usize),
                    end: at,
                };
                let mut it = unsafe { at.cell_iterator(Direction::RightDown, Some(end)) };
                while let Some(p) = unsafe { it.next() } {
                    let sc = unsafe { (*p.row_and_cell().1).semantic_content() };
                    match sc {
                        SemanticContent::Prompt | SemanticContent::Input => result.end = p,
                        SemanticContent::Output => break,
                    }
                }
                Some(result)
            }

            // For input, include the start of input to the end of input; prompts in the middle
            // (continuation prompts) are skipped.
            SemanticContent::Input => {
                let mut it = unsafe { at.cell_iterator(Direction::RightDown, Some(end)) };

                // Find the start.
                let mut result = 'find_start: {
                    while let Some(p) = unsafe { it.next() } {
                        let sc = unsafe { (*p.row_and_cell().1).semantic_content() };
                        match sc {
                            SemanticContent::Prompt => {}
                            SemanticContent::Input => {
                                break 'find_start Untracked { start: p, end: p };
                            }
                            SemanticContent::Output => return None,
                        }
                    }
                    // No input found.
                    return None;
                };

                // Find the end.
                while let Some(p) = unsafe { it.next() } {
                    let sc = unsafe { (*p.row_and_cell().1).semantic_content() };
                    match sc {
                        // Prompts can be nested in our input for continuation.
                        SemanticContent::Prompt => {}
                        // Output means we're done.
                        SemanticContent::Output => break,
                        SemanticContent::Input => result.end = p,
                    }
                }
                Some(result)
            }

            SemanticContent::Output => {
                let mut it = unsafe { at.cell_iterator(Direction::RightDown, Some(end)) };

                // Find the start.
                let mut result = 'find_start: {
                    while let Some(p) = unsafe { it.next() } {
                        let cell = unsafe { *p.row_and_cell().1 };
                        match cell.semantic_content() {
                            SemanticContent::Prompt | SemanticContent::Input => {}
                            SemanticContent::Output => {
                                // Skip empty cells: they default to .output but aren't real output.
                                if !cell.has_text() {
                                    continue;
                                }
                                break 'find_start Untracked { start: p, end: p };
                            }
                        }
                    }
                    // No output found.
                    return None;
                };

                // Find the end.
                while let Some(p) = unsafe { it.next() } {
                    let cell = unsafe { *p.row_and_cell().1 };
                    match cell.semantic_content() {
                        SemanticContent::Prompt | SemanticContent::Input => break,
                        // Only extend to cells with actual text.
                        SemanticContent::Output => {
                            if cell.has_text() {
                                result.end = p;
                            }
                        }
                    }
                }
                Some(result)
            }
        }
    }
}

/// Iterates prompt rows. Port of `PromptIterator`.
///
/// This is the simplified variant used by `scrollPrompt`/`highlightSemanticContent`/
/// `selectOutput`: it yields only rows whose `semantic_prompt == Prompt` (skipping
/// continuations) and takes no limit.
pub(crate) struct PromptIterator {
    current: Option<Pin>,
    direction: Direction,
}

impl PromptIterator {
    pub(crate) fn new(start: Pin, direction: Direction) -> Self {
        PromptIterator {
            current: Some(start),
            direction,
        }
    }

    /// # Safety
    /// Node chain live.
    pub(crate) unsafe fn next(&mut self) -> Option<Pin> {
        unsafe {
            loop {
                let mut p = self.current?;
                let sp = (*p.row_and_cell().0).semantic_prompt();
                let is_prompt = matches!(
                    sp,
                    crate::page::SemanticPrompt::Prompt
                        | crate::page::SemanticPrompt::PromptContinuation
                );
                // Advance current for next call.
                let next = match self.direction {
                    Direction::RightDown => p.down(1),
                    Direction::LeftUp => p.up(1),
                };
                self.current = next;
                if is_prompt && sp == crate::page::SemanticPrompt::Prompt {
                    p.x = 0;
                    return Some(p);
                }
                self.current?;
            }
        }
    }
}

/// Grow a u16 capacity dimension: from zero, jump to `default` (doubling zero
/// stays zero — see `8307349ec` note at the call site); otherwise double,
/// clamping at max. Returns false only if already maxed. Port of the growth
/// logic in `increaseCapacity`.
fn grow_u16(field: &mut u16, default: u16) -> bool {
    let old = *field;
    if old == 0 {
        *field = default;
        return true;
    }
    let new = match old.checked_mul(2) {
        Some(v) => v,
        None => {
            if old < u16::MAX {
                u16::MAX
            } else {
                return false;
            }
        }
    };
    *field = new;
    true
}

/// Grow a u32 capacity dimension. Zero → `default`, else double-and-clamp.
/// See [`grow_u16`].
fn grow_u32(field: &mut u32, default: u32) -> bool {
    let old = *field;
    if old == 0 {
        *field = default;
        return true;
    }
    let new = match old.checked_mul(2) {
        Some(v) => v,
        None => {
            if old < u32::MAX {
                u32::MAX
            } else {
                return false;
            }
        }
    };
    *field = new;
    true
}

#[cfg(test)]
mod grow_tests {
    use super::{grow_u16, grow_u32};
    use crate::page::Capacity;

    // Regression for upstream 8307349ec: growth from a zero dimension must
    // jump to the standard default, not double (0*2 == 0 "succeeds" without
    // growing → caller retry loops spin forever / drop data). The defaults
    // are exactly `Capacity::new(0, 0)`'s field values.
    #[test]
    fn grow_from_zero_jumps_to_default() {
        let d = Capacity::new(0, 0);

        let mut styles: u16 = 0;
        assert!(grow_u16(&mut styles, d.styles));
        assert_eq!(styles, d.styles);
        assert!(styles > 0);

        let mut hyperlink: u16 = 0;
        assert!(grow_u16(&mut hyperlink, d.hyperlink_bytes));
        assert_eq!(hyperlink, d.hyperlink_bytes);
        assert!(hyperlink > 0);

        let mut grapheme: u32 = 0;
        assert!(grow_u32(&mut grapheme, d.grapheme_bytes));
        assert_eq!(grapheme, d.grapheme_bytes);
        assert!(grapheme > 0);

        let mut string: u32 = 0;
        assert!(grow_u32(&mut string, d.string_bytes));
        assert_eq!(string, d.string_bytes);
        assert!(string > 0);
    }

    // Non-zero dimensions still double, and clamp at max (returning false only
    // when already maxed) — the growth contract preserved from before the fix.
    #[test]
    fn grow_from_nonzero_doubles_and_clamps() {
        let mut v: u16 = 16;
        assert!(grow_u16(&mut v, 999));
        assert_eq!(v, 32);

        // Clamp: doubling would overflow u16 but we're below MAX → jump to MAX.
        let mut near: u16 = 40000;
        assert!(grow_u16(&mut near, 999));
        assert_eq!(near, u16::MAX);

        // Already at MAX → no growth possible.
        let mut maxed: u16 = u16::MAX;
        assert!(!grow_u16(&mut maxed, 999));
        assert_eq!(maxed, u16::MAX);

        let mut w: u32 = 100;
        assert!(grow_u32(&mut w, 999));
        assert_eq!(w, 200);
        let mut maxed32: u32 = u32::MAX;
        assert!(!grow_u32(&mut maxed32, 999));
    }
}
