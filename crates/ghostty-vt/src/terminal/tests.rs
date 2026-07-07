//! Ported inline tests from `Terminal.zig`.
//!
//! PROGRESS: this chunk ports the tier-1 operations (init, charset, simple
//! motion, clamped cursor moves, save/restore, tabs, margins, cursor-pos,
//! partial index). The print path, erase/scroll/insert-delete-line family, and
//! alt-screen/reset are NOT yet ported (they need additional Screen surface —
//! see the PROGRESS note in `docs/analysis/terminal.md`). Their tests are
//! therefore deferred and will be added with those operations.

use super::*;

fn term(cols: CellCountInt, rows: CellCountInt) -> Terminal {
    Terminal::new(Options {
        cols,
        rows,
        max_scrollback: 0,
        colors: Colors::default(),
    })
}

// Zig: "Terminal: input with no control characters" (structure/init smoke).
#[test]
fn init_dimensions_and_region() {
    let t = term(80, 24);
    assert_eq!(t.cols, 80);
    assert_eq!(t.rows, 24);
    assert_eq!(t.scrolling_region.top, 0);
    assert_eq!(t.scrolling_region.bottom, 23);
    assert_eq!(t.scrolling_region.left, 0);
    assert_eq!(t.scrolling_region.right, 79);
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 0);
}

// Zig: "Terminal: setCursorPosition".
#[test]
fn set_cursor_position() {
    let mut t = term(80, 80);

    t.set_cursor_pos(1, 1);
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 0);

    // Setting it to 0 should keep it zero (1 based)
    t.set_cursor_pos(0, 0);
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 0);

    // Should clamp to size
    t.set_cursor_pos(81, 81);
    assert_eq!(t.screen().cursor.x, 79);
    assert_eq!(t.screen().cursor.y, 79);

    // Origin mode off should move to specified position.
    t.set_cursor_pos(21, 21);
    assert_eq!(t.screen().cursor.x, 20);
    assert_eq!(t.screen().cursor.y, 20);
}

// Zig: "Terminal: setCursorPosition with origin mode enabled".
#[test]
fn set_cursor_position_origin_mode() {
    let mut t = term(80, 80);

    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_top_and_bottom_margin(21, 40);
    t.set_left_and_right_margin(21, 40);
    t.modes.set(Mode::Origin, true);

    // Move to top-left of the region.
    t.set_cursor_pos(1, 1);
    assert_eq!(t.screen().cursor.x, 20);
    assert_eq!(t.screen().cursor.y, 20);

    // Clamp to region.
    t.set_cursor_pos(100, 100);
    assert_eq!(t.screen().cursor.x, 39);
    assert_eq!(t.screen().cursor.y, 39);
}

// Zig: "Terminal: cursorLeft no wrap" (motion clamp).
#[test]
fn cursor_left_no_wrap() {
    let mut t = term(80, 24);
    t.set_cursor_pos(1, 10); // x=9
    t.cursor_left(3);
    assert_eq!(t.screen().cursor.x, 6);
    // Clamp at 0.
    t.cursor_left(100);
    assert_eq!(t.screen().cursor.x, 0);
}

// Zig: "Terminal: cursorRight resets wrap"-adjacent (clamped right move).
#[test]
fn cursor_right_clamps_to_region() {
    let mut t = term(80, 24);
    t.set_cursor_pos(1, 1);
    t.cursor_right(200);
    assert_eq!(t.screen().cursor.x, 79);
}

// Zig: "Terminal: cursorDown/cursorUp clamp to region".
#[test]
fn cursor_up_down_clamp() {
    let mut t = term(80, 24);
    t.set_cursor_pos(12, 1); // y=11
    t.cursor_up(3);
    assert_eq!(t.screen().cursor.y, 8);
    t.cursor_down(100);
    assert_eq!(t.screen().cursor.y, 23);
    t.cursor_up(100);
    assert_eq!(t.screen().cursor.y, 0);
}

// Zig: "Terminal: saveCursor / restoreCursor round-trip".
#[test]
fn save_restore_cursor() {
    let mut t = term(80, 24);
    t.set_cursor_pos(5, 5); // x=4, y=4
    t.save_cursor();
    t.set_cursor_pos(1, 1);
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 0);
    t.restore_cursor();
    assert_eq!(t.screen().cursor.x, 4);
    assert_eq!(t.screen().cursor.y, 4);
}

// Zig: "Terminal: restoreCursor defaults when never saved".
#[test]
fn restore_cursor_without_save() {
    let mut t = term(80, 24);
    t.set_cursor_pos(5, 5);
    t.restore_cursor();
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 0);
}

// Zig: "Terminal: horizontal tabs" (forward tab hits 8-col stops).
#[test]
fn horizontal_tab_stops() {
    let mut t = term(80, 24);
    t.set_cursor_pos(1, 1);
    t.horizontal_tab();
    assert_eq!(t.screen().cursor.x, 8);
    t.horizontal_tab();
    assert_eq!(t.screen().cursor.x, 16);
}

// Zig: "Terminal: horizontal tab back".
#[test]
fn horizontal_tab_back() {
    let mut t = term(80, 24);
    t.set_cursor_pos(1, 20); // x=19
    t.horizontal_tab_back();
    assert_eq!(t.screen().cursor.x, 16);
    t.horizontal_tab_back();
    assert_eq!(t.screen().cursor.x, 8);
}

// Zig: "Terminal: tabClear / tabSet".
#[test]
fn tab_clear_and_set() {
    let mut t = term(80, 24);
    // Clear all stops, set a custom one at col 3.
    t.tab_clear(TabClear::All);
    t.set_cursor_pos(1, 4); // x=3
    t.tab_set();
    t.set_cursor_pos(1, 1);
    t.horizontal_tab();
    assert_eq!(t.screen().cursor.x, 3);
}

// Zig: "Terminal: setTopAndBottomMargin moves cursor to origin".
#[test]
fn set_top_bottom_margin() {
    let mut t = term(80, 24);
    t.set_cursor_pos(10, 10);
    t.set_top_and_bottom_margin(5, 15);
    assert_eq!(t.scrolling_region.top, 4);
    assert_eq!(t.scrolling_region.bottom, 14);
    // Cursor moved to 1,1 (or region origin without origin mode).
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 0);
}

// Zig: "Terminal: setLeftAndRightMargin gated on mode".
#[test]
fn set_left_right_margin_gated() {
    let mut t = term(80, 24);
    // Without the mode enabled, does nothing.
    t.set_left_and_right_margin(5, 15);
    assert_eq!(t.scrolling_region.left, 0);
    assert_eq!(t.scrolling_region.right, 79);

    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(5, 15);
    assert_eq!(t.scrolling_region.left, 4);
    assert_eq!(t.scrolling_region.right, 14);
}

// Zig: "Terminal: pwd / title round-trip".
#[test]
fn pwd_and_title() {
    let mut t = term(80, 24);
    assert_eq!(t.get_pwd(), None);
    assert_eq!(t.get_title(), None);
    t.set_pwd(b"/home/user");
    t.set_title(b"hello");
    assert_eq!(t.get_pwd(), Some(&b"/home/user"[..]));
    assert_eq!(t.get_title(), Some(&b"hello"[..]));
}

// Zig: "Terminal: index moves down within region".
#[test]
fn index_moves_down() {
    let mut t = term(80, 24);
    t.set_cursor_pos(1, 1);
    t.index();
    assert_eq!(t.screen().cursor.y, 1);
    t.index();
    assert_eq!(t.screen().cursor.y, 2);
}

// Zig: "Terminal: reverseIndex moves up".
#[test]
fn reverse_index_moves_up() {
    let mut t = term(80, 24);
    t.set_cursor_pos(3, 1); // y=2
    t.reverse_index();
    assert_eq!(t.screen().cursor.y, 1);
}

// Zig: "Terminal: carriageReturn to column 0".
#[test]
fn carriage_return_col_0() {
    let mut t = term(80, 24);
    t.set_cursor_pos(1, 10);
    t.carriage_return();
    assert_eq!(t.screen().cursor.x, 0);
}

// ---- print path ---------------------------------------------------------

use crate::charsets::{ActiveSlot, Charset, Slots};
use crate::page::Wide;

// Zig: "Terminal: print single very long line" (issue 1400 no-crash).
#[test]
fn print_single_very_long_line() {
    let mut t = term(5, 5);
    for _ in 0..1000 {
        t.print('x' as u32);
    }
}

// Zig: "Terminal: print writes a basic string".
#[test]
fn print_basic_string() {
    let mut t = term(80, 24);
    t.print_string("hello");
    assert_eq!(t.screen().cursor.x, 5);
    assert_eq!(t.plain_string(), "hello");
}

// Zig: "Terminal: print wide char".
#[test]
fn print_wide_char() {
    let mut t = term(80, 80);
    t.print(0x1F600); // smiley
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 2);

    // Cell 0,0 is the wide char; 1,0 is the spacer tail.
    let cell0 = t
        .screen()
        .pages
        .get_cell(crate::point::Point::screen(0, 0))
        .unwrap()
        .cell;
    // SAFETY: cell pointer live for the duration.
    unsafe {
        assert_eq!((*cell0).codepoint(), 0x1F600);
        assert_eq!((*cell0).wide(), Wide::Wide);
    }
    let cell1 = t
        .screen()
        .pages
        .get_cell(crate::point::Point::screen(1, 0))
        .unwrap()
        .cell;
    unsafe {
        assert_eq!((*cell1).wide(), Wide::SpacerTail);
    }
}

// Zig: "Terminal: print wide char at edge creates spacer head".
#[test]
fn print_wide_char_at_edge_spacer_head() {
    let mut t = term(10, 10);
    t.set_cursor_pos(1, 10); // x=9 (last col)
    t.print(0x1F600);
    assert_eq!(t.screen().cursor.y, 1);
    assert_eq!(t.screen().cursor.x, 2);

    let head = t
        .screen()
        .pages
        .get_cell(crate::point::Point::screen(9, 0))
        .unwrap()
        .cell;
    unsafe {
        assert_eq!((*head).wide(), Wide::SpacerHead);
    }
    let wide = t
        .screen()
        .pages
        .get_cell(crate::point::Point::screen(0, 1))
        .unwrap()
        .cell;
    unsafe {
        assert_eq!((*wide).codepoint(), 0x1F600);
        assert_eq!((*wide).wide(), Wide::Wide);
    }
    let tail = t
        .screen()
        .pages
        .get_cell(crate::point::Point::screen(1, 1))
        .unwrap()
        .cell;
    unsafe {
        assert_eq!((*tail).wide(), Wide::SpacerTail);
    }
}

// Zig: "Terminal: print charset".
#[test]
fn print_charset() {
    let mut t = term(80, 80);
    // G1/G2/G3 have no effect on GL.
    t.configure_charset(Slots::G1, Charset::DecSpecial);
    t.configure_charset(Slots::G2, Charset::DecSpecial);
    t.configure_charset(Slots::G3, Charset::DecSpecial);

    t.print('`' as u32);
    t.configure_charset(Slots::G0, Charset::Utf8);
    t.print('`' as u32);
    t.configure_charset(Slots::G0, Charset::Ascii);
    t.print('`' as u32);
    t.configure_charset(Slots::G0, Charset::DecSpecial);
    t.print('`' as u32);
    assert_eq!(t.plain_string(), "```\u{25C6}");
}

// Zig: "Terminal: print charset outside of ASCII".
#[test]
fn print_charset_outside_ascii() {
    let mut t = term(80, 80);
    t.configure_charset(Slots::G0, Charset::DecSpecial);
    t.print('`' as u32);
    t.print(0x1F600);
    // The dec-special maps '`' to a diamond; the wide emoji falls outside the
    // charset table so it maps to space + spacer tail.
    assert_eq!(t.plain_string(), "\u{25C6} ");
}

// Zig: "Terminal: print invoke charset".
#[test]
fn print_invoke_charset() {
    let mut t = term(80, 80);
    t.configure_charset(Slots::G1, Charset::DecSpecial);
    t.print('`' as u32);
    t.invoke_charset(ActiveSlot::Gl, Slots::G1, false);
    t.print('`' as u32);
    t.print('`' as u32);
    t.invoke_charset(ActiveSlot::Gl, Slots::G0, false);
    t.print('`' as u32);
    assert_eq!(t.plain_string(), "`\u{25C6}\u{25C6}`");
}

// Zig: "Terminal: print invoke charset single".
#[test]
fn print_invoke_charset_single() {
    let mut t = term(80, 80);
    t.configure_charset(Slots::G1, Charset::DecSpecial);
    t.print('`' as u32);
    t.invoke_charset(ActiveSlot::Gl, Slots::G1, true);
    t.print('`' as u32);
    t.print('`' as u32);
    assert_eq!(t.plain_string(), "`\u{25C6}`");
}

// Zig: print soft-wrap at the column limit.
#[test]
fn print_soft_wrap() {
    let mut t = term(5, 5);
    t.print_string("hello"); // fills row 0, pending wrap
    assert_eq!(t.screen().cursor.x, 4);
    assert!(t.screen().cursor.pending_wrap);
    t.print('x' as u32); // triggers wrap
    assert_eq!(t.screen().cursor.y, 1);
    assert_eq!(t.screen().cursor.x, 1);
    assert_eq!(t.plain_string_unwrapped(), "hellox");
}
