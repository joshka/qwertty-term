//! The paged scrollback list (port of `src/terminal/PageList.zig`, commit `2da015cd6`).
//!
//! `PageList` strings offset-addressed [`Page`](crate::page::Page)s into an intrusive
//! doubly-linked list and layers on the abstractions the terminal `Screen` needs:
//!
//! - a scrollable [`Viewport`] (active / top / pinned),
//! - persistent [`Pin`]s that stay valid across page mutations (the crux),
//! - grow/scroll/erase paths with byte-based scrollback eviction,
//! - lazy reflow on column resize.
//!
//! See `docs/analysis/pagelist.md` for the maintainer-grade survey this ports.
//!
//! # Memory model / unsafe boundary
//!
//! Nodes are heap-boxed and referred to by raw `*mut Node`, mirroring Zig's intrusive
//! list. `Pin.node`, `Node.prev/next`, and the viewport all hold raw node pointers.
//! The invariant (upheld structurally): every node pointer in the list, in a tracked
//! pin, or in the viewport, points at a live boxed node owned by `pool.nodes` until
//! `destroy_node` frees it. All list splicing and node/row/cell pointer access is
//! `unsafe` and isolated here; the public API is safe.
//!
//! Several public methods accept `*mut Pin` / `*mut Node` — these mirror the Zig API
//! (`untrackPin(*Pin)`, `split(Pin)`, tracked-pin remap) where a pin/node is a handle
//! previously vended by this same `PageList`. Their contracts are documented per-method,
//! so `clippy::not_unsafe_ptr_arg_deref` is allowed module-wide rather than marking the
//! whole surface `unsafe`.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::page::size::CellCountInt;
use crate::page::{Capacity, Cell as PageCell, Page, Row};
use crate::point::{Coordinate, Point, Tag};

mod iter;
mod pin;
mod reflow;
mod resize;

pub use iter::{CellIterator, Chunk, Direction, PageIterator, RowIterator};
pub use ops::{Cell, IncreaseCapacity, Scroll, Scrollbar};
pub use pin::{CellSubset, Pin};
pub use resize::{Resize, ResizeCursor, SplitError};

/// The number of nodes/pages to preheat pools with. Port of `page_preheat = 4`.
const PAGE_PREHEAT: usize = 4;

// ---- Node + intrusive list ----

/// A single node in the [`PageList`] linked list. Port of `PageList.Node`.
pub(crate) struct Node {
    pub(crate) prev: *mut Node,
    pub(crate) next: *mut Node,
    pub(crate) data: Page,
    pub(crate) serial: u64,
}

impl Node {
    fn new(data: Page, serial: u64) -> Box<Node> {
        Box::new(Node {
            prev: std::ptr::null_mut(),
            next: std::ptr::null_mut(),
            data,
            serial,
        })
    }
}

/// The intrusive doubly-linked list of nodes. Port of `DoublyLinkedList(Node)`.
///
/// Holds raw head/tail pointers; nodes are owned by [`NodePool`]. All operations are
/// `unsafe` because they dereference the raw links, but each is a faithful, small port
/// of the corresponding `IntrusiveDoublyLinkedList` op.
pub(crate) struct NodeList {
    pub(crate) first: *mut Node,
    pub(crate) last: *mut Node,
}

impl NodeList {
    fn empty() -> Self {
        NodeList {
            first: std::ptr::null_mut(),
            last: std::ptr::null_mut(),
        }
    }

    /// Append `node` to the tail.
    ///
    /// # Safety
    /// `node` must be a live node not currently in any list.
    unsafe fn append(&mut self, node: *mut Node) {
        unsafe {
            (*node).prev = self.last;
            (*node).next = std::ptr::null_mut();
            if self.last.is_null() {
                self.first = node;
            } else {
                (*self.last).next = node;
            }
            self.last = node;
        }
    }

    /// Prepend `node` to the head.
    ///
    /// # Safety
    /// `node` must be a live node not currently in any list.
    unsafe fn prepend(&mut self, node: *mut Node) {
        unsafe {
            (*node).next = self.first;
            (*node).prev = std::ptr::null_mut();
            if self.first.is_null() {
                self.last = node;
            } else {
                (*self.first).prev = node;
            }
            self.first = node;
        }
    }

    /// Insert `node` immediately after `after`.
    ///
    /// # Safety
    /// Both pointers live; `node` not in any list; `after` in this list.
    unsafe fn insert_after(&mut self, after: *mut Node, node: *mut Node) {
        unsafe {
            (*node).prev = after;
            let next = (*after).next;
            (*node).next = next;
            (*after).next = node;
            if next.is_null() {
                self.last = node;
            } else {
                (*next).prev = node;
            }
        }
    }

    /// Insert `node` immediately before `before`.
    ///
    /// # Safety
    /// Both pointers live; `node` not in any list; `before` in this list.
    unsafe fn insert_before(&mut self, before: *mut Node, node: *mut Node) {
        unsafe {
            (*node).next = before;
            let prev = (*before).prev;
            (*node).prev = prev;
            (*before).prev = node;
            if prev.is_null() {
                self.first = node;
            } else {
                (*prev).next = node;
            }
        }
    }

    /// Remove `node` from the list (does not free it).
    ///
    /// # Safety
    /// `node` must be in this list.
    unsafe fn remove(&mut self, node: *mut Node) {
        unsafe {
            let prev = (*node).prev;
            let next = (*node).next;
            if prev.is_null() {
                self.first = next;
            } else {
                (*prev).next = next;
            }
            if next.is_null() {
                self.last = prev;
            } else {
                (*next).prev = prev;
            }
            (*node).prev = std::ptr::null_mut();
            (*node).next = std::ptr::null_mut();
        }
    }

    /// Pop the head node from the list (does not free it).
    ///
    /// # Safety
    /// Caller must ensure the returned node is handled (reinserted or destroyed).
    unsafe fn pop_first(&mut self) -> *mut Node {
        let node = self.first;
        if !node.is_null() {
            unsafe { self.remove(node) };
        }
        node
    }
}

// ---- Memory pool ----

/// Pools for nodes, page buffers, and pins. Port of `PageList.MemoryPool`.
///
/// A functional model of Zig's arena-backed pools: nodes and pins are boxed and kept
/// on free-lists so pointer identity is stable and repeated alloc/free is cheap
/// (preheating fills the free-list). Page buffers are allocated via [`Page::init`]
/// (page-aligned zeroed memory) — the std-vs-non-standard distinction is tracked by
/// the page's own `capacity`/layout, which drives byte accounting.
struct MemoryPool {
    /// Free-list of reusable boxed nodes (their `data` page is replaced on reuse).
    /// The `Box` is load-bearing: nodes must have a stable address (raw `*mut Node`
    /// pointers live in the list/pins/viewport), so we cannot store them inline in the
    /// `Vec`, which would move them on reallocation.
    #[allow(clippy::vec_box)]
    free_nodes: Vec<Box<Node>>,
    /// Free-list of reusable boxed pins (same stable-address requirement).
    #[allow(clippy::vec_box)]
    free_pins: Vec<Box<Pin>>,
}

impl MemoryPool {
    fn init(preheat: usize) -> MemoryPool {
        let mut free_pins = Vec::new();
        for _ in 0..preheat.max(8) {
            free_pins.push(Box::new(Pin::default()));
        }
        MemoryPool {
            free_nodes: Vec::with_capacity(preheat),
            free_pins,
        }
    }

    /// Allocate a node holding `data` with `serial`, returning a raw owning pointer.
    fn create_node(&mut self, data: Page, serial: u64) -> *mut Node {
        let boxed = if let Some(mut b) = self.free_nodes.pop() {
            b.prev = std::ptr::null_mut();
            b.next = std::ptr::null_mut();
            b.data = data;
            b.serial = serial;
            b
        } else {
            Node::new(data, serial)
        };
        Box::into_raw(boxed)
    }

    /// Reclaim a node's box onto the free-list (its page is dropped/replaced).
    ///
    /// # Safety
    /// `node` was produced by `create_node` and is no longer referenced.
    unsafe fn destroy_node(&mut self, node: *mut Node) {
        let boxed = unsafe { Box::from_raw(node) };
        self.free_nodes.push(boxed);
    }

    /// Allocate a pin, returning a raw owning pointer.
    fn create_pin(&mut self, p: Pin) -> *mut Pin {
        let boxed = if let Some(mut b) = self.free_pins.pop() {
            *b = p;
            b
        } else {
            Box::new(p)
        };
        Box::into_raw(boxed)
    }

    /// Reclaim a pin's box onto the free-list.
    ///
    /// # Safety
    /// `p` was produced by `create_pin` and is no longer referenced.
    unsafe fn destroy_pin(&mut self, p: *mut Pin) {
        let boxed = unsafe { Box::from_raw(p) };
        self.free_pins.push(boxed);
    }
}

/// The standard page capacity's byte size. Port of `std_size`.
fn std_size() -> usize {
    crate::page::size_of_std_page()
}

// ---- Viewport ----

/// The viewport location. Port of `PageList.Viewport`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viewport {
    /// Pinned to the active area (write-free scrolling).
    Active,
    /// Pinned to the top of the scrollback.
    Top,
    /// Pinned to the tracked `viewport_pin`.
    Pin,
}

// ---- helpers ----

/// The initial capacity for a page of `cols` columns. Port of `initialCapacity`.
///
/// Uses `std_capacity.adjust(cols)` if it fits `std_size`, else a non-standard capacity
/// with exactly `cols` columns. Always yields ≥1 row.
fn initial_capacity(cols: CellCountInt) -> Capacity {
    if let Ok(cap) = Capacity::std().adjust_cols(cols) {
        return cap;
    }
    let mut cap = Capacity::std();
    cap.cols = cols;
    cap
}

/// The minimum "max size" (bytes) for `cols`x`rows`. Port of `minMaxSize`.
///
/// Enough std pages to hold the active area, plus one extra page (so the active area
/// can straddle two pages).
fn min_max_size(cols: CellCountInt, rows: CellCountInt) -> usize {
    let cap = initial_capacity(cols);
    let pages_exact = if cap.rows as usize >= rows as usize {
        1
    } else {
        (rows as usize).div_ceil(cap.rows as usize)
    };
    let pages = pages_exact + 1;
    debug_assert!(pages >= 2);
    std_size() * pages
}

// ---- PageList ----

/// The paged scrollback list. Port of `PageList`.
pub struct PageList {
    pool: MemoryPool,
    pub(crate) pages: NodeList,

    page_serial: u64,
    page_serial_min: u64,

    /// Total bytes of allocated page buffers (not pool preheat).
    page_size: usize,
    /// Scrollback byte cap (explicit); `usize::MAX` when unbounded.
    explicit_max_size: usize,
    /// Byte floor: always fits active area + 2 pages.
    min_max_size: usize,

    /// Cached sum of page row counts.
    total_rows: usize,

    /// Tracked pins, kept up to date across mutations. Stored as raw owning
    /// pointers (allocated from the pool); order is insertion order.
    tracked_pins: Vec<*mut Pin>,

    pub(crate) viewport: Viewport,
    pub(crate) viewport_pin: *mut Pin,
    viewport_pin_row_offset: Option<usize>,

    pub(crate) cols: CellCountInt,
    pub(crate) rows: CellCountInt,
}

impl Drop for PageList {
    fn drop(&mut self) {
        // Free all node pages and their boxes.
        let mut it = self.pages.first;
        while !it.is_null() {
            let next = unsafe { (*it).next };
            // Reclaim the box (drops the Page, freeing its backing memory).
            drop(unsafe { Box::from_raw(it) });
            it = next;
        }
        // Free all tracked-pin boxes.
        for &p in &self.tracked_pins {
            drop(unsafe { Box::from_raw(p) });
        }
        // Free-list boxes drop automatically.
    }
}

impl PageList {
    /// Initialize a new page list of `cols`x`rows` with `max_size` bytes of scrollback
    /// (unbounded when `None`). Port of `PageList.init`.
    pub fn init(cols: CellCountInt, rows: CellCountInt, max_size: Option<usize>) -> PageList {
        let mut pool = MemoryPool::init(PAGE_PREHEAT);
        let mut page_serial: u64 = 0;
        let (pages, page_size) = init_pages(&mut pool, &mut page_serial, cols, rows);
        let min_max = min_max_size(cols, rows);

        let viewport_pin = pool.create_pin(Pin::at(pages.first));
        let tracked_pins: Vec<*mut Pin> = vec![viewport_pin];

        let result = PageList {
            pool,
            pages,
            page_serial,
            page_serial_min: 0,
            page_size,
            explicit_max_size: max_size.unwrap_or(usize::MAX),
            min_max_size: min_max,
            total_rows: rows as usize,
            tracked_pins,
            viewport: Viewport::Active,
            viewport_pin,
            viewport_pin_row_offset: None,
            cols,
            rows,
        };
        result.assert_integrity();
        result
    }

    // ---- integrity ----

    /// Assert PageList integrity (opt-in via the `slow_runtime_safety`
    /// feature; walks every page and tracked pin). Panics on violation.
    /// Port of `assertIntegrity`.
    #[inline]
    pub(crate) fn assert_integrity(&self) {
        #[cfg(feature = "slow_runtime_safety")]
        {
            if let Err(e) = self.verify_integrity() {
                panic!("PageList integrity check failed: {e:?}");
            }
        }
    }

    /// Verify integrity. Port of `verifyIntegrity`.
    #[cfg(feature = "slow_runtime_safety")]
    fn verify_integrity(&self) -> Result<(), IntegrityError> {
        // total_rows matches actual, and no serial below min.
        let mut actual_total: usize = 0;
        let mut node = self.pages.first;
        while !node.is_null() {
            unsafe {
                actual_total += (*node).data.size.rows as usize;
                if (*node).serial < self.page_serial_min {
                    return Err(IntegrityError::PageSerialInvalid);
                }
                node = (*node).next;
            }
        }
        if actual_total != self.total_rows {
            return Err(IntegrityError::TotalRowsMismatch);
        }

        // Every tracked pin must be valid.
        for &p in &self.tracked_pins {
            if !self.pin_is_valid(unsafe { *p }) {
                return Err(IntegrityError::TrackedPinInvalid);
            }
        }

        if self.viewport == Viewport::Pin {
            let actual_offset = {
                let mut offset: usize = 0;
                let mut n = self.pages.last;
                let vp = unsafe { *self.viewport_pin };
                let mut found = None;
                while !n.is_null() {
                    unsafe {
                        offset += (*n).data.size.rows as usize;
                        if n == vp.node {
                            offset -= vp.y as usize;
                            found = Some(self.total_rows - offset);
                            break;
                        }
                        n = (*n).prev;
                    }
                }
                match found {
                    Some(o) => o,
                    None => return Err(IntegrityError::ViewportPinOffsetMismatch),
                }
            };
            if let Some(cached) = self.viewport_pin_row_offset
                && cached != actual_offset
            {
                return Err(IntegrityError::ViewportPinOffsetMismatch);
            }
            if self.total_rows - actual_offset < self.rows as usize {
                return Err(IntegrityError::ViewportPinInsufficientRows);
            }
        }

        Ok(())
    }

    // ---- basic accessors ----

    /// Total rows currently represented (cached). Slow variant walks the list.
    pub fn total_rows(&self) -> usize {
        self.total_rows
    }

    /// Total rows by walking the list (test/debug). Port of `totalRows`.
    pub fn total_rows_slow(&self) -> usize {
        let mut rows = 0;
        let mut node = self.pages.first;
        while !node.is_null() {
            unsafe {
                rows += (*node).data.size.rows as usize;
                node = (*node).next;
            }
        }
        rows
    }

    /// Total number of pages (test/debug). Port of `totalPages`.
    pub fn total_pages(&self) -> usize {
        let mut pages = 0;
        let mut node = self.pages.first;
        while !node.is_null() {
            unsafe {
                pages += 1;
                node = (*node).next;
            }
        }
        pages
    }

    /// The current active dimensions.
    pub fn cols(&self) -> CellCountInt {
        self.cols
    }
    pub fn rows(&self) -> CellCountInt {
        self.rows
    }

    /// The actual max size (bytes). Port of `maxSize`.
    pub fn max_size(&self) -> usize {
        self.explicit_max_size.max(self.min_max_size)
    }

    /// The active-area top-left node/y helper used widely.
    #[allow(dead_code)]
    pub(crate) fn active_top_left(&self) -> Pin {
        self.get_top_left(Tag::Active)
    }

    // ---- node/viewport accessors for the Screen layer (same-crate) ----

    /// The first node (top of scrollback). Port of `pages.first`.
    pub(crate) fn head_node(&self) -> *mut Node {
        self.pages.first
    }
    /// The last node (bottom of the active area). Port of `pages.last`.
    pub(crate) fn last_node(&self) -> *mut Node {
        self.pages.last
    }
    /// Immutable page for a node.
    ///
    /// # Safety
    /// `node` must be a live node in this list.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) unsafe fn node_data(&self, node: *mut Node) -> &Page {
        unsafe { &(*node).data }
    }
    /// Mutable page for a node.
    ///
    /// # Safety
    /// `node` must be a live node in this list.
    #[allow(clippy::mut_from_ref)]
    pub(crate) unsafe fn node_data_mut(&self, node: *mut Node) -> &mut Page {
        unsafe { &mut (*node).data }
    }
    /// Raw mutable pointer to a node's page, WITHOUT creating an intermediate
    /// `&mut Page` (so callers can hold several such pointers into distinct or
    /// aliasing nodes without tripping Stacked Borrows). Port of `&node.data`.
    ///
    /// # Safety
    /// `node` must be a live node in this list.
    pub(crate) unsafe fn node_page_ptr(&self, node: *mut Node) -> *mut Page {
        unsafe { &raw mut (*node).data }
    }
    /// Whether the viewport is pinned to the active area. Port of
    /// `viewport == .active` (used by `viewportIsBottom`).
    pub(crate) fn viewport_is_active(&self) -> bool {
        self.viewport == Viewport::Active
    }

    // ---- test-support accessors (same-crate only) ----

    #[cfg(test)]
    pub(crate) fn first_node(&self) -> *mut Node {
        self.pages.first
    }
    #[cfg(test)]
    pub(crate) fn page_size_bytes(&self) -> usize {
        self.page_size
    }
    #[cfg(test)]
    pub(crate) fn viewport_state(&self) -> Viewport {
        self.viewport
    }
    #[cfg(test)]
    pub(crate) fn viewport_pin_ptr(&self) -> *mut Pin {
        self.viewport_pin
    }
    #[cfg(test)]
    pub(crate) fn node_page(&self, node: *mut Node) -> &Page {
        unsafe { &(*node).data }
    }
    #[cfg(test)]
    pub(crate) fn node_next(&self, node: *mut Node) -> *mut Node {
        unsafe { (*node).next }
    }
    #[cfg(test)]
    pub(crate) fn node_prev(&self, node: *mut Node) -> *mut Node {
        unsafe { (*node).prev }
    }
}

/// Initialize the first pages that make up the active area. Port of `initPages`.
fn init_pages(
    pool: &mut MemoryPool,
    serial: &mut u64,
    cols: CellCountInt,
    rows: CellCountInt,
) -> (NodeList, usize) {
    let mut page_list = NodeList::empty();
    let mut page_size = 0usize;
    let cap = initial_capacity(cols);

    let mut rem = rows;
    while rem > 0 {
        let mut page = Page::init(cap);
        let n = rem.min(page.capacity.rows);
        page.size.rows = n;
        rem -= n;
        let byte_len = crate::page::page_byte_len(&page);
        let node = pool.create_node(page, *serial);
        unsafe { page_list.append(node) };
        page_size += byte_len;
        *serial += 1;
    }
    debug_assert!(!page_list.first.is_null());
    (page_list, page_size)
}

/// Integrity violations. Port of `PageList.IntegrityError`.
#[cfg(feature = "slow_runtime_safety")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntegrityError {
    PageSerialInvalid,
    TotalRowsMismatch,
    TrackedPinInvalid,
    ViewportPinOffsetMismatch,
    ViewportPinInsufficientRows,
}

// The larger operation groups live in submodules but operate on `PageList` via
// `impl` blocks there (grow/scroll/erase/clone in `ops`, pins in `pin`, iterators
// in `iter`, resize/reflow in `resize`/`reflow`).
mod ops;

#[cfg(test)]
mod tests;
