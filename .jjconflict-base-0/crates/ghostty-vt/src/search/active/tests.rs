//! Inline tests ported 1:1 from `src/terminal/search/active.zig` (commit `2da015cd6`).
//! 2 tests.
//!
//! Zig `vtStream().nextSlice("...\r\n...")` maps to `print` + `carriage_return()` +
//! `linefeed()`; `\x1b[2J` → `erase_display(Complete)`; `\x1b[H` → `set_cursor_pos(1, 1)`.

use super::ActiveSearch;
use crate::csi::EraseDisplay;
use crate::point::{Coordinate, Point, Tag};
use crate::terminal::{Options as TermOptions, Terminal};

fn term(cols: u16, rows: u16) -> Terminal {
    Terminal::new(TermOptions {
        cols,
        rows,
        max_scrollback: 10_000,
        colors: Default::default(),
    })
}

/// Print `text`, translating a `\r\n` pair into carriage-return + linefeed.
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

    let mut search = ActiveSearch::init(b"Fizz");
    search.update(&t.screen().pages);

    let h = search.next().unwrap();
    expect_active(&t, &h, 0, 0, 3, 0);
    let h = search.next().unwrap();
    expect_active(&t, &h, 0, 2, 3, 2);
    assert!(search.next().is_none());
}

#[test]
fn clear_screen_and_search() {
    let mut t = term(10, 10);
    feed(&mut t, "Fizz\r\nBuzz\r\nFizz\r\nBang");

    let mut search = ActiveSearch::init(b"Fizz");
    search.update(&t.screen().pages);

    // Clear screen + cursor home + new content.
    t.erase_display(EraseDisplay::Complete, false);
    t.set_cursor_pos(1, 1);
    feed(&mut t, "Buzz\r\nFizz\r\nBuzz");
    search.update(&t.screen().pages);

    let h = search.next().unwrap();
    expect_active(&t, &h, 0, 1, 3, 1);
    assert!(search.next().is_none());
}
