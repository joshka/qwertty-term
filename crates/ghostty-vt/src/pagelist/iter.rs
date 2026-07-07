//! Row/page/cell iterators over a page-list region (port of the PageList iterators).

use super::pin::Pin;
use super::{Node, PageList, Point};
use crate::page::Row;
use crate::page::size::CellCountInt;

/// Iteration direction. Port of `PageList.Direction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    LeftUp,
    RightDown,
}

/// A maximal contiguous run of rows in a single page. Port of `PageIterator.Chunk`.
#[derive(Debug, Clone, Copy)]
pub struct Chunk {
    pub(crate) node: *mut Node,
    /// Start y (inclusive).
    pub start: CellCountInt,
    /// End y (exclusive).
    pub end: CellCountInt,
}

impl Chunk {
    /// The rows of this chunk.
    ///
    /// # Safety
    /// The chunk's node must be live.
    #[allow(dead_code)]
    pub(crate) unsafe fn rows(self) -> *mut [Row] {
        unsafe {
            let base = (*self.node).data.get_row(self.start as usize);
            std::ptr::slice_from_raw_parts_mut(base, (self.end - self.start) as usize)
        }
    }

    /// True if this chunk covers every row in the page. Port of `fullPage`.
    ///
    /// # Safety
    /// Node live.
    pub(crate) unsafe fn full_page(self) -> bool {
        self.start == 0 && self.end == unsafe { (*self.node).data.size.rows }
    }
}

/// Limit for the page iterator.
#[derive(Clone, Copy)]
enum Limit {
    None,
    Row(Pin),
}

/// Iterates page chunks in a region. Port of `PageIterator`.
pub struct PageIterator {
    row: Option<Pin>,
    limit: Limit,
    direction: Direction,
}

impl PageIterator {
    #[allow(dead_code)]
    pub(crate) fn direction(&self) -> Direction {
        self.direction
    }

    /// # Safety
    /// All node pointers in the iterator must be live.
    pub(crate) unsafe fn next(&mut self) -> Option<Chunk> {
        match self.direction {
            Direction::LeftUp => unsafe { self.next_up() },
            Direction::RightDown => unsafe { self.next_down() },
        }
    }

    unsafe fn next_down(&mut self) -> Option<Chunk> {
        let row = self.row?;
        unsafe {
            match self.limit {
                Limit::None => {
                    let next = (*row.node).next;
                    self.row = if next.is_null() {
                        None
                    } else {
                        Some(Pin::at(next))
                    };
                    Some(Chunk {
                        node: row.node,
                        start: row.y,
                        end: (*row.node).data.size.rows,
                    })
                }
                Limit::Row(limit_row) => {
                    if limit_row.node != row.node {
                        let next = (*row.node).next;
                        self.row = if next.is_null() {
                            None
                        } else {
                            Some(Pin::at(next))
                        };
                        Some(Chunk {
                            node: row.node,
                            start: row.y,
                            end: (*row.node).data.size.rows,
                        })
                    } else {
                        self.row = None;
                        if row.y > limit_row.y {
                            return None;
                        }
                        Some(Chunk {
                            node: row.node,
                            start: row.y,
                            end: limit_row.y + 1,
                        })
                    }
                }
            }
        }
    }

    unsafe fn next_up(&mut self) -> Option<Chunk> {
        let row = self.row?;
        unsafe {
            match self.limit {
                Limit::None => {
                    let prev = (*row.node).prev;
                    self.row = if prev.is_null() {
                        None
                    } else {
                        Some(Pin::with(prev, (*prev).data.size.rows - 1, 0))
                    };
                    Some(Chunk {
                        node: row.node,
                        start: 0,
                        end: row.y + 1,
                    })
                }
                Limit::Row(limit_row) => {
                    if limit_row.node != row.node {
                        let prev = (*row.node).prev;
                        self.row = if prev.is_null() {
                            None
                        } else {
                            Some(Pin::with(prev, (*prev).data.size.rows - 1, 0))
                        };
                        Some(Chunk {
                            node: row.node,
                            start: 0,
                            end: row.y + 1,
                        })
                    } else {
                        self.row = None;
                        if row.y < limit_row.y {
                            return None;
                        }
                        Some(Chunk {
                            node: row.node,
                            start: limit_row.y,
                            end: row.y + 1,
                        })
                    }
                }
            }
        }
    }
}

/// Iterates rows in a region, yielding a [`Pin`] per row. Port of `RowIterator`.
pub struct RowIterator {
    page_it: PageIterator,
    chunk: Option<Chunk>,
    offset: CellCountInt,
}

impl RowIterator {
    /// # Safety
    /// All node pointers must be live.
    pub(crate) unsafe fn next(&mut self) -> Option<Pin> {
        let chunk = self.chunk?;
        let row = Pin::with(chunk.node, self.offset, 0);
        match self.page_it.direction {
            Direction::RightDown => {
                self.offset += 1;
                if self.offset >= chunk.end {
                    self.chunk = unsafe { self.page_it.next() };
                    if let Some(c) = self.chunk {
                        self.offset = c.start;
                    }
                }
            }
            Direction::LeftUp => {
                if self.offset == 0 {
                    self.chunk = unsafe { self.page_it.next() };
                    if let Some(c) = self.chunk {
                        self.offset = c.end - 1;
                    }
                } else if self.offset == chunk.start {
                    self.chunk = None;
                } else {
                    self.offset -= 1;
                }
            }
        }
        Some(row)
    }
}

/// Iterates cells in a region, yielding a [`Pin`] per cell. Port of `CellIterator`.
#[allow(dead_code)]
pub struct CellIterator {
    row_it: RowIterator,
    cell: Option<Pin>,
}

impl CellIterator {
    /// # Safety
    /// All node pointers must be live.
    #[allow(dead_code)]
    pub(crate) unsafe fn next(&mut self) -> Option<Pin> {
        let cell = self.cell?;
        match self.row_it.page_it.direction {
            Direction::RightDown => {
                let cols = unsafe { (*cell.node).data.size.cols };
                if cell.x + 1 < cols {
                    let mut c = cell;
                    c.x += 1;
                    self.cell = Some(c);
                } else {
                    self.cell = unsafe { self.row_it.next() };
                }
            }
            Direction::LeftUp => {
                if cell.x > 0 {
                    let mut c = cell;
                    c.x -= 1;
                    self.cell = Some(c);
                } else if let Some(next) = unsafe { self.row_it.next() } {
                    let mut c = next;
                    c.x = unsafe { (*next.node).data.size.cols } - 1;
                    self.cell = Some(c);
                } else {
                    self.cell = None;
                }
            }
        }
        Some(cell)
    }
}

// ---- Pin-based iterator entry points ----

impl Pin {
    /// Page iterator from this pin. Port of `Pin.pageIterator`.
    ///
    /// # Safety
    /// Node chain live.
    pub(crate) unsafe fn page_iterator(
        self,
        direction: Direction,
        limit: Option<Pin>,
    ) -> PageIterator {
        PageIterator {
            row: Some(self),
            limit: match limit {
                Some(p) => Limit::Row(p),
                None => Limit::None,
            },
            direction,
        }
    }

    /// Row iterator from this pin. Port of `Pin.rowIterator`.
    ///
    /// # Safety
    /// Node chain live.
    pub(crate) unsafe fn row_iterator(
        self,
        direction: Direction,
        limit: Option<Pin>,
    ) -> RowIterator {
        let mut page_it = unsafe { self.page_iterator(direction, limit) };
        let chunk = unsafe { page_it.next() };
        match chunk {
            None => RowIterator {
                page_it,
                chunk: None,
                offset: 0,
            },
            Some(c) => {
                let offset = match direction {
                    Direction::RightDown => c.start,
                    Direction::LeftUp => c.end - 1,
                };
                RowIterator {
                    page_it,
                    chunk: Some(c),
                    offset,
                }
            }
        }
    }

    /// Cell iterator from this pin. Port of `Pin.cellIterator`.
    ///
    /// # Safety
    /// Node chain live.
    pub(crate) unsafe fn cell_iterator(
        self,
        direction: Direction,
        limit: Option<Pin>,
    ) -> CellIterator {
        let mut row_it = unsafe { self.row_iterator(direction, limit) };
        let cell = unsafe { row_it.next() };
        match cell {
            None => CellIterator { row_it, cell: None },
            Some(mut c) => {
                c.x = self.x;
                CellIterator {
                    row_it,
                    cell: Some(c),
                }
            }
        }
    }

    /// Prompt iterator from this pin. Port of `Pin.promptIterator` (null limit;
    /// see [`super::ops::PromptIterator`] for the simplification note).
    pub(crate) fn prompt_iterator(self, direction: Direction) -> super::ops::PromptIterator {
        super::ops::PromptIterator::new(self, direction)
    }
}

// ---- PageList iterator entry points ----

impl PageList {
    /// Page iterator over a point region. Port of `pageIterator`.
    pub fn page_iterator(
        &self,
        direction: Direction,
        tl_pt: Point,
        bl_pt: Option<Point>,
    ) -> PageIterator {
        let tl_pin = self.pin(tl_pt).unwrap();
        let bl_pin = match bl_pt {
            Some(pt) => self.pin(pt).unwrap(),
            None => match self.get_bottom_right(tl_pt.tag) {
                Some(p) => p,
                None => {
                    return PageIterator {
                        row: None,
                        limit: Limit::None,
                        direction,
                    };
                }
            },
        };
        match direction {
            Direction::RightDown => unsafe {
                tl_pin.page_iterator(Direction::RightDown, Some(bl_pin))
            },
            Direction::LeftUp => unsafe { bl_pin.page_iterator(Direction::LeftUp, Some(tl_pin)) },
        }
    }

    /// Row iterator over a point region. Port of `rowIterator`.
    pub fn row_iterator(
        &self,
        direction: Direction,
        tl_pt: Point,
        bl_pt: Option<Point>,
    ) -> RowIterator {
        let tl_pin = self.pin(tl_pt).unwrap();
        let bl_pin = match bl_pt {
            Some(pt) => self.pin(pt).unwrap(),
            None => match self.get_bottom_right(tl_pt.tag) {
                Some(p) => p,
                None => {
                    // Empty region: an iterator that yields nothing.
                    return RowIterator {
                        page_it: PageIterator {
                            row: None,
                            limit: Limit::None,
                            direction,
                        },
                        chunk: None,
                        offset: 0,
                    };
                }
            },
        };
        match direction {
            Direction::RightDown => unsafe {
                tl_pin.row_iterator(Direction::RightDown, Some(bl_pin))
            },
            Direction::LeftUp => unsafe { bl_pin.row_iterator(Direction::LeftUp, Some(tl_pin)) },
        }
    }

    /// Cell iterator over a point region. Port of `cellIterator`.
    pub fn cell_iterator(
        &self,
        direction: Direction,
        tl_pt: Point,
        bl_pt: Option<Point>,
    ) -> CellIterator {
        let tl_pin = self.pin(tl_pt).unwrap();
        let bl_pin = match bl_pt {
            Some(pt) => self.pin(pt).unwrap(),
            None => match self.get_bottom_right(tl_pt.tag) {
                Some(p) => p,
                None => {
                    return CellIterator {
                        row_it: RowIterator {
                            page_it: PageIterator {
                                row: None,
                                limit: Limit::None,
                                direction,
                            },
                            chunk: None,
                            offset: 0,
                        },
                        cell: None,
                    };
                }
            },
        };
        match direction {
            Direction::RightDown => unsafe {
                tl_pin.cell_iterator(Direction::RightDown, Some(bl_pin))
            },
            Direction::LeftUp => unsafe { bl_pin.cell_iterator(Direction::LeftUp, Some(tl_pin)) },
        }
    }
}
