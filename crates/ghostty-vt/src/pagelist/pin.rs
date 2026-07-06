//! Pins: fixed references into the page list (port of `PageList.Pin` +
//! the tracked-pin machinery).

use super::{Coordinate, Node, PageCell, PageList, Point, Row, Tag, Viewport};
use crate::page::Page;
use crate::page::size::CellCountInt;

/// A fixed x/y reference into a specific page node. Port of `PageList.Pin`.
///
/// A pin stays valid across scrolling because it follows the node pointer, but is
/// invalidated by mutations that move rows or free the page — unless it is *tracked*,
/// in which case [`PageList`] updates it on every mutating op.
#[derive(Debug, Clone, Copy)]
pub struct Pin {
    /// The node this pin references. Raw pointer into the list.
    pub(crate) node: *mut Node,
    pub(crate) y: CellCountInt,
    pub(crate) x: CellCountInt,
    /// Set when a tracked pin's page was pruned with no sensible new home.
    pub garbage: bool,
}

impl Default for Pin {
    fn default() -> Self {
        Pin {
            node: std::ptr::null_mut(),
            y: 0,
            x: 0,
            garbage: false,
        }
    }
}

/// Which subset of a row's cells [`Pin::cells`] returns. Port of `CellSubset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellSubset {
    All,
    Left,
    Right,
}

/// Result of an overflow-aware vertical move.
pub(crate) enum Overflow {
    Offset(Pin),
    #[allow(dead_code)]
    Overflow {
        end: Pin,
        remaining: usize,
    },
}

impl Pin {
    pub(crate) fn at(node: *mut Node) -> Pin {
        Pin {
            node,
            y: 0,
            x: 0,
            garbage: false,
        }
    }

    pub(crate) fn with(node: *mut Node, y: CellCountInt, x: CellCountInt) -> Pin {
        Pin {
            node,
            y,
            x,
            garbage: false,
        }
    }

    /// The x/y coordinates.
    pub fn x(self) -> CellCountInt {
        self.x
    }
    pub fn y(self) -> CellCountInt {
        self.y
    }

    /// Row and cell pointers at this pin. Port of `rowAndCell`.
    ///
    /// # Safety
    /// The pin must be valid (node live, x/y in bounds).
    pub(crate) unsafe fn row_and_cell(self) -> (*mut Row, *mut PageCell) {
        unsafe {
            (*self.node)
                .data
                .get_row_and_cell(self.x as usize, self.y as usize)
        }
    }

    /// The page this pin points into.
    ///
    /// # Safety
    /// The pin's node must be live.
    #[allow(dead_code)]
    pub(crate) unsafe fn page(self) -> *mut Page {
        unsafe { &mut (*self.node).data }
    }

    /// The cells of the pin's row, per `subset`. Port of `cells`.
    ///
    /// # Safety
    /// Pin valid; returned slice valid until the page mutates.
    pub(crate) unsafe fn cells(self, subset: CellSubset) -> *mut [PageCell] {
        unsafe {
            let (row, _) = self.row_and_cell();
            let all = (*self.node).data.get_cells(row);
            let len = all.len();
            let base = all.cast::<PageCell>();
            match subset {
                CellSubset::All => all,
                CellSubset::Left => std::ptr::slice_from_raw_parts_mut(base, self.x as usize + 1),
                CellSubset::Right => std::ptr::slice_from_raw_parts_mut(
                    base.add(self.x as usize),
                    len - self.x as usize,
                ),
            }
        }
    }

    /// Equality: same node, y, x. Port of `eql`.
    pub fn eql(self, other: Pin) -> bool {
        self.node == other.node && self.y == other.y && self.x == other.x
    }

    /// True if `self` is before `other` (list traversal). Port of `before`.
    ///
    /// # Safety
    /// Both pins' nodes must be live.
    pub(crate) unsafe fn before(self, other: Pin) -> bool {
        if self.node == other.node {
            if self.y < other.y {
                return true;
            }
            if self.y > other.y {
                return false;
            }
            return self.x < other.x;
        }
        let mut node = unsafe { (*self.node).next };
        while !node.is_null() {
            if node == other.node {
                return true;
            }
            node = unsafe { (*node).next };
        }
        false
    }

    /// True if `self` is between `top` and `bottom` inclusive. Port of `isBetween`.
    ///
    /// # Safety
    /// All pins' nodes must be live.
    #[allow(dead_code)]
    pub(crate) unsafe fn is_between(self, top: Pin, bottom: Pin) -> bool {
        unsafe {
            if self.node == top.node {
                if self.y < top.y {
                    return false;
                }
                if self.y > top.y {
                    return if self.node == bottom.node {
                        self.y <= bottom.y
                    } else {
                        true
                    };
                }
                debug_assert_eq!(self.y, top.y);
                if self.x < top.x {
                    return false;
                }
            }
            if self.node == bottom.node {
                if self.y > bottom.y {
                    return false;
                }
                if self.y < bottom.y {
                    return true;
                }
                debug_assert_eq!(self.y, bottom.y);
                return self.x <= bottom.x;
            }
            if top.node == bottom.node {
                return false;
            }
            let mut node = (*top.node).next;
            while !node.is_null() {
                if node == bottom.node {
                    break;
                }
                if node == self.node {
                    return true;
                }
                node = (*node).next;
            }
            false
        }
    }

    /// Move left `n` columns (asserts in-row). Port of `left`.
    #[allow(dead_code)]
    pub(crate) fn left(self, n: usize) -> Pin {
        debug_assert!(n <= self.x as usize);
        let mut r = self;
        r.x -= n as CellCountInt;
        r
    }

    /// Move right `n` columns (asserts in-row). Port of `right`.
    ///
    /// # Safety
    /// Node live for the cols bound assert.
    #[allow(dead_code)]
    pub(crate) unsafe fn right(self, n: usize) -> Pin {
        debug_assert!(self.x as usize + n < unsafe { (*self.node).data.size.cols } as usize);
        let mut r = self;
        r.x = r.x.saturating_add(n as CellCountInt);
        r
    }

    /// Move down `n` rows or None on overflow. Port of `down`.
    ///
    /// # Safety
    /// Node chain live.
    pub(crate) unsafe fn down(self, n: usize) -> Option<Pin> {
        match unsafe { self.down_overflow(n) } {
            Overflow::Offset(p) => Some(p),
            Overflow::Overflow { .. } => None,
        }
    }

    /// Move up `n` rows or None on overflow. Port of `up`.
    ///
    /// # Safety
    /// Node chain live.
    pub(crate) unsafe fn up(self, n: usize) -> Option<Pin> {
        match unsafe { self.up_overflow(n) } {
            Overflow::Offset(p) => Some(p),
            Overflow::Overflow { .. } => None,
        }
    }

    /// Move down `n` rows, returning overflow if past the end. Port of `downOverflow`.
    ///
    /// # Safety
    /// Node chain live.
    pub(crate) unsafe fn down_overflow(self, n: usize) -> Overflow {
        unsafe {
            let rows = (*self.node).data.size.rows as usize - (self.y as usize + 1);
            if n <= rows {
                return Overflow::Offset(Pin::with(
                    self.node,
                    (self.y as usize + n) as CellCountInt,
                    self.x,
                ));
            }
            let mut node = self.node;
            let mut n_left = n - rows;
            loop {
                let next = (*node).next;
                if next.is_null() {
                    return Overflow::Overflow {
                        end: Pin::with(node, (*node).data.size.rows - 1, self.x),
                        remaining: n_left,
                    };
                }
                node = next;
                let nrows = (*node).data.size.rows as usize;
                if n_left <= nrows {
                    return Overflow::Offset(Pin::with(node, (n_left - 1) as CellCountInt, self.x));
                }
                n_left -= nrows;
            }
        }
    }

    /// Move up `n` rows, returning overflow if past the start. Port of `upOverflow`.
    ///
    /// # Safety
    /// Node chain live.
    pub(crate) unsafe fn up_overflow(self, n: usize) -> Overflow {
        unsafe {
            if n <= self.y as usize {
                return Overflow::Offset(Pin::with(self.node, self.y - n as CellCountInt, self.x));
            }
            let mut node = self.node;
            let mut n_left = n - self.y as usize;
            loop {
                let prev = (*node).prev;
                if prev.is_null() {
                    return Overflow::Overflow {
                        end: Pin::with(node, 0, self.x),
                        remaining: n_left,
                    };
                }
                node = prev;
                let nrows = (*node).data.size.rows as usize;
                if n_left <= nrows {
                    return Overflow::Offset(Pin::with(
                        node,
                        (nrows - n_left) as CellCountInt,
                        self.x,
                    ));
                }
                n_left -= nrows;
            }
        }
    }
}

// ---- PageList pin/viewport machinery ----

impl PageList {
    /// Convert `pt` to an untracked pin, or None if out of bounds. Port of `pin`.
    pub fn pin(&self, pt: Point) -> Option<Pin> {
        let x = pt.coord().x;
        if x >= self.cols {
            return None;
        }
        let tl = self.get_top_left(pt.tag);
        let mut p = unsafe { tl.down(pt.coord().y as usize) }?;
        p.x = x;
        Some(p)
    }

    /// Track `p`, returning a stable pointer updated across mutations. Port of `trackPin`.
    pub fn track_pin(&mut self, p: Pin) -> *mut Pin {
        debug_assert!(self.pin_is_valid(p));
        let tracked = self.pool.create_pin(p);
        self.tracked_pins.push(tracked);
        tracked
    }

    /// Untrack and free `p`. Port of `untrackPin`.
    pub fn untrack_pin(&mut self, p: *mut Pin) {
        debug_assert!(p != self.viewport_pin);
        if let Some(idx) = self.tracked_pins.iter().position(|&x| x == p) {
            self.tracked_pins.swap_remove(idx);
            unsafe { self.pool.destroy_pin(p) };
        }
    }

    /// Number of tracked pins. Port of `countTrackedPins`.
    pub fn count_tracked_pins(&self) -> usize {
        self.tracked_pins.len()
    }

    /// The tracked-pin pointers. Port of `trackedPins`.
    pub fn tracked_pins(&self) -> &[*mut Pin] {
        &self.tracked_pins
    }

    /// Validate a pin (node present + x/y in bounds). Port of `pinIsValid`.
    pub(crate) fn pin_is_valid(&self, p: Pin) -> bool {
        let mut node = self.pages.first;
        while !node.is_null() {
            if node == p.node {
                return unsafe {
                    (p.y as usize) < (*node).data.size.rows as usize
                        && (p.x as usize) < (*node).data.size.cols as usize
                };
            }
            node = unsafe { (*node).next };
        }
        false
    }

    /// True if pin is within the active area. Port of `pinIsActive`.
    pub(crate) fn pin_is_active(&self, p: Pin) -> bool {
        let active = self.get_top_left(Tag::Active);
        if p.node == active.node {
            return p.y >= active.y;
        }
        let mut node = unsafe { (*active.node).next };
        while !node.is_null() {
            if node == p.node {
                return true;
            }
            node = unsafe { (*node).next };
        }
        false
    }

    /// True if pin is at scrollback top. Port of `pinIsTop`.
    pub(crate) fn pin_is_top(&self, p: Pin) -> bool {
        p.y == 0 && p.node == self.pages.first
    }

    /// Convert a pin to a point in the given tag frame, or None if out of range.
    /// Port of `pointFromPin`.
    pub fn point_from_pin(&self, tag: Tag, p: Pin) -> Option<Point> {
        let tl = self.get_top_left(tag);
        let mut coord = Coordinate { x: p.x, y: 0 };
        if p.node == tl.node {
            if tl.y > p.y {
                return None;
            }
            coord.y = (p.y - tl.y) as u32;
        } else {
            coord.y += (unsafe { (*tl.node).data.size.rows } - tl.y) as u32;
            let mut node = unsafe { (*tl.node).next };
            loop {
                if node.is_null() {
                    return None;
                }
                if node == p.node {
                    coord.y += p.y as u32;
                    break;
                }
                coord.y += unsafe { (*node).data.size.rows } as u32;
                node = unsafe { (*node).next };
            }
        }
        Some(Point::new(tag, coord))
    }

    /// The top-left pin for a tag. Port of `getTopLeft`.
    pub fn get_top_left(&self, tag: Tag) -> Pin {
        match tag {
            Tag::Screen | Tag::History => Pin::at(self.pages.first),
            Tag::Viewport => match self.viewport {
                Viewport::Active => self.get_top_left(Tag::Active),
                Viewport::Top => self.get_top_left(Tag::Screen),
                Viewport::Pin => unsafe { *self.viewport_pin },
            },
            Tag::Active => {
                let mut rem = self.rows as usize;
                let mut node = self.pages.last;
                while !node.is_null() {
                    let nrows = unsafe { (*node).data.size.rows } as usize;
                    if rem <= nrows {
                        return Pin::with(node, (nrows - rem) as CellCountInt, 0);
                    }
                    rem -= nrows;
                    node = unsafe { (*node).prev };
                }
                unreachable!("active always has enough rows");
            }
        }
    }

    /// The bottom-right pin for a tag, or None if that region is empty. Port of
    /// `getBottomRight`.
    pub fn get_bottom_right(&self, tag: Tag) -> Option<Pin> {
        match tag {
            Tag::Screen | Tag::Active => {
                let node = self.pages.last;
                Some(Pin::with(
                    node,
                    unsafe { (*node).data.size.rows } - 1,
                    unsafe { (*node).data.size.cols } - 1,
                ))
            }
            Tag::Viewport => {
                let mut br = self.get_top_left(Tag::Viewport);
                br = unsafe { br.down(self.rows as usize - 1) }.unwrap();
                br.x = unsafe { (*br.node).data.size.cols } - 1;
                Some(br)
            }
            Tag::History => {
                let mut br = self.get_top_left(Tag::Active);
                br = unsafe { br.up(1) }?;
                br.x = unsafe { (*br.node).data.size.cols } - 1;
                Some(br)
            }
        }
    }

    /// The row offset from the top of the viewport. Port of `viewportRowOffset`.
    pub(crate) fn viewport_row_offset(&mut self) -> usize {
        match self.viewport {
            Viewport::Top => 0,
            Viewport::Active => self.total_rows - self.rows as usize,
            Viewport::Pin => {
                if let Some(cached) = self.viewport_pin_row_offset {
                    return cached;
                }
                let vp = unsafe { *self.viewport_pin };
                let mut offset = 0usize;
                let mut node = self.pages.last;
                let top_offset = loop {
                    debug_assert!(!node.is_null());
                    unsafe {
                        offset += (*node).data.size.rows as usize;
                        if node == vp.node {
                            offset -= vp.y as usize;
                            break self.total_rows - offset;
                        }
                        node = (*node).prev;
                    }
                };
                self.viewport_pin_row_offset = Some(top_offset);
                self.assert_integrity();
                top_offset
            }
        }
    }

    /// Fix up the viewport after `removed` rows were removed. Port of `fixupViewport`.
    pub(crate) fn fixup_viewport(&mut self, removed: usize) {
        match self.viewport {
            Viewport::Active => {}
            Viewport::Pin => {
                if self.pin_is_active(unsafe { *self.viewport_pin }) {
                    self.viewport = Viewport::Active;
                } else if let Some(v) = self.viewport_pin_row_offset.as_mut() {
                    if *v < removed {
                        self.viewport = Viewport::Top;
                    } else {
                        *v -= removed;
                    }
                }
            }
            Viewport::Top => {
                if self.pin_is_active(Pin::at(self.pages.first)) {
                    self.viewport = Viewport::Active;
                }
            }
        }
    }

    /// Invalidate the cached viewport row offset (used by resize).
    pub(crate) fn invalidate_viewport_offset(&mut self) {
        self.viewport_pin_row_offset = None;
    }

    /// Access the cached viewport row offset mutably (for in-place fixups).
    pub(crate) fn viewport_offset_cache(&mut self) -> &mut Option<usize> {
        &mut self.viewport_pin_row_offset
    }

    /// Read the cached viewport row offset.
    pub(crate) fn viewport_offset_cache_copy(&self) -> Option<usize> {
        self.viewport_pin_row_offset
    }

    /// Set the cached viewport row offset.
    pub(crate) fn viewport_offset_cache_set(&mut self, v: Option<usize>) {
        self.viewport_pin_row_offset = v;
    }
}
