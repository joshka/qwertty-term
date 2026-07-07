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

// ---- test helpers for the erase/scroll/edit/SGR/alt-screen tier ----------

use crate::csi::{EraseDisplay, EraseLine};
use crate::page::ContentTag;
use crate::point::Point;
use crate::sgr::Attribute;

impl Terminal {
    /// Test helper: is the cell at `pt` dirty? Port of `isDirty`.
    fn is_dirty(&self, pt: Point) -> bool {
        self.screen().pages.is_dirty(pt)
    }
    /// Test helper: clear all dirty bits. Port of `clearDirty`.
    fn clear_dirty(&mut self) {
        self.screen_mut().pages.clear_dirty();
    }
}

/// The (content_tag, rgb) at an active-area cell.
fn bg_rgb_at(t: &Terminal, x: u16, y: u32) -> (ContentTag, (u8, u8, u8)) {
    let lc = t.screen().pages.get_cell(Point::active(x, y)).unwrap();
    unsafe { ((*lc.cell).content_tag(), (*lc.cell).color_rgb()) }
}

/// Grapheme codepoints attached to a screen cell, if any.
fn graphemes_at(t: &Terminal, x: u16, y: u32) -> Option<Vec<u32>> {
    let lc = t.screen().pages.get_cell(Point::screen(x, y)).unwrap();
    unsafe {
        let page = t.screen().pages.node_data(lc.node);
        page.lookup_grapheme(lc.cell).map(|s| (*s).to_vec())
    }
}

/// (codepoint, has_grapheme, wide) at a screen cell.
fn cell_info(t: &Terminal, x: u16, y: u32) -> (u32, bool, Wide) {
    let lc = t.screen().pages.get_cell(Point::screen(x, y)).unwrap();
    unsafe {
        (
            (*lc.cell).codepoint(),
            (*lc.cell).has_grapheme(),
            (*lc.cell).wide(),
        )
    }
}

// ---- eraseChars ----------------------------------------------------------

// Zig: "Terminal: eraseChars simple operation".
#[test]
fn erase_chars_simple() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.clear_dirty();
    t.erase_chars(2);
    t.print('X' as u32);
    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(!t.is_dirty(Point::active(0, 1)));
    assert_eq!(t.plain_string(), "X C");
}

// Zig: "Terminal: eraseChars minimum one".
#[test]
fn erase_chars_minimum_one() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.erase_chars(0);
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "XBC");
}

// Zig: "Terminal: eraseChars beyond screen edge".
#[test]
fn erase_chars_beyond_edge() {
    let mut t = term(5, 5);
    t.print_string("  ABC");
    t.set_cursor_pos(1, 4);
    t.erase_chars(10);
    assert_eq!(t.plain_string(), "  A");
}

// Zig: "Terminal: eraseChars wide character".
#[test]
fn erase_chars_wide_char() {
    let mut t = term(5, 5);
    t.print('橋' as u32);
    t.print_string("BC");
    t.set_cursor_pos(1, 1);
    t.erase_chars(1);
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "X BC");
}

// Zig: "Terminal: eraseChars resets pending wrap".
#[test]
fn erase_chars_resets_pending_wrap() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.erase_chars(1);
    assert!(!t.screen().cursor.pending_wrap);
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "ABCDX");
}

// Zig: "Terminal: eraseChars preserves background sgr".
#[test]
fn erase_chars_preserves_bg() {
    let mut t = term(10, 10);
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.erase_chars(2);
    assert_eq!(t.plain_string(), "  C");
    assert_eq!(bg_rgb_at(&t, 0, 0), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
    assert_eq!(bg_rgb_at(&t, 1, 0), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
}

// Zig: "Terminal: eraseChars protected attributes respected with iso".
#[test]
fn erase_chars_protected_iso() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Iso);
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.erase_chars(2);
    assert_eq!(t.plain_string(), "ABC");
}

// Zig: "Terminal: eraseChars protected attributes ignored with dec set".
#[test]
fn erase_chars_protected_dec_ignored() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Dec);
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.erase_chars(2);
    assert_eq!(t.plain_string(), "  C");
}

// ---- deleteChars ---------------------------------------------------------

// Zig: "Terminal: deleteChars simple operation".
#[test]
fn delete_chars_simple() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 2);
    t.delete_chars(2);
    assert_eq!(t.plain_string(), "ADE");
}

// Zig: "Terminal: deleteChars background sgr".
#[test]
fn delete_chars_bg() {
    let mut t = term(5, 5);
    t.print_string("ABC12");
    t.set_cursor_pos(1, 2);
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.delete_chars(2);
    // Trailing filled cells carry the bg color.
    assert_eq!(bg_rgb_at(&t, 4, 0), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
}

// ---- insertBlanks --------------------------------------------------------

// Zig: "Terminal: insertBlanks simple".
#[test]
fn insert_blanks_simple() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.insert_blanks(2);
    assert_eq!(t.plain_string(), "  ABC");
}

// Zig: "Terminal: insertBlanks pushes off end".
#[test]
fn insert_blanks_pushes_off_end() {
    let mut t = term(3, 2);
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.insert_blanks(2);
    assert_eq!(t.plain_string(), "  A");
}

// Zig: "Terminal: insertBlanks preserves background sgr".
#[test]
fn insert_blanks_bg() {
    let mut t = term(10, 10);
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.insert_blanks(2);
    assert_eq!(t.plain_string(), "  ABC");
    assert_eq!(bg_rgb_at(&t, 0, 0), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
    assert_eq!(bg_rgb_at(&t, 1, 0), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
}

// ---- insertLines ---------------------------------------------------------

// Zig: "Terminal: insertLines simple".
#[test]
fn insert_lines_simple() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.clear_dirty();
    t.insert_lines(1);
    assert!(!t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));
    assert!(t.is_dirty(Point::active(0, 3)));
    assert_eq!(t.plain_string(), "ABC\n\nDEF\nGHI");
}

// Zig: "Terminal: insertLines colors with bg color".
#[test]
fn insert_lines_bg() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.insert_lines(1);
    assert_eq!(t.plain_string(), "ABC\n\nDEF\nGHI");
    for x in 0..t.cols {
        assert_eq!(bg_rgb_at(&t, x, 1), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
    }
}

// Zig: "Terminal: insertLines outside of scroll region".
#[test]
fn insert_lines_outside_region() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_top_and_bottom_margin(3, 4);
    t.set_cursor_pos(2, 2);
    t.insert_lines(1);
    assert_eq!(t.plain_string(), "ABC\nDEF\nGHI");
}

// Zig: "Terminal: insertLines zero".
#[test]
fn insert_lines_zero() {
    let mut t = term(2, 5);
    t.insert_lines(0);
}

// Zig: "Terminal: insertLines more than remaining".
#[test]
fn insert_lines_more_than_remaining() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.insert_lines(20);
    assert_eq!(t.plain_string(), "ABC");
}

// Zig: "Terminal: insertLines resets pending wrap".
#[test]
fn insert_lines_resets_pending_wrap() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.insert_lines(1);
    assert!(!t.screen().cursor.pending_wrap);
}

// ---- deleteLines ---------------------------------------------------------

// Zig: "Terminal: deleteLines simple".
#[test]
fn delete_lines_simple() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.clear_dirty();
    t.delete_lines(1);
    assert!(!t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert_eq!(t.plain_string(), "ABC\nGHI");
}

// Zig: "Terminal: deleteLines with scroll region".
#[test]
fn delete_lines_with_region() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.carriage_return();
    t.linefeed();
    t.print_string("123");
    t.set_top_and_bottom_margin(1, 3);
    t.set_cursor_pos(1, 1);
    t.delete_lines(1);
    assert_eq!(t.plain_string(), "DEF\nGHI\n\n123");
}

// Zig: "Terminal: deleteLines resets pending wrap".
#[test]
fn delete_lines_resets_pending_wrap() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.delete_lines(1);
    assert!(!t.screen().cursor.pending_wrap);
}

// ---- scrollUp / scrollDown -----------------------------------------------

// Zig: "Terminal: scrollUp simple".
#[test]
fn scroll_up_simple() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    let cursor = (t.screen().cursor.x, t.screen().cursor.y);
    t.scroll_up(1);
    assert_eq!((t.screen().cursor.x, t.screen().cursor.y), cursor);
    assert_eq!(t.plain_string(), "DEF\nGHI");
}

// Zig: "Terminal: scrollUp top/bottom scroll region".
#[test]
fn scroll_up_region() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_top_and_bottom_margin(2, 3);
    t.set_cursor_pos(1, 1);
    t.scroll_up(1);
    assert_eq!(t.plain_string(), "ABC\nGHI");
}

// Zig: "Terminal: scrollUp preserves pending wrap".
#[test]
fn scroll_up_preserves_pending_wrap() {
    let mut t = term(5, 5);
    t.set_cursor_pos(5, 5);
    t.print_string("A");
    assert!(t.screen().cursor.pending_wrap);
    t.scroll_up(1);
    assert!(t.screen().cursor.pending_wrap);
}

// Zig: "Terminal: scrollDown simple".
#[test]
fn scroll_down_simple() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    let cursor = (t.screen().cursor.x, t.screen().cursor.y);
    t.scroll_down(1);
    assert_eq!((t.screen().cursor.x, t.screen().cursor.y), cursor);
    assert_eq!(t.plain_string(), "\nABC\nDEF\nGHI");
}

// Zig: "Terminal: scrollDown outside of scroll region".
#[test]
fn scroll_down_outside_region() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_top_and_bottom_margin(3, 4);
    t.set_cursor_pos(2, 2);
    t.scroll_down(1);
    assert_eq!(t.plain_string(), "ABC\nDEF\n\nGHI");
}

// ---- eraseLine -----------------------------------------------------------

// Zig: "Terminal: eraseLine right simple".
#[test]
fn erase_line_right() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 3);
    t.erase_line(EraseLine::Right, false);
    assert_eq!(t.plain_string(), "AB");
}

// Zig: "Terminal: eraseLine left simple".
#[test]
fn erase_line_left() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 3);
    t.erase_line(EraseLine::Left, false);
    assert_eq!(t.plain_string(), "   DE");
}

// Zig: "Terminal: eraseLine complete".
#[test]
fn erase_line_complete() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 3);
    t.erase_line(EraseLine::Complete, false);
    assert_eq!(t.plain_string(), "");
}

// Zig: "Terminal: eraseLine resets wrap".
#[test]
fn erase_line_resets_wrap() {
    let mut t = term(5, 5);
    t.print_string("ABCDE123");
    let wrapped = {
        let lc = t.screen().pages.get_cell(Point::active(0, 0)).unwrap();
        unsafe { (*lc.row).wrap() }
    };
    assert!(wrapped);
    t.set_cursor_pos(1, 1);
    t.erase_line(EraseLine::Right, false);
    let wrapped = {
        let lc = t.screen().pages.get_cell(Point::active(0, 0)).unwrap();
        unsafe { (*lc.row).wrap() }
    };
    assert!(!wrapped);
}

// ---- eraseDisplay --------------------------------------------------------

// Zig: "Terminal: eraseDisplay simple erase below".
#[test]
fn erase_display_below() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.erase_display(EraseDisplay::Below, false);
    assert_eq!(t.plain_string(), "ABC\nD");
}

// Zig: "Terminal: eraseDisplay erase above".
#[test]
fn erase_display_above() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.erase_display(EraseDisplay::Above, false);
    assert_eq!(t.plain_string(), "\n  F\nGHI");
}

// Zig: "Terminal: eraseDisplay complete".
#[test]
fn erase_display_complete() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.set_cursor_pos(2, 2);
    t.erase_display(EraseDisplay::Complete, false);
    assert_eq!(t.plain_string(), "");
}

// ---- decaln --------------------------------------------------------------

// Zig: "Terminal: DECALN".
#[test]
fn decaln_fills_e() {
    let mut t = term(2, 2);
    t.set_cursor_pos(1, 1);
    t.print_string("XY");
    t.carriage_return();
    t.linefeed();
    t.print_string("ZW");
    t.decaln();
    assert_eq!(t.plain_string(), "EE\nEE");
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 0);
}

// ---- setAttribute --------------------------------------------------------

// Zig-adjacent: setAttribute bold flags cursor style.
#[test]
fn set_attribute_bold() {
    let mut t = term(5, 5);
    t.set_attribute(Attribute::Bold);
    assert!(t.screen().cursor.style.flags.bold);
    t.set_attribute(Attribute::ResetBold);
    assert!(!t.screen().cursor.style.flags.bold);
}

// setAttribute direct-color fg/bg.
#[test]
fn set_attribute_colors() {
    use crate::page::style::Color;
    let mut t = term(5, 5);
    t.set_attribute(Attribute::DirectColorFg(crate::color::Rgb::new(1, 2, 3)));
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(4, 5, 6)));
    assert_eq!(
        t.screen().cursor.style.fg_color,
        Color::Rgb(crate::color::Rgb::new(1, 2, 3))
    );
    assert_eq!(
        t.screen().cursor.style.bg_color,
        Color::Rgb(crate::color::Rgb::new(4, 5, 6))
    );
    t.set_attribute(Attribute::Unset);
    assert!(t.screen().cursor.style.is_default());
}

// ---- alternate screen / fullReset ---------------------------------------

// Zig: "Terminal: fullReset".
#[test]
fn full_reset_basics() {
    let mut t = term(10, 10);
    t.print_string("hello");
    t.set_top_and_bottom_margin(2, 5);
    t.modes.set(Mode::Origin, true);
    t.full_reset();
    assert_eq!(t.plain_string(), "");
    assert_eq!(t.scrolling_region.top, 0);
    assert_eq!(t.scrolling_region.bottom, 9);
    assert!(!t.modes.get(Mode::Origin));
    assert_eq!(t.screens.active_key(), ScreenKey::Primary);
}

// Zig: "Terminal: switch to alternate screen and back (mode 1049)".
#[test]
fn alternate_screen_1049() {
    let mut t = term(10, 10);
    t.print_string("primary");
    assert_eq!(t.plain_string(), "primary");

    // Enter alt via 1049: saves cursor, clears alt on entry, copies cursor
    // from primary (which is at col 7 after "primary").
    t.switch_screen_mode(SwitchScreenMode::M1049, true);
    assert_eq!(t.screens.active_key(), ScreenKey::Alternate);
    assert_eq!(t.plain_string(), "");
    assert_eq!(t.screen().cursor.x, 7);
    t.set_cursor_pos(1, 1);
    t.print_string("alt");
    assert_eq!(t.plain_string(), "alt");

    // Leave alt: restores cursor, primary content intact.
    t.switch_screen_mode(SwitchScreenMode::M1049, false);
    assert_eq!(t.screens.active_key(), ScreenKey::Primary);
    assert_eq!(t.plain_string(), "primary");
}

// Zig: mode 47 copies the cursor and does not clear.
#[test]
fn alternate_screen_47_copies_cursor() {
    let mut t = term(10, 10);
    t.set_cursor_pos(3, 4); // y=2, x=3
    t.switch_screen_mode(SwitchScreenMode::M47, true);
    assert_eq!(t.screens.active_key(), ScreenKey::Alternate);
    assert_eq!(t.screen().cursor.x, 3);
    assert_eq!(t.screen().cursor.y, 2);
    t.switch_screen_mode(SwitchScreenMode::M47, false);
    assert_eq!(t.screens.active_key(), ScreenKey::Primary);
}

// ---- grapheme clustering (mode 2027) -------------------------------------

// Zig: "Terminal: print multicodepoint grapheme, mode 2027".
#[test]
fn grapheme_multicodepoint_2027() {
    let mut t = term(80, 80);
    t.modes.set(Mode::GraphemeCluster, true);
    // 👨‍👩‍👧
    t.print(0x1F468);
    t.print(0x200D);
    t.print(0x1F469);
    t.print(0x200D);
    t.print(0x1F467);

    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 2);
    assert!(t.is_dirty(Point::screen(0, 0)));

    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0x1F468);
    assert!(has_g);
    assert_eq!(wide, Wide::Wide);
    assert_eq!(graphemes_at(&t, 0, 0).unwrap().len(), 4);

    let (cp1, has_g1, wide1) = cell_info(&t, 1, 0);
    assert_eq!(cp1, 0);
    assert!(!has_g1);
    assert_eq!(wide1, Wide::SpacerTail);
}

// Zig: "Terminal: multicodepoint grapheme marks dirty on every codepoint".
#[test]
fn grapheme_marks_dirty_each_codepoint() {
    let mut t = term(80, 80);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x1F468);
    t.clear_dirty();
    t.print(0x200D);
    assert!(t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: keypad sequence VS16" — VS16 makes a wide char with 2027.
#[test]
fn grapheme_vs16_makes_wide() {
    let mut t = term(80, 80);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x23); // '#'
    t.print(0xFE0F); // VS16
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 2);
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0x23);
    assert!(has_g);
    assert_eq!(wide, Wide::Wide);
}

// Zig: "Terminal: keypad sequence VS15" — VS15 stays narrow.
#[test]
fn grapheme_vs15_stays_narrow() {
    let mut t = term(80, 80);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x23); // '#'
    t.print(0xFE0E); // VS15
    assert_eq!(t.screen().cursor.x, 1);
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0x23);
    assert!(has_g);
    assert_eq!(wide, Wide::Narrow);
}

// Zig: "Terminal: Fitzpatrick skin tone next valid base".
#[test]
fn grapheme_fitzpatrick_skin_tone() {
    let mut t = term(80, 80);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x1F44B); // 👋
    t.print(0x1F3FF); // dark skin tone
    assert_eq!(t.screen().cursor.x, 2);
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0x1F44B);
    assert!(has_g);
    assert_eq!(wide, Wide::Wide);
}

// Zig: "Terminal: Fitzpatrick skin tone next to non-base".
#[test]
fn grapheme_fitzpatrick_non_base() {
    let mut t = term(80, 80);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x22); // "
    t.print(0x1F3FF); // dark skin tone (wide, does not join with ")
    t.print(0x22); // "
    assert_eq!(t.screen().cursor.x, 4);
    assert_eq!(cell_info(&t, 0, 0), (0x22, false, Wide::Narrow));
    assert_eq!(cell_info(&t, 1, 0), (0x1F3FF, false, Wide::Wide));
    assert_eq!(cell_info(&t, 3, 0), (0x22, false, Wide::Narrow));
}

// Zig: "Terminal: VS16 to make wide character on next line" — the grapheme
// wide-effect wrap path with a spacer_head + cross-row transfer.
#[test]
fn grapheme_vs16_wraps_to_next_line() {
    let mut t = term(3, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    t.cursor_right(2);
    t.print('#' as u32);
    assert_eq!(t.screen().cursor.x, 2);
    assert!(t.screen().cursor.pending_wrap);

    t.print(0xFE0F); // VS16 → wide, must wrap
    assert_eq!(t.screen().cursor.y, 1);
    assert_eq!(t.screen().cursor.x, 2);
    assert!(!t.screen().cursor.pending_wrap);

    // Previous cell becomes a spacer_head.
    assert_eq!(cell_info(&t, 2, 0), (0, false, Wide::SpacerHead));
    // The '#' is now wide on the next line with the VS16 grapheme.
    let (cp, has_g, wide) = cell_info(&t, 0, 1);
    assert_eq!(cp, '#' as u32);
    assert!(has_g);
    assert_eq!(wide, Wide::Wide);
    assert_eq!(graphemes_at(&t, 0, 1).unwrap(), vec![0xFE0F]);
    // Spacer tail.
    assert_eq!(cell_info(&t, 1, 1), (0, false, Wide::SpacerTail));
}

// Zig: "Terminal: print grapheme ò should be narrow" — a base + combining mark
// that does NOT widen.
#[test]
fn grapheme_combining_mark_narrow() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x6F); // 'o'
    t.print(0x0301); // combining acute accent
    assert_eq!(t.screen().cursor.x, 1);
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0x6F);
    assert!(has_g);
    assert_eq!(wide, Wide::Narrow);
}

// Zig: "Terminal: print multicodepoint grapheme, disabled mode 2027" — with
// clustering off, each codepoint lands in its own cell(s).
#[test]
fn grapheme_multicodepoint_disabled() {
    let mut t = term(80, 80);
    // grapheme_cluster mode off (default)
    t.print(0x1F468);
    t.print(0x200D);
    t.print(0x1F469);
    t.print(0x200D);
    t.print(0x1F467);
    // Three wide emoji (6 cells) + two ZWJ attached as zero-width graphemes.
    assert_eq!(t.screen().cursor.x, 6);
}
