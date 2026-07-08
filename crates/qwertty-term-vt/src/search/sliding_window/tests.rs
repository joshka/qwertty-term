//! Inline tests ported 1:1 from `src/terminal/search/sliding_window.zig` (commit
//! `2da015cd6`). 24 tests, no consolidation.
//!
//! Zig vt-stream setups that print `\r\n` map to `carriage_return()` + `linefeed()`; the
//! soft-wrapped tests drive a `Terminal` the same way. Needle-swap tests use the
//! `#[cfg(test)]` `test_change_needle` helper (mirrors Zig's `testChangeNeedle`).

use super::{Direction, SlidingWindow};
use crate::pagelist::Node;
use crate::point::{Coordinate, Point, Tag};
use crate::screen::{Options as ScreenOptions, Screen};
use crate::terminal::{Options as TermOptions, Terminal};

fn screen(cols: u16, rows: u16, max_scrollback: usize) -> Screen {
    Screen::init(ScreenOptions {
        cols,
        rows,
        max_scrollback,
    })
}

fn term(cols: u16, rows: u16) -> Terminal {
    Terminal::new(TermOptions {
        cols,
        rows,
        max_scrollback: 0,
        colors: Default::default(),
    })
}

/// Assert a highlight's untracked start/end map to the given active points.
fn expect_active(s: &Screen, h: &crate::highlight::Flattened, sx: u16, sy: u16, ex: u16, ey: u16) {
    let sel = h.untracked();
    assert_eq!(
        s.pages.point_from_pin(Tag::Active, sel.start),
        Some(Point::new(
            Tag::Active,
            Coordinate {
                x: sx,
                y: sy as u32
            }
        ))
    );
    assert_eq!(
        s.pages.point_from_pin(Tag::Active, sel.end),
        Some(Point::new(
            Tag::Active,
            Coordinate {
                x: ex,
                y: ey as u32
            }
        ))
    );
}

#[test]
fn empty_on_init() {
    let w = SlidingWindow::init(Direction::Forward, b"boo!");
    assert_eq!(w.data_len(), 0);
    assert_eq!(w.meta_len(), 0);
}

#[test]
fn single_append() {
    let mut w = SlidingWindow::init(Direction::Forward, b"boo!");
    let mut s = screen(80, 24, 0);
    s.test_write_string("hello. boo! hello. boo!");

    assert_eq!(s.pages.first_node(), s.pages.last_node());
    let node = s.pages.first_node();
    w.append(node);

    let h = w.next().unwrap();
    expect_active(&s, &h, 7, 0, 10, 0);
    let h = w.next().unwrap();
    expect_active(&s, &h, 19, 0, 22, 0);
    assert!(w.next().is_none());
    assert!(w.next().is_none());
}

#[test]
fn single_append_case_insensitive_ascii() {
    let mut w = SlidingWindow::init(Direction::Forward, b"Boo!");
    let mut s = screen(80, 24, 0);
    s.test_write_string("hello. boo! hello. boo!");

    assert_eq!(s.pages.first_node(), s.pages.last_node());
    let node = s.pages.first_node();
    w.append(node);

    let h = w.next().unwrap();
    expect_active(&s, &h, 7, 0, 10, 0);
    let h = w.next().unwrap();
    expect_active(&s, &h, 19, 0, 22, 0);
    assert!(w.next().is_none());
    assert!(w.next().is_none());
}

#[test]
fn single_append_single_char() {
    let mut w = SlidingWindow::init(Direction::Forward, b"b");
    let mut s = screen(80, 24, 0);
    s.test_write_string("hello. boo! hello. boo!");

    assert_eq!(s.pages.first_node(), s.pages.last_node());
    let node = s.pages.first_node();
    w.append(node);

    let h = w.next().unwrap();
    expect_active(&s, &h, 7, 0, 7, 0);
    let h = w.next().unwrap();
    expect_active(&s, &h, 19, 0, 19, 0);
    assert!(w.next().is_none());
    assert!(w.next().is_none());
}

#[test]
fn single_append_no_match() {
    let mut w = SlidingWindow::init(Direction::Forward, b"nope!");
    let mut s = screen(80, 24, 0);
    s.test_write_string("hello. boo! hello. boo!");

    assert_eq!(s.pages.first_node(), s.pages.last_node());
    let node = s.pages.first_node();
    w.append(node);

    assert!(w.next().is_none());
    assert!(w.next().is_none());
    assert_eq!(w.meta_len(), 1);
}

/// Fill the first page so its final bytes are "boo!", spill onto a second page ending
/// "hello. boo!". Returns the first node.
fn two_page_boo(s: &mut Screen) -> *mut Node {
    let first_page_rows = unsafe { (*s.pages.first_node()).data.capacity.rows } as usize;
    for _ in 0..first_page_rows - 1 {
        s.test_write_string("\n");
    }
    for _ in 0..(s.pages.cols() as usize - 4) {
        s.test_write_string("x");
    }
    s.test_write_string("boo!");
    assert_eq!(s.pages.first_node(), s.pages.last_node());
    s.test_write_string("\n");
    assert_ne!(s.pages.first_node(), s.pages.last_node());
    s.test_write_string("hello. boo!");
    s.pages.first_node()
}

#[test]
fn two_pages() {
    let mut w = SlidingWindow::init(Direction::Forward, b"boo!");
    let mut s = screen(80, 24, 1000);
    let node = two_page_boo(&mut s);
    w.append(node);
    w.append(unsafe { (*node).next });

    let h = w.next().unwrap();
    expect_active(&s, &h, 76, 22, 79, 22);
    let h = w.next().unwrap();
    expect_active(&s, &h, 7, 23, 10, 23);
    assert!(w.next().is_none());
    assert!(w.next().is_none());
}

#[test]
fn two_pages_single_char() {
    let mut w = SlidingWindow::init(Direction::Forward, b"b");
    let mut s = screen(80, 24, 1000);
    let node = two_page_boo(&mut s);
    w.append(node);
    w.append(unsafe { (*node).next });

    let h = w.next().unwrap();
    expect_active(&s, &h, 76, 22, 76, 22);
    let h = w.next().unwrap();
    expect_active(&s, &h, 7, 23, 7, 23);
    assert!(w.next().is_none());
    assert!(w.next().is_none());
}

#[test]
fn two_pages_match_across_boundary() {
    let mut w = SlidingWindow::init(Direction::Forward, b"hello, world");
    let mut s = screen(80, 24, 1000);

    let first_page_rows = unsafe { (*s.pages.first_node()).data.capacity.rows } as usize;
    for _ in 0..first_page_rows - 1 {
        s.test_write_string("\n");
    }
    for _ in 0..(s.pages.cols() as usize - 4) {
        s.test_write_string("x");
    }
    s.test_write_string("hell");
    assert_eq!(s.pages.first_node(), s.pages.last_node());
    s.test_write_string("o, world!");
    assert_ne!(s.pages.first_node(), s.pages.last_node());

    let node = s.pages.first_node();
    w.append(node);
    w.append(unsafe { (*node).next });

    let h = w.next().unwrap();
    expect_active(&s, &h, 76, 22, 7, 23);
    assert!(w.next().is_none());
    assert!(w.next().is_none());
    assert_eq!(w.meta_len(), 2);
}

#[test]
fn two_pages_no_match_across_boundary_with_newline() {
    let mut w = SlidingWindow::init(Direction::Forward, b"hello, world");
    let mut s = screen(80, 24, 1000);

    let first_page_rows = unsafe { (*s.pages.first_node()).data.capacity.rows } as usize;
    for _ in 0..first_page_rows - 1 {
        s.test_write_string("\n");
    }
    for _ in 0..(s.pages.cols() as usize - 4) {
        s.test_write_string("x");
    }
    s.test_write_string("hell");
    assert_eq!(s.pages.first_node(), s.pages.last_node());
    s.test_write_string("\no, world!");
    assert_ne!(s.pages.first_node(), s.pages.last_node());

    let node = s.pages.first_node();
    w.append(node);
    w.append(unsafe { (*node).next });

    assert!(w.next().is_none());
    assert!(w.next().is_none());
    assert_eq!(w.meta_len(), 2);
}

#[test]
fn two_pages_no_match_across_boundary_with_newline_reverse() {
    let mut w = SlidingWindow::init(Direction::Reverse, b"hello, world");
    let mut s = screen(80, 24, 1000);

    let first_page_rows = unsafe { (*s.pages.first_node()).data.capacity.rows } as usize;
    for _ in 0..first_page_rows - 1 {
        s.test_write_string("\n");
    }
    for _ in 0..(s.pages.cols() as usize - 4) {
        s.test_write_string("x");
    }
    s.test_write_string("hell");
    assert_eq!(s.pages.first_node(), s.pages.last_node());
    s.test_write_string("\no, world!");
    assert_ne!(s.pages.first_node(), s.pages.last_node());

    let node = s.pages.first_node();
    w.append(unsafe { (*node).next });
    w.append(node);

    assert!(w.next().is_none());
    assert!(w.next().is_none());
}

#[test]
fn two_pages_no_match_prunes_first_page() {
    let mut w = SlidingWindow::init(Direction::Forward, b"nope!");
    let mut s = screen(80, 24, 1000);
    let node = two_page_boo(&mut s);
    w.append(node);
    w.append(unsafe { (*node).next });

    assert!(w.next().is_none());
    assert!(w.next().is_none());
    // Pruned the first page because the second has enough text to fit the needle.
    assert_eq!(w.meta_len(), 1);
}

#[test]
fn two_pages_no_match_keeps_both_pages() {
    let mut s = screen(80, 24, 1000);
    let node = two_page_boo(&mut s);

    let first_page_rows = unsafe { (*s.pages.first_node()).data.capacity.rows } as usize;
    // Imaginary needle that doesn't match, sized so both pages are needed.
    let needle = vec![b'x'; first_page_rows * s.pages.cols() as usize];

    let mut w = SlidingWindow::init(Direction::Forward, &needle);
    w.append(node);
    w.append(unsafe { (*node).next });

    assert!(w.next().is_none());
    assert!(w.next().is_none());
    assert_eq!(w.meta_len(), 2);
}

#[test]
fn single_append_across_circular_buffer_boundary() {
    let mut w = SlidingWindow::init(Direction::Forward, b"abc");
    let mut s = screen(80, 24, 0);
    s.test_write_string("XXXXXXXXXXXXXXXXXXXboo!XXXXX");

    assert_eq!(s.pages.first_node(), s.pages.last_node());
    let node = s.pages.first_node();
    w.append(node);
    w.append(node);
    {
        let (a, b) = w.data_slice_lens();
        assert!(a > 0);
        assert_eq!(b, 0);
    }

    assert!(w.next().is_none());
    assert_eq!(w.meta_len(), 1);

    w.test_change_needle(b"boo");

    w.append(node);
    {
        let (a, b) = w.data_slice_lens();
        assert!(a > 0);
        assert!(b > 0);
    }
    let h = w.next().unwrap();
    expect_active(&s, &h, 19, 0, 21, 0);
    assert!(w.next().is_none());
}

#[test]
fn single_append_match_on_boundary() {
    let mut w = SlidingWindow::init(Direction::Forward, b"abcd");
    let mut s = screen(80, 24, 0);
    s.test_write_string("o!XXXXXXXXXXXXXXXXXXXbo");

    assert_eq!(s.pages.first_node(), s.pages.last_node());
    let node = s.pages.first_node();
    // Surgically mark the last row soft-wrapped.
    let page = unsafe { &(*node).data };
    let last_row = page.get_row(page.size.rows as usize - 1);
    unsafe { (*last_row).set_wrap(true) };

    w.append(node);
    w.append(node);
    {
        let (a, b) = w.data_slice_lens();
        assert!(a > 0);
        assert_eq!(b, 0);
    }

    assert!(w.next().is_none());
    assert_eq!(w.meta_len(), 1);

    w.test_change_needle(b"boo!");

    w.append(node);
    {
        let (a, b) = w.data_slice_lens();
        assert!(a > 0);
        assert!(b > 0);
    }
    let h = w.next().unwrap();
    expect_active(&s, &h, 21, 0, 1, 0);
    assert!(w.next().is_none());
}

#[test]
fn single_append_reversed() {
    let mut w = SlidingWindow::init(Direction::Reverse, b"boo!");
    let mut s = screen(80, 24, 0);
    s.test_write_string("hello. boo! hello. boo!");

    assert_eq!(s.pages.first_node(), s.pages.last_node());
    let node = s.pages.first_node();
    w.append(node);

    let h = w.next().unwrap();
    expect_active(&s, &h, 19, 0, 22, 0);
    let h = w.next().unwrap();
    expect_active(&s, &h, 7, 0, 10, 0);
    assert!(w.next().is_none());
    assert!(w.next().is_none());
}

#[test]
fn single_append_no_match_reversed() {
    let mut w = SlidingWindow::init(Direction::Reverse, b"nope!");
    let mut s = screen(80, 24, 0);
    s.test_write_string("hello. boo! hello. boo!");

    assert_eq!(s.pages.first_node(), s.pages.last_node());
    let node = s.pages.first_node();
    w.append(node);

    assert!(w.next().is_none());
    assert!(w.next().is_none());
    assert_eq!(w.meta_len(), 1);
}

#[test]
fn two_pages_reversed() {
    let mut w = SlidingWindow::init(Direction::Reverse, b"boo!");
    let mut s = screen(80, 24, 1000);
    let node = two_page_boo(&mut s);
    w.append(unsafe { (*node).next });
    w.append(node);

    let h = w.next().unwrap();
    expect_active(&s, &h, 7, 23, 10, 23);
    let h = w.next().unwrap();
    expect_active(&s, &h, 76, 22, 79, 22);
    assert!(w.next().is_none());
    assert!(w.next().is_none());
}

#[test]
fn two_pages_match_across_boundary_reversed() {
    let mut w = SlidingWindow::init(Direction::Reverse, b"hello, world");
    let mut s = screen(80, 24, 1000);

    let first_page_rows = unsafe { (*s.pages.first_node()).data.capacity.rows } as usize;
    for _ in 0..first_page_rows - 1 {
        s.test_write_string("\n");
    }
    for _ in 0..(s.pages.cols() as usize - 4) {
        s.test_write_string("x");
    }
    s.test_write_string("hell");
    assert_eq!(s.pages.first_node(), s.pages.last_node());
    s.test_write_string("o, world!");
    assert_ne!(s.pages.first_node(), s.pages.last_node());

    let node = s.pages.first_node();
    w.append(unsafe { (*node).next });
    w.append(node);

    let h = w.next().unwrap();
    expect_active(&s, &h, 76, 22, 7, 23);
    assert!(w.next().is_none());
    assert!(w.next().is_none());
    // In reverse mode the last-appended meta (first original page) holds needle.len-1 bytes,
    // so pruning occurs.
    assert_eq!(w.meta_len(), 1);
}

#[test]
fn two_pages_no_match_prunes_first_page_reversed() {
    let mut w = SlidingWindow::init(Direction::Reverse, b"nope!");
    let mut s = screen(80, 24, 1000);
    let node = two_page_boo(&mut s);
    w.append(unsafe { (*node).next });
    w.append(node);

    assert!(w.next().is_none());
    assert!(w.next().is_none());
    assert_eq!(w.meta_len(), 1);
}

#[test]
fn two_pages_no_match_keeps_both_pages_reversed() {
    let mut s = screen(80, 24, 1000);
    let node = two_page_boo(&mut s);

    let first_page_rows = unsafe { (*s.pages.first_node()).data.capacity.rows } as usize;
    let needle = vec![b'x'; first_page_rows * s.pages.cols() as usize];

    let mut w = SlidingWindow::init(Direction::Reverse, &needle);
    w.append(unsafe { (*node).next });
    w.append(node);

    assert!(w.next().is_none());
    assert!(w.next().is_none());
    assert_eq!(w.meta_len(), 2);
}

#[test]
fn single_append_across_circular_buffer_boundary_reversed() {
    let mut w = SlidingWindow::init(Direction::Reverse, b"abc");
    let mut s = screen(80, 24, 0);
    s.test_write_string("XXXXXXXXXXXXXXXXXXXboo!XXXXX");

    assert_eq!(s.pages.first_node(), s.pages.last_node());
    let node = s.pages.first_node();
    w.append(node);
    w.append(node);
    {
        let (a, b) = w.data_slice_lens();
        assert!(a > 0);
        assert_eq!(b, 0);
    }

    assert!(w.next().is_none());
    assert_eq!(w.meta_len(), 1);

    // test_change_needle doesn't reverse, so pass the reversed needle for reverse mode.
    w.test_change_needle(b"oob");

    w.append(node);
    {
        let (a, b) = w.data_slice_lens();
        assert!(a > 0);
        assert!(b > 0);
    }
    let h = w.next().unwrap();
    expect_active(&s, &h, 19, 0, 21, 0);
    assert!(w.next().is_none());
}

#[test]
fn single_append_match_on_boundary_reversed() {
    let mut w = SlidingWindow::init(Direction::Reverse, b"abcd");
    let mut s = screen(80, 24, 0);
    s.test_write_string("o!XXXXXXXXXXXXXXXXXXXbo");

    assert_eq!(s.pages.first_node(), s.pages.last_node());
    let node = s.pages.first_node();
    let page = unsafe { &(*node).data };
    let last_row = page.get_row(page.size.rows as usize - 1);
    unsafe { (*last_row).set_wrap(true) };

    w.append(node);
    w.append(node);
    {
        let (a, b) = w.data_slice_lens();
        assert!(a > 0);
        assert_eq!(b, 0);
    }

    assert!(w.next().is_none());
    assert_eq!(w.meta_len(), 1);

    w.test_change_needle(b"!oob");

    w.append(node);
    {
        let (a, b) = w.data_slice_lens();
        assert!(a > 0);
        assert!(b > 0);
    }
    let h = w.next().unwrap();
    expect_active(&s, &h, 21, 0, 1, 0);
    assert!(w.next().is_none());
}

#[test]
fn single_append_soft_wrapped() {
    let mut w = SlidingWindow::init(Direction::Forward, b"boo!");
    let mut t = term(4, 5);
    t.print_string("A");
    t.carriage_return();
    t.linefeed();
    t.print_string("xxboo!");
    t.carriage_return();
    t.linefeed();
    t.print_string("C");

    let s = t.screen();
    assert_eq!(s.pages.first_node(), s.pages.last_node());
    let node = s.pages.first_node();
    w.append(node);

    let h = w.next().unwrap();
    expect_active(s, &h, 2, 1, 1, 2);
    assert!(w.next().is_none());
    assert!(w.next().is_none());
}

#[test]
fn single_append_reversed_soft_wrapped() {
    let mut w = SlidingWindow::init(Direction::Reverse, b"boo!");
    let mut t = term(4, 5);
    t.print_string("A");
    t.carriage_return();
    t.linefeed();
    t.print_string("xxboo!");
    t.carriage_return();
    t.linefeed();
    t.print_string("C");

    let s = t.screen();
    assert_eq!(s.pages.first_node(), s.pages.last_node());
    let node = s.pages.first_node();
    w.append(node);

    let h = w.next().unwrap();
    expect_active(s, &h, 2, 1, 1, 2);
    assert!(w.next().is_none());
    assert!(w.next().is_none());
}

// Tests a real bug: a whitespace-only page that encodes to zero bytes would crash.
#[test]
fn append_whitespace_only_node() {
    let mut w = SlidingWindow::init(Direction::Forward, b"x");
    let s = screen(80, 24, 0);

    // Setting the empty page to wrap yields a zero-byte page.
    let node = s.pages.first_node();
    let page = unsafe { &(*node).data };
    let last_row = page.get_row(page.size.rows as usize - 1);
    unsafe { (*last_row).set_wrap(true) };

    assert_eq!(s.pages.first_node(), s.pages.last_node());
    w.append(node);
    assert!(w.next().is_none());
}
