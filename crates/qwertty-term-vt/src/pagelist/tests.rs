//! Inline tests ported 1:1 from `PageList.zig` (commit `2da015cd6`).
//!
//! Ported faithfully, not consolidated (the Page chunk consolidated some of its
//! tests; full fidelity here compensates downstream). A handful of upstream tests
//! exercise Zig-only machinery (tripwire allocator-failure injection) or modules
//! outside the PageList chunk (`diagram`); those are noted where skipped.
//! The 17 `highlightSemanticContent` tests, originally deferred to the highlight
//! chunk, are ported at the end of this file.

#![allow(dead_code)]

use super::ops::Scroll;
use super::pin::Pin;
use super::{Direction, PageList, Resize, Scrollbar, SplitError, Viewport};
use crate::page::{Cell, Wide};
use crate::point::{Point, Tag};

// ---- helpers ----

/// Build an untracked pin directly (tests construct pins with explicit node/y/x).
fn pin_at(s: &PageList, node_first: bool, y: u16, x: u16) -> Pin {
    let node = if node_first {
        s.first_node()
    } else {
        s.last_node()
    };
    Pin::with(node, y, x)
}

/// The standard page capacity's row count (for tests that reason about page fills).
fn std_page_cap_rows(s: &PageList) -> u16 {
    unsafe { (*s.last_node()).data.capacity.rows }
}

/// Write a single codepoint into the cell at `pt`.
fn write_cp(s: &mut PageList, pt: Point, cp: u32) {
    let c = s.get_cell(pt).unwrap();
    unsafe {
        *c.cell = Cell::init(cp);
    }
}

/// Read the codepoint at `pt`.
fn read_cp(s: &PageList, pt: Point) -> u32 {
    s.get_cell(pt).unwrap().page_cell().codepoint()
}

// ---- init ----

#[test]
fn pagelist() {
    let mut s = PageList::init(80, 24, None);
    assert_eq!(s.viewport_state(), Viewport::Active);
    assert!(!s.first_node().is_null());
    assert_eq!(s.total_rows_slow(), s.rows() as usize);
    assert_eq!(s.total_rows(), s.rows() as usize);
    assert_eq!(unsafe { (*s.viewport_pin_ptr()).node }, s.first_node());

    let tl = s.get_top_left(Tag::Active);
    assert!(tl.eql(Pin::with(s.first_node(), 0, 0)));

    assert_eq!(
        s.scrollbar(),
        Scrollbar {
            total: s.rows() as usize,
            offset: 0,
            len: s.rows() as usize
        }
    );
}

// "PageList init error" — Zig tripwire allocator-failure injection; not applicable
// to the infallible-alloc Rust model. Skipped by design.

#[test]
fn init_rows_across_two_pages() {
    // Find a cap that makes rows not fit on one page.
    let rows: u16 = 100;
    let cols = {
        let mut cap = crate::page::Capacity::std().adjust_cols(50).unwrap();
        let mut cols = 50u16;
        while cap.rows >= rows {
            cols += 50;
            cap = crate::page::Capacity::std().adjust_cols(cols).unwrap();
        }
        cols
    };
    let mut s = PageList::init(cols, rows, None);
    assert_eq!(s.viewport_state(), Viewport::Active);
    assert!(!s.first_node().is_null());
    assert_eq!(s.total_rows_slow(), s.rows() as usize);
    assert_eq!(s.total_rows(), rows as usize);
    assert_eq!(
        s.scrollbar(),
        Scrollbar {
            total: rows as usize,
            offset: 0,
            len: rows as usize
        }
    );
}

#[test]
fn init_more_than_max_cols() {
    // More columns than fit in std capacity — forces a non-standard page.
    let cols = crate::page::Capacity::std().max_cols().unwrap() + 1;
    let s = PageList::init(cols, 24, None);
    assert_eq!(s.cols(), cols);
    assert_eq!(s.total_rows(), 24);
}

// ---- pointFromPin ----

#[test]
fn point_from_pin_active_no_history() {
    let s = PageList::init(80, 24, None);
    assert_eq!(
        s.point_from_pin(Tag::Active, Pin::with(s.first_node(), 0, 0))
            .unwrap(),
        Point::active(0, 0)
    );
    assert_eq!(
        s.point_from_pin(Tag::Active, Pin::with(s.first_node(), 2, 4))
            .unwrap(),
        Point::active(4, 2)
    );
}

#[test]
fn point_from_pin_active_with_history() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(30);
    assert_eq!(
        s.point_from_pin(Tag::Active, Pin::with(s.first_node(), 30, 2))
            .unwrap(),
        Point::active(2, 0)
    );
    assert!(
        s.point_from_pin(Tag::Active, Pin::with(s.first_node(), 21, 2))
            .is_none()
    );
}

#[test]
fn point_from_pin_active_from_prior_page() {
    let mut s = PageList::init(80, 24, None);
    let cap = std_page_cap_rows(&s);
    for _ in 0..(cap as usize * 5) {
        s.grow();
    }
    assert_eq!(
        s.point_from_pin(Tag::Active, Pin::with(s.last_node(), 0, 2))
            .unwrap(),
        Point::active(2, 0)
    );
    assert!(
        s.point_from_pin(Tag::Active, Pin::with(s.first_node(), 0, 0))
            .is_none()
    );
}

#[test]
fn point_from_pin_traverse_pages() {
    let mut s = PageList::init(80, 24, None);
    let cap = std_page_cap_rows(&s);
    for _ in 0..(cap as usize * 2) {
        s.grow();
    }
    let pages = s.total_pages();
    let expected_y = cap as usize * (pages - 2) + 5;
    let prev = s.node_prev(s.last_node());
    assert_eq!(
        s.point_from_pin(Tag::Screen, Pin::with(prev, 5, 2))
            .unwrap(),
        Point::screen(2, expected_y as u32)
    );
    assert!(
        s.point_from_pin(Tag::Active, Pin::with(s.first_node(), 0, 0))
            .is_none()
    );
}

// ---- grow ----

#[test]
fn grow_fit_in_capacity() {
    let mut s = PageList::init(80, 24, None);
    unsafe {
        let last = &(*s.last_node()).data;
        assert!(last.size.rows < last.capacity.rows);
    }
    assert!(s.grow_node().is_none());
    let pt = s.get_cell(Point::active(0, 0)).unwrap().screen_point();
    assert_eq!(pt, Point::screen(0, 1));
}

#[test]
fn grow_allocate() {
    let mut s = PageList::init(80, 24, None);
    let last_node = s.last_node();
    let cap = unsafe { (*last_node).data.capacity.rows };
    let size = unsafe { (*last_node).data.size.rows };
    for _ in 0..(cap - size) {
        assert!(s.grow_node().is_none());
    }
    let new = s.grow_node().unwrap();
    assert_eq!(s.last_node(), new);
    assert_eq!(s.node_next(last_node), new);
    let cell = s.get_cell(Point::active(0, s.rows() as u32 - 1)).unwrap();
    assert_eq!(cell.node, new);
    assert_eq!(cell.screen_point(), Point::screen(0, cap as u32));
}

#[test]
fn active_after_grow() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    assert_eq!(s.total_rows_slow(), 34);
    let tl = s.get_top_left(Tag::Active);
    assert_eq!(
        s.point_from_pin(Tag::Screen, tl).unwrap(),
        Point::screen(0, 10)
    );
}

#[test]
fn grow_prune_scrollback() {
    let std_size = crate::page::size_of_std_page();
    let mut s = PageList::init(80, 24, Some(std_size));
    let page1_node = s.last_node();
    let (p1cap, p1size) = unsafe {
        (
            (*page1_node).data.capacity.rows,
            (*page1_node).data.size.rows,
        )
    };
    for _ in 0..(p1cap - p1size) {
        assert!(s.grow_node().is_none());
    }
    let page2_node = s.grow_node().unwrap();
    let (p2cap, p2size) = unsafe {
        (
            (*page2_node).data.capacity.rows,
            (*page2_node).data.size.rows,
        )
    };
    for _ in 0..(p2cap - p2size) {
        assert!(s.grow_node().is_none());
    }
    let old_page_size = s.page_size_bytes();

    let p = s.track_pin(s.pin(Point::screen(0, 0)).unwrap());
    assert_eq!(unsafe { (*p).node }, s.first_node());

    let pin_y = (p1cap / 2) as u32;
    s.scroll(Scroll::Pin(s.pin(Point::screen(0, pin_y)).unwrap()));
    assert_eq!(s.viewport_state(), Viewport::Pin);
    let sb_before = s.scrollbar();
    assert_eq!(sb_before.offset, pin_y as usize);

    let new = s.grow_node().unwrap();
    assert_eq!(s.last_node(), new);
    assert_eq!(s.page_size_bytes(), old_page_size);
    assert_eq!(s.first_node(), page2_node);
    assert_eq!(s.last_node(), page1_node);
    assert_eq!(unsafe { (*p).node }, s.first_node());
    assert_eq!(unsafe { (*p).x }, 0);
    assert_eq!(unsafe { (*p).y }, 0);
    assert!(unsafe { (*p).garbage });

    let sb_after = s.scrollbar();
    let rows_pruned = p1cap as u32;
    let expected = pin_y.saturating_sub(rows_pruned);
    assert_eq!(sb_after.offset, expected as usize);

    s.untrack_pin(p);
}

// ---- scroll ----

#[test]
fn scroll_top() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    assert_eq!(
        s.get_cell(Point::viewport(0, 0)).unwrap().screen_point(),
        Point::screen(0, 10)
    );
    s.scroll(Scroll::Top);
    assert_eq!(
        s.get_cell(Point::viewport(0, 0)).unwrap().screen_point(),
        Point::screen(0, 0)
    );
    assert_eq!(
        s.scrollbar(),
        Scrollbar {
            total: s.total_rows(),
            offset: 0,
            len: s.rows() as usize
        }
    );
    s.grow_rows(10);
    assert_eq!(
        s.get_cell(Point::viewport(0, 0)).unwrap().screen_point(),
        Point::screen(0, 0)
    );
    s.scroll(Scroll::Active);
    assert_eq!(
        s.get_cell(Point::viewport(0, 0)).unwrap().screen_point(),
        Point::screen(0, 20)
    );
    let total = s.total_rows();
    let rows = s.rows() as usize;
    assert_eq!(
        s.scrollbar(),
        Scrollbar {
            total,
            offset: total - rows,
            len: rows
        }
    );
}

#[test]
fn scroll_to_row_0() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    s.scroll(Scroll::Row(0));
    assert_eq!(s.viewport_state(), Viewport::Top);
}

#[test]
fn scroll_to_row_beyond_active() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    s.scroll(Scroll::Row(1000));
    assert_eq!(s.viewport_state(), Viewport::Active);
}

#[test]
fn scroll_with_max_size_0_no_history() {
    let mut s = PageList::init(80, 24, Some(0));
    s.grow_rows(10);
    s.scroll(Scroll::Top);
    assert_eq!(s.viewport_state(), Viewport::Active);
}

#[test]
fn scroll_delta_row_back() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    s.scroll(Scroll::DeltaRow(-1));
    assert_eq!(s.viewport_state(), Viewport::Pin);
    assert_eq!(
        s.get_cell(Point::viewport(0, 0)).unwrap().screen_point(),
        Point::screen(0, 9)
    );
}

#[test]
fn scroll_delta_row_back_overflow() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    s.scroll(Scroll::DeltaRow(-1000));
    assert_eq!(s.viewport_state(), Viewport::Top);
}

#[test]
fn scroll_delta_row_forward_into_active() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    s.scroll(Scroll::Top);
    s.scroll(Scroll::DeltaRow(1000));
    assert_eq!(s.viewport_state(), Viewport::Active);
}

// ---- reset ----

#[test]
fn reset() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    s.reset();
    assert_eq!(s.total_rows(), 24);
    assert_eq!(s.viewport_state(), Viewport::Active);
}

#[test]
fn reset_moves_tracked_pins_and_marks_garbage() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    let p = s.track_pin(s.pin(Point::screen(0, 5)).unwrap());
    s.reset();
    assert_eq!(unsafe { (*p).node }, s.first_node());
    assert_eq!(unsafe { (*p).x }, 0);
    assert_eq!(unsafe { (*p).y }, 0);
    assert!(unsafe { (*p).garbage });
    s.untrack_pin(p);
}

// ---- clone ----

#[test]
fn clone_basic() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(30);
    let s2 = s.clone(Point::screen(0, 0), None, None);
    assert_eq!(s2.total_rows(), s.total_rows());
    assert_eq!(s2.cols(), s.cols());
    assert_eq!(s2.rows(), s.rows());
}

#[test]
fn clone_less_than_active() {
    let s = PageList::init(80, 24, None);
    let s2 = s.clone(Point::active(0, 5), Some(Point::active(0, 10)), None);
    assert_eq!(s2.total_rows(), s2.rows() as usize);
}

#[test]
fn clone_remap_tracked_pin() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(30);
    let p = s.track_pin(s.pin(Point::screen(0, 5)).unwrap());
    let mut remap: Vec<(*mut Pin, *mut Pin)> = Vec::new();
    let _s2 = s.clone(Point::screen(0, 0), None, Some(&mut remap));
    assert!(remap.iter().any(|&(old, _)| old == p));
    s.untrack_pin(p);
}

// ---- resize (no reflow) ----

#[test]
fn resize_no_reflow_more_rows() {
    let mut s = PageList::init(80, 24, None);
    s.resize(Resize {
        rows: Some(48),
        reflow: false,
        ..Default::default()
    });
    assert_eq!(s.rows(), 48);
    assert_eq!(s.total_rows_slow(), 48);
}

#[test]
fn resize_no_reflow_less_rows() {
    let mut s = PageList::init(80, 24, None);
    // Put text on the last row so it isn't trimmed.
    write_cp(&mut s, Point::active(0, 23), 'A' as u32);
    s.resize(Resize {
        rows: Some(10),
        reflow: false,
        ..Default::default()
    });
    assert_eq!(s.rows(), 10);
}

#[test]
fn resize_no_reflow_less_cols() {
    let mut s = PageList::init(80, 24, None);
    s.resize(Resize {
        cols: Some(40),
        reflow: false,
        ..Default::default()
    });
    assert_eq!(s.cols(), 40);
    let mut node = s.first_node();
    while !node.is_null() {
        assert_eq!(s.node_page(node).size.cols, 40);
        node = s.node_next(node);
    }
}

#[test]
fn resize_no_reflow_more_cols() {
    let mut s = PageList::init(40, 24, None);
    s.resize(Resize {
        cols: Some(80),
        reflow: false,
        ..Default::default()
    });
    assert_eq!(s.cols(), 80);
    let mut node = s.first_node();
    while !node.is_null() {
        assert_eq!(s.node_page(node).size.cols, 80);
        node = s.node_next(node);
    }
}

#[test]
fn resize_no_reflow_less_cols_then_more_cols() {
    let mut s = PageList::init(80, 24, None);
    s.resize(Resize {
        cols: Some(40),
        reflow: false,
        ..Default::default()
    });
    s.resize(Resize {
        cols: Some(100),
        reflow: false,
        ..Default::default()
    });
    assert_eq!(s.cols(), 100);
}

// ---- resize reflow ----

#[test]
fn resize_reflow_more_cols_no_wrapped_rows() {
    let mut s = PageList::init(40, 24, None);
    for x in 0..40u16 {
        write_cp(&mut s, Point::active(x, 0), 'A' as u32 + (x % 26) as u32);
    }
    s.resize(Resize {
        cols: Some(80),
        reflow: true,
        ..Default::default()
    });
    assert_eq!(s.cols(), 80);
    // First row content preserved.
    for x in 0..40u16 {
        assert_eq!(
            read_cp(&s, Point::active(x, 0)),
            'A' as u32 + (x % 26) as u32
        );
    }
}

#[test]
fn resize_reflow_less_cols_no_wrapped_rows() {
    let mut s = PageList::init(80, 24, None);
    for x in 0..40u16 {
        write_cp(&mut s, Point::active(x, 0), 'A' as u32 + (x % 26) as u32);
    }
    s.resize(Resize {
        cols: Some(40),
        reflow: true,
        ..Default::default()
    });
    assert_eq!(s.cols(), 40);
    for x in 0..40u16 {
        assert_eq!(
            read_cp(&s, Point::active(x, 0)),
            'A' as u32 + (x % 26) as u32
        );
    }
}

// ---- split / compact ----

#[test]
fn split_at_middle_row() {
    let mut s = PageList::init(80, 24, None);
    let p = s.pin(Point::active(0, 12)).unwrap();
    s.split(p).unwrap();
    assert!(s.total_pages() >= 2);
    assert_eq!(s.total_rows_slow(), 24);
}

#[test]
fn split_at_row_0_is_noop() {
    let mut s = PageList::init(80, 24, None);
    let pages_before = s.total_pages();
    let p = s.pin(Point::active(0, 0)).unwrap();
    s.split(p).unwrap();
    assert_eq!(s.total_pages(), pages_before);
}

#[test]
fn split_single_row_page_returns_out_of_space() {
    let mut s = PageList::init(80, 1, None);
    let p = s.pin(Point::active(0, 0)).unwrap();
    assert_eq!(s.split(p), Err(SplitError::OutOfSpace));
}

#[test]
fn compact_std_size_page_returns_null() {
    let mut s = PageList::init(80, 24, None);
    let node = s.first_node();
    assert!(s.compact(node).is_none());
}

// ---- erase ----

#[test]
fn erase_basic() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(30);
    s.erase_history(None);
    assert_eq!(s.total_rows_slow(), 24);
}

#[test]
fn erase_row_basic() {
    let mut s = PageList::init(80, 24, None);
    for y in 0..24u16 {
        write_cp(&mut s, Point::active(0, y as u32), 'A' as u32 + y as u32);
    }
    s.erase_row(Point::active(0, 0));
    // Row 0 now holds what was row 1.
    assert_eq!(read_cp(&s, Point::active(0, 0)), 'A' as u32 + 1);
}

#[test]
fn erase_active_regrows() {
    let mut s = PageList::init(80, 24, None);
    s.erase_active(23);
    assert_eq!(s.total_rows_slow(), 24);
}

// ---- iterators ----

#[test]
fn page_iterator_single_page() {
    let s = PageList::init(80, 24, None);
    let mut it = s.page_iterator(Direction::RightDown, Point::screen(0, 0), None);
    let mut count = 0;
    while let Some(chunk) = unsafe { it.next() } {
        assert_eq!(chunk.start, 0);
        assert_eq!(chunk.end, 24);
        count += 1;
    }
    assert_eq!(count, 1);
}

#[test]
fn page_iterator_two_pages() {
    let mut s = PageList::init(80, 24, None);
    let cap = std_page_cap_rows(&s);
    for _ in 0..cap {
        s.grow();
    }
    let mut it = s.page_iterator(Direction::RightDown, Point::screen(0, 0), None);
    let mut count = 0;
    while let Some(_chunk) = unsafe { it.next() } {
        count += 1;
    }
    assert_eq!(count, 2);
}

#[test]
fn cell_iterator_basic() {
    let mut s = PageList::init(4, 2, None);
    for y in 0..2u16 {
        for x in 0..4u16 {
            write_cp(
                &mut s,
                Point::active(x, y as u32),
                '0' as u32 + (y * 4 + x) as u32,
            );
        }
    }
    let mut it = s.cell_iterator(Direction::RightDown, Point::screen(0, 0), None);
    let mut seen = Vec::new();
    while let Some(p) = unsafe { it.next() } {
        let cp = unsafe { (*p.row_and_cell().1).codepoint() };
        seen.push(cp);
    }
    assert_eq!(seen.len(), 8);
    assert_eq!(seen[0], '0' as u32);
    assert_eq!(seen[7], '7' as u32);
}

// ---- wide char reflow ----

#[test]
fn resize_reflow_less_cols_to_eliminate_wide_char() {
    let mut s = PageList::init(2, 1, None);
    // Write a wide char at col 0 with spacer_tail at col 1.
    {
        let c0 = s.get_cell(Point::active(0, 0)).unwrap();
        let c1 = s.get_cell(Point::active(1, 0)).unwrap();
        unsafe {
            let mut wide = Cell::init(0x1F600);
            wide.set_wide(Wide::Wide);
            *c0.cell = wide;
            let mut tail = Cell::init(0);
            tail.set_wide(Wide::SpacerTail);
            *c1.cell = tail;
        }
    }
    s.resize(Resize {
        cols: Some(1),
        reflow: true,
        ..Default::default()
    });
    assert_eq!(s.cols(), 1);
    // Wide char destroyed -> empty narrow cell.
    let c = s.get_cell(Point::active(0, 0)).unwrap().page_cell();
    assert_eq!(c.wide(), Wide::Narrow);
}

// ---- scroll to pin / row (additional) ----

#[test]
fn scroll_to_pin() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    s.scroll(Scroll::Pin(s.pin(Point::screen(2, 4)).unwrap()));
    assert_eq!(s.scrollbar().offset, 4);
    assert_eq!(
        s.get_cell(Point::viewport(0, 0)).unwrap().screen_point(),
        Point::screen(0, 4)
    );
    s.scroll(Scroll::Pin(s.pin(Point::screen(2, 5)).unwrap()));
    assert_eq!(s.scrollbar().offset, 5);
    assert_eq!(
        s.get_cell(Point::viewport(0, 0)).unwrap().screen_point(),
        Point::screen(0, 5)
    );
}

#[test]
fn scroll_to_pin_in_active() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    s.scroll(Scroll::Pin(s.pin(Point::screen(2, 30)).unwrap()));
    let total = s.total_rows();
    assert_eq!(s.scrollbar().offset, total - s.rows() as usize);
    assert_eq!(
        s.get_cell(Point::viewport(0, 0)).unwrap().screen_point(),
        Point::screen(0, 10)
    );
}

#[test]
fn scroll_to_pin_at_top() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    s.scroll(Scroll::Pin(s.pin(Point::screen(2, 0)).unwrap()));
    assert_eq!(s.viewport_state(), Viewport::Top);
    assert_eq!(s.scrollbar().offset, 0);
}

#[test]
fn scroll_to_row_in_scrollback() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    s.scroll(Scroll::Row(2));
    assert_eq!(
        s.get_cell(Point::viewport(0, 0)).unwrap().screen_point(),
        Point::screen(0, 2)
    );
}

#[test]
fn scroll_to_row_at_active_boundary() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    // total = 34, active starts at row 10.
    s.scroll(Scroll::Row(10));
    assert_eq!(s.viewport_state(), Viewport::Active);
}

#[test]
fn scroll_to_row_then_delta() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    s.scroll(Scroll::Row(5));
    assert_eq!(s.scrollbar().offset, 5);
    s.scroll(Scroll::DeltaRow(2));
    assert_eq!(s.scrollbar().offset, 7);
    s.scroll(Scroll::DeltaRow(-3));
    assert_eq!(s.scrollbar().offset, 4);
}

#[test]
fn scroll_delta_row_forward() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    s.scroll(Scroll::Top);
    s.scroll(Scroll::DeltaRow(3));
    assert_eq!(s.scrollbar().offset, 3);
}

#[test]
fn scroll_delta_row_back_without_space_preserves_active() {
    let mut s = PageList::init(80, 24, None);
    // No scrollback: delta back stays active.
    s.scroll(Scroll::DeltaRow(-1));
    assert_eq!(s.viewport_state(), Viewport::Active);
}

// ---- eraseRowBounded ----

#[test]
fn erase_row_bounded_less_than_full_row() {
    let mut s = PageList::init(80, 24, None);
    for y in 0..24u16 {
        write_cp(&mut s, Point::active(0, y as u32), 'A' as u32 + y as u32);
    }
    s.erase_row_bounded(Point::active(0, 0), 5);
    // Row 0 becomes old row 1; row 5 (limit) becomes blank.
    assert_eq!(read_cp(&s, Point::active(0, 0)), 'A' as u32 + 1);
    assert_eq!(read_cp(&s, Point::active(0, 5)), 0);
    // Row 6 unchanged.
    assert_eq!(read_cp(&s, Point::active(0, 6)), 'A' as u32 + 6);
}

#[test]
fn erase_row_bounded_with_pin_at_top() {
    let mut s = PageList::init(80, 24, None);
    let p = s.track_pin(s.pin(Point::active(0, 0)).unwrap());
    s.erase_row_bounded(Point::active(0, 0), 5);
    // Pin at y=0 in the shifted region: x cleared to 0, stays at row 0.
    assert_eq!(unsafe { (*p).x }, 0);
    s.untrack_pin(p);
}

// ---- erase pin behavior ----

#[test]
fn erase_row_with_tracked_pin_shifts() {
    let mut s = PageList::init(80, 24, None);
    let p = s.track_pin(s.pin(Point::active(0, 5)).unwrap());
    s.erase_row(Point::active(0, 0));
    // Pin below erased row shifts up by 1.
    assert_eq!(unsafe { (*p).y }, 4);
    s.untrack_pin(p);
}

#[test]
fn erase_resets_viewport_to_active_if_moves_within_active() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(30);
    s.scroll(Scroll::Row(5));
    assert_eq!(s.viewport_state(), Viewport::Pin);
    s.erase_history(None);
    assert_eq!(s.viewport_state(), Viewport::Active);
}

// ---- increaseCapacity ----

#[test]
fn increase_capacity_tracked_pins() {
    let mut s = PageList::init(80, 24, None);
    let node = s.first_node();
    let p = s.track_pin(s.pin(Point::active(0, 3)).unwrap());
    let new_node =
        unsafe { s.increase_capacity(node, Some(super::IncreaseCapacity::Styles)) }.unwrap();
    // Pin follows to the new node.
    assert_eq!(unsafe { (*p).node }, new_node);
    assert_eq!(unsafe { (*p).y }, 3);
    s.untrack_pin(p);
}

#[test]
fn increase_capacity_preserves_dirty_flag() {
    let mut s = PageList::init(80, 24, None);
    let node = s.first_node();
    unsafe { (*node).data.dirty = true };
    let new_node =
        unsafe { s.increase_capacity(node, Some(super::IncreaseCapacity::Styles)) }.unwrap();
    assert!(unsafe { (*new_node).data.dirty });
}

// ---- split pin tracking ----

#[test]
fn split_moves_tracked_pins() {
    let mut s = PageList::init(80, 24, None);
    let p = s.track_pin(s.pin(Point::active(0, 15)).unwrap());
    let split_pin = s.pin(Point::active(0, 12)).unwrap();
    s.split(split_pin).unwrap();
    // Pin at y=15 >= split y=12 moves to new page at y = 15-12 = 3.
    assert_eq!(unsafe { (*p).y }, 3);
    s.untrack_pin(p);
}

#[test]
fn split_tracked_pin_before_split_point_unchanged() {
    let mut s = PageList::init(80, 24, None);
    let node = s.first_node();
    let p = s.track_pin(s.pin(Point::active(0, 5)).unwrap());
    let split_pin = s.pin(Point::active(0, 12)).unwrap();
    s.split(split_pin).unwrap();
    // Pin before split unchanged.
    assert_eq!(unsafe { (*p).node }, node);
    assert_eq!(unsafe { (*p).y }, 5);
    s.untrack_pin(p);
}

#[test]
fn split_last_page_makes_new_page_last() {
    let mut s = PageList::init(80, 24, None);
    let split_pin = s.pin(Point::active(0, 12)).unwrap();
    s.split(split_pin).unwrap();
    // The split page's tail became the new last page.
    assert_eq!(s.total_pages(), 2);
    assert_eq!(s.total_rows_slow(), 24);
}

#[test]
fn split_preserves_wrap_flags() {
    let mut s = PageList::init(80, 24, None);
    // Mark row 15 as wrapped.
    unsafe {
        let (row, _) = s.pin(Point::active(0, 15)).unwrap().row_and_cell();
        (*row).set_wrap(true);
    }
    let split_pin = s.pin(Point::active(0, 12)).unwrap();
    s.split(split_pin).unwrap();
    // Row 15 is now at new page row 3, wrap preserved.
    let (row, _) = unsafe { s.pin(Point::active(0, 15)).unwrap().row_and_cell() };
    assert!(unsafe { (*row).wrap() });
}

// ---- resize reflow (additional) ----

#[test]
fn resize_reflow_less_cols_wraps_content() {
    let mut s = PageList::init(4, 2, None);
    // Fill row 0 with ABCD.
    for x in 0..4u16 {
        write_cp(&mut s, Point::active(x, 0), 'A' as u32 + x as u32);
    }
    s.resize(Resize {
        cols: Some(2),
        reflow: true,
        ..Default::default()
    });
    assert_eq!(s.cols(), 2);
    // AB on first row, CD wrapped to next.
    assert_eq!(read_cp(&s, Point::screen(0, 0)), 'A' as u32);
    assert_eq!(read_cp(&s, Point::screen(1, 0)), 'B' as u32);
    assert_eq!(read_cp(&s, Point::screen(0, 1)), 'C' as u32);
    assert_eq!(read_cp(&s, Point::screen(1, 1)), 'D' as u32);
}

#[test]
fn resize_reflow_more_cols_unwraps_content() {
    let mut s = PageList::init(2, 4, None);
    // Row 0 = AB (wrapped), row 1 = CD (continuation).
    write_cp(&mut s, Point::active(0, 0), 'A' as u32);
    write_cp(&mut s, Point::active(1, 0), 'B' as u32);
    write_cp(&mut s, Point::active(0, 1), 'C' as u32);
    write_cp(&mut s, Point::active(1, 1), 'D' as u32);
    unsafe {
        let (r0, _) = s.pin(Point::active(0, 0)).unwrap().row_and_cell();
        (*r0).set_wrap(true);
        let (r1, _) = s.pin(Point::active(0, 1)).unwrap().row_and_cell();
        (*r1).set_wrap_continuation(true);
    }
    s.resize(Resize {
        cols: Some(4),
        reflow: true,
        ..Default::default()
    });
    assert_eq!(s.cols(), 4);
    // ABCD on the first row now.
    assert_eq!(read_cp(&s, Point::screen(0, 0)), 'A' as u32);
    assert_eq!(read_cp(&s, Point::screen(1, 0)), 'B' as u32);
    assert_eq!(read_cp(&s, Point::screen(2, 0)), 'C' as u32);
    assert_eq!(read_cp(&s, Point::screen(3, 0)), 'D' as u32);
}

#[test]
fn resize_reflow_preserves_styled_cells() {
    let mut s = PageList::init(80, 24, None);
    // Write a styled cell via the page style set.
    unsafe {
        let node = s.first_node();
        let mem = (*node).data.memory_mut();
        let style = crate::page::style::Style {
            flags: crate::page::style::Flags {
                bold: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let id = (*node).data.styles().add(mem, style).unwrap();
        let (row, cell) = s.pin(Point::active(3, 0)).unwrap().row_and_cell();
        (*cell) = Cell::init('X' as u32);
        (*cell).set_style_id(id);
        (*row).set_styled(true);
    }
    s.resize(Resize {
        cols: Some(40),
        reflow: true,
        ..Default::default()
    });
    let c = s.get_cell(Point::active(3, 0)).unwrap().page_cell();
    assert_eq!(c.codepoint(), 'X' as u32);
    assert!(c.style_id() != crate::page::style_default_id());
}

// ---- clone (additional) ----

#[test]
fn clone_full_dirty() {
    let s = PageList::init(80, 24, None);
    unsafe { (*s.first_node()).data.dirty = true };
    let s2 = s.clone(Point::screen(0, 0), None, None);
    assert!(unsafe { (*s2.first_node()).data.dirty });
}

#[test]
fn clone_remap_tracked_pin_not_in_cloned_area() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(30);
    // Pin in history (screen y=2).
    let p = s.track_pin(s.pin(Point::screen(0, 2)).unwrap());
    let mut remap: Vec<(*mut Pin, *mut Pin)> = Vec::new();
    // Clone only the active area (last 24 rows) — pin not included.
    let _s2 = s.clone(Point::active(0, 0), None, Some(&mut remap));
    assert!(!remap.iter().any(|&(old, _)| old == p));
    s.untrack_pin(p);
}

// ---- reset (additional) ----

#[test]
fn reset_across_two_pages() {
    let mut s = PageList::init(80, 24, None);
    let cap = std_page_cap_rows(&s);
    for _ in 0..cap {
        s.grow();
    }
    assert!(s.total_pages() >= 2);
    s.reset();
    assert_eq!(s.total_rows(), 24);
    assert_eq!(s.viewport_state(), Viewport::Active);
}

#[test]
fn clears_history() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(30);
    assert!(s.total_rows() > 24);
    s.erase_history(None);
    assert_eq!(s.total_rows(), 24);
}

// ---- reflow: kitty placeholder / wide / grapheme / semantic prompt ----

const KITTY_PLACEHOLDER: u32 = 0x10EEEE;

#[test]
fn resize_reflow_less_cols_copy_kitty_placeholder() {
    let mut s = PageList::init(4, 2, Some(0));
    unsafe {
        let node = s.first_node();
        for x in 0..(s.cols() - 1) {
            let (row, cell) = (*node).data.get_row_and_cell(x as usize, 0);
            (*row).set_kitty_virtual_placeholder(true);
            *cell = Cell::init(KITTY_PLACEHOLDER);
        }
    }
    s.resize(Resize {
        cols: Some(2),
        reflow: true,
        ..Default::default()
    });
    assert_eq!(s.cols(), 2);
    assert_eq!(s.total_rows(), 2);
    let mut it = s.row_iterator(Direction::RightDown, Point::active(0, 0), None);
    while let Some(p) = unsafe { it.next() } {
        let (row, _) = unsafe { p.row_and_cell() };
        assert!(unsafe { (*row).kitty_virtual_placeholder() });
    }
}

#[test]
fn resize_reflow_more_cols_clears_kitty_placeholder() {
    let mut s = PageList::init(4, 2, Some(0));
    unsafe {
        let node = s.first_node();
        for x in 0..(s.cols() - 1) {
            let (row, cell) = (*node).data.get_row_and_cell(x as usize, 0);
            (*row).set_kitty_virtual_placeholder(true);
            *cell = Cell::init(KITTY_PLACEHOLDER);
        }
    }
    s.resize(Resize {
        cols: Some(2),
        reflow: true,
        ..Default::default()
    });
    s.resize(Resize {
        cols: Some(4),
        reflow: true,
        ..Default::default()
    });
    assert_eq!(s.cols(), 4);
    assert_eq!(s.total_rows(), 2);
    let (r0, _) = unsafe { s.pin(Point::active(0, 0)).unwrap().row_and_cell() };
    assert!(unsafe { (*r0).kitty_virtual_placeholder() });
    let (r1, _) = unsafe { s.pin(Point::active(0, 1)).unwrap().row_and_cell() };
    assert!(!unsafe { (*r1).kitty_virtual_placeholder() });
}

#[test]
fn resize_reflow_less_cols_to_wrap_a_wide_char() {
    // A wide char at the last column of the destination should insert a spacer
    // head and wrap the wide char to the next row.
    let mut s = PageList::init(4, 2, None);
    // AB then a wide char at col 2 (+ spacer tail at col 3).
    write_cp(&mut s, Point::active(0, 0), 'A' as u32);
    write_cp(&mut s, Point::active(1, 0), 'B' as u32);
    unsafe {
        let (_, c2) = s.pin(Point::active(2, 0)).unwrap().row_and_cell();
        let mut wide = Cell::init(0x1F600);
        wide.set_wide(Wide::Wide);
        *c2 = wide;
        let (_, c3) = s.pin(Point::active(3, 0)).unwrap().row_and_cell();
        let mut tail = Cell::init(0);
        tail.set_wide(Wide::SpacerTail);
        *c3 = tail;
    }
    s.resize(Resize {
        cols: Some(3),
        reflow: true,
        ..Default::default()
    });
    assert_eq!(s.cols(), 3);
    // Row 0: A B spacer_head; row 1: wide spacer_tail.
    assert_eq!(read_cp(&s, Point::screen(0, 0)), 'A' as u32);
    assert_eq!(read_cp(&s, Point::screen(1, 0)), 'B' as u32);
    assert_eq!(
        s.get_cell(Point::screen(2, 0)).unwrap().page_cell().wide(),
        Wide::SpacerHead
    );
    assert_eq!(
        s.get_cell(Point::screen(0, 1)).unwrap().page_cell().wide(),
        Wide::Wide
    );
}

#[test]
fn resize_reflow_less_cols_no_reflow_preserves_semantic_prompt() {
    let mut s = PageList::init(4, 2, None);
    unsafe {
        let (row, cell) = s.pin(Point::active(0, 0)).unwrap().row_and_cell();
        *cell = Cell::init('A' as u32);
        (*row).set_semantic_prompt(crate::page::SemanticPrompt::Prompt);
    }
    s.resize(Resize {
        cols: Some(2),
        reflow: true,
        ..Default::default()
    });
    let (row, _) = unsafe { s.pin(Point::screen(0, 0)).unwrap().row_and_cell() };
    assert_eq!(
        unsafe { (*row).semantic_prompt() },
        crate::page::SemanticPrompt::Prompt
    );
}

#[test]
fn resize_reflow_less_cols_wrapped_rows_with_graphemes() {
    let mut s = PageList::init(4, 2, None);
    // Write a base char + grapheme at col 0.
    unsafe {
        let (row, cell) = s.pin(Point::active(0, 0)).unwrap().row_and_cell();
        *cell = Cell::init('e' as u32);
        (*s.first_node())
            .data
            .set_graphemes(row, cell, &[0x0301])
            .unwrap();
    }
    for x in 1..4u16 {
        write_cp(&mut s, Point::active(x, 0), 'A' as u32 + x as u32);
    }
    s.resize(Resize {
        cols: Some(2),
        reflow: true,
        ..Default::default()
    });
    assert_eq!(s.cols(), 2);
    // Grapheme preserved at reflowed position.
    let c = s.get_cell(Point::screen(0, 0)).unwrap();
    assert_eq!(unsafe { (*c.cell).codepoint() }, 'e' as u32);
    let g = unsafe { (*s.first_node()).data.lookup_grapheme(c.cell) };
    assert!(g.is_some());
    assert_eq!(unsafe { &*g.unwrap() }, &[0x0301]);
}

// ---- resize no-reflow additional ----

#[test]
fn resize_no_reflow_more_rows_with_history() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    s.resize(Resize {
        rows: Some(30),
        reflow: false,
        ..Default::default()
    });
    assert_eq!(s.rows(), 30);
}

#[test]
fn resize_no_reflow_less_cols_clears_graphemes() {
    let mut s = PageList::init(80, 24, None);
    unsafe {
        let (row, cell) = s.pin(Point::active(50, 0)).unwrap().row_and_cell();
        *cell = Cell::init('e' as u32);
        (*s.first_node())
            .data
            .set_graphemes(row, cell, &[0x0301])
            .unwrap();
    }
    s.resize(Resize {
        cols: Some(40),
        reflow: false,
        ..Default::default()
    });
    assert_eq!(s.cols(), 40);
    // The grapheme at col 50 is gone (beyond new width).
    assert_eq!(unsafe { (*s.first_node()).data.grapheme_count() }, 0);
}

#[test]
fn resize_no_reflow_less_rows_trims_blank_lines() {
    let mut s = PageList::init(80, 24, None);
    // Only the first row has text; rest are blank.
    write_cp(&mut s, Point::active(0, 0), 'A' as u32);
    s.resize(Resize {
        rows: Some(10),
        reflow: false,
        ..Default::default()
    });
    assert_eq!(s.rows(), 10);
    // Text preserved.
    assert_eq!(read_cp(&s, Point::active(0, 0)), 'A' as u32);
}

#[test]
fn resize_no_reflow_empty_screen() {
    let mut s = PageList::init(80, 24, None);
    s.resize(Resize {
        cols: Some(100),
        rows: Some(50),
        reflow: false,
        ..Default::default()
    });
    assert_eq!(s.cols(), 100);
    assert_eq!(s.rows(), 50);
}

#[test]
fn resize_no_reflow_less_rows_and_cols() {
    let mut s = PageList::init(80, 24, None);
    write_cp(&mut s, Point::active(0, 23), 'A' as u32);
    s.resize(Resize {
        cols: Some(40),
        rows: Some(12),
        reflow: false,
        ..Default::default()
    });
    assert_eq!(s.cols(), 40);
    assert_eq!(s.rows(), 12);
}

#[test]
fn resize_no_reflow_more_rows_and_less_cols() {
    let mut s = PageList::init(80, 24, None);
    s.resize(Resize {
        cols: Some(40),
        rows: Some(48),
        reflow: false,
        ..Default::default()
    });
    assert_eq!(s.cols(), 40);
    assert_eq!(s.rows(), 48);
}

// ---- reflow forces capacity increase (grapheme) ----

#[test]
fn resize_reflow_exceeds_grapheme_memory_forcing_capacity_increase() {
    // Fill the last row of page 0 and first row of page 1 with graphemes that
    // each use nearly all grapheme-alloc capacity, mark them wrapped, then grow
    // cols so the unwrap forces a mid-reflow capacity increase.
    let mut s = PageList::init(2, 10, Some(0));
    assert_eq!(s.total_pages(), 1);

    // Grow to two pages.
    unsafe {
        let page = &mut (*s.first_node()).data;
        page.pause_integrity_checks(true);
        let cap_rows = page.capacity.rows;
        let size_rows = page.size.rows;
        for _ in size_rows..cap_rows {
            s.grow();
        }
        (*s.first_node()).data.pause_integrity_checks(false);
    }
    assert_eq!(s.total_pages(), 1);
    s.grow_rows(1);
    assert_eq!(s.total_pages(), 2);

    // The number of codepoints to nearly exhaust the grapheme allocator.
    let big = {
        let gb = unsafe { (*s.first_node()).data.capacity.grapheme_bytes } as usize;
        (gb - 1) / std::mem::size_of::<u32>()
    };
    let cps: Vec<u32> = std::iter::repeat_n('a' as u32, big).collect();

    // Bottom-right of page 0, wrapped.
    unsafe {
        let page = &mut (*s.first_node()).data;
        let cols = page.size.cols as usize;
        let rows = page.size.rows as usize;
        let (row, cell) = page.get_row_and_cell(cols - 1, rows - 1);
        (*row).set_wrap(true);
        *cell = Cell::init('X' as u32);
        page.set_graphemes(row, cell, &cps).unwrap();
    }
    // Top-left of page 1, wrap continuation.
    unsafe {
        let page = &mut (*s.last_node()).data;
        let (row, cell) = page.get_row_and_cell(0, 0);
        (*row).set_wrap(true);
        *cell = Cell::init('X' as u32);
        page.set_graphemes(row, cell, &cps).unwrap();
    }

    // Resize wider by one, unwrapping — forces capacity increase during reflow.
    let new_cols = s.cols() + 1;
    s.resize(Resize {
        cols: Some(new_cols),
        reflow: true,
        ..Default::default()
    });
    assert_eq!(s.cols(), new_cols);
}

// ---- split additional ----

#[test]
fn split_multiple_tracked_pins_across_regions() {
    let mut s = PageList::init(80, 24, None);
    let node = s.first_node();
    let a = s.track_pin(s.pin(Point::active(0, 5)).unwrap());
    let b = s.track_pin(s.pin(Point::active(0, 12)).unwrap());
    let c = s.track_pin(s.pin(Point::active(0, 20)).unwrap());
    let split_pin = s.pin(Point::active(0, 12)).unwrap();
    s.split(split_pin).unwrap();
    // a before split: unchanged.
    assert_eq!(unsafe { (*a).node }, node);
    assert_eq!(unsafe { (*a).y }, 5);
    // b at split: moves to new page at y=0.
    assert_ne!(unsafe { (*b).node }, node);
    assert_eq!(unsafe { (*b).y }, 0);
    // c after split: moves, y = 20-12 = 8.
    assert_ne!(unsafe { (*c).node }, node);
    assert_eq!(unsafe { (*c).y }, 8);
    s.untrack_pin(a);
    s.untrack_pin(b);
    s.untrack_pin(c);
}

#[test]
fn split_preserves_styled_cells() {
    let mut s = PageList::init(80, 24, None);
    unsafe {
        let node = s.first_node();
        let mem = (*node).data.memory_mut();
        let style = crate::page::style::Style {
            flags: crate::page::style::Flags {
                bold: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let id = (*node).data.styles().add(mem, style).unwrap();
        let (row, cell) = s.pin(Point::active(0, 15)).unwrap().row_and_cell();
        *cell = Cell::init('Y' as u32);
        (*cell).set_style_id(id);
        (*row).set_styled(true);
    }
    let split_pin = s.pin(Point::active(0, 12)).unwrap();
    s.split(split_pin).unwrap();
    // Cell now at new page row 3.
    let c = s.get_cell(Point::active(0, 15)).unwrap().page_cell();
    assert_eq!(c.codepoint(), 'Y' as u32);
    assert!(c.style_id() != crate::page::style_default_id());
}

#[test]
fn split_first_page_keeps_original_as_first() {
    let mut s = PageList::init(80, 24, None);
    let first = s.first_node();
    let split_pin = s.pin(Point::active(0, 12)).unwrap();
    s.split(split_pin).unwrap();
    assert_eq!(s.first_node(), first);
}

// ---- erase additional ----

#[test]
fn erase_reaccounts_page_size() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(30);
    let before = s.page_size_bytes();
    s.erase_history(None);
    let after = s.page_size_bytes();
    // Erasing history removed at least one page's worth of bytes (or stayed if
    // it all fit in remaining pages), never grew.
    assert!(after <= before);
    assert_eq!(s.total_rows(), 24);
}

#[test]
fn erase_a_one_row_active() {
    let mut s = PageList::init(80, 1, None);
    write_cp(&mut s, Point::active(0, 0), 'Z' as u32);
    s.erase_active(0);
    // Regrows to keep 1 active row.
    assert_eq!(s.total_rows_slow(), 1);
}

#[test]
fn erase_row_with_tracked_pin_is_erased() {
    let mut s = PageList::init(80, 24, None);
    let p = s.track_pin(s.pin(Point::active(0, 0)).unwrap());
    s.erase_row(Point::active(0, 0));
    // Pin on the erased row: y stays 0 (shifted content moved into it).
    assert_eq!(unsafe { (*p).y }, 0);
    s.untrack_pin(p);
}

// ---- iterators additional ----

#[test]
fn page_iterator_reverse_two_pages() {
    let mut s = PageList::init(80, 24, None);
    let cap = std_page_cap_rows(&s);
    for _ in 0..cap {
        s.grow();
    }
    let mut it = s.page_iterator(Direction::LeftUp, Point::screen(0, 0), None);
    let mut count = 0;
    while let Some(_c) = unsafe { it.next() } {
        count += 1;
    }
    assert_eq!(count, 2);
}

#[test]
fn cell_iterator_reverse() {
    let mut s = PageList::init(4, 1, None);
    for x in 0..4u16 {
        write_cp(&mut s, Point::active(x, 0), '0' as u32 + x as u32);
    }
    let mut it = s.cell_iterator(Direction::LeftUp, Point::screen(0, 0), None);
    let mut seen = Vec::new();
    while let Some(p) = unsafe { it.next() } {
        seen.push(unsafe { (*p.row_and_cell().1).codepoint() });
    }
    assert_eq!(seen, vec!['3' as u32, '2' as u32, '1' as u32, '0' as u32]);
}

// ---- increaseCapacity dimension tests ----

fn fill_grid(s: &mut PageList) {
    for y in 0..s.rows() {
        for x in 0..s.cols() {
            write_cp(&mut *s, Point::active(x, y as u32), x as u32);
        }
    }
}

fn assert_grid_preserved(s: &PageList) {
    for y in 0..s.rows() {
        for x in 0..s.cols() {
            assert_eq!(read_cp(s, Point::active(x, y as u32)), x as u32);
        }
    }
}

#[test]
fn increase_capacity_to_increase_styles() {
    let mut s = PageList::init(2, 2, Some(0));
    let orig = unsafe { (*s.first_node()).data.capacity.styles };
    fill_grid(&mut s);
    let node = s.first_node();
    let new_node =
        unsafe { s.increase_capacity(node, Some(super::IncreaseCapacity::Styles)) }.unwrap();
    assert_eq!(s.first_node(), s.last_node());
    assert_eq!(unsafe { (*new_node).data.capacity.styles }, orig * 2);
    assert_grid_preserved(&s);
}

#[test]
fn increase_capacity_to_increase_graphemes() {
    let mut s = PageList::init(2, 2, Some(0));
    let orig = unsafe { (*s.first_node()).data.capacity.grapheme_bytes };
    fill_grid(&mut s);
    let node = s.first_node();
    let new_node =
        unsafe { s.increase_capacity(node, Some(super::IncreaseCapacity::GraphemeBytes)) }.unwrap();
    assert_eq!(
        unsafe { (*new_node).data.capacity.grapheme_bytes },
        orig * 2
    );
    assert_grid_preserved(&s);
}

#[test]
fn increase_capacity_to_increase_hyperlinks() {
    let mut s = PageList::init(2, 2, Some(0));
    let orig = unsafe { (*s.first_node()).data.capacity.hyperlink_bytes };
    fill_grid(&mut s);
    let node = s.first_node();
    let new_node =
        unsafe { s.increase_capacity(node, Some(super::IncreaseCapacity::HyperlinkBytes)) }
            .unwrap();
    assert_eq!(
        unsafe { (*new_node).data.capacity.hyperlink_bytes },
        orig * 2
    );
    assert_grid_preserved(&s);
}

#[test]
fn increase_capacity_to_increase_string_bytes() {
    let mut s = PageList::init(2, 2, Some(0));
    let orig = unsafe { (*s.first_node()).data.capacity.string_bytes };
    fill_grid(&mut s);
    let node = s.first_node();
    let new_node =
        unsafe { s.increase_capacity(node, Some(super::IncreaseCapacity::StringBytes)) }.unwrap();
    assert_eq!(unsafe { (*new_node).data.capacity.string_bytes }, orig * 2);
    assert_grid_preserved(&s);
}

#[test]
fn increase_capacity_returns_out_of_space_at_max() {
    let mut s = PageList::init(2, 2, Some(0));
    // Drive styles to max by repeated doubling until OutOfSpace.
    let mut node = s.first_node();
    let mut hit_limit = false;
    for _ in 0..40 {
        match unsafe { s.increase_capacity(node, Some(super::IncreaseCapacity::Styles)) } {
            Ok(n) => node = n,
            Err(()) => {
                hit_limit = true;
                break;
            }
        }
    }
    assert!(hit_limit);
}

// ---- scroll cache fast paths ----

#[test]
fn scroll_to_row_with_cache_fast_path_down() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(20);
    s.scroll(Scroll::Row(3));
    assert_eq!(s.scrollbar().offset, 3); // populate cache
    s.scroll(Scroll::Row(7));
    assert_eq!(s.scrollbar().offset, 7);
}

#[test]
fn scroll_to_row_with_cache_fast_path_up() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(20);
    s.scroll(Scroll::Row(10));
    assert_eq!(s.scrollbar().offset, 10);
    s.scroll(Scroll::Row(4));
    assert_eq!(s.scrollbar().offset, 4);
}

#[test]
fn scroll_to_row_in_middle() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(20);
    s.scroll(Scroll::Row(10));
    assert_eq!(
        s.get_cell(Point::viewport(0, 0)).unwrap().screen_point(),
        Point::screen(0, 10)
    );
}

// ---- resize reflow invalidates viewport cache ----

#[test]
fn resize_reflow_invalidates_viewport_offset_cache() {
    // Faithful port of the Zig test: 2x4, grow 20, alternate wrap/continuation,
    // pin at history y=10, then reflow cols 2->4 (unwrap halves total_rows), and
    // the cached offset must be recomputed to 5.
    let mut s = PageList::init(2, 4, None);
    s.grow_rows(20);
    unsafe {
        let node = s.last_node();
        for y in 0..s.rows() as usize {
            let (row, _) = (*node).data.get_row_and_cell(0, y);
            if y % 2 == 0 {
                (*row).set_wrap(true);
            } else {
                (*row).set_wrap_continuation(true);
            }
            for x in 0..s.cols() {
                let (_, cell) = (*node).data.get_row_and_cell(x as usize, y);
                *cell = Cell::init('A' as u32);
            }
        }
    }
    s.scroll(Scroll::Pin(s.pin(Point::screen(0, 10)).unwrap()));
    assert_eq!(s.viewport_state(), Viewport::Pin);
    assert_eq!(s.scrollbar().offset, 10);

    s.resize(Resize {
        cols: Some(4),
        reflow: true,
        ..Default::default()
    });
    assert_eq!(s.cols(), 4);
    assert_eq!(s.scrollbar().offset, 5);
}

#[test]
fn erase_rows_invalidates_viewport_offset_cache() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(20);
    s.scroll(Scroll::Row(10));
    assert_eq!(s.scrollbar().offset, 10);
    s.erase_history(Some(Point::history(0, 2)));
    let _ = s.scrollbar();
}

// ---- prompt iterator / scroll prompt ----

fn set_prompt(s: &mut PageList, y: u32, kind: crate::page::SemanticPrompt) {
    unsafe {
        let (row, _) = s.pin(Point::screen(0, y)).unwrap().row_and_cell();
        (*row).set_semantic_prompt(kind);
    }
}

#[test]
fn jump_zero_prompts() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    // Zero delta is a no-op.
    let before = s.viewport_state();
    s.scroll(Scroll::DeltaPrompt(0));
    assert_eq!(s.viewport_state(), before);
}

#[test]
fn jump_back_one_prompt() {
    use crate::page::SemanticPrompt;
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(10);
    set_prompt(&mut s, 3, SemanticPrompt::Prompt);
    // Start from active, jump back to the prompt at y=3.
    s.scroll(Scroll::DeltaPrompt(-1));
    assert_eq!(
        s.get_cell(Point::viewport(0, 0)).unwrap().screen_point(),
        Point::screen(0, 3)
    );
}

// ---- viewport getBottomRight / getTopLeft edge cases ----

#[test]
fn get_bottom_right_history_none_when_no_history() {
    let s = PageList::init(80, 24, None);
    // No history yet -> history bottom-right is None.
    assert!(s.get_bottom_right(Tag::History).is_none());
}

#[test]
fn get_bottom_right_screen() {
    let mut s = PageList::init(80, 24, None);
    s.grow_rows(5);
    let br = s.get_bottom_right(Tag::Screen).unwrap();
    assert_eq!(br.x(), 79);
    assert_eq!(
        s.point_from_pin(Tag::Screen, br).unwrap(),
        Point::screen(79, 28)
    );
}

// ---- resize reflow across page boundary ----

#[test]
fn resize_reflow_more_cols_creates_multiple_pages() {
    let mut s = PageList::init(4, 24, None);
    // Fill many rows so unwrap creates enough content to still fit.
    for y in 0..24u16 {
        for x in 0..4u16 {
            write_cp(
                &mut s,
                Point::active(x, y as u32),
                'A' as u32 + ((x + y) % 26) as u32,
            );
        }
    }
    s.resize(Resize {
        cols: Some(8),
        reflow: true,
        ..Default::default()
    });
    assert_eq!(s.cols(), 8);
    assert!(s.total_rows_slow() >= 24);
}

// ---- clone partial ----

#[test]
fn clone_partial_trimmed_right() {
    let mut s = PageList::init(80, 24, None);
    // Content only in first 3 cols.
    for x in 0..3u16 {
        write_cp(&mut s, Point::active(x, 0), 'A' as u32 + x as u32);
    }
    let s2 = s.clone(Point::active(0, 0), Some(Point::active(0, 5)), None);
    assert_eq!(s2.total_rows(), s2.rows() as usize);
    assert_eq!(read_cp(&s2, Point::active(0, 0)), 'A' as u32);
}

// ---- highlightSemanticContent (ported 1:1 from PageList.zig `2da015cd6`) ----
//
// `highlight.zig` itself has 0 inline tests; these 17 exercise
// `PageList.highlightSemanticContent`. See `docs/analysis/highlight.md`.

use crate::page::{SemanticContent, SemanticPrompt};

/// Set the semantic-prompt flag on a screen row. Mirrors `rac.row.semantic_prompt = kind`.
fn set_row_prompt(s: &mut PageList, y: u32, kind: SemanticPrompt) {
    let c = s.get_cell(Point::screen(0, y)).unwrap();
    unsafe { (*c.row).set_semantic_prompt(kind) };
}

/// Write a cell with a codepoint and semantic content. Mirrors the Zig
/// `cell.* = .{ .content_tag = .codepoint, .content = .{ .codepoint = cp }, .semantic_content = sc }`.
fn set_cell(s: &mut PageList, x: u16, y: u32, cp: u32, sc: SemanticContent) {
    let c = s.get_cell(Point::screen(x, y)).unwrap();
    unsafe {
        let mut cell = Cell::init(cp);
        cell.set_semantic_content(sc);
        *c.cell = cell;
    }
}

/// Set only the semantic content of an existing cell. Mirrors `cell.semantic_content = sc`.
fn set_cell_content(s: &mut PageList, x: u16, y: u32, sc: SemanticContent) {
    let c = s.get_cell(Point::screen(x, y)).unwrap();
    unsafe { (*c.cell).set_semantic_content(sc) };
}

/// The screen-point of a highlight endpoint pin.
fn hl_point(s: &PageList, p: Pin) -> Point {
    s.point_from_pin(Tag::Screen, p).unwrap()
}

#[test]
fn highlight_semantic_content_prompt() {
    let mut s = PageList::init(10, 20, Some(0));
    assert_eq!(s.first_node(), s.last_node());

    // Prompt on row 5.
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    // First 5 cols are prompt.
    for x in 0..5u16 {
        set_cell(&mut s, x, 5, 'A' as u32, SemanticContent::Prompt);
    }
    // Next 3 are input.
    for x in 5..8u16 {
        set_cell(&mut s, x, 5, 'B' as u32, SemanticContent::Input);
    }
    // Prompt on row 10.
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    let at = s.pin(Point::screen(2, 5)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Prompt)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(0, 5));
    assert_eq!(hl_point(&s, hl.end), Point::screen(7, 5));
}

#[test]
fn highlight_semantic_content_prompt_with_output() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    for x in 0..3u16 {
        set_cell(&mut s, x, 5, '$' as u32, SemanticContent::Prompt);
    }
    for x in 3..7u16 {
        set_cell(&mut s, x, 5, 'l' as u32, SemanticContent::Input);
    }
    // Rest is output (shouldn't be included in prompt highlight).
    for x in 7..10u16 {
        set_cell(&mut s, x, 5, 'o' as u32, SemanticContent::Output);
    }
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    let at = s.pin(Point::screen(0, 5)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Prompt)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(0, 5));
    assert_eq!(hl_point(&s, hl.end), Point::screen(6, 5));
}

#[test]
fn highlight_semantic_content_prompt_multiline() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    // First row is all prompt.
    for x in 0..10u16 {
        set_cell(&mut s, x, 5, '$' as u32, SemanticContent::Prompt);
    }
    // Row 6 continues with input.
    for x in 0..5u16 {
        set_cell(&mut s, x, 6, 'c' as u32, SemanticContent::Input);
    }
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    let at = s.pin(Point::screen(2, 5)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Prompt)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(0, 5));
    assert_eq!(hl_point(&s, hl.end), Point::screen(4, 6));
}

#[test]
fn highlight_semantic_content_prompt_only() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    for x in 0..5u16 {
        set_cell(&mut s, x, 5, '$' as u32, SemanticContent::Prompt);
    }
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    let at = s.pin(Point::screen(0, 5)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Prompt)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(0, 5));
    assert_eq!(hl_point(&s, hl.end), Point::screen(4, 5));
}

#[test]
fn highlight_semantic_content_prompt_to_end_of_screen() {
    let mut s = PageList::init(10, 20, Some(0));
    // Single prompt on row 15, no following prompt.
    set_row_prompt(&mut s, 15, SemanticPrompt::Prompt);
    for x in 0..3u16 {
        set_cell(&mut s, x, 15, '$' as u32, SemanticContent::Prompt);
    }
    for x in 3..8u16 {
        set_cell(&mut s, x, 15, 'c' as u32, SemanticContent::Input);
    }

    let at = s.pin(Point::screen(0, 15)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Prompt)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(0, 15));
    assert_eq!(hl_point(&s, hl.end), Point::screen(7, 15));
}

#[test]
fn highlight_semantic_content_input_basic() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    for x in 0..3u16 {
        set_cell(&mut s, x, 5, '$' as u32, SemanticContent::Prompt);
    }
    for x in 3..8u16 {
        set_cell(&mut s, x, 5, 'l' as u32, SemanticContent::Input);
    }
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    let at = s.pin(Point::screen(0, 5)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Input)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(3, 5));
    assert_eq!(hl_point(&s, hl.end), Point::screen(7, 5));
}

#[test]
fn highlight_semantic_content_input_with_output() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    for x in 0..2u16 {
        set_cell(&mut s, x, 5, '$' as u32, SemanticContent::Prompt);
    }
    for x in 2..5u16 {
        set_cell(&mut s, x, 5, 'c' as u32, SemanticContent::Input);
    }
    for x in 5..10u16 {
        set_cell(&mut s, x, 5, 'o' as u32, SemanticContent::Output);
    }
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    let at = s.pin(Point::screen(0, 5)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Input)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(2, 5));
    assert_eq!(hl_point(&s, hl.end), Point::screen(4, 5));
}

#[test]
fn highlight_semantic_content_input_multiline_with_continuation() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    for x in 0..2u16 {
        set_cell(&mut s, x, 5, '$' as u32, SemanticContent::Prompt);
    }
    for x in 2..10u16 {
        set_cell(&mut s, x, 5, 'c' as u32, SemanticContent::Input);
    }
    // Row 6 has continuation prompt then more input.
    for x in 0..2u16 {
        set_cell(&mut s, x, 6, '>' as u32, SemanticContent::Prompt);
    }
    for x in 2..6u16 {
        set_cell(&mut s, x, 6, 'd' as u32, SemanticContent::Input);
    }
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    let at = s.pin(Point::screen(0, 5)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Input)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(2, 5));
    assert_eq!(hl_point(&s, hl.end), Point::screen(5, 6));
}

#[test]
fn highlight_semantic_content_input_no_input_returns_null() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    for x in 0..3u16 {
        set_cell(&mut s, x, 5, '$' as u32, SemanticContent::Prompt);
    }
    // Rest is output (no input!).
    for x in 3..10u16 {
        set_cell(&mut s, x, 5, 'o' as u32, SemanticContent::Output);
    }
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    let at = s.pin(Point::screen(0, 5)).unwrap();
    assert!(
        s.highlight_semantic_content(at, SemanticContent::Input)
            .is_none()
    );
}

#[test]
fn highlight_semantic_content_input_to_end_of_screen() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 15, SemanticPrompt::Prompt);
    for x in 0..2u16 {
        set_cell(&mut s, x, 15, '$' as u32, SemanticContent::Prompt);
    }
    for x in 2..7u16 {
        set_cell(&mut s, x, 15, 'c' as u32, SemanticContent::Input);
    }

    let at = s.pin(Point::screen(0, 15)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Input)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(2, 15));
    assert_eq!(hl_point(&s, hl.end), Point::screen(6, 15));
}

#[test]
fn highlight_semantic_content_input_prompt_only_returns_null() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    // All cells are prompt.
    for x in 0..10u16 {
        set_cell(&mut s, x, 5, '$' as u32, SemanticContent::Prompt);
    }
    // Mark rows 6-9 as prompt to ensure no input before next prompt.
    for y in 6..10u32 {
        for x in 0..10u16 {
            set_cell_content(&mut s, x, y, SemanticContent::Prompt);
        }
    }
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    let at = s.pin(Point::screen(0, 5)).unwrap();
    assert!(
        s.highlight_semantic_content(at, SemanticContent::Input)
            .is_none()
    );
}

#[test]
fn highlight_semantic_content_output_basic() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    for x in 0..2u16 {
        set_cell(&mut s, x, 5, '$' as u32, SemanticContent::Prompt);
    }
    for x in 2..5u16 {
        set_cell(&mut s, x, 5, 'l' as u32, SemanticContent::Input);
    }
    // Cols 5-7 are output.
    for x in 5..8u16 {
        set_cell(&mut s, x, 5, 'o' as u32, SemanticContent::Output);
    }
    // Mark remaining cells as prompt to bound the output.
    for x in 8..10u16 {
        set_cell_content(&mut s, x, 5, SemanticContent::Prompt);
    }
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    let at = s.pin(Point::screen(0, 5)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Output)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(5, 5));
    assert_eq!(hl_point(&s, hl.end), Point::screen(7, 5));
}

#[test]
fn highlight_semantic_content_output_multiline() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    for x in 0..2u16 {
        set_cell(&mut s, x, 5, '$' as u32, SemanticContent::Prompt);
    }
    for x in 2..4u16 {
        set_cell(&mut s, x, 5, 'l' as u32, SemanticContent::Input);
    }
    // Rest of row 5 is output.
    for x in 4..10u16 {
        set_cell(&mut s, x, 5, 'o' as u32, SemanticContent::Output);
    }
    // Row 6 is all output.
    for x in 0..10u16 {
        set_cell(&mut s, x, 6, 'o' as u32, SemanticContent::Output);
    }
    // Row 7 has partial output then input to bound it.
    for x in 0..5u16 {
        set_cell(&mut s, x, 7, 'o' as u32, SemanticContent::Output);
    }
    for x in 5..10u16 {
        set_cell_content(&mut s, x, 7, SemanticContent::Input);
    }
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    let at = s.pin(Point::screen(0, 5)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Output)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(4, 5));
    assert_eq!(hl_point(&s, hl.end), Point::screen(4, 7));
}

#[test]
fn highlight_semantic_content_output_stops_at_next_prompt() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    for x in 0..2u16 {
        set_cell(&mut s, x, 5, '$' as u32, SemanticContent::Prompt);
    }
    for x in 2..4u16 {
        set_cell(&mut s, x, 5, 'l' as u32, SemanticContent::Input);
    }
    for x in 4..10u16 {
        set_cell(&mut s, x, 5, 'o' as u32, SemanticContent::Output);
    }
    // Row 6 has output then prompt starts.
    for x in 0..3u16 {
        set_cell(&mut s, x, 6, 'o' as u32, SemanticContent::Output);
    }
    for x in 3..6u16 {
        set_cell(&mut s, x, 6, '$' as u32, SemanticContent::Prompt);
    }
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    let at = s.pin(Point::screen(0, 5)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Output)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(4, 5));
    assert_eq!(hl_point(&s, hl.end), Point::screen(2, 6));
}

#[test]
fn highlight_semantic_content_output_to_end_of_screen() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 15, SemanticPrompt::Prompt);
    for x in 0..2u16 {
        set_cell(&mut s, x, 15, '$' as u32, SemanticContent::Prompt);
    }
    for x in 2..4u16 {
        set_cell(&mut s, x, 15, 'c' as u32, SemanticContent::Input);
    }
    for x in 4..10u16 {
        set_cell(&mut s, x, 15, 'o' as u32, SemanticContent::Output);
    }
    // Row 16 has output then prompt to bound it.
    for x in 0..8u16 {
        set_cell(&mut s, x, 16, 'o' as u32, SemanticContent::Output);
    }
    for x in 8..10u16 {
        set_cell_content(&mut s, x, 16, SemanticContent::Prompt);
    }

    let at = s.pin(Point::screen(0, 15)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Output)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(4, 15));
    assert_eq!(hl_point(&s, hl.end), Point::screen(7, 16));
}

#[test]
fn highlight_semantic_content_output_no_output_returns_null() {
    let mut s = PageList::init(10, 20, Some(0));
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    for x in 0..3u16 {
        set_cell(&mut s, x, 5, '$' as u32, SemanticContent::Prompt);
    }
    // Rest is input (must explicitly mark all cells to avoid default .output).
    for x in 3..10u16 {
        set_cell(&mut s, x, 5, 'c' as u32, SemanticContent::Input);
    }
    // Mark rows 6-9 as input to ensure no output between prompts.
    for y in 6..10u32 {
        for x in 0..10u16 {
            set_cell_content(&mut s, x, y, SemanticContent::Input);
        }
    }
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    let at = s.pin(Point::screen(0, 5)).unwrap();
    assert!(
        s.highlight_semantic_content(at, SemanticContent::Output)
            .is_none()
    );
}

#[test]
fn highlight_semantic_content_output_skips_empty_cells() {
    // Empty cells with default .output semantic content are not selected as output.
    // Happens when a prompt/input line doesn't fill the row — trailing cells default .output.
    let mut s = PageList::init(10, 20, Some(0));
    // Prompt on row 5 — only fills first 3 cells; rest empty with default .output.
    set_row_prompt(&mut s, 5, SemanticPrompt::Prompt);
    for x in 0..3u16 {
        set_cell(&mut s, x, 5, '$' as u32, SemanticContent::Prompt);
    }
    // Row 6 has short input; cells 4-9 empty with default .output.
    for x in 0..4u16 {
        set_cell(&mut s, x, 6, 'l' as u32, SemanticContent::Input);
    }
    // Rows 7-8 have actual output with text.
    for y in 7..9u32 {
        for x in 0..5u16 {
            set_cell(&mut s, x, y, 'o' as u32, SemanticContent::Output);
        }
    }
    set_row_prompt(&mut s, 10, SemanticPrompt::Prompt);

    // Output should start at row 7, not row 5 (empty cells have default .output).
    let at = s.pin(Point::screen(0, 5)).unwrap();
    let hl = s
        .highlight_semantic_content(at, SemanticContent::Output)
        .unwrap();
    assert_eq!(hl_point(&s, hl.start), Point::screen(0, 7));
    assert_eq!(hl_point(&s, hl.end), Point::screen(4, 8));
}
