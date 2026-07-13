//! Inline tests ported 1:1 from `src/terminal/search/pagelist.zig` (commit `2da015cd6`).
//! 6 tests.
//!
//! Zig `vtStream().nextSlice("...\r\n...")` maps to `print` + `carriage_return()` +
//! `linefeed()`.

use super::PageListSearch;
use crate::pagelist::PageList;
use crate::point::{Coordinate, Point, Tag};
use crate::screen::selection::Selection;
use crate::terminal::{Options as TermOptions, Terminal};

fn term(cols: u16, rows: u16) -> Terminal {
    Terminal::new(TermOptions {
        cols,
        rows,
        max_scrollback: 10_000,
        colors: Default::default(),
    })
}

fn feed(t: &mut Terminal, text: &str) {
    for c in text.chars() {
        match c {
            '\r' => {}
            '\n' => {
                t.carriage_return();
                t.linefeed();
            }
            _ => t.print(c as u32),
        }
    }
}

fn expect_active(
    t: &Terminal,
    h: &crate::highlight::Flattened,
    sx: u16,
    sy: u16,
    ex: u16,
    ey: u16,
) {
    let sel = h.untracked();
    let pages = &t.screen().pages;
    assert_eq!(
        pages.point_from_pin(Tag::Active, sel.start),
        Some(Point::new(
            Tag::Active,
            Coordinate {
                x: sx,
                y: sy as u32
            }
        ))
    );
    assert_eq!(
        pages.point_from_pin(Tag::Active, sel.end),
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
fn simple_search() {
    let mut t = term(10, 10);
    feed(&mut t, "Fizz\r\nBuzz\r\nFizz\r\nBang");

    let start = t.screen().pages.last_node();
    let mut search = PageListSearch::init(b"Fizz", &mut t.screen_mut().pages, start);

    let h = search.next().unwrap();
    expect_active(&t, &h, 0, 2, 3, 2);
    let h = search.next().unwrap();
    expect_active(&t, &h, 0, 0, 3, 0);
    assert!(search.next().is_none());

    // Single page: nothing more to feed.
    assert!(!search.feed());

    search.deinit(&mut t.screen_mut().pages);
}

// Regression (upstream 5d8eb78b7): `feed` must reset the tracked pin's x/y to
// the new node's bottom-right cell. A preceding page made shorter by a split
// would otherwise leave the pin out of bounds and trip the PageList integrity
// check on the next operation.
#[test]
fn feed_keeps_pin_within_shorter_page() {
    use crate::pagelist::Pin;

    let mut pages = PageList::init(10, 2, None);

    // Fill the first page to capacity, then allocate and fill a second page.
    // (Explicit `loop`/break rather than `while cond`: `grow()` mutates the page
    // through a raw pointer, which clippy's while_immutable_condition can't see.)
    let first = pages.first_node();
    loop {
        let (rows, cap) = unsafe { ((*first).data.size.rows, (*first).data.capacity.rows) };
        if rows >= cap {
            break;
        }
        pages.grow();
    }
    pages.grow(); // last page full -> creates the second page
    let second = pages.last_node();
    loop {
        let (rows, cap) = unsafe { ((*second).data.size.rows, (*second).data.capacity.rows) };
        if rows >= cap {
            break;
        }
        pages.grow();
    }

    // Split the first page at row 1 so `first` becomes a shorter intermediate page.
    pages.split(Pin::with(first, 1, 0)).unwrap();
    let shorter = unsafe { (*first).next };
    assert!(
        unsafe { (*shorter).data.size.rows } < unsafe { (*second).data.size.rows },
        "the split should leave a shorter intermediate page"
    );

    // Search from the last page and feed once: the pin advances into the
    // shorter page and must remain valid (this panicked before the fix).
    let mut search = PageListSearch::init(b"x", &mut pages, second);
    assert!(search.feed());
    assert_eq!(unsafe { (*search.pin).node }, shorter);
    assert!(pages.pin_is_valid(unsafe { *search.pin }));
    search.deinit(&mut pages);
}

#[test]
fn feed_multiple_pages_with_matches() {
    let mut t = term(10, 10);

    let first_page_rows = unsafe { (*t.screen().pages.first_node()).data.capacity.rows } as usize;
    for _ in 0..first_page_rows - 1 {
        feed(&mut t, "\r\n");
    }
    feed(&mut t, "Fizz");
    assert_eq!(t.screen().pages.first_node(), t.screen().pages.last_node());

    feed(&mut t, "\r\n");
    assert_ne!(t.screen().pages.first_node(), t.screen().pages.last_node());
    feed(&mut t, "Buzz\r\nFizz");

    let start = t.screen().pages.last_node();
    let mut search = PageListSearch::init(b"Fizz", &mut t.screen_mut().pages, start);

    // First match on the last page.
    assert!(search.next().is_some());
    assert!(search.next().is_none());

    // Feed loads the first page.
    assert!(search.feed());
    assert!(search.next().is_some());
    assert!(search.next().is_none());

    // No more pages.
    assert!(!search.feed());

    search.deinit(&mut t.screen_mut().pages);
}

#[test]
fn feed_multiple_pages_no_matches() {
    let mut t = term(10, 10);

    let first_page_rows = unsafe { (*t.screen().pages.first_node()).data.capacity.rows } as usize;
    for _ in 0..first_page_rows - 1 {
        feed(&mut t, "\r\n");
    }
    feed(&mut t, "Hello");

    feed(&mut t, "\r\n");
    assert_ne!(t.screen().pages.first_node(), t.screen().pages.last_node());
    feed(&mut t, "World");

    let start = t.screen().pages.last_node();
    let mut search = PageListSearch::init(b"Nope", &mut t.screen_mut().pages, start);

    assert!(search.next().is_none());
    assert!(search.feed());
    assert!(search.next().is_none());
    assert!(!search.feed());

    search.deinit(&mut t.screen_mut().pages);
}

#[test]
fn feed_iteratively_through_multiple_matches() {
    let mut t = term(80, 24);

    let first_page_rows = unsafe { (*t.screen().pages.first_node()).data.capacity.rows } as usize;
    for _ in 0..first_page_rows - 1 {
        feed(&mut t, "\r\n");
    }
    feed(&mut t, "Page1Test");
    assert_eq!(t.screen().pages.first_node(), t.screen().pages.last_node());

    feed(&mut t, "\r\n");
    assert_ne!(t.screen().pages.first_node(), t.screen().pages.last_node());
    feed(&mut t, "Page2Test");

    let start = t.screen().pages.last_node();
    let mut search = PageListSearch::init(b"Test", &mut t.screen_mut().pages, start);

    // Match on page 2.
    assert!(search.next().is_some());
    assert!(search.next().is_none());

    // Feed page 1.
    assert!(search.feed());
    assert!(search.next().is_some());
    assert!(search.next().is_none());

    assert!(!search.feed());

    search.deinit(&mut t.screen_mut().pages);
}

#[test]
fn feed_with_match_spanning_page_boundary() {
    let mut t = term(80, 24);

    let first_page_rows = unsafe { (*t.screen().pages.first_node()).data.capacity.rows } as usize;
    for _ in 0..first_page_rows - 1 {
        feed(&mut t, "\r\n");
    }
    for _ in 0..(t.screen().pages.cols() as usize - 2) {
        feed(&mut t, "x");
    }
    feed(&mut t, "Te");
    assert_eq!(t.screen().pages.first_node(), t.screen().pages.last_node());

    // Second page starts with "st".
    feed(&mut t, "st");
    assert_ne!(t.screen().pages.first_node(), t.screen().pages.last_node());

    let start = t.screen().pages.last_node();
    let mut search = PageListSearch::init(b"Test", &mut t.screen_mut().pages, start);

    // No complete match on the last page alone (only "st").
    assert!(search.next().is_none());

    // Feed the first page — enough data to find "Test".
    assert!(search.feed());

    let h = search.next().unwrap();
    let sel = h.untracked();
    assert_ne!(sel.start.node, sel.end.node);
    {
        let selection = Selection::init(sel.start, sel.end, false);
        let str = t.screen().selection_string(&selection, false);
        assert_eq!(str, "Test");
    }

    assert!(search.next().is_none());
    assert!(!search.feed());

    search.deinit(&mut t.screen_mut().pages);
}

#[test]
fn feed_with_match_spanning_page_boundary_with_newline() {
    let mut t = term(80, 24);

    let first_page_rows = unsafe { (*t.screen().pages.first_node()).data.capacity.rows } as usize;
    for _ in 0..first_page_rows - 1 {
        feed(&mut t, "\r\n");
    }
    for _ in 0..(t.screen().pages.cols() as usize - 2) {
        feed(&mut t, "x");
    }
    feed(&mut t, "Te");
    assert_eq!(t.screen().pages.first_node(), t.screen().pages.last_node());

    feed(&mut t, "\r\n");
    assert_ne!(t.screen().pages.first_node(), t.screen().pages.last_node());
    feed(&mut t, "st");

    let start = t.screen().pages.last_node();
    let mut search = PageListSearch::init(b"Test", &mut t.screen_mut().pages, start);

    // No matches: broke with an explicit newline.
    assert!(search.next().is_none());
    assert!(search.feed());
    assert!(search.next().is_none());
    assert!(!search.feed());

    search.deinit(&mut t.screen_mut().pages);
}

#[test]
fn feed_with_pruned_page() {
    // Zero forces minimum max size to effectively two pages.
    let mut p = PageList::init(80, 24, Some(0));

    // Grow to capacity.
    let page1_node = p.last_node();
    let page1_cap = unsafe { (*page1_node).data.capacity.rows } as usize;
    let page1_size = unsafe { (*page1_node).data.size.rows } as usize;
    for _ in 0..page1_cap - page1_size {
        assert!(p.grow_node().is_none());
    }

    // Grow and allocate one more page, then fill it.
    let page2_node = p.grow_node().unwrap();
    let page2_cap = unsafe { (*page2_node).data.capacity.rows } as usize;
    let page2_size = unsafe { (*page2_node).data.size.rows } as usize;
    for _ in 0..page2_cap - page2_size {
        assert!(p.grow_node().is_none());
    }

    let start = p.last_node();
    let mut search = PageListSearch::init(b"Test", &mut p, start);
    assert!(search.feed());
    assert!(!search.feed());

    // Next grow should reuse the first page since we're at max size.
    let new = p.grow_node().unwrap();
    assert_eq!(p.last_node(), new);

    // Our first should now be page2 and our last should be page1.
    assert_eq!(p.first_node(), page2_node);
    assert_eq!(p.last_node(), page1_node);

    // Feed should still do nothing.
    assert!(!search.feed());

    search.deinit(&mut p);
}
