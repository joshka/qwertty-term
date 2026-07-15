//! Inline tests ported 1:1 from `src/terminal/search/viewport.zig` (commit `2da015cd6`).
//! 4 tests.
//!
//! Zig `vtStream().nextSlice("...\r\n...")` maps to `print` + `carriage_return()` +
//! `linefeed()`; `\x1b[2J` → `erase_display(Complete)`; `\x1b[H` → `set_cursor_pos(1, 1)`;
//! `scrollViewport(.top)` → `scroll_viewport(ScrollViewport::Top)`.

use super::ViewportSearch;
use crate::csi::EraseDisplay;
use crate::point::{Coordinate, Point, Tag};
use crate::terminal::{Options as TermOptions, ScrollViewport, Terminal};

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

fn expect_pt(
    t: &Terminal,
    h: &crate::highlight::Flattened,
    tag: Tag,
    sx: u16,
    sy: u16,
    ex: u16,
    ey: u16,
) {
    let sel = h.untracked();
    let pages = &t.screen().pages;
    assert_eq!(
        pages.point_from_pin(tag, sel.start),
        Some(Point::new(
            tag,
            Coordinate {
                x: sx,
                y: sy as u32
            }
        ))
    );
    assert_eq!(
        pages.point_from_pin(tag, sel.end),
        Some(Point::new(
            tag,
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

    let mut search = ViewportSearch::init(b"Fizz");
    assert!(search.update(&mut t.screen_mut().pages));
    // Viewport contains active, so update always re-searches.
    assert!(search.update(&mut t.screen_mut().pages));

    let h = search.next().unwrap();
    expect_pt(&t, &h, Tag::Active, 0, 0, 3, 0);
    let h = search.next().unwrap();
    expect_pt(&t, &h, Tag::Active, 0, 2, 3, 2);
    assert!(search.next().is_none());
}

#[test]
fn clear_screen_and_search() {
    let mut t = term(10, 10);
    feed(&mut t, "Fizz\r\nBuzz\r\nFizz\r\nBang");

    let mut search = ViewportSearch::init(b"Fizz");
    assert!(search.update(&mut t.screen_mut().pages));

    t.erase_display(EraseDisplay::Complete, false);
    t.set_cursor_pos(1, 1);
    feed(&mut t, "Buzz\r\nFizz\r\nBuzz");
    assert!(search.update(&mut t.screen_mut().pages));

    let h = search.next().unwrap();
    expect_pt(&t, &h, Tag::Active, 0, 1, 3, 1);
    assert!(search.next().is_none());
}

#[test]
fn clear_screen_and_search_dirty_tracking() {
    let mut t = term(10, 10);
    feed(&mut t, "Fizz\r\nBuzz\r\nFizz\r\nBang");

    let mut search = ViewportSearch::init(b"Fizz");
    // Turn on dirty tracking.
    search.active_dirty = Some(false);

    // Should update since we've never searched before.
    assert!(search.update(&mut t.screen_mut().pages));
    // Should not update since nothing changed.
    assert!(!search.update(&mut t.screen_mut().pages));

    t.erase_display(EraseDisplay::Complete, false);
    t.set_cursor_pos(1, 1);
    feed(&mut t, "Buzz\r\nFizz\r\nBuzz");

    // Should still not update since active area isn't marked dirty.
    assert!(!search.update(&mut t.screen_mut().pages));

    // Mark dirty.
    search.active_dirty = Some(true);
    assert!(search.update(&mut t.screen_mut().pages));

    let h = search.next().unwrap();
    expect_pt(&t, &h, Tag::Active, 0, 1, 3, 1);
    assert!(search.next().is_none());
}

#[test]
fn history_search_no_active_area() {
    let mut t = term(10, 2);

    // Fill up the first page.
    let first_page_rows = unsafe { (*t.screen().pages.first_node()).data.capacity.rows } as usize;
    feed(&mut t, "Fizz\r\n");
    for _ in 1..first_page_rows - 1 {
        feed(&mut t, "\r\n");
    }
    assert_eq!(t.screen().pages.first_node(), t.screen().pages.last_node());

    // Create a second page.
    feed(&mut t, "\r\n");
    assert_ne!(t.screen().pages.first_node(), t.screen().pages.last_node());
    feed(&mut t, "Buzz\r\nFizz");

    t.scroll_viewport(ScrollViewport::Top);

    let mut search = ViewportSearch::init(b"Fizz");
    assert!(search.update(&mut t.screen_mut().pages));

    let h = search.next().unwrap();
    expect_pt(&t, &h, Tag::Screen, 0, 0, 3, 0);
    assert!(search.next().is_none());

    // Viewport doesn't contain active.
    assert!(!search.update(&mut t.screen_mut().pages));
    assert!(search.next().is_none());
}
