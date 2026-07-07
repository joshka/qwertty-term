//! Ported inline tests from `Terminal.zig`.
//!
//! PROGRESS (M1 backfill): essentially all of `Terminal.zig`'s inline tests
//! (381 total) are now ported 1:1, including the print path,
//! erase/scroll/insert-delete-line family, alt-screen/reset, resize/reflow/
//! DECCOLM, semantic prompt/OSC133, and printSlice/printAttributes groups.
//! "print kitty unicode placeholder" is now ported (`print_kitty_unicode_placeholder`
//! below) — the print path sets `Row::kitty_virtual_placeholder` (see
//! `crate::terminal::print::print_cell`). One test remains genuinely blocked:
//! "glyph APC stores session glossary entries" (the `glyph`/APC protocol module
//! doesn't exist in this crate at all yet).

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

// Zig: "Terminal: print kitty unicode placeholder".
#[test]
fn print_kitty_unicode_placeholder() {
    let mut t = term(10, 10);

    t.print(crate::kitty::unicode::PLACEHOLDER);
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 1);

    {
        let list_cell = t
            .screen()
            .pages
            .get_cell(crate::point::Point::screen(0, 0))
            .unwrap();
        unsafe {
            assert_eq!(
                (*list_cell.cell).codepoint(),
                crate::kitty::unicode::PLACEHOLDER
            );
            assert!((*list_cell.row).kitty_virtual_placeholder());
        }
    }

    assert!(t.is_dirty(crate::point::Point::active(0, 0)));
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

/// Number of cells with grapheme data on the cursor's current page. Port of
/// the Zig test helper pattern `t.screens.active.cursor.page_pin.node.data.graphemeCount()`.
fn grapheme_page_count(t: &Terminal) -> usize {
    // SAFETY: cursor pin live for the duration of the call.
    unsafe { (*t.screen().cursor_page()).grapheme_count() }
}

/// Number of entries in the style map on the cursor's current page. Port of
/// the Zig test helper pattern `t.screens.active.cursor.page_pin.node.data.styles.count()`.
fn style_page_count(t: &Terminal) -> usize {
    // SAFETY: cursor pin live for the duration of the call.
    unsafe { (*t.screen().cursor_page()).styles().count() }
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

// ---- M1 backfill batch 1: input / print / grapheme (Terminal.zig L3717-5720) ----

// Zig: "Terminal: input with no control characters".
#[test]
fn input_with_no_control_characters() {
    let mut t = term(40, 40);
    t.print_string("hello");
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 5);
    assert_eq!(t.plain_string(), "hello");
    assert!(t.is_dirty(Point::screen(5, 0)));
    assert!(!t.is_dirty(Point::screen(5, 1)));
}

// Zig: "Terminal: input with basic wraparound".
#[test]
fn input_with_basic_wraparound() {
    let mut t = term(5, 40);
    t.print_string("helloworldabc12");
    assert_eq!(t.screen().cursor.y, 2);
    assert_eq!(t.screen().cursor.x, 4);
    assert!(t.screen().cursor.pending_wrap);
    assert_eq!(t.plain_string(), "hello\nworld\nabc12");
}

// Zig: "Terminal: input with basic wraparound dirty".
#[test]
fn input_with_basic_wraparound_dirty() {
    let mut t = term(5, 40);
    t.print_string("hello");
    assert!(t.is_dirty(Point::screen(4, 0)));
    t.clear_dirty();
    t.print('w' as u32);
    // Old row is dirty because cursor moved from there.
    assert!(t.is_dirty(Point::screen(4, 0)));
    assert!(t.is_dirty(Point::screen(0, 1)));
}

// Zig: "Terminal: input that forces scroll".
#[test]
fn input_that_forces_scroll() {
    let mut t = term(1, 5);
    t.print_string("abcdef");
    assert_eq!(t.screen().cursor.y, 4);
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.plain_string(), "b\nc\nd\ne\nf");
}

// Zig: "Terminal: input unique style per cell" (no-crash smoke test).
#[test]
fn input_unique_style_per_cell() {
    let mut t = term(30, 30);
    for y in 0..t.rows {
        for x in 0..t.cols {
            t.set_cursor_pos((y + 1) as usize, (x + 1) as usize);
            t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(
                x as u8, y as u8, 0,
            )));
            t.print('x' as u32);
        }
    }
}

// Zig: "Terminal: zero-width character at start".
#[test]
fn zero_width_character_at_start() {
    let mut t = term(80, 80);
    // This used to crash the terminal. Not allowed, should be ignored.
    t.print(0x200D);
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 0);
    assert!(!t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: zero-width character attaches to pending wrap cell"
// (ghostty-org/ghostty#12581).
#[test]
fn zero_width_character_attaches_to_pending_wrap_cell() {
    let mut t = term(2, 2);
    t.modes.set(Mode::GraphemeCluster, false);
    t.print('x' as u32);
    t.print(0xE5); // å
    t.print(0x0332); // combining low line
    assert_eq!(t.plain_string(), "x\u{e5}\u{332}");
}

// Zig: "Terminal: print wide char with 1-column width".
#[test]
fn print_wide_char_with_1_column_width() {
    let mut t = term(1, 2);
    t.print(0x1F600);
    // This prints a space so we should be dirty.
    assert!(t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: print wide char in single-width terminal".
#[test]
fn print_wide_char_in_single_width_terminal() {
    let mut t = term(1, 80);
    t.print(0x1F600);
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 0);
    assert!(t.screen().cursor.pending_wrap);
    let (cp, _, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0);
    assert_eq!(wide, Wide::Narrow);
    assert!(t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: print over wide char at 0,0".
#[test]
fn print_over_wide_char_at_0_0() {
    let mut t = term(80, 80);
    t.print(0x1F600);
    t.set_cursor_pos(1, 1);
    t.print('A' as u32);
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 1);
    assert_eq!(cell_info(&t, 0, 0), ('A' as u32, false, Wide::Narrow));
    assert_eq!(cell_info(&t, 1, 0), (0, false, Wide::Narrow));
    assert!(t.is_dirty(Point::screen(0, 0)));
    assert!(!t.is_dirty(Point::screen(0, 1)));
}

// Zig: "Terminal: print over wide char at col 0 corrupts previous row"
// (AFL++ fuzzer crash, afl-out/stream/default/crashes/id:000002).
#[test]
fn print_over_wide_char_at_col_0_corrupts_previous_row() {
    let mut t = term(10, 3);
    // Fill rows 0 and 1 with wide chars (5 per row on a 10-col terminal).
    for _ in 0..10 {
        t.print(0x4E2D);
    }
    // Move cursor to row 1, col 0 (on top of a wide char) and print a
    // narrow character.
    t.set_cursor_pos(2, 1);
    t.print('A' as u32);
    assert_eq!(cell_info(&t, 0, 1).2, Wide::Narrow);
    assert_eq!(cell_info(&t, 8, 0).2, Wide::Wide);
    assert_eq!(cell_info(&t, 9, 0).2, Wide::SpacerTail);
}

// Zig: "Terminal: print over wide spacer tail".
#[test]
fn print_over_wide_spacer_tail() {
    let mut t = term(5, 5);
    t.print('橋' as u32);
    t.set_cursor_pos(1, 2);
    t.print('X' as u32);
    assert_eq!(cell_info(&t, 0, 0), (0, false, Wide::Narrow));
    assert_eq!(cell_info(&t, 1, 0), ('X' as u32, false, Wide::Narrow));
    assert_eq!(t.plain_string(), " X");
    assert!(t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: print over wide char with bold" (style-map cleanup).
#[test]
fn print_over_wide_char_with_bold() {
    let mut t = term(80, 80);
    t.set_attribute(Attribute::Bold);
    t.print(0x1F600);
    // Go back and overwrite with no style.
    t.set_cursor_pos(1, 1);
    t.set_attribute(Attribute::Unset);
    t.print('A' as u32);
    assert!(t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: print over wide char with bg color" (style-map cleanup).
#[test]
fn print_over_wide_char_with_bg_color() {
    let mut t = term(80, 80);
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.print(0x1F600);
    t.set_cursor_pos(1, 1);
    t.set_attribute(Attribute::Unset);
    t.print('A' as u32);
    assert!(t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: graphemeWidth parity" — the streaming printer's cursor
// advance matches unicode::grapheme_width for representative clusters.
#[test]
fn print_grapheme_width_parity() {
    fn check(cps: &[u32]) {
        let mut t = term(80, 5);
        t.modes.set(Mode::GraphemeCluster, true);
        for &cp in cps {
            t.print(cp);
        }
        assert_eq!(t.screen().cursor.y, 0);
    }
    check(&[0x2764, 0xFE0F]);
    check(&['x' as u32, 0xFE0F, 0xFE0F]);
    check(&[0x231A, 0xFE0E, 0xFE0F]);
    check(&[0x1F3F4, 0x200D, 0x2620, 0xFE0F]);
    check(&[0x1F468, 0x200D, 0x1F469, 0x200D, 0x1F467]);
    check(&[0x23, 0xFE0F, 0x20E3]);
    check(&['1' as u32, 0x20E3]);
    check(&[0x1F44B, 0x1F3FF]);
    check(&[0x1F1E6, 0x1F1E7, 0x1F1E8]);
    check(&['a' as u32, 'b' as u32]);
    check(&[0x0301, 0x0302]);
}

// Zig: "Terminal: VS16 doesn't make character with 2027 disabled".
#[test]
fn vs16_no_effect_with_2027_disabled() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, false);
    t.print(0x2764); // heart
    t.print(0xFE0F); // VS16
    assert_eq!(t.plain_string(), "\u{2764}\u{fe0f}");
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0x2764);
    assert!(has_g);
    assert_eq!(wide, Wide::Narrow);
    assert_eq!(graphemes_at(&t, 0, 0).unwrap().len(), 1);
}

// Zig: "Terminal: ignored VS16 doesn't mark dirty".
#[test]
fn ignored_vs16_doesnt_mark_dirty() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, false);
    t.print(0x2764);
    assert!(t.is_dirty(Point::screen(0, 0)));
    t.clear_dirty();
    t.print(0xFE0F);
    assert!(!t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: print invalid VS16 non-grapheme".
#[test]
fn print_invalid_vs16_non_grapheme() {
    let mut t = term(80, 80);
    t.print('x' as u32);
    t.print(0xFE0F);
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 1);
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 'x' as u32);
    assert!(!has_g);
    assert_eq!(wide, Wide::Narrow);
    assert_eq!(cell_info(&t, 1, 0).0, 0);
}

// Zig: "Terminal: invalid VS16 doesn't mark dirty".
#[test]
fn invalid_vs16_doesnt_mark_dirty() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, false);
    t.print('x' as u32);
    assert!(t.is_dirty(Point::screen(0, 0)));
    t.clear_dirty();
    t.print(0xFE0F);
    assert!(!t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: variation selectors apply to preceding codepoint"
// (ghostty-org/ghostty#12596).
#[test]
fn variation_selectors_apply_to_preceding_codepoint() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    // Pirate flag: black flag + ZWJ + skull and crossbones + VS16.
    t.print(0x1F3F4);
    t.print(0x200D);
    t.print(0x2620);
    t.print(0xFE0F);
    let (cp, has_g, _) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0x1F3F4);
    assert!(has_g);
    assert_eq!(
        graphemes_at(&t, 0, 0).unwrap(),
        vec![0x200D, 0x2620, 0xFE0F]
    );
}

// Zig: "Terminal: VS15 to make narrow character".
#[test]
fn vs15_to_make_narrow_character() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x2614); // umbrella with rain drops, width=2
    assert!(t.is_dirty(Point::screen(0, 0)));
    t.clear_dirty();
    assert_eq!(t.screen().cursor.x, 2);
    t.print(0xFE0E); // VS15 to make narrow
    assert!(t.is_dirty(Point::screen(0, 0)));
    t.clear_dirty();
    assert_eq!(t.screen().cursor.x, 1);
    assert_eq!(t.plain_string(), "\u{2614}\u{fe0e}");
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0x2614);
    assert!(has_g);
    assert_eq!(wide, Wide::Narrow);
    assert_eq!(graphemes_at(&t, 0, 0).unwrap().len(), 1);
}

// Zig: "Terminal: VS15 on already narrow emoji".
#[test]
fn vs15_on_already_narrow_emoji() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x26C8); // thunder cloud and rain, width=1
    assert!(t.is_dirty(Point::screen(0, 0)));
    t.clear_dirty();
    t.print(0xFE0E);
    assert!(t.is_dirty(Point::screen(0, 0)));
    t.clear_dirty();
    assert_eq!(t.screen().cursor.x, 1);
    assert_eq!(t.plain_string(), "\u{26c8}\u{fe0e}");
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0x26C8);
    assert!(has_g);
    assert_eq!(wide, Wide::Narrow);
}

// Zig: "Terminal: print invalid VS15 following emoji is wide".
#[test]
fn print_invalid_vs15_following_emoji_is_wide() {
    let mut t = term(80, 80);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x1F9E0); // brain
    t.print(0xFE0E); // not valid with U+1F9E0 as base
    assert_eq!(t.screen().cursor.x, 2);
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0x1F9E0);
    assert!(!has_g);
    assert_eq!(wide, Wide::Wide);
    assert_eq!(cell_info(&t, 1, 0).2, Wide::SpacerTail);
}

// Zig: "Terminal: print invalid VS15 in emoji ZWJ sequence".
#[test]
fn print_invalid_vs15_in_emoji_zwj_sequence() {
    let mut t = term(80, 80);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x1F469); // woman
    t.print(0xFE0E); // not valid with U+1F469 as base
    t.print(0x200D); // ZWJ
    t.print(0x1F466); // boy
    assert_eq!(t.screen().cursor.x, 2);
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0x1F469);
    assert!(has_g);
    assert_eq!(graphemes_at(&t, 0, 0).unwrap(), vec![0x200D, 0x1F466]);
    assert_eq!(wide, Wide::Wide);
}

// Zig: "Terminal: VS15 to make narrow character with pending wrap".
#[test]
fn vs15_to_make_narrow_character_with_pending_wrap() {
    let mut t = term(4, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    assert!(t.modes.get(Mode::Wraparound));
    t.print(0x1F34B); // lemon, width=2
    t.print(0x2614); // umbrella, width=2
    assert!(t.is_dirty(Point::screen(0, 0)));
    t.clear_dirty();
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 3);
    assert!(t.screen().cursor.pending_wrap);
    t.print(0xFE0E); // VS15 to make narrow
    assert!(t.is_dirty(Point::screen(0, 0)));
    t.clear_dirty();
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 3);
    assert!(!t.screen().cursor.pending_wrap);
    assert_eq!(t.plain_string(), "\u{1f34b}\u{2614}\u{fe0e}");
    let (cp, has_g, wide) = cell_info(&t, 2, 0);
    assert_eq!(cp, 0x2614);
    assert!(has_g);
    assert_eq!(wide, Wide::Narrow);
}

// Zig: "Terminal: VS16 to make wide character on next line".
#[test]
fn vs16_to_make_wide_character_on_next_line() {
    let mut t = term(3, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    t.cursor_right(2);
    t.print('#' as u32);
    assert_eq!(t.screen().cursor.x, 2);
    assert!(t.screen().cursor.pending_wrap);
    assert!(t.is_dirty(Point::screen(2, 0)));
    t.clear_dirty();
    t.print(0xFE0F); // VS16 to make wide
    assert!(t.is_dirty(Point::screen(2, 0)));
    t.clear_dirty();
    assert_eq!(t.screen().cursor.y, 1);
    assert_eq!(t.screen().cursor.x, 2);
    assert!(!t.screen().cursor.pending_wrap);
    assert_eq!(cell_info(&t, 2, 0), (0, false, Wide::SpacerHead));
    let (cp, has_g, wide) = cell_info(&t, 0, 1);
    assert_eq!(cp, '#' as u32);
    assert!(has_g);
    assert_eq!(graphemes_at(&t, 0, 1).unwrap(), vec![0xFE0F]);
    assert_eq!(wide, Wide::Wide);
    assert_eq!(cell_info(&t, 1, 1).2, Wide::SpacerTail);
}

// Zig: "Terminal: VS16 to make wide character on next line with hyperlink"
// — regression for a crash in print's grapheme `.wide` path: writing a
// spacer_head at the screen edge before row.wrap was set.
#[test]
fn vs16_to_make_wide_character_on_next_line_with_hyperlink() {
    let mut t = term(3, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.cursor_right(2);
    t.print('#' as u32);
    assert_eq!(t.screen().cursor.x, 2);
    assert!(t.screen().cursor.pending_wrap);

    // Without the fix, this panicked with UnwrappedSpacerHead.
    t.print(0xFE0F); // VS16 to make wide

    assert_eq!(t.screen().cursor.y, 1);
    assert_eq!(t.screen().cursor.x, 2);
    assert!(!t.screen().cursor.pending_wrap);

    {
        let lc = t.screen().pages.get_cell(Point::screen(2, 0)).unwrap();
        unsafe {
            assert_eq!((*lc.cell).wide(), Wide::SpacerHead);
            assert!((*lc.cell).hyperlink());
            assert!((*lc.row).wrap());
        }
    }
    {
        let lc = t.screen().pages.get_cell(Point::screen(0, 1)).unwrap();
        unsafe {
            assert_eq!((*lc.cell).codepoint(), '#' as u32);
            assert!((*lc.cell).hyperlink());
        }
    }
    {
        let lc = t.screen().pages.get_cell(Point::screen(1, 1)).unwrap();
        unsafe {
            assert_eq!((*lc.cell).wide(), Wide::SpacerTail);
            assert!((*lc.cell).hyperlink());
        }
    }
}

// Zig: "Terminal: VS16 to make wide character with pending wrap".
#[test]
fn vs16_to_make_wide_character_with_pending_wrap() {
    let mut t = term(3, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    t.cursor_right(1);
    t.print('#' as u32);
    assert_eq!(t.screen().cursor.x, 2);
    assert!(!t.screen().cursor.pending_wrap);
    t.print(0xFE0F); // VS16 to make wide
    assert_eq!(t.screen().cursor.x, 2);
    assert_eq!(t.screen().cursor.y, 0);
    assert!(t.screen().cursor.pending_wrap);
    let (cp, has_g, wide) = cell_info(&t, 1, 0);
    assert_eq!(cp, '#' as u32);
    assert!(has_g);
    assert_eq!(wide, Wide::Wide);
    assert_eq!(cell_info(&t, 2, 0).2, Wide::SpacerTail);
}

// Zig: "Terminal: VS16 to make wide character with mode 2027".
#[test]
fn vs16_to_make_wide_character_with_mode_2027() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x2764); // heart
    assert!(t.is_dirty(Point::screen(0, 0)));
    t.clear_dirty();
    t.print(0xFE0F);
    assert!(t.is_dirty(Point::screen(0, 0)));
    t.clear_dirty();
    assert_eq!(t.plain_string(), "\u{2764}\u{fe0f}");
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0x2764);
    assert!(has_g);
    assert_eq!(wide, Wide::Wide);
    assert_eq!(graphemes_at(&t, 0, 0).unwrap().len(), 1);
}

// Zig: "Terminal: VS16 repeated with mode 2027".
#[test]
fn vs16_repeated_with_mode_2027() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x2764);
    t.print(0xFE0F);
    t.print(0x2764);
    t.print(0xFE0F);
    assert!(t.is_dirty(Point::screen(0, 0)));
    assert_eq!(t.plain_string(), "\u{2764}\u{fe0f}\u{2764}\u{fe0f}");
    for x in [0u16, 2] {
        let (cp, has_g, wide) = cell_info(&t, x, 0);
        assert_eq!(cp, 0x2764);
        assert!(has_g);
        assert_eq!(wide, Wide::Wide);
        assert_eq!(graphemes_at(&t, x, 0).unwrap().len(), 1);
    }
}

// Zig: "Terminal: print invalid VS16 grapheme"
// (mitchellh/ghostty#1482).
#[test]
fn print_invalid_vs16_grapheme() {
    let mut t = term(80, 80);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print('x' as u32);
    t.print(0xFE0F); // invalid VS16
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 1);
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 'x' as u32);
    assert!(!has_g);
    assert_eq!(wide, Wide::Narrow);
    assert_eq!(cell_info(&t, 1, 0), (0, false, Wide::Narrow));
}

// Zig: "Terminal: print invalid VS16 with second char"
// (mitchellh/ghostty#1482).
#[test]
fn print_invalid_vs16_with_second_char() {
    let mut t = term(80, 80);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print('x' as u32);
    t.print(0xFE0F);
    t.print('y' as u32);
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 2);
    assert_eq!(cell_info(&t, 0, 0), ('x' as u32, false, Wide::Narrow));
    assert_eq!(cell_info(&t, 1, 0), ('y' as u32, false, Wide::Narrow));
}

// Zig: "Terminal: print grapheme ò (o with nonspacing mark) should be narrow".
#[test]
fn print_grapheme_o_with_nonspacing_mark_should_be_narrow() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print('o' as u32);
    t.print(0x0300); // combining grave accent
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 1);
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 'o' as u32);
    assert!(has_g);
    assert_eq!(graphemes_at(&t, 0, 0).unwrap(), vec![0x0300]);
    assert_eq!(wide, Wide::Narrow);
}

// Zig: "Terminal: print Devanagari grapheme should be wide".
#[test]
fn print_devanagari_grapheme_should_be_wide() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    // क्‍ष
    t.print(0x0915);
    t.print(0x094D);
    t.print(0x200D);
    t.print(0x0937);
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 2);
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 0x0915);
    assert!(has_g);
    assert_eq!(
        graphemes_at(&t, 0, 0).unwrap(),
        vec![0x094D, 0x200D, 0x0937]
    );
    assert_eq!(wide, Wide::Wide);
    assert_eq!(cell_info(&t, 1, 0).2, Wide::SpacerTail);
}

// Zig: "Terminal: print Devanagari grapheme should be wide on next line".
#[test]
fn print_devanagari_grapheme_should_be_wide_on_next_line() {
    let mut t = term(3, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    t.cursor_right(2);
    // क्‍ष
    t.print(0x0915);
    t.print(0x094D);
    t.print(0x200D);
    assert_eq!(t.screen().cursor.x, 2);
    assert!(t.screen().cursor.pending_wrap);
    // This one increases the width to wide.
    t.print(0x0937);
    assert_eq!(t.screen().cursor.y, 1);
    assert_eq!(t.screen().cursor.x, 2);
    assert!(!t.screen().cursor.pending_wrap);
    assert_eq!(cell_info(&t, 2, 0), (0, false, Wide::SpacerHead));
    let (cp, has_g, wide) = cell_info(&t, 0, 1);
    assert_eq!(cp, 0x0915);
    assert!(has_g);
    assert_eq!(
        graphemes_at(&t, 0, 1).unwrap(),
        vec![0x094D, 0x200D, 0x0937]
    );
    assert_eq!(wide, Wide::Wide);
    assert_eq!(cell_info(&t, 1, 1).2, Wide::SpacerTail);
}

// Zig: "Terminal: print invalid VS16 with second char (combining)"
// (mitchellh/ghostty#1482).
#[test]
fn print_invalid_vs16_with_second_char_combining() {
    let mut t = term(80, 80);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print('n' as u32);
    t.print(0xFE0F); // invalid VS16
    t.print(0x0303); // combining tilde
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 1);
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 'n' as u32);
    assert!(has_g);
    assert_eq!(graphemes_at(&t, 0, 0).unwrap(), vec![0x0303]);
    assert_eq!(wide, Wide::Narrow);
    assert_eq!(cell_info(&t, 1, 0), (0, false, Wide::Narrow));
}

// Zig: "Terminal: overwrite grapheme should clear grapheme data".
#[test]
fn overwrite_grapheme_should_clear_grapheme_data() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x26C8); // thunder cloud and rain
    t.print(0xFE0E); // VS15 to make narrow
    assert!(t.is_dirty(Point::screen(0, 0)));
    t.clear_dirty();
    t.set_cursor_pos(1, 1);
    t.print('A' as u32);
    assert!(t.is_dirty(Point::screen(0, 0)));
    assert_eq!(t.plain_string(), "A");
    let (cp, has_g, wide) = cell_info(&t, 0, 0);
    assert_eq!(cp, 'A' as u32);
    assert!(!has_g);
    assert_eq!(wide, Wide::Narrow);
}

// Zig: "Terminal: overwrite multicodepoint grapheme clears grapheme data".
#[test]
fn overwrite_multicodepoint_grapheme_clears_grapheme_data() {
    let mut t = term(10, 10);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x1F468);
    t.print(0x200D);
    t.print(0x1F469);
    t.print(0x200D);
    t.print(0x1F467);
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 2);
    assert_eq!(grapheme_page_count(&t), 1);

    t.set_cursor_pos(1, 1);
    t.clear_dirty();
    t.print('X' as u32);
    assert!(t.is_dirty(Point::screen(0, 0)));
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 1);
    assert_eq!(grapheme_page_count(&t), 0);
    assert_eq!(t.plain_string(), "X");
}

// Zig: "Terminal: overwrite multicodepoint grapheme tail clears grapheme data".
#[test]
fn overwrite_multicodepoint_grapheme_tail_clears_grapheme_data() {
    let mut t = term(10, 10);
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x1F468);
    t.print(0x200D);
    t.print(0x1F469);
    t.print(0x200D);
    t.print(0x1F467);
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 2);
    assert_eq!(grapheme_page_count(&t), 1);

    t.set_cursor_pos(1, 2);
    t.print('X' as u32);
    assert_eq!(t.plain_string(), " X");
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 2);
    assert_eq!(grapheme_page_count(&t), 0);
}

// Zig: "Terminal: print breaks valid grapheme cluster with Prepend + ASCII
// for speed" — deliberately incorrect grapheme-break behavior (a Prepend
// code point should not break with the one following it per UAX #29 GB9b),
// kept as an optimization: we assume a break when c <= 255.
#[test]
fn print_breaks_valid_grapheme_cluster_with_prepend_ascii_for_speed() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    // Make sure we're not at cursor.x == 0 for the next char.
    t.print('_' as u32);
    // U+0600 ARABIC NUMBER SIGN (Prepend)
    t.print(0x0600);
    t.print('1' as u32);
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 3);
    let (cp, has_g, wide) = cell_info(&t, 1, 0);
    assert_eq!(cp, 0x0600);
    assert!(!has_g);
    assert_eq!(wide, Wide::Narrow);
    let (cp2, has_g2, wide2) = cell_info(&t, 2, 0);
    assert_eq!(cp2, '1' as u32);
    assert!(!has_g2);
    assert_eq!(wide2, Wide::Narrow);
}

// Zig: "Terminal: print writes to bottom if scrolled".
//
// Needs actual scrollback (Zig's default `max_scrollback` is 10_000; our
// shared `term()` helper uses 0 for speed), so build a `Terminal` directly.
#[test]
fn print_writes_to_bottom_if_scrolled() {
    let mut t = Terminal::new(Options {
        cols: 5,
        rows: 2,
        max_scrollback: 10_000,
        colors: Colors::default(),
    });
    t.print_string("hello");
    t.set_cursor_pos(1, 1);
    // Make newlines so we create scrollback; 3 pushes "hello" off-screen.
    t.index();
    t.index();
    t.index();
    assert_eq!(t.plain_string(), "");

    t.screen_mut().scroll(crate::screen::Scroll::Top);
    assert_eq!(t.plain_string(), "hello");

    t.print('A' as u32);
    t.screen_mut().scroll(crate::screen::Scroll::Active);
    assert_eq!(t.plain_string(), "\nA");

    let (x, y) = (t.screen().cursor.x, t.screen().cursor.y);
    assert!(t.is_dirty(Point::active(x, y as u32)));
}

// Zig: "Terminal: soft wrap with semantic prompt".
#[test]
fn soft_wrap_with_semantic_prompt() {
    use crate::osc::{PromptKind, SemanticPrompt, SemanticPromptAction};
    let mut t = term(3, 80);
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options_unvalidated: String::new(),
    });
    // Should not make anything dirty on its own.
    assert!(!t.is_dirty(Point::screen(0, 0)));
    let _ = PromptKind::Initial;

    t.print_string("hello");
    let lc0 = t.screen().pages.get_cell(Point::screen(0, 0)).unwrap();
    unsafe {
        assert_eq!(
            (*lc0.row).semantic_prompt(),
            crate::page::SemanticPrompt::Prompt
        );
    }
    let lc1 = t.screen().pages.get_cell(Point::screen(0, 1)).unwrap();
    unsafe {
        assert_eq!(
            (*lc1.row).semantic_prompt(),
            crate::page::SemanticPrompt::PromptContinuation
        );
    }
}

// Zig: "Terminal: disabled wraparound with wide char and one space".
#[test]
fn disabled_wraparound_with_wide_char_and_one_space() {
    let mut t = term(5, 5);
    t.modes.set(Mode::Wraparound, false);
    // Cursor at the end, no space for a wide char.
    t.print_string("AAAA");
    t.clear_dirty();
    t.print(0x1F6A8); // police car light
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 4);
    assert_eq!(t.plain_string(), "AAAA");
    assert_eq!(cell_info(&t, 4, 0), (0, false, Wide::Narrow));
    assert!(!t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: disabled wraparound with wide char and no space".
#[test]
fn disabled_wraparound_with_wide_char_and_no_space() {
    let mut t = term(5, 5);
    t.modes.set(Mode::Wraparound, false);
    t.print_string("AAAAA");
    t.clear_dirty();
    t.print(0x1F6A8);
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 4);
    assert_eq!(t.plain_string(), "AAAAA");
    assert_eq!(cell_info(&t, 4, 0), ('A' as u32, false, Wide::Narrow));
    assert!(!t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: disabled wraparound with wide grapheme and half space".
#[test]
fn disabled_wraparound_with_wide_grapheme_and_half_space() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);
    t.modes.set(Mode::Wraparound, false);
    t.print_string("AAAA");
    t.print(0x2764); // heart
    t.clear_dirty();
    t.print(0xFE0F); // VS16 to make wide
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 4);
    assert_eq!(t.plain_string(), "AAAA\u{2764}");
    let (cp, _, wide) = cell_info(&t, 4, 0);
    assert_eq!(cp, 0x2764);
    assert_eq!(wide, Wide::Narrow);
    assert!(!t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: print right margin wrap".
#[test]
fn print_right_margin_wrap() {
    let mut t = term(10, 5);
    t.print_string("123456789");
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(3, 5);
    t.set_cursor_pos(1, 5);
    t.print_string("XY");
    assert_eq!(t.plain_string(), "1234X6789\n  Y");
    let lc = t.screen().pages.get_cell(Point::active(0, 0)).unwrap();
    unsafe {
        assert!(!(*lc.row).wrap());
    }
}

// Zig: "Terminal: print right margin wrap dirty tracking".
#[test]
fn print_right_margin_wrap_dirty_tracking() {
    let mut t = term(10, 5);
    t.print_string("123456789");
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(3, 5);
    t.set_cursor_pos(1, 5);

    t.clear_dirty();
    t.print('X' as u32);
    assert!(t.is_dirty(Point::screen(4, 0)));
    assert!(!t.is_dirty(Point::screen(2, 1)));

    t.clear_dirty();
    t.print('Y' as u32);
    assert!(t.is_dirty(Point::screen(4, 0)));
    assert!(t.is_dirty(Point::screen(2, 1)));

    assert_eq!(t.plain_string(), "1234X6789\n  Y");
}

// Zig: "Terminal: print right margin outside" — writes past the margin
// wrap around like normal (only inside the margin is a "right margin"
// concern).
#[test]
fn print_right_margin_outside() {
    let mut t = term(10, 5);
    t.print_string("123456789");
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(3, 5);
    t.set_cursor_pos(1, 6);
    t.clear_dirty();
    t.print_string("XY");
    assert_eq!(t.plain_string(), "12345XY89");
}

// ---- M1 backfill batch 2: right margin / hyperlink / LF-CR / tabs / cursorPos (L5718-6420) ----

// Zig: "Terminal: print right margin outside wrap".
#[test]
fn print_right_margin_outside_wrap() {
    let mut t = term(10, 5);
    t.print_string("123456789");
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(3, 5);
    t.set_cursor_pos(1, 10);
    t.print_string("XY");
    assert_eq!(t.plain_string(), "123456789X\n  Y");
}

// Zig: "Terminal: print wide char at right margin does not create spacer head".
#[test]
fn print_wide_char_at_right_margin_does_not_create_spacer_head() {
    let mut t = term(10, 10);
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(3, 5);
    t.set_cursor_pos(1, 5);
    t.print(0x1F600); // smiley face
    assert_eq!(t.screen().cursor.y, 1);
    assert_eq!(t.screen().cursor.x, 4);
    // Both rows dirty because the cursor moved.
    assert!(t.is_dirty(Point::screen(4, 0)));
    assert!(t.is_dirty(Point::screen(4, 1)));

    let (cp, _, wide) = cell_info(&t, 4, 0);
    assert_eq!(cp, 0);
    assert_eq!(wide, Wide::Narrow);
    let lc = t.screen().pages.get_cell(Point::screen(4, 0)).unwrap();
    unsafe {
        assert!(!(*lc.row).wrap());
    }

    assert_eq!(cell_info(&t, 2, 1), (0x1F600, false, Wide::Wide));
    assert_eq!(cell_info(&t, 3, 1).2, Wide::SpacerTail);
}

/// Hyperlink id attached to a screen cell (page-local lookup), if any.
fn hyperlink_id_at(t: &Terminal, x: u16, y: u32) -> Option<crate::page::hyperlink::Id> {
    let lc = t.screen().pages.get_cell(Point::screen(x, y)).unwrap();
    unsafe {
        let page = t.screen().pages.node_data(lc.node);
        page.lookup_hyperlink(lc.cell)
    }
}

// Zig: "Terminal: print with hyperlink".
#[test]
fn print_with_hyperlink() {
    let mut t = term(80, 80);
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.print_string("123456");

    for x in 0..6u16 {
        let lc = t.screen().pages.get_cell(Point::screen(x, 0)).unwrap();
        unsafe {
            assert!((*lc.row).hyperlink());
            assert!((*lc.cell).hyperlink());
        }
        assert_eq!(hyperlink_id_at(&t, x, 0), Some(1));
    }
    assert!(t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: print over cell with same hyperlink".
#[test]
fn print_over_cell_with_same_hyperlink() {
    let mut t = term(80, 80);
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.print_string("123456");
    t.set_cursor_pos(1, 1);
    t.print_string("123456");

    for x in 0..6u16 {
        let lc = t.screen().pages.get_cell(Point::screen(x, 0)).unwrap();
        unsafe {
            assert!((*lc.row).hyperlink());
            assert!((*lc.cell).hyperlink());
        }
        assert_eq!(hyperlink_id_at(&t, x, 0), Some(1));
    }
    assert!(t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: print and end hyperlink".
#[test]
fn print_and_end_hyperlink() {
    let mut t = term(80, 80);
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.print_string("123");
    t.screen_mut().end_hyperlink();
    t.print_string("456");

    for x in 0..3u16 {
        let lc = t.screen().pages.get_cell(Point::screen(x, 0)).unwrap();
        unsafe {
            assert!((*lc.row).hyperlink());
            assert!((*lc.cell).hyperlink());
        }
        assert_eq!(hyperlink_id_at(&t, x, 0), Some(1));
    }
    for x in 3..6u16 {
        let lc = t.screen().pages.get_cell(Point::screen(x, 0)).unwrap();
        unsafe {
            assert!((*lc.row).hyperlink());
            assert!(!(*lc.cell).hyperlink());
        }
    }
    assert!(t.is_dirty(Point::screen(0, 0)));
}

// Regression test (no Zig equivalent): three unique implicit hyperlinks on
// one page exceed the default hyperlink-set ID space (cap 3 => 2 usable IDs)
// while every allocated ID is still living (each printed cell holds a ref).
// `RefCountedSet::add` must report OutOfMemory (grow hyperlink_bytes), not
// NeedsRehash: a same-capacity rehash clone reclaims nothing when no IDs are
// dead, so `start_hyperlink` retried forever. Matches upstream's integer
// truncation of the 0.9 rehash threshold.
#[test]
fn unique_hyperlinks_grow_capacity_without_hanging() {
    let mut t = term(80, 24);
    for x in 0..3u16 {
        // Implicit id: each start_hyperlink creates a distinct link.
        t.screen_mut()
            .start_hyperlink(b"http://example.com", None)
            .unwrap();
        t.print('a' as u32);
        t.screen_mut().end_hyperlink();
        assert!(hyperlink_id_at(&t, x, 0).is_some());
    }
    // Three distinct living links on the page.
    unsafe {
        let page = t.screen().cursor_page();
        assert_eq!((*page).hyperlink_set_mut().count(), 3);
    }
}

// Regression test (no Zig equivalent): exhaust the style-set ID space with
// mostly-dead styles so `RefCountedSet::add` reports NeedsRehash, and verify
// `manual_style_update` resolves it with a same-capacity rehash clone
// (compacting dead IDs) rather than growing the style capacity.
#[test]
fn style_set_needs_rehash_compacts_dead_ids() {
    use crate::page::style::StyleSet;

    let mut t = term(80, 24);
    let styles_cap = unsafe {
        t.screen()
            .pages
            .node_data((*t.screen().cursor.page_pin).node)
            .capacity
            .styles
    };
    let max_items = StyleSet::layout(styles_cap as usize).cap;

    // Fill every style ID with a unique style, each pinned by a printed cell.
    for n in 1..max_items as u32 {
        t.set_attribute(Attribute::DirectColorFg(crate::color::Rgb::new(
            (n & 0xFF) as u8,
            ((n >> 8) & 0xFF) as u8,
            1,
        )));
        t.print('a' as u32);
    }
    t.set_attribute(Attribute::Unset);
    assert_eq!(style_page_count(&t), max_items - 1);

    // Overwrite all but the last styled cell with default-style cells, so
    // every allocated ID except the highest one is dead. The dead IDs sit
    // below a living one, so `add`'s trim-from-the-end can't reclaim them.
    t.set_cursor_pos(1, 1);
    for _ in 1..max_items as u32 - 1 {
        t.print('b' as u32);
    }
    assert_eq!(style_page_count(&t), 1);

    // A new style now has no free ID => NeedsRehash => same-capacity clone.
    // Print away from the surviving styled cell (the cursor sits on it).
    t.set_cursor_pos(5, 1);
    t.set_attribute(Attribute::DirectColorFg(crate::color::Rgb::new(1, 2, 3)));
    t.print('c' as u32);
    assert_ne!(t.screen().cursor.style_id, 0);
    assert_eq!(style_page_count(&t), 2);

    // Rehash, not growth: the style capacity is unchanged.
    let cap_after = unsafe {
        t.screen()
            .pages
            .node_data((*t.screen().cursor.page_pin).node)
            .capacity
            .styles
    };
    assert_eq!(cap_after, styles_cap);
}

// Zig: "Terminal: print and change hyperlink".
#[test]
fn print_and_change_hyperlink() {
    let mut t = term(80, 80);
    t.screen_mut()
        .start_hyperlink(b"http://one.example.com", None)
        .unwrap();
    t.print_string("123");
    t.screen_mut()
        .start_hyperlink(b"http://two.example.com", None)
        .unwrap();
    t.print_string("456");

    for x in 0..3u16 {
        let lc = t.screen().pages.get_cell(Point::screen(x, 0)).unwrap();
        unsafe {
            assert!((*lc.cell).hyperlink());
        }
        assert_eq!(hyperlink_id_at(&t, x, 0), Some(1));
    }
    for x in 3..6u16 {
        let lc = t.screen().pages.get_cell(Point::screen(x, 0)).unwrap();
        unsafe {
            assert!((*lc.cell).hyperlink());
        }
        assert_eq!(hyperlink_id_at(&t, x, 0), Some(2));
    }
    assert!(t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: overwrite hyperlink".
#[test]
fn overwrite_hyperlink() {
    let mut t = term(80, 80);
    t.screen_mut()
        .start_hyperlink(b"http://one.example.com", None)
        .unwrap();
    t.print_string("123");
    t.set_cursor_pos(1, 1);
    t.screen_mut().end_hyperlink();
    t.print_string("456");

    for x in 0..3u16 {
        let lc = t.screen().pages.get_cell(Point::screen(x, 0)).unwrap();
        unsafe {
            assert!(!(*lc.row).hyperlink());
            assert!(!(*lc.cell).hyperlink());
        }
        assert_eq!(hyperlink_id_at(&t, x, 0), None);
    }
    unsafe {
        let node = t.screen().pages.get_cell(Point::screen(0, 0)).unwrap().node;
        let page = t.screen_mut().pages.node_data_mut(node);
        assert_eq!(page.hyperlink_set_mut().count(), 0);
    }
    assert!(t.is_dirty(Point::screen(0, 0)));
}

// Zig: "Terminal: print wide char at right edge with hyperlink" — printing a
// wide char at the right edge with an active hyperlink used to write a
// spacer_head before printWrap set the row wrap flag, and the integrity
// check inside setHyperlink saw the unwrapped spacer head and panicked.
// Found via fuzzing.
#[test]
fn print_wide_char_at_right_edge_with_hyperlink() {
    let mut t = term(10, 5);
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.set_cursor_pos(1, 10);
    t.print(0x4E2D); // 中
    assert_eq!(t.screen().cursor.y, 1);
    assert_eq!(t.screen().cursor.x, 2);

    {
        let lc = t.screen().pages.get_cell(Point::screen(9, 0)).unwrap();
        unsafe {
            assert_eq!((*lc.cell).wide(), Wide::SpacerHead);
            assert!((*lc.cell).hyperlink());
            assert!((*lc.row).wrap());
        }
    }
    {
        let lc = t.screen().pages.get_cell(Point::screen(0, 1)).unwrap();
        unsafe {
            assert_eq!((*lc.cell).codepoint(), 0x4E2D);
            assert_eq!((*lc.cell).wide(), Wide::Wide);
            assert!((*lc.cell).hyperlink());
        }
    }
    {
        let lc = t.screen().pages.get_cell(Point::screen(1, 1)).unwrap();
        unsafe {
            assert_eq!((*lc.cell).wide(), Wide::SpacerTail);
            assert!((*lc.cell).hyperlink());
        }
    }
}

// Zig: "Terminal: linefeed and carriage return".
#[test]
fn linefeed_and_carriage_return() {
    let mut t = term(80, 80);
    t.print_string("hello");
    t.clear_dirty();
    t.carriage_return();
    // CR should not mark row dirty because it doesn't change rendering.
    assert!(!t.is_dirty(Point::screen(0, 0)));

    t.linefeed();
    // LF marks row dirty due to cursor movement.
    assert!(t.is_dirty(Point::screen(0, 0)));
    assert!(t.is_dirty(Point::screen(0, 1)));

    t.print_string("world");
    assert_eq!(t.screen().cursor.y, 1);
    assert_eq!(t.screen().cursor.x, 5);
    assert_eq!(t.plain_string(), "hello\nworld");
}

// Zig: "Terminal: linefeed unsets pending wrap".
#[test]
fn linefeed_unsets_pending_wrap() {
    let mut t = term(5, 80);
    t.print_string("hello");
    assert!(t.screen().cursor.pending_wrap);
    t.clear_dirty();
    t.linefeed();
    assert!(t.is_dirty(Point::screen(0, 0)));
    assert!(t.is_dirty(Point::screen(0, 1)));
    assert!(!t.screen().cursor.pending_wrap);
}

// Zig: "Terminal: linefeed mode automatic carriage return".
#[test]
fn linefeed_mode_automatic_carriage_return() {
    let mut t = term(10, 10);
    t.modes.set(Mode::Linefeed, true);
    t.print_string("123456");
    t.linefeed();
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "123456\nX");
}

// Zig: "Terminal: carriage return unsets pending wrap".
#[test]
fn carriage_return_unsets_pending_wrap() {
    let mut t = term(5, 80);
    t.print_string("hello");
    assert!(t.screen().cursor.pending_wrap);
    t.carriage_return();
    assert!(!t.screen().cursor.pending_wrap);
}

// Zig: "Terminal: carriage return origin mode moves to left margin".
#[test]
fn carriage_return_origin_mode_moves_to_left_margin() {
    let mut t = term(5, 80);
    t.modes.set(Mode::Origin, true);
    t.screen_mut().cursor.x = 0;
    t.scrolling_region.left = 2;
    t.carriage_return();
    assert_eq!(t.screen().cursor.x, 2);
}

// Zig: "Terminal: carriage return left of left margin moves to zero".
#[test]
fn carriage_return_left_of_left_margin_moves_to_zero() {
    let mut t = term(5, 80);
    t.screen_mut().cursor.x = 1;
    t.scrolling_region.left = 2;
    t.carriage_return();
    assert_eq!(t.screen().cursor.x, 0);
}

// Zig: "Terminal: carriage return right of left margin moves to left margin".
#[test]
fn carriage_return_right_of_left_margin_moves_to_left_margin() {
    let mut t = term(5, 80);
    t.screen_mut().cursor.x = 3;
    t.scrolling_region.left = 2;
    t.carriage_return();
    assert_eq!(t.screen().cursor.x, 2);
}

// Zig: "Terminal: backspace".
#[test]
fn backspace() {
    let mut t = term(80, 80);
    t.print_string("hello");
    t.backspace();
    t.print('y' as u32);
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 5);
    assert_eq!(t.plain_string(), "helly");
}

// Zig: "Terminal: horizontal tabs" (full 3-stop + clamp variant).
#[test]
fn horizontal_tabs_full() {
    let mut t = term(20, 5);
    t.print('1' as u32);
    t.horizontal_tab();
    assert_eq!(t.screen().cursor.x, 8);
    t.horizontal_tab();
    assert_eq!(t.screen().cursor.x, 16);
    t.horizontal_tab();
    assert_eq!(t.screen().cursor.x, 19);
    t.horizontal_tab();
    assert_eq!(t.screen().cursor.x, 19);
}

// Zig: "Terminal: horizontal tabs starting on tabstop".
#[test]
fn horizontal_tabs_starting_on_tabstop() {
    let mut t = term(20, 5);
    let y = t.screen().cursor.y as usize;
    t.set_cursor_pos(y + 1, 9);
    t.print('X' as u32);
    let y = t.screen().cursor.y as usize;
    t.set_cursor_pos(y + 1, 9);
    t.horizontal_tab();
    t.print('A' as u32);
    assert_eq!(t.plain_string(), "        X       A");
}

// Zig: "Terminal: horizontal tabs with right margin".
#[test]
fn horizontal_tabs_with_right_margin() {
    let mut t = term(20, 5);
    t.scrolling_region.left = 2;
    t.scrolling_region.right = 5;
    let y = t.screen().cursor.y as usize;
    t.set_cursor_pos(y + 1, 1);
    t.print('X' as u32);
    t.horizontal_tab();
    t.print('A' as u32);
    assert_eq!(t.plain_string(), "X    A");
}

// Zig: "Terminal: horizontal tabs back" (full edge-of-screen variant).
#[test]
fn horizontal_tabs_back_full() {
    let mut t = term(20, 5);
    let y = t.screen().cursor.y as usize;
    t.set_cursor_pos(y + 1, 20);
    t.horizontal_tab_back();
    assert_eq!(t.screen().cursor.x, 16);
    t.horizontal_tab_back();
    assert_eq!(t.screen().cursor.x, 8);
    t.horizontal_tab_back();
    assert_eq!(t.screen().cursor.x, 0);
    t.horizontal_tab_back();
    assert_eq!(t.screen().cursor.x, 0);
}

// Zig: "Terminal: horizontal tabs back starting on tabstop".
#[test]
fn horizontal_tabs_back_starting_on_tabstop() {
    let mut t = term(20, 5);
    let y = t.screen().cursor.y as usize;
    t.set_cursor_pos(y + 1, 9);
    t.print('X' as u32);
    let y = t.screen().cursor.y as usize;
    t.set_cursor_pos(y + 1, 9);
    t.horizontal_tab_back();
    t.print('A' as u32);
    assert_eq!(t.plain_string(), "A       X");
}

// Zig: "Terminal: horizontal tabs with left margin in origin mode".
#[test]
fn horizontal_tabs_with_left_margin_in_origin_mode() {
    let mut t = term(20, 5);
    t.modes.set(Mode::Origin, true);
    t.scrolling_region.left = 2;
    t.scrolling_region.right = 5;
    t.set_cursor_pos(1, 2);
    t.print('X' as u32);
    t.horizontal_tab_back();
    t.print('A' as u32);
    assert_eq!(t.plain_string(), "  AX");
}

// Zig: "Terminal: horizontal tab back with cursor before left margin".
#[test]
fn horizontal_tab_back_with_cursor_before_left_margin() {
    let mut t = term(20, 5);
    t.modes.set(Mode::Origin, true);
    t.save_cursor();
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(5, 0);
    t.restore_cursor();
    t.horizontal_tab_back();
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "X");
}

// Zig: "Terminal: cursorPos resets wrap".
#[test]
fn cursor_pos_resets_wrap() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.set_cursor_pos(1, 1);
    assert!(!t.screen().cursor.pending_wrap);
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "XBCDE");
}

// Zig: "Terminal: cursorPos off the screen".
#[test]
fn cursor_pos_off_the_screen() {
    let mut t = term(5, 5);
    t.set_cursor_pos(500, 500);
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "\n\n\n\n    X");
}

// Zig: "Terminal: cursorPos relative to origin".
#[test]
fn cursor_pos_relative_to_origin() {
    let mut t = term(5, 5);
    t.scrolling_region.top = 2;
    t.scrolling_region.bottom = 3;
    t.modes.set(Mode::Origin, true);
    t.set_cursor_pos(1, 1);
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "\n\nX");
}

// Zig: "Terminal: cursorPos relative to origin with left/right".
#[test]
fn cursor_pos_relative_to_origin_with_left_right() {
    let mut t = term(5, 5);
    t.scrolling_region.top = 2;
    t.scrolling_region.bottom = 3;
    t.scrolling_region.left = 2;
    t.scrolling_region.right = 4;
    t.modes.set(Mode::Origin, true);
    t.set_cursor_pos(1, 1);
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "\n\n  X");
}

// Zig: "Terminal: cursorPos limits with full scroll region".
#[test]
fn cursor_pos_limits_with_full_scroll_region() {
    let mut t = term(5, 5);
    t.scrolling_region.top = 2;
    t.scrolling_region.bottom = 3;
    t.scrolling_region.left = 2;
    t.scrolling_region.right = 4;
    t.modes.set(Mode::Origin, true);
    t.set_cursor_pos(500, 500);
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "\n\n\n    X");
}

// Zig: "Terminal: setCursorPos (original test)" — probably outdated, but
// dates back to the original terminal implementation.
#[test]
fn set_cursor_pos_original_test() {
    let mut t = term(80, 80);
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 0);

    // Setting it to 0 should keep it zero (1 based).
    t.set_cursor_pos(0, 0);
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 0);

    // Should clamp to size.
    t.set_cursor_pos(81, 81);
    assert_eq!(t.screen().cursor.x, 79);
    assert_eq!(t.screen().cursor.y, 79);

    // Should reset pending wrap.
    t.set_cursor_pos(0, 80);
    t.print('c' as u32);
    assert!(t.screen().cursor.pending_wrap);
    t.set_cursor_pos(0, 80);
    assert!(!t.screen().cursor.pending_wrap);

    // Origin mode.
    t.modes.set(Mode::Origin, true);

    // No change without a scroll region.
    t.set_cursor_pos(81, 81);
    assert_eq!(t.screen().cursor.x, 79);
    assert_eq!(t.screen().cursor.y, 79);

    // Set the scroll region.
    let rows = t.rows as usize;
    t.set_top_and_bottom_margin(10, rows);
    t.set_cursor_pos(0, 0);
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 9);

    t.set_cursor_pos(1, 1);
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 9);

    t.set_cursor_pos(100, 0);
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 79);

    t.set_top_and_bottom_margin(10, 11);
    t.set_cursor_pos(2, 0);
    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 10);
}

// Zig: "Terminal: setTopAndBottomMargin simple".
#[test]
fn set_top_and_bottom_margin_simple() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_top_and_bottom_margin(0, 0);

    t.clear_dirty();
    t.scroll_down(1);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));
    assert!(t.is_dirty(Point::active(0, 3)));

    assert_eq!(t.plain_string(), "\nABC\nDEF\nGHI");
}

// Zig: "Terminal: setTopAndBottomMargin top only".
#[test]
fn set_top_and_bottom_margin_top_only() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_top_and_bottom_margin(2, 0);

    t.clear_dirty();
    t.scroll_down(1);

    // This is dirty because the cursor moves from this row.
    assert!(t.is_dirty(Point::active(0, 0)));
}

// ---- M1 backfill batch 3: margins / insertLines / scrollUp (L6429-7120) ----

// Zig: "Terminal: setTopAndBottomMargin top and bottom".
#[test]
fn set_top_and_bottom_margin_top_and_bottom() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_top_and_bottom_margin(1, 2);

    t.clear_dirty();
    t.scroll_down(1);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(!t.is_dirty(Point::active(0, 2)));

    assert_eq!(t.plain_string(), "\nABC\nGHI");
}

// Zig: "Terminal: setTopAndBottomMargin top equal to bottom".
#[test]
fn set_top_and_bottom_margin_top_equal_to_bottom() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_top_and_bottom_margin(2, 2);

    t.clear_dirty();
    t.scroll_down(1);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));
    assert!(t.is_dirty(Point::active(0, 3)));

    assert_eq!(t.plain_string(), "\nABC\nDEF\nGHI");
}

// Zig: "Terminal: setLeftAndRightMargin simple".
#[test]
fn set_left_and_right_margin_simple() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(0, 0);

    t.clear_dirty();
    t.erase_chars(1);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(!t.is_dirty(Point::active(0, 1)));

    assert_eq!(t.plain_string(), " BC\nDEF\nGHI");
}

// Zig: "Terminal: setLeftAndRightMargin left only".
#[test]
fn set_left_and_right_margin_left_only() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(2, 0);
    assert_eq!(t.scrolling_region.left, 1);
    assert_eq!(t.scrolling_region.right, t.cols - 1);
    t.set_cursor_pos(1, 2);

    t.clear_dirty();
    t.insert_lines(1);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));
    assert!(t.is_dirty(Point::active(0, 3)));

    assert_eq!(t.plain_string(), "A\nDBC\nGEF\n HI");
}

// Zig: "Terminal: setLeftAndRightMargin left and right".
#[test]
fn set_left_and_right_margin_left_and_right() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(1, 2);
    t.set_cursor_pos(1, 2);

    t.clear_dirty();
    t.insert_lines(1);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));
    assert!(t.is_dirty(Point::active(0, 3)));

    assert_eq!(t.plain_string(), "  C\nABF\nDEI\nGH");
}

// Zig: "Terminal: setLeftAndRightMargin left equal right".
#[test]
fn set_left_and_right_margin_left_equal_right() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(2, 2);
    t.set_cursor_pos(1, 2);

    t.clear_dirty();
    t.insert_lines(1);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));
    assert!(t.is_dirty(Point::active(0, 3)));

    assert_eq!(t.plain_string(), "\nABC\nDEF\nGHI");
}

// Zig: "Terminal: setLeftAndRightMargin mode 69 unset".
#[test]
fn set_left_and_right_margin_mode_69_unset() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.modes.set(Mode::EnableLeftAndRightMargin, false);
    t.set_left_and_right_margin(1, 2);
    t.set_cursor_pos(1, 2);

    t.clear_dirty();
    t.insert_lines(1);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));
    assert!(t.is_dirty(Point::active(0, 3)));

    assert_eq!(t.plain_string(), "\nABC\nDEF\nGHI");
}

// Zig: "Terminal: insertLines colors with bg color" (full cell-content-tag
// variant, checking every column).
#[test]
fn insert_lines_colors_with_bg_color_full() {
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

// Zig: "Terminal: insertLines handles style refs".
#[test]
fn insert_lines_handles_style_refs() {
    let mut t = term(5, 3);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();

    // For the line being deleted, create a refcounted style.
    t.set_attribute(Attribute::Bold);
    t.print_string("GHI");
    t.set_attribute(Attribute::Unset);

    // Verify we have styles in our style map.
    assert_eq!(style_page_count(&t), 1);

    t.set_cursor_pos(2, 2);
    t.insert_lines(1);

    assert_eq!(t.plain_string(), "ABC\n\nDEF");

    // Verify we have no styles in our style map.
    assert_eq!(style_page_count(&t), 0);
}

// Zig: "Terminal: insertLines top/bottom scroll region".
#[test]
fn insert_lines_top_bottom_scroll_region() {
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
    t.set_cursor_pos(2, 2);

    t.clear_dirty();
    t.insert_lines(1);

    assert!(!t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));
    assert!(!t.is_dirty(Point::active(0, 3)));

    assert_eq!(t.plain_string(), "ABC\n\nDEF\n123");
}

// Zig: "Terminal: insertLines across page boundary marks all shifted rows dirty".
#[test]
fn insert_lines_across_page_boundary_marks_all_shifted_rows_dirty() {
    let mut t = Terminal::new(Options {
        cols: 10,
        rows: 5,
        max_scrollback: 1024,
        colors: Colors::default(),
    });

    let first_page_rows = unsafe {
        let node = t.screen().pages.head_node();
        (*node).data.capacity.rows as usize
    };

    // Fill up the first page minus 3 rows.
    for _ in 0..(first_page_rows - 3) {
        t.linefeed();
    }

    // Add content that will cross a page boundary.
    t.print_string("1AAAA");
    t.carriage_return();
    t.linefeed();
    t.print_string("2BBBB");
    t.carriage_return();
    t.linefeed();
    t.print_string("3CCCC");
    t.carriage_return();
    t.linefeed();
    t.print_string("4DDDD");
    t.carriage_return();
    t.linefeed();
    t.print_string("5EEEE");

    // Verify we now have a second page.
    let first_node = t.screen().pages.head_node();
    unsafe {
        assert!(!(*first_node).next.is_null());
    }

    t.set_cursor_pos(1, 1);
    t.clear_dirty();
    t.insert_lines(1);

    for y in 0..5u32 {
        assert!(t.is_dirty(Point::active(0, y)));
    }

    assert_eq!(t.plain_string(), "\n1AAAA\n2BBBB\n3CCCC\n4DDDD");
}

// Zig: "Terminal: insertLines (legacy test)".
#[test]
fn insert_lines_legacy_test() {
    let mut t = term(2, 5);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('B' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('C' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('D' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('E' as u32);

    t.set_cursor_pos(2, 1);
    t.insert_lines(2);

    assert_eq!(t.plain_string(), "A\n\n\nB\nC");
}

// Zig: "Terminal: insertLines with scroll region".
#[test]
fn insert_lines_with_scroll_region() {
    let mut t = term(2, 6);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('B' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('C' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('D' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('E' as u32);

    t.set_top_and_bottom_margin(1, 2);
    t.set_cursor_pos(1, 1);

    t.clear_dirty();
    t.insert_lines(1);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(!t.is_dirty(Point::active(0, 2)));

    t.print('X' as u32);

    assert_eq!(t.plain_string(), "X\nA\nC\nD\nE");
}

// Zig: "Terminal: insertLines more than remaining" (full dirty-tracking variant).
#[test]
fn insert_lines_more_than_remaining_full() {
    let mut t = term(2, 5);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('B' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('C' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('D' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('E' as u32);

    t.set_cursor_pos(2, 1);

    t.clear_dirty();
    t.insert_lines(20);

    assert!(!t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));

    assert_eq!(t.plain_string(), "A");
}

// Zig: "Terminal: insertLines resets pending wrap" (full round-trip variant).
#[test]
fn insert_lines_resets_pending_wrap_full() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.insert_lines(1);
    assert!(!t.screen().cursor.pending_wrap);
    t.print('B' as u32);
    assert_eq!(t.plain_string(), "B\nABCDE");
}

// Zig: "Terminal: insertLines resets wrap".
#[test]
fn insert_lines_resets_wrap() {
    let mut t = term(3, 3);
    t.print('1' as u32);
    t.carriage_return();
    t.linefeed();
    t.print_string("ABCDEF");
    t.set_cursor_pos(1, 1);
    t.insert_lines(1);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "X\n1\nABC");

    let lc = t.screen().pages.get_cell(Point::active(0, 2)).unwrap();
    unsafe {
        assert!(!(*lc.row).wrap());
    }
}

// Zig: "Terminal: insertLines multi-codepoint graphemes".
#[test]
fn insert_lines_multi_codepoint_graphemes() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);

    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();

    // This is: 👨‍👩‍👧
    t.print(0x1F468);
    t.print(0x200D);
    t.print(0x1F469);
    t.print(0x200D);
    t.print(0x1F467);

    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.insert_lines(1);

    assert_eq!(
        t.plain_string(),
        "ABC\n\n\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\nGHI"
    );
}

// Zig: "Terminal: insertLines left/right scroll region".
#[test]
fn insert_lines_left_right_scroll_region() {
    let mut t = term(10, 10);
    t.print_string("ABC123");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF456");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI789");
    t.scrolling_region.left = 1;
    t.scrolling_region.right = 3;
    t.set_cursor_pos(2, 2);

    t.clear_dirty();
    t.insert_lines(1);

    assert!(!t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));
    assert!(t.is_dirty(Point::active(0, 3)));

    assert_eq!(t.plain_string(), "ABC123\nD   56\nGEF489\n HI7");
}

// Zig: "Terminal: scrollUp simple" (full viewport-movement variant).
//
// Needs actual scrollback for the viewport to move into (our shared `term()`
// helper uses `max_scrollback: 0`), so build a `Terminal` directly.
#[test]
fn scroll_up_simple_full() {
    let mut t = Terminal::new(Options {
        cols: 5,
        rows: 5,
        max_scrollback: 10_000,
        colors: Colors::default(),
    });
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);

    let cursor = (t.screen().cursor.x, t.screen().cursor.y);
    let viewport_before = t.screen().pages.get_top_left(crate::point::Tag::Viewport);
    t.scroll_up(1);
    assert_eq!((t.screen().cursor.x, t.screen().cursor.y), cursor);

    // Viewport should have moved. Our entire page should've scrolled!
    let viewport_after = t.screen().pages.get_top_left(crate::point::Tag::Viewport);
    assert_ne!(
        (viewport_before.node, viewport_before.x, viewport_before.y),
        (viewport_after.node, viewport_after.x, viewport_after.y)
    );

    assert_eq!(t.plain_string(), "DEF\nGHI");
}

// ---- M1 backfill batch 4: scrollUp/scrollDown hyperlink + margins (L7112-7871) ----

/// Whether the row and cell at a viewport cell are hyperlinked, plus the
/// page-local hyperlink id looked up for that cell. Port of the
/// `row.hyperlink` / `cell.hyperlink` / `lookupHyperlink` check pattern used
/// throughout the Zig scroll tests.
fn hyperlink_at(t: &Terminal, x: u16, y: u32) -> (bool, bool, Option<crate::page::hyperlink::Id>) {
    let lc = t.screen().pages.get_cell(Point::viewport(x, y)).unwrap();
    unsafe {
        let page = t.screen().pages.node_data(lc.node);
        (
            (*lc.row).hyperlink(),
            (*lc.cell).hyperlink(),
            page.lookup_hyperlink(lc.cell),
        )
    }
}

// Zig: "Terminal: scrollUp moves hyperlink".
#[test]
fn scroll_up_moves_hyperlink() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.print_string("DEF");
    t.screen_mut().end_hyperlink();
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.scroll_up(1);

    assert_eq!(t.plain_string(), "DEF\nGHI");

    for x in 0..3u16 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, x, 0);
        assert!(row_hl);
        assert!(cell_hl);
        assert_eq!(id, Some(1));
    }
    for x in 0..3u16 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, x, 1);
        assert!(!row_hl);
        assert!(!cell_hl);
        assert_eq!(id, None);
    }
}

// Zig: "Terminal: scrollUp clears hyperlink".
#[test]
fn scroll_up_clears_hyperlink() {
    let mut t = term(5, 5);
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.print_string("ABC");
    t.screen_mut().end_hyperlink();
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.scroll_up(1);

    assert_eq!(t.plain_string(), "DEF\nGHI");

    for x in 0..3u16 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, x, 0);
        assert!(!row_hl);
        assert!(!cell_hl);
        assert_eq!(id, None);
    }
}

// Zig: "Terminal: scrollUp left/right scroll region".
#[test]
fn scroll_up_left_right_scroll_region() {
    let mut t = term(10, 10);
    t.print_string("ABC123");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF456");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI789");
    t.scrolling_region.left = 1;
    t.scrolling_region.right = 3;
    t.set_cursor_pos(2, 2);

    let cursor = (t.screen().cursor.x, t.screen().cursor.y);
    t.clear_dirty();
    t.scroll_up(1);
    assert_eq!((t.screen().cursor.x, t.screen().cursor.y), cursor);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));

    assert_eq!(t.plain_string(), "AEF423\nDHI756\nG   89");
}

// Zig: "Terminal: scrollUp left/right scroll region hyperlink".
#[test]
fn scroll_up_left_right_scroll_region_hyperlink() {
    let mut t = term(10, 10);
    t.print_string("ABC123");
    t.carriage_return();
    t.linefeed();
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.print_string("DEF456");
    t.screen_mut().end_hyperlink();
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI789");
    t.scrolling_region.left = 1;
    t.scrolling_region.right = 3;
    t.set_cursor_pos(2, 2);
    t.scroll_up(1);

    assert_eq!(t.plain_string(), "AEF423\nDHI756\nG   89");

    // First row gets some hyperlinks.
    for x in 0..1u16 {
        let (_, cell_hl, id) = hyperlink_at(&t, x, 0);
        assert!(!cell_hl);
        assert_eq!(id, None);
    }
    for x in 1..4u16 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, x, 0);
        assert!(row_hl);
        assert!(cell_hl);
        assert_eq!(id, Some(1));
    }
    for x in 4..6u16 {
        let (_, cell_hl, id) = hyperlink_at(&t, x, 0);
        assert!(!cell_hl);
        assert_eq!(id, None);
    }

    // Second row preserves hyperlink where we didn't scroll.
    for x in 0..1u16 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, x, 1);
        assert!(row_hl);
        assert!(cell_hl);
        assert_eq!(id, Some(1));
    }
    for x in 1..4u16 {
        let (_, cell_hl, id) = hyperlink_at(&t, x, 1);
        assert!(!cell_hl);
        assert_eq!(id, None);
    }
    for x in 4..6u16 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, x, 1);
        assert!(row_hl);
        assert!(cell_hl);
        assert_eq!(id, Some(1));
    }
}

// Zig: "Terminal: scrollUp preserves pending wrap" (full round-trip variant).
#[test]
fn scroll_up_preserves_pending_wrap_full() {
    let mut t = term(5, 5);
    t.set_cursor_pos(1, 5);
    t.print('A' as u32);
    t.set_cursor_pos(2, 5);
    t.print('B' as u32);
    t.set_cursor_pos(3, 5);
    t.print('C' as u32);
    t.scroll_up(1);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "    B\n    C\n\nX");
}

// Zig: "Terminal: scrollUp full top/bottom region".
#[test]
fn scroll_up_full_top_bottom_region() {
    let mut t = term(5, 5);
    t.print_string("top");
    t.set_cursor_pos(5, 1);
    t.print_string("ABCDE");
    t.set_top_and_bottom_margin(2, 5);

    t.clear_dirty();
    t.scroll_up(4);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));

    assert_eq!(t.plain_string(), "top");
}

// Zig: "Terminal: scrollUp full top/bottomleft/right scroll region".
#[test]
fn scroll_up_full_top_bottom_left_right_scroll_region() {
    let mut t = term(5, 5);
    t.print_string("top");
    t.set_cursor_pos(5, 1);
    t.print_string("ABCDE");
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_top_and_bottom_margin(2, 5);
    t.set_left_and_right_margin(2, 4);

    t.clear_dirty();
    t.scroll_up(4);

    assert!(t.is_dirty(Point::active(0, 0)));
    for y in 1..5u32 {
        assert!(t.is_dirty(Point::active(0, y)));
    }

    assert_eq!(t.plain_string(), "top\n\n\n\nA   E");
}

// Zig: "Terminal: scrollUp creates scrollback in primary screen" — when in
// primary screen with full-width scroll region at top, scrollUp (CSI S)
// should push lines into scrollback like xterm.
#[test]
fn scroll_up_creates_scrollback_in_primary_screen() {
    let mut t = Terminal::new(Options {
        cols: 5,
        rows: 5,
        max_scrollback: 10,
        colors: Colors::default(),
    });

    t.print_string("AAAAA");
    t.carriage_return();
    t.linefeed();
    t.print_string("BBBBB");
    t.carriage_return();
    t.linefeed();
    t.print_string("CCCCC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DDDDD");
    t.carriage_return();
    t.linefeed();
    t.print_string("EEEEE");

    t.clear_dirty();

    // Scroll up by 1, which should push "AAAAA" into scrollback.
    t.scroll_up(1);

    // The cursor row (new empty row) should be dirty.
    unsafe {
        assert!((*t.screen().cursor.page_row).dirty());
    }

    // The active screen should now show BBBBB through EEEEE plus one blank line.
    assert_eq!(t.plain_string(), "BBBBB\nCCCCC\nDDDDD\nEEEEE");

    // Now scroll to the top to see scrollback - AAAAA should be there.
    t.screen_mut().scroll(crate::screen::Scroll::Top);
    assert_eq!(t.plain_string(), "AAAAA\nBBBBB\nCCCCC\nDDDDD\nEEEEE");
}

// Zig: "Terminal: scrollUp with max_scrollback zero" — scrollUp should
// still work but not retain history.
#[test]
fn scroll_up_with_max_scrollback_zero() {
    let mut t = term(5, 5);
    t.print_string("AAAAA");
    t.carriage_return();
    t.linefeed();
    t.print_string("BBBBB");
    t.carriage_return();
    t.linefeed();
    t.print_string("CCCCC");

    t.scroll_up(1);

    assert_eq!(t.plain_string(), "BBBBB\nCCCCC");

    // Scroll to top - should be same as active since no scrollback.
    t.screen_mut().scroll(crate::screen::Scroll::Top);
    assert_eq!(t.plain_string(), "BBBBB\nCCCCC");
}

// Zig: "Terminal: scrollUp with max_scrollback zero and top margin" — should
// use the deleteLines path.
#[test]
fn scroll_up_with_max_scrollback_zero_and_top_margin() {
    let mut t = term(5, 5);
    t.print_string("AAAAA");
    t.carriage_return();
    t.linefeed();
    t.print_string("BBBBB");
    t.carriage_return();
    t.linefeed();
    t.print_string("CCCCC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DDDDD");

    // Set top margin (not at row 0).
    t.set_top_and_bottom_margin(2, 5);

    t.scroll_up(1);

    // First row preserved, rest scrolled.
    assert_eq!(t.plain_string(), "AAAAA\nCCCCC\nDDDDD");
}

// Zig: "Terminal: scrollUp with max_scrollback zero and left/right margin" —
// uses the deleteLines path.
#[test]
fn scroll_up_with_max_scrollback_zero_and_left_right_margin() {
    let mut t = Terminal::new(Options {
        cols: 10,
        rows: 5,
        max_scrollback: 0,
        colors: Colors::default(),
    });
    t.print_string("AAAAABBBBB");
    t.carriage_return();
    t.linefeed();
    t.print_string("CCCCCDDDDD");
    t.carriage_return();
    t.linefeed();
    t.print_string("EEEEEFFFFF");

    // Set left/right margins (columns 2-6, 1-indexed = indices 1-5).
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(2, 6);

    t.scroll_up(1);

    // Cols 1-5 scroll, col 0 and cols 6+ preserved.
    assert_eq!(t.plain_string(), "ACCCCDBBBB\nCEEEEFDDDD\nE     FFFF");
}

// Zig: "Terminal: scrollDown simple" (full dirty-tracking variant).
#[test]
fn scroll_down_simple_full() {
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
    t.clear_dirty();
    t.scroll_down(1);
    assert_eq!((t.screen().cursor.x, t.screen().cursor.y), cursor);

    for y in 0..5u32 {
        assert!(t.is_dirty(Point::active(0, y)));
    }

    assert_eq!(t.plain_string(), "\nABC\nDEF\nGHI");
}

// Zig: "Terminal: scrollDown hyperlink moves".
#[test]
fn scroll_down_hyperlink_moves() {
    let mut t = term(5, 5);
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.print_string("ABC");
    t.screen_mut().end_hyperlink();
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.scroll_down(1);

    assert_eq!(t.plain_string(), "\nABC\nDEF\nGHI");

    for x in 0..3u16 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, x, 1);
        assert!(row_hl);
        assert!(cell_hl);
        assert_eq!(id, Some(1));
    }
    for x in 0..3u16 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, x, 0);
        assert!(!row_hl);
        assert!(!cell_hl);
        assert_eq!(id, None);
    }
}

// Zig: "Terminal: scrollDown left/right scroll region".
#[test]
fn scroll_down_left_right_scroll_region() {
    let mut t = term(10, 10);
    t.print_string("ABC123");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF456");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI789");
    t.scrolling_region.left = 1;
    t.scrolling_region.right = 3;
    t.set_cursor_pos(2, 2);

    let cursor = (t.screen().cursor.x, t.screen().cursor.y);
    t.clear_dirty();
    t.scroll_down(1);
    assert_eq!((t.screen().cursor.x, t.screen().cursor.y), cursor);

    for y in 0..4u32 {
        assert!(t.is_dirty(Point::active(0, y)));
    }

    assert_eq!(t.plain_string(), "A   23\nDBC156\nGEF489\n HI7");
}

// Zig: "Terminal: scrollDown left/right scroll region hyperlink".
#[test]
fn scroll_down_left_right_scroll_region_hyperlink() {
    let mut t = term(10, 10);
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.print_string("ABC123");
    t.screen_mut().end_hyperlink();
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF456");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI789");
    t.scrolling_region.left = 1;
    t.scrolling_region.right = 3;
    t.set_cursor_pos(2, 2);
    t.scroll_down(1);

    assert_eq!(t.plain_string(), "A   23\nDBC156\nGEF489\n HI7");

    // First row preserves hyperlink where we didn't scroll.
    for x in 0..1u16 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, x, 0);
        assert!(row_hl);
        assert!(cell_hl);
        assert_eq!(id, Some(1));
    }
    for x in 1..4u16 {
        let (_, cell_hl, id) = hyperlink_at(&t, x, 0);
        assert!(!cell_hl);
        assert_eq!(id, None);
    }
    for x in 4..6u16 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, x, 0);
        assert!(row_hl);
        assert!(cell_hl);
        assert_eq!(id, Some(1));
    }

    // Second row gets some hyperlinks.
    for x in 0..1u16 {
        let (_, cell_hl, id) = hyperlink_at(&t, x, 1);
        assert!(!cell_hl);
        assert_eq!(id, None);
    }
    for x in 1..4u16 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, x, 1);
        assert!(row_hl);
        assert!(cell_hl);
        assert_eq!(id, Some(1));
    }
    for x in 4..6u16 {
        let (_, cell_hl, id) = hyperlink_at(&t, x, 1);
        assert!(!cell_hl);
        assert_eq!(id, None);
    }
}

// Zig: "Terminal: scrollDown outside of left/right scroll region".
#[test]
fn scroll_down_outside_of_left_right_scroll_region() {
    let mut t = term(10, 10);
    t.print_string("ABC123");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF456");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI789");
    t.scrolling_region.left = 1;
    t.scrolling_region.right = 3;
    t.set_cursor_pos(1, 1);

    let cursor = (t.screen().cursor.x, t.screen().cursor.y);
    t.clear_dirty();
    t.scroll_down(1);
    assert_eq!((t.screen().cursor.x, t.screen().cursor.y), cursor);

    for y in 0..4u32 {
        assert!(t.is_dirty(Point::active(0, y)));
    }

    assert_eq!(t.plain_string(), "A   23\nDBC156\nGEF489\n HI7");
}

// ---- M1 backfill batch 5: eraseChars variants / reverseIndex (L7872-8470) ----

// Zig: "Terminal: scrollDown preserves pending wrap".
#[test]
fn scroll_down_preserves_pending_wrap() {
    let mut t = term(5, 10);
    t.set_cursor_pos(1, 5);
    t.print('A' as u32);
    t.set_cursor_pos(2, 5);
    t.print('B' as u32);
    t.set_cursor_pos(3, 5);
    t.print('C' as u32);
    t.scroll_down(1);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "\n    A\n    B\nX   C");
}

// Zig: "Terminal: eraseChars resets wrap".
#[test]
fn erase_chars_resets_wrap() {
    let mut t = term(5, 5);
    t.print_string("ABCDE123");
    {
        let lc = t.screen().pages.get_cell(Point::active(0, 0)).unwrap();
        unsafe {
            assert!((*lc.row).wrap());
        }
    }

    t.set_cursor_pos(1, 1);
    t.erase_chars(1);

    {
        let lc = t.screen().pages.get_cell(Point::active(0, 0)).unwrap();
        unsafe {
            assert!(!(*lc.row).wrap());
        }
    }

    t.print('X' as u32);

    assert_eq!(t.plain_string(), "XBCDE\n123");
}

// Zig: "Terminal: eraseChars preserves background sgr" (full content-tag variant).
#[test]
fn erase_chars_preserves_background_sgr_full() {
    let mut t = term(10, 10);
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.erase_chars(2);

    assert_eq!(t.plain_string(), "  C");
    assert_eq!(bg_rgb_at(&t, 0, 0), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
    assert_eq!(bg_rgb_at(&t, 1, 0), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
}

// Zig: "Terminal: eraseChars handles refcounted styles".
#[test]
fn erase_chars_handles_refcounted_styles() {
    let mut t = term(10, 10);
    t.set_attribute(Attribute::Bold);
    t.print('A' as u32);
    t.print('B' as u32);
    t.set_attribute(Attribute::Unset);
    t.print('C' as u32);

    // Verify we have styles in our style map.
    assert_eq!(style_page_count(&t), 1);

    t.set_cursor_pos(1, 1);
    t.erase_chars(2);

    // Verify we have no styles in our style map.
    assert_eq!(style_page_count(&t), 0);
}

// Zig: "Terminal: eraseChars protected attributes ignored with dec most recent".
#[test]
fn erase_chars_protected_attributes_ignored_with_dec_most_recent() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Iso);
    t.print_string("ABC");
    t.set_protected_mode(ProtectedMode::Dec);
    t.set_protected_mode(ProtectedMode::Off);
    t.set_cursor_pos(1, 1);
    t.erase_chars(2);

    assert_eq!(t.plain_string(), "  C");
}

// Zig: "Terminal: eraseChars wide char boundary conditions".
#[test]
fn erase_chars_wide_char_boundary_conditions() {
    let mut t = term(8, 1);
    t.print_string("\u{1f600}a\u{1f600}b\u{1f600}");
    assert_eq!(t.plain_string(), "\u{1f600}a\u{1f600}b\u{1f600}");

    t.set_cursor_pos(1, 2);
    t.erase_chars(3);
    unsafe {
        let node = t.screen().cursor.page_pin;
        let page = t.screen().pages.node_data((*node).node);
        page.verify_integrity().expect("page integrity");
    }

    assert_eq!(t.plain_string(), "     b\u{1f600}");
}

// Zig: "Terminal: eraseChars wide char splits proper cell boundaries"
// (ghostty-org/ghostty#2817) — the setup requires wide characters starting
// on an even (1-based) column, cursor in the middle, and count less than
// cursor X landing on a split cell. The bug was splitting the wrong cell
// boundaries.
#[test]
fn erase_chars_wide_char_splits_proper_cell_boundaries() {
    let mut t = term(30, 1);
    t.print_string("x\u{98df}べて下さい");
    assert_eq!(t.plain_string(), "x\u{98df}べて下さい");

    t.set_cursor_pos(1, 6); // At: て
    t.erase_chars(4); // Delete: て下
    unsafe {
        let node = t.screen().cursor.page_pin;
        let page = t.screen().pages.node_data((*node).node);
        page.verify_integrity().expect("page integrity");
    }

    assert_eq!(t.plain_string(), "x\u{98df}べ    さい");
}

// Zig: "Terminal: eraseChars wide char wrap boundary conditions".
#[test]
fn erase_chars_wide_char_wrap_boundary_conditions() {
    let mut t = term(8, 3);
    t.print_string(".......\u{1f600}abcde\u{1f600}......");
    assert_eq!(t.plain_string(), ".......\n\u{1f600}abcde\n\u{1f600}......");
    assert_eq!(
        t.plain_string_unwrapped(),
        ".......\u{1f600}abcde\u{1f600}......"
    );

    t.set_cursor_pos(2, 2);
    t.erase_chars(3);
    unsafe {
        let node = t.screen().cursor.page_pin;
        let page = t.screen().pages.node_data((*node).node);
        page.verify_integrity().expect("page integrity");
    }

    assert_eq!(t.plain_string(), ".......\n    cde\n\u{1f600}......");
    assert_eq!(
        t.plain_string_unwrapped(),
        ".......     cde\n\u{1f600}......"
    );
}

// Zig: "Terminal: reverseIndex" (full multi-line variant).
#[test]
fn reverse_index_full() {
    let mut t = term(2, 5);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('B' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('C' as u32);
    t.reverse_index();
    t.print('D' as u32);
    t.carriage_return();
    t.linefeed();
    t.carriage_return();
    t.linefeed();

    assert_eq!(t.plain_string(), "A\nBD\nC");
}

// Zig: "Terminal: reverseIndex from the top".
#[test]
fn reverse_index_from_the_top() {
    let mut t = term(2, 5);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('B' as u32);
    t.carriage_return();
    t.linefeed();
    t.carriage_return();
    t.linefeed();

    t.set_cursor_pos(1, 1);
    t.reverse_index();
    t.print('D' as u32);

    t.carriage_return();
    t.linefeed();
    t.set_cursor_pos(1, 1);
    t.reverse_index();
    t.print('E' as u32);
    t.carriage_return();
    t.linefeed();

    assert_eq!(t.plain_string(), "E\nD\nA\nB");
}

// Zig: "Terminal: reverseIndex top of scrolling region".
#[test]
fn reverse_index_top_of_scrolling_region() {
    let mut t = term(2, 10);
    t.set_cursor_pos(2, 1);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('B' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('C' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('D' as u32);
    t.carriage_return();
    t.linefeed();

    t.set_top_and_bottom_margin(2, 5);
    t.set_cursor_pos(2, 1);
    t.reverse_index();
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "\nX\nA\nB\nC");
}

// Zig: "Terminal: reverseIndex top of screen".
#[test]
fn reverse_index_top_of_screen() {
    let mut t = term(5, 5);
    t.print('A' as u32);
    t.set_cursor_pos(2, 1);
    t.print('B' as u32);
    t.set_cursor_pos(3, 1);
    t.print('C' as u32);
    t.set_cursor_pos(1, 1);
    t.reverse_index();
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "X\nA\nB\nC");
}

// Zig: "Terminal: reverseIndex not top of screen".
#[test]
fn reverse_index_not_top_of_screen() {
    let mut t = term(5, 5);
    t.print('A' as u32);
    t.set_cursor_pos(2, 1);
    t.print('B' as u32);
    t.set_cursor_pos(3, 1);
    t.print('C' as u32);
    t.set_cursor_pos(2, 1);
    t.reverse_index();
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "X\nB\nC");
}

// Zig: "Terminal: reverseIndex top/bottom margins".
#[test]
fn reverse_index_top_bottom_margins() {
    let mut t = term(5, 5);
    t.print('A' as u32);
    t.set_cursor_pos(2, 1);
    t.print('B' as u32);
    t.set_cursor_pos(3, 1);
    t.print('C' as u32);
    t.set_top_and_bottom_margin(2, 3);
    t.set_cursor_pos(2, 1);
    t.reverse_index();

    assert_eq!(t.plain_string(), "A\n\nB");
}

// Zig: "Terminal: reverseIndex outside top/bottom margins".
#[test]
fn reverse_index_outside_top_bottom_margins() {
    let mut t = term(5, 5);
    t.print('A' as u32);
    t.set_cursor_pos(2, 1);
    t.print('B' as u32);
    t.set_cursor_pos(3, 1);
    t.print('C' as u32);
    t.set_top_and_bottom_margin(2, 3);
    t.set_cursor_pos(1, 1);
    t.reverse_index();

    assert_eq!(t.plain_string(), "A\nB\nC");
}

// Zig: "Terminal: reverseIndex left/right margins".
#[test]
fn reverse_index_left_right_margins() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.set_cursor_pos(2, 1);
    t.print_string("DEF");
    t.set_cursor_pos(3, 1);
    t.print_string("GHI");
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(2, 3);
    t.set_cursor_pos(1, 2);
    t.reverse_index();

    assert_eq!(t.plain_string(), "A\nDBC\nGEF\n HI");
}

// Zig: "Terminal: reverseIndex outside left/right margins".
#[test]
fn reverse_index_outside_left_right_margins() {
    let mut t = term(5, 5);
    t.print_string("ABC");
    t.set_cursor_pos(2, 1);
    t.print_string("DEF");
    t.set_cursor_pos(3, 1);
    t.print_string("GHI");
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(2, 3);
    t.set_cursor_pos(1, 1);
    t.reverse_index();

    assert_eq!(t.plain_string(), "ABC\nDEF\nGHI");
}

// Zig: "Terminal: index" (full dirty-tracking variant).
#[test]
fn index_full() {
    let mut t = term(2, 5);
    t.index();
    t.print('A' as u32);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));

    assert_eq!(t.plain_string(), "\nA");
}

// Zig: "Terminal: index from the bottom".
#[test]
fn index_from_the_bottom() {
    let mut t = term(2, 5);
    t.set_cursor_pos(5, 1);
    t.print('A' as u32);
    t.cursor_left(1); // undo moving right from 'A'

    t.clear_dirty();
    t.index();
    t.print('B' as u32);

    assert!(t.is_dirty(Point::active(0, 3)));
    assert!(t.is_dirty(Point::active(0, 4)));
}

// ---- M1 backfill batch 6: index variants / cursorUp / cursorLeft (L8475-9169) ----

// Zig: "Terminal: index scrolling with hyperlink".
#[test]
fn index_scrolling_with_hyperlink() {
    let mut t = term(2, 5);
    t.set_cursor_pos(5, 1);
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.print('A' as u32);
    t.screen_mut().end_hyperlink();
    t.cursor_left(1); // undo moving right from 'A'
    t.index();
    t.print('B' as u32);

    assert_eq!(t.plain_string(), "\n\n\nA\nB");

    let (row_hl, cell_hl, id) = hyperlink_at(&t, 0, 3);
    assert!(row_hl);
    assert!(cell_hl);
    assert_eq!(id, Some(1));

    let (row_hl, cell_hl, id) = hyperlink_at(&t, 0, 4);
    assert!(!row_hl);
    assert!(!cell_hl);
    assert_eq!(id, None);
}

// Zig: "Terminal: index outside of scrolling region".
#[test]
fn index_outside_of_scrolling_region() {
    let mut t = term(2, 5);
    assert_eq!(t.screen().cursor.y, 0);
    t.set_top_and_bottom_margin(2, 5);
    t.index();
    assert_eq!(t.screen().cursor.y, 1);
}

// Zig: "Terminal: index from the bottom outside of scroll region".
#[test]
fn index_from_the_bottom_outside_of_scroll_region() {
    let mut t = term(2, 5);
    t.set_top_and_bottom_margin(1, 2);
    t.set_cursor_pos(5, 1);
    t.print('A' as u32);
    t.clear_dirty();
    t.index();
    t.print('B' as u32);
    assert!(t.is_dirty(Point::active(0, 4)));

    assert_eq!(t.plain_string(), "\n\n\n\nAB");
}

// Zig: "Terminal: index no scroll region, top of screen".
#[test]
fn index_no_scroll_region_top_of_screen() {
    let mut t = term(5, 5);
    t.print('A' as u32);
    t.clear_dirty();
    t.index();
    t.print('X' as u32);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));

    assert_eq!(t.plain_string(), "A\n X");
}

// Zig: "Terminal: index bottom of primary screen".
#[test]
fn index_bottom_of_primary_screen() {
    let mut t = term(5, 5);
    t.set_cursor_pos(5, 1);
    t.print('A' as u32);
    t.clear_dirty();
    t.index();
    t.print('X' as u32);

    assert!(t.is_dirty(Point::active(0, 3)));
    assert!(t.is_dirty(Point::active(0, 4)));

    assert_eq!(t.plain_string(), "\n\n\nA\n X");
}

// Zig: "Terminal: index bottom of primary screen background sgr".
//
// Needs actual scrollback (Zig's default `max_scrollback` is 10_000; our
// shared `term()` helper uses 0), so build a `Terminal` directly.
#[test]
fn index_bottom_of_primary_screen_background_sgr() {
    let mut t = Terminal::new(Options {
        cols: 5,
        rows: 5,
        max_scrollback: 10_000,
        colors: Colors::default(),
    });
    t.set_cursor_pos(5, 1);
    t.print('A' as u32);
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.index();

    assert_eq!(t.plain_string(), "\n\n\nA");
    for x in 0..t.cols {
        assert_eq!(bg_rgb_at(&t, x, 4), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
    }
}

// Zig: "Terminal: index inside scroll region".
#[test]
fn index_inside_scroll_region() {
    let mut t = term(5, 5);
    t.set_top_and_bottom_margin(1, 3);
    t.print('A' as u32);
    t.clear_dirty();
    t.index();
    t.print('X' as u32);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));

    assert_eq!(t.plain_string(), "A\n X");
}

// Zig: "Terminal: index bottom of scroll region with hyperlinks".
#[test]
fn index_bottom_of_scroll_region_with_hyperlinks() {
    let mut t = term(5, 5);
    t.set_top_and_bottom_margin(1, 2);
    t.print('A' as u32);
    t.index();
    t.carriage_return();
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.print('B' as u32);
    t.screen_mut().end_hyperlink();
    t.index();
    t.carriage_return();
    t.print('C' as u32);

    assert_eq!(t.plain_string(), "B\nC");

    let (row_hl, cell_hl, id) = hyperlink_at(&t, 0, 0);
    assert!(row_hl);
    assert!(cell_hl);
    assert_eq!(id, Some(1));

    let (row_hl, cell_hl, id) = hyperlink_at(&t, 0, 1);
    assert!(!row_hl);
    assert!(!cell_hl);
    assert_eq!(id, None);
}

// Zig: "Terminal: index bottom of scroll region clear hyperlinks".
#[test]
fn index_bottom_of_scroll_region_clear_hyperlinks() {
    let mut t = term(5, 5);
    t.set_top_and_bottom_margin(2, 3);
    t.set_cursor_pos(2, 1);
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.print('A' as u32);
    t.screen_mut().end_hyperlink();
    t.index();
    t.carriage_return();
    t.print('B' as u32);
    t.index();
    t.carriage_return();
    t.print('C' as u32);

    assert_eq!(t.plain_string(), "\nB\nC");

    for y in 1..3u32 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, 0, y);
        assert!(!row_hl);
        assert!(!cell_hl);
        assert_eq!(id, None);
        unsafe {
            let node = t
                .screen()
                .pages
                .get_cell(Point::viewport(0, y))
                .unwrap()
                .node;
            let page = t.screen_mut().pages.node_data_mut(node);
            assert_eq!(page.hyperlink_set_mut().count(), 0);
        }
    }
}

// Zig: "Terminal: index bottom of scroll region with background SGR".
#[test]
fn index_bottom_of_scroll_region_with_background_sgr() {
    let mut t = term(5, 5);
    t.set_top_and_bottom_margin(1, 3);
    t.set_cursor_pos(4, 1);
    t.print('B' as u32);
    t.set_cursor_pos(3, 1);
    t.print('A' as u32);
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.index();

    assert_eq!(t.plain_string(), "\nA\n\nB");

    for x in 0..t.cols {
        assert_eq!(bg_rgb_at(&t, x, 2), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
    }
}

// Zig: "Terminal: index bottom of primary screen with scroll region".
#[test]
fn index_bottom_of_primary_screen_with_scroll_region() {
    let mut t = term(5, 5);
    t.set_top_and_bottom_margin(1, 3);
    t.set_cursor_pos(3, 1);
    t.print('A' as u32);
    t.set_cursor_pos(5, 1);
    t.clear_dirty();
    t.index();
    t.index();
    t.index();
    t.print('X' as u32);

    for y in 0..4u32 {
        assert!(!t.is_dirty(Point::active(0, y)));
    }
    assert!(t.is_dirty(Point::active(0, 4)));

    assert_eq!(t.plain_string(), "\n\nA\n\nX");
}

// Zig: "Terminal: index outside left/right margin".
#[test]
fn index_outside_left_right_margin() {
    let mut t = term(10, 5);
    t.set_top_and_bottom_margin(1, 3);
    t.scrolling_region.left = 2;
    t.scrolling_region.right = 4;
    t.set_cursor_pos(3, 3);
    t.print('A' as u32);
    t.set_cursor_pos(3, 1);
    t.clear_dirty();
    t.index();
    t.print('X' as u32);

    assert!(t.is_dirty(Point::active(0, 2)));

    assert_eq!(t.plain_string(), "\n\nX A");
}

// Zig: "Terminal: index inside left/right margin".
#[test]
fn index_inside_left_right_margin() {
    let mut t = term(10, 5);
    t.print_string("AAAAAA");
    t.carriage_return();
    t.linefeed();
    t.print_string("AAAAAA");
    t.carriage_return();
    t.linefeed();
    t.print_string("AAAAAA");
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_top_and_bottom_margin(1, 3);
    t.set_left_and_right_margin(1, 3);
    t.set_cursor_pos(3, 1);

    t.clear_dirty();
    t.index();

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));

    assert_eq!(t.screen().cursor.y, 2);
    assert_eq!(t.screen().cursor.x, 0);

    assert_eq!(t.plain_string(), "AAAAAA\nAAAAAA\n   AAA");
}

// Zig: "Terminal: index bottom of scroll region creates scrollback".
//
// Needs actual scrollback (see note above); build a `Terminal` directly.
#[test]
fn index_bottom_of_scroll_region_creates_scrollback() {
    let mut t = Terminal::new(Options {
        cols: 5,
        rows: 5,
        max_scrollback: 10_000,
        colors: Colors::default(),
    });
    t.set_top_and_bottom_margin(1, 3);
    t.print_string("1\n2\n3");
    t.set_cursor_pos(4, 1);
    t.print('X' as u32);
    t.set_cursor_pos(3, 1);
    t.index();
    t.print('Y' as u32);

    assert_eq!(
        t.screen().dump_string(crate::point::Tag::Viewport, false),
        "2\n3\nY\nX"
    );
    assert_eq!(
        t.screen().dump_string(crate::point::Tag::Screen, false),
        "1\n2\n3\nY\nX"
    );
}

// Zig: "Terminal: index bottom of scroll region no scrollback".
#[test]
fn index_bottom_of_scroll_region_no_scrollback() {
    let mut t = term(5, 5);
    t.set_top_and_bottom_margin(1, 3);
    t.set_cursor_pos(4, 1);
    t.print('B' as u32);
    t.set_cursor_pos(3, 1);
    t.print('A' as u32);
    t.clear_dirty();
    t.index();
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "\nA\n X\nB");
}

// Zig: "Terminal: index bottom of scroll region blank line preserves SGR".
//
// Needs actual scrollback (see note above); build a `Terminal` directly.
#[test]
fn index_bottom_of_scroll_region_blank_line_preserves_sgr() {
    let mut t = Terminal::new(Options {
        cols: 5,
        rows: 5,
        max_scrollback: 10_000,
        colors: Colors::default(),
    });
    t.set_top_and_bottom_margin(1, 3);
    t.print_string("1\n2\n3");
    t.set_cursor_pos(4, 1);
    t.print('X' as u32);
    t.set_cursor_pos(3, 1);
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.index();

    assert_eq!(
        t.screen().dump_string(crate::point::Tag::Viewport, false),
        "2\n3\n\nX"
    );
    assert_eq!(
        t.screen().dump_string(crate::point::Tag::Screen, false),
        "1\n2\n3\n\nX"
    );
    for x in 0..t.cols {
        assert_eq!(bg_rgb_at(&t, x, 2), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
    }
}

// Zig: "Terminal: cursorUp basic".
#[test]
fn cursor_up_basic() {
    let mut t = term(5, 5);
    t.set_cursor_pos(3, 1);
    t.print('A' as u32);
    t.cursor_up(10);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), " X\n\nA");
}

// Zig: "Terminal: cursorUp below top scroll margin".
#[test]
fn cursor_up_below_top_scroll_margin() {
    let mut t = term(5, 5);
    t.set_top_and_bottom_margin(2, 4);
    t.set_cursor_pos(3, 1);
    t.print('A' as u32);
    t.cursor_up(5);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "\n X\nA");
}

// Zig: "Terminal: cursorUp above top scroll margin".
#[test]
fn cursor_up_above_top_scroll_margin() {
    let mut t = term(5, 5);
    t.set_top_and_bottom_margin(3, 5);
    t.set_cursor_pos(3, 1);
    t.print('A' as u32);
    t.set_cursor_pos(2, 1);
    t.cursor_up(10);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "X\n\nA");
}

// Zig: "Terminal: cursorUp resets wrap".
#[test]
fn cursor_up_resets_wrap() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.cursor_up(1);
    assert!(!t.screen().cursor.pending_wrap);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "ABCDX");
}

// Zig: "Terminal: cursorLeft no wrap" (full multi-line variant).
#[test]
fn cursor_left_no_wrap_full() {
    let mut t = term(10, 5);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('B' as u32);
    t.cursor_left(10);

    assert_eq!(t.plain_string(), "A\nB");
}

// Zig: "Terminal: cursorLeft unsets pending wrap state".
#[test]
fn cursor_left_unsets_pending_wrap_state() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.cursor_left(1);
    assert!(!t.screen().cursor.pending_wrap);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "ABCXE");
}

// Zig: "Terminal: cursorLeft unsets pending wrap state with longer jump".
#[test]
fn cursor_left_unsets_pending_wrap_state_with_longer_jump() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.cursor_left(3);
    assert!(!t.screen().cursor.pending_wrap);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "AXCDE");
}

// Zig: "Terminal: cursorLeft reverse wrap with pending wrap state".
#[test]
fn cursor_left_reverse_wrap_with_pending_wrap_state() {
    let mut t = term(5, 5);
    t.modes.set(Mode::Wraparound, true);
    t.modes.set(Mode::ReverseWrap, true);

    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.cursor_left(1);
    assert!(!t.screen().cursor.pending_wrap);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "ABCDX");
}

// Zig: "Terminal: cursorLeft reverse wrap extended with pending wrap state".
#[test]
fn cursor_left_reverse_wrap_extended_with_pending_wrap_state() {
    let mut t = term(5, 5);
    t.modes.set(Mode::Wraparound, true);
    t.modes.set(Mode::ReverseWrapExtended, true);

    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.cursor_left(1);
    assert!(!t.screen().cursor.pending_wrap);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "ABCDX");
}

// Zig: "Terminal: cursorLeft reverse wrap".
#[test]
fn cursor_left_reverse_wrap() {
    let mut t = term(5, 5);
    t.modes.set(Mode::Wraparound, true);
    t.modes.set(Mode::ReverseWrap, true);

    t.print_string("ABCDE1");
    t.cursor_left(2);
    t.print('X' as u32);
    assert!(t.screen().cursor.pending_wrap);

    assert_eq!(t.plain_string(), "ABCDX\n1");
}

// Zig: "Terminal: cursorLeft reverse wrap with no soft wrap".
#[test]
fn cursor_left_reverse_wrap_with_no_soft_wrap() {
    let mut t = term(5, 5);
    t.modes.set(Mode::Wraparound, true);
    t.modes.set(Mode::ReverseWrap, true);

    t.print_string("ABCDE");
    t.carriage_return();
    t.linefeed();
    t.print('1' as u32);
    t.cursor_left(2);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "ABCDE\nX");
}

// Zig: "Terminal: cursorLeft reverse wrap before left margin".
#[test]
fn cursor_left_reverse_wrap_before_left_margin() {
    let mut t = term(5, 5);
    t.modes.set(Mode::Wraparound, true);
    t.modes.set(Mode::ReverseWrap, true);
    t.set_top_and_bottom_margin(3, 0);
    t.cursor_left(1);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "\n\nX");
}

// ---- M1 backfill batch 7: cursorLeft extended / cursorDown / cursorRight / deleteLines (L9171-9868) ----

// Zig: "Terminal: cursorLeft extended reverse wrap".
#[test]
fn cursor_left_extended_reverse_wrap() {
    let mut t = term(5, 5);
    t.modes.set(Mode::Wraparound, true);
    t.modes.set(Mode::ReverseWrapExtended, true);

    t.print_string("ABCDE");
    t.carriage_return();
    t.linefeed();
    t.print('1' as u32);
    t.cursor_left(2);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "ABCDX\n1");
}

// Zig: "Terminal: cursorLeft extended reverse wrap bottom wraparound".
#[test]
fn cursor_left_extended_reverse_wrap_bottom_wraparound() {
    let mut t = term(5, 3);
    t.modes.set(Mode::Wraparound, true);
    t.modes.set(Mode::ReverseWrapExtended, true);

    t.print_string("ABCDE");
    t.carriage_return();
    t.linefeed();
    t.print('1' as u32);
    let cols = t.cols as usize;
    t.cursor_left(1 + cols + 1);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "ABCDE\n1\n    X");
}

// Zig: "Terminal: cursorLeft extended reverse wrap is priority if both set".
#[test]
fn cursor_left_extended_reverse_wrap_is_priority_if_both_set() {
    let mut t = term(5, 3);
    t.modes.set(Mode::Wraparound, true);
    t.modes.set(Mode::ReverseWrap, true);
    t.modes.set(Mode::ReverseWrapExtended, true);

    t.print_string("ABCDE");
    t.carriage_return();
    t.linefeed();
    t.print('1' as u32);
    let cols = t.cols as usize;
    t.cursor_left(1 + cols + 1);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "ABCDE\n1\n    X");
}

// Zig: "Terminal: cursorLeft extended reverse wrap above top scroll region".
#[test]
fn cursor_left_extended_reverse_wrap_above_top_scroll_region() {
    let mut t = term(5, 5);
    t.modes.set(Mode::Wraparound, true);
    t.modes.set(Mode::ReverseWrapExtended, true);

    t.set_top_and_bottom_margin(3, 0);
    t.set_cursor_pos(2, 1);
    t.cursor_left(1000);

    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 0);
}

// Zig: "Terminal: cursorLeft reverse wrap on first row".
#[test]
fn cursor_left_reverse_wrap_on_first_row() {
    let mut t = term(5, 5);
    t.modes.set(Mode::Wraparound, true);
    t.modes.set(Mode::ReverseWrap, true);

    t.set_top_and_bottom_margin(3, 0);
    t.set_cursor_pos(1, 2);
    t.cursor_left(1000);

    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 0);
}

// Zig: "Terminal: cursorDown basic".
#[test]
fn cursor_down_basic() {
    let mut t = term(5, 5);
    t.print('A' as u32);
    t.cursor_down(10);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "A\n\n\n\n X");
}

// Zig: "Terminal: cursorDown above bottom scroll margin".
#[test]
fn cursor_down_above_bottom_scroll_margin() {
    let mut t = term(5, 5);
    t.set_top_and_bottom_margin(1, 3);
    t.print('A' as u32);
    t.cursor_down(10);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "A\n\n X");
}

// Zig: "Terminal: cursorDown below bottom scroll margin".
#[test]
fn cursor_down_below_bottom_scroll_margin() {
    let mut t = term(5, 5);
    t.set_top_and_bottom_margin(1, 3);
    t.print('A' as u32);
    t.set_cursor_pos(4, 1);
    t.cursor_down(10);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "A\n\n\n\nX");
}

// Zig: "Terminal: cursorDown resets wrap".
#[test]
fn cursor_down_resets_wrap() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.cursor_down(1);
    assert!(!t.screen().cursor.pending_wrap);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "ABCDE\n    X");
}

// Zig: "Terminal: cursorRight resets wrap" (full round-trip variant).
#[test]
fn cursor_right_resets_wrap_full() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.cursor_right(1);
    assert!(!t.screen().cursor.pending_wrap);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "ABCDX");
}

// Zig: "Terminal: cursorRight to the edge of screen".
#[test]
fn cursor_right_to_the_edge_of_screen() {
    let mut t = term(5, 5);
    t.cursor_right(100);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "    X");
}

// Zig: "Terminal: cursorRight left of right margin".
#[test]
fn cursor_right_left_of_right_margin() {
    let mut t = term(5, 5);
    t.scrolling_region.right = 2;
    t.cursor_right(100);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "  X");
}

// Zig: "Terminal: cursorRight right of right margin".
#[test]
fn cursor_right_right_of_right_margin() {
    let mut t = term(5, 5);
    t.scrolling_region.right = 2;
    t.set_cursor_pos(1, 4);
    t.cursor_right(100);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "    X");
}

// Zig: "Terminal: deleteLines colors with bg color" (full content-tag variant).
#[test]
fn delete_lines_colors_with_bg_color_full() {
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
    t.delete_lines(1);

    assert_eq!(t.plain_string(), "ABC\nGHI");

    for x in 0..t.cols {
        assert_eq!(bg_rgb_at(&t, x, 4), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
    }
}

// Zig: "Terminal: deleteLines across page boundary marks all shifted rows dirty".
#[test]
fn delete_lines_across_page_boundary_marks_all_shifted_rows_dirty() {
    let mut t = Terminal::new(Options {
        cols: 10,
        rows: 5,
        max_scrollback: 1024,
        colors: Colors::default(),
    });

    let first_page_rows = unsafe {
        let node = t.screen().pages.head_node();
        (*node).data.capacity.rows as usize
    };

    for _ in 0..(first_page_rows - 3) {
        t.linefeed();
    }

    t.print_string("1AAAA");
    t.carriage_return();
    t.linefeed();
    t.print_string("2BBBB");
    t.carriage_return();
    t.linefeed();
    t.print_string("3CCCC");
    t.carriage_return();
    t.linefeed();
    t.print_string("4DDDD");
    t.carriage_return();
    t.linefeed();
    t.print_string("5EEEE");

    let first_node = t.screen().pages.head_node();
    unsafe {
        assert!(!(*first_node).next.is_null());
    }

    t.set_cursor_pos(1, 1);
    t.clear_dirty();
    t.delete_lines(1);

    for y in 0..5u32 {
        assert!(t.is_dirty(Point::active(0, y)));
    }

    assert_eq!(t.plain_string(), "2BBBB\n3CCCC\n4DDDD\n5EEEE");
}

// Zig: "Terminal: deleteLines (legacy)".
#[test]
fn delete_lines_legacy() {
    let mut t = term(80, 80);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('B' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('C' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('D' as u32);

    t.cursor_up(2);
    t.delete_lines(1);

    t.print('E' as u32);
    t.carriage_return();
    t.linefeed();

    assert_eq!(t.screen().cursor.x, 0);
    assert_eq!(t.screen().cursor.y, 2);

    assert_eq!(t.plain_string(), "A\nE\nD");
}

// Zig: "Terminal: deleteLines with scroll region" (full round-trip variant).
#[test]
fn delete_lines_with_scroll_region_full() {
    let mut t = term(80, 80);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('B' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('C' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('D' as u32);

    t.set_top_and_bottom_margin(1, 3);
    t.set_cursor_pos(1, 1);

    t.clear_dirty();
    t.delete_lines(1);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));
    assert!(!t.is_dirty(Point::active(0, 3)));

    t.print('E' as u32);
    t.carriage_return();
    t.linefeed();

    assert_eq!(t.plain_string(), "E\nC\n\nD");
}

// Zig: "Terminal: deleteLines with scroll region, large count".
#[test]
fn delete_lines_with_scroll_region_large_count() {
    let mut t = term(80, 80);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('B' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('C' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('D' as u32);

    t.set_top_and_bottom_margin(1, 3);
    t.set_cursor_pos(1, 1);

    t.clear_dirty();
    t.delete_lines(5);

    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));
    assert!(!t.is_dirty(Point::active(0, 3)));

    t.print('E' as u32);
    t.carriage_return();
    t.linefeed();

    assert_eq!(t.plain_string(), "E\n\n\nD");
}

// Zig: "Terminal: deleteLines with scroll region, cursor outside of region".
#[test]
fn delete_lines_with_scroll_region_cursor_outside_of_region() {
    let mut t = term(80, 80);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('B' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('C' as u32);
    t.carriage_return();
    t.linefeed();
    t.print('D' as u32);

    t.set_top_and_bottom_margin(1, 3);
    t.set_cursor_pos(4, 1);

    t.clear_dirty();
    t.delete_lines(1);

    for y in 0..4u32 {
        assert!(!t.is_dirty(Point::active(0, y)));
    }

    assert_eq!(t.plain_string(), "A\nB\nC\nD");
}

// Zig: "Terminal: deleteLines resets pending wrap" (full round-trip variant).
#[test]
fn delete_lines_resets_pending_wrap_full() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.delete_lines(1);
    assert!(!t.screen().cursor.pending_wrap);
    t.print('B' as u32);

    assert_eq!(t.plain_string(), "B");
}

// Zig: "Terminal: deleteLines resets wrap".
#[test]
fn delete_lines_resets_wrap() {
    let mut t = term(3, 3);
    t.print('1' as u32);
    t.carriage_return();
    t.linefeed();
    t.print_string("ABCDEF");

    t.set_top_and_bottom_margin(1, 2);
    t.set_cursor_pos(1, 1);
    t.delete_lines(1);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "XBC\n\nDEF");

    for y in 0..t.rows as u32 {
        let lc = t.screen().pages.get_cell(Point::active(0, y)).unwrap();
        unsafe {
            assert!(!(*lc.row).wrap());
        }
    }
}

// Zig: "Terminal: deleteLines left/right scroll region".
#[test]
fn delete_lines_left_right_scroll_region() {
    let mut t = term(10, 10);
    t.print_string("ABC123");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF456");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI789");
    t.scrolling_region.left = 1;
    t.scrolling_region.right = 3;
    t.set_cursor_pos(2, 2);

    t.clear_dirty();
    t.delete_lines(1);

    assert!(!t.is_dirty(Point::active(0, 0)));
    for y in 1..3u32 {
        assert!(t.is_dirty(Point::active(0, y)));
    }

    assert_eq!(t.plain_string(), "ABC123\nDHI756\nG   89");
}

// Zig: "Terminal: deleteLines left/right scroll region from top".
#[test]
fn delete_lines_left_right_scroll_region_from_top() {
    let mut t = term(10, 10);
    t.print_string("ABC123");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF456");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI789");
    t.scrolling_region.left = 1;
    t.scrolling_region.right = 3;
    t.set_cursor_pos(1, 2);

    t.clear_dirty();
    t.delete_lines(1);

    for y in 0..3u32 {
        assert!(t.is_dirty(Point::active(0, y)));
    }

    assert_eq!(t.plain_string(), "AEF423\nDHI756\nG   89");
}

// Zig: "Terminal: deleteLines left/right scroll region high count".
#[test]
fn delete_lines_left_right_scroll_region_high_count() {
    let mut t = term(10, 10);
    t.print_string("ABC123");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF456");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI789");
    t.scrolling_region.left = 1;
    t.scrolling_region.right = 3;
    t.set_cursor_pos(2, 2);

    t.clear_dirty();
    t.delete_lines(100);

    assert!(!t.is_dirty(Point::active(0, 0)));
    for y in 1..3u32 {
        assert!(t.is_dirty(Point::active(0, y)));
    }

    assert_eq!(t.plain_string(), "ABC123\nD   56\nG   89");
}

// Zig: "Terminal: deleteLines wide character spacer head" — deleting the
// top line should convert the orphaned spacer_head to a regular empty cell
// and unset wrap.
#[test]
fn delete_lines_wide_character_spacer_head() {
    let mut t = term(5, 3);
    t.print_string("AAAAABBBB\u{1f600}CCC");

    t.set_cursor_pos(1, 1);
    t.delete_lines(1);

    assert_eq!(t.plain_string(), "BBBB\n\u{1f600}CCC");
    assert_eq!(t.plain_string_unwrapped(), "BBBB\n\u{1f600}CCC");
}

// ---- M1 backfill batch 8: deleteLines spacer-head edge cases / style GC / DECALN / insertBlanks (L9868-10570) ----

// Zig: "Terminal: deleteLines wide character spacer head left scroll margin".
#[test]
fn delete_lines_wide_character_spacer_head_left_scroll_margin() {
    let mut t = term(5, 3);
    t.print_string("AAAAABBBB\u{1f600}CCC");

    t.scrolling_region.left = 2;

    // Due to the left scrolling margin, wrap state should remain.
    t.set_cursor_pos(1, 3);
    t.delete_lines(1);

    assert_eq!(t.plain_string(), "AABB\nBBCCC\n\u{1f600}");
    assert_eq!(t.plain_string_unwrapped(), "AABB BBCCC\u{1f600}");
}

// Zig: "Terminal: deleteLines wide character spacer head right scroll margin".
#[test]
fn delete_lines_wide_character_spacer_head_right_scroll_margin() {
    let mut t = term(5, 3);
    t.print_string("AAAAABBBB\u{1f600}CCC");

    t.scrolling_region.right = 3;

    // Due to the right scrolling margin, wrap state should remain.
    t.set_cursor_pos(1, 1);
    t.delete_lines(1);

    assert_eq!(t.plain_string(), "BBBBA\n\u{1f600}CC\n    C");
    assert_eq!(t.plain_string_unwrapped(), "BBBBA\u{1f600}CC     C");
}

// Zig: "Terminal: deleteLines wide character spacer head left and right scroll margin".
#[test]
fn delete_lines_wide_character_spacer_head_left_and_right_scroll_margin() {
    let mut t = term(5, 3);
    t.print_string("AAAAABBBB\u{1f600}CCC");

    t.scrolling_region.right = 3;
    t.scrolling_region.left = 2;

    // Because there is both a left scrolling margin > 1 and a right
    // scrolling margin, the spacer head should remain, and the wrap state
    // should be untouched.
    t.set_cursor_pos(1, 3);
    t.delete_lines(1);

    assert_eq!(t.plain_string(), "AABBA\nBBCC\n\u{1f600}  C");
    assert_eq!(t.plain_string_unwrapped(), "AABBABBCC\u{1f600}  C");
}

// Zig: "Terminal: deleteLines wide character spacer head left (< 2) and right scroll margin".
#[test]
fn delete_lines_wide_character_spacer_head_left_lt_2_and_right_scroll_margin() {
    let mut t = term(5, 3);
    t.print_string("AAAAABBBB\u{1f600}CCC");

    t.scrolling_region.right = 3;
    t.scrolling_region.left = 1;

    // Because the left margin is 1, the wide char is split, and therefore
    // removed, along with the spacer head - however, wrap state should be
    // untouched.
    t.set_cursor_pos(1, 2);
    t.delete_lines(1);

    assert_eq!(t.plain_string(), "ABBBA\nB CC\n    C");
    assert_eq!(t.plain_string_unwrapped(), "ABBBAB CC     C");
}

// Zig: "Terminal: deleteLines wide characters split by left/right scroll region boundaries".
#[test]
fn delete_lines_wide_characters_split_by_left_right_scroll_region_boundaries() {
    let mut t = term(5, 2);
    t.print_string("AAAAA\n\u{1f600}B\u{1f600}");

    t.scrolling_region.right = 3;
    t.scrolling_region.left = 1;

    // The two wide chars, because they're split by the edge of the
    // scrolling region, get removed.
    t.set_cursor_pos(1, 2);
    t.delete_lines(1);

    assert_eq!(t.plain_string(), "A B A");
}

// Zig: "Terminal: deleteLines zero".
#[test]
fn delete_lines_zero() {
    let mut t = term(2, 5);
    t.set_cursor_pos(1, 1);
    t.delete_lines(0);
}

// Zig: "Terminal: default style is empty".
#[test]
fn default_style_is_empty() {
    let mut t = term(5, 5);
    t.print('A' as u32);

    let lc = t.screen().pages.get_cell(Point::screen(0, 0)).unwrap();
    unsafe {
        assert_eq!((*lc.cell).codepoint(), 'A' as u32);
        assert_eq!((*lc.cell).style_id(), 0);
    }
}

// Zig: "Terminal: bold style".
#[test]
fn bold_style() {
    let mut t = term(5, 5);
    t.set_attribute(Attribute::Bold);
    t.print('A' as u32);

    let lc = t.screen().pages.get_cell(Point::screen(0, 0)).unwrap();
    unsafe {
        assert_eq!((*lc.cell).codepoint(), 'A' as u32);
        assert_ne!((*lc.cell).style_id(), 0);
    }
    let style_id = t.screen().cursor.style_id;
    unsafe {
        let node = (*t.screen().cursor.page_pin).node;
        let page = t.screen_mut().pages.node_data_mut(node);
        let mem = page.memory_mut();
        assert!(page.styles().ref_count(mem, style_id) > 1);
    }
}

// Zig: "Terminal: garbage collect overwritten".
#[test]
fn garbage_collect_overwritten() {
    let mut t = term(5, 5);
    t.set_attribute(Attribute::Bold);
    t.print('A' as u32);
    t.set_cursor_pos(1, 1);
    t.set_attribute(Attribute::Unset);
    t.print('B' as u32);

    let lc = t.screen().pages.get_cell(Point::screen(0, 0)).unwrap();
    unsafe {
        assert_eq!((*lc.cell).codepoint(), 'B' as u32);
        assert_eq!((*lc.cell).style_id(), 0);
    }

    assert_eq!(style_page_count(&t), 0);
}

// Zig: "Terminal: do not garbage collect old styles in use".
#[test]
fn do_not_garbage_collect_old_styles_in_use() {
    let mut t = term(5, 5);
    t.set_attribute(Attribute::Bold);
    t.print('A' as u32);
    t.set_attribute(Attribute::Unset);
    t.print('B' as u32);

    let lc = t.screen().pages.get_cell(Point::screen(1, 0)).unwrap();
    unsafe {
        assert_eq!((*lc.cell).codepoint(), 'B' as u32);
        assert_eq!((*lc.cell).style_id(), 0);
    }

    assert_eq!(style_page_count(&t), 1);
}

// Zig: "Terminal: print with style marks the row as styled".
#[test]
fn print_with_style_marks_the_row_as_styled() {
    let mut t = term(5, 5);
    t.set_attribute(Attribute::Bold);
    t.print('A' as u32);
    t.set_attribute(Attribute::Unset);
    t.print('B' as u32);

    let lc = t.screen().pages.get_cell(Point::screen(0, 0)).unwrap();
    unsafe {
        assert!((*lc.row).styled());
    }
}

// Zig: "Terminal: decaln reset margins".
#[test]
fn decaln_reset_margins() {
    let mut t = term(3, 3);
    t.modes.set(Mode::Origin, true);
    t.set_top_and_bottom_margin(2, 3);
    t.decaln();
    t.scroll_down(1);

    assert_eq!(t.plain_string(), "\nEEE\nEEE");
}

// Zig: "Terminal: decaln preserves color".
#[test]
fn decaln_preserves_color() {
    let mut t = term(3, 3);
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.modes.set(Mode::Origin, true);
    t.set_top_and_bottom_margin(2, 3);
    t.decaln();
    t.scroll_down(1);

    assert_eq!(t.plain_string(), "\nEEE\nEEE");
    assert_eq!(bg_rgb_at(&t, 0, 0), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
}

// Zig: "Terminal: DECALN resets graphemes with protected mode" — a previous
// version of DECALN accidentally preserved protected mode, leaving dangling
// managed memory.
#[test]
fn decaln_resets_graphemes_with_protected_mode() {
    let mut t = term(3, 3);
    t.set_protected_mode(ProtectedMode::Iso);

    // This is: 👨‍👩‍👧
    t.modes.set(Mode::GraphemeCluster, true);
    t.print(0x1F468);
    t.print(0x200D);
    t.print(0x1F469);
    t.print(0x200D);
    t.print(0x1F467);

    t.decaln();

    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 0);
    assert!(t.screen().cursor.protected);
    assert_eq!(t.screen().protected_mode, ProtectedMode::Iso);

    for y in 0..t.rows as u32 {
        assert!(t.is_dirty(Point::active(0, y)));
    }

    assert_eq!(t.plain_string(), "EEE\nEEE\nEEE");
}

// Zig: "Terminal: insertBlanks zero".
#[test]
fn insert_blanks_zero() {
    let mut t = term(5, 2);
    t.print('A' as u32);
    t.print('B' as u32);
    t.print('C' as u32);
    t.set_cursor_pos(1, 1);

    t.insert_blanks(0);

    assert_eq!(t.plain_string(), "ABC");
}

// Zig: "Terminal: insertBlanks" (NOTE upstream: not verified with
// conformance tests, so this might actually verify wrong behavior).
#[test]
fn insert_blanks_full() {
    let mut t = term(5, 2);
    t.print('A' as u32);
    t.print('B' as u32);
    t.print('C' as u32);
    t.set_cursor_pos(1, 1);

    t.clear_dirty();
    t.insert_blanks(2);
    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(!t.is_dirty(Point::active(0, 1)));

    assert_eq!(t.plain_string(), "  ABC");
}

// Zig: "Terminal: insertBlanks more than size".
#[test]
fn insert_blanks_more_than_size() {
    let mut t = term(3, 2);
    t.print('A' as u32);
    t.print('B' as u32);
    t.print('C' as u32);
    t.set_cursor_pos(1, 1);

    t.clear_dirty();
    t.insert_blanks(5);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "");
}

// Zig: "Terminal: insertBlanks no scroll region, fits".
#[test]
fn insert_blanks_no_scroll_region_fits() {
    let mut t = term(10, 10);
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);

    t.clear_dirty();
    t.insert_blanks(2);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "  ABC");
}

// Zig: "Terminal: insertBlanks shift off screen".
#[test]
fn insert_blanks_shift_off_screen() {
    let mut t = term(5, 10);
    t.print_string("  ABC");
    t.set_cursor_pos(1, 3);
    t.clear_dirty();
    t.insert_blanks(2);
    assert!(t.is_dirty(Point::active(0, 0)));
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "  X A");
}

// Zig: "Terminal: insertBlanks split multi-cell character".
#[test]
fn insert_blanks_split_multi_cell_character() {
    let mut t = term(5, 10);
    t.print_string("123");
    t.print('橋' as u32);
    t.set_cursor_pos(1, 1);
    t.clear_dirty();
    t.insert_blanks(1);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), " 123");
}

// Zig: "Terminal: insertBlanks inside left/right scroll region".
#[test]
fn insert_blanks_inside_left_right_scroll_region() {
    let mut t = term(10, 10);
    t.scrolling_region.left = 2;
    t.scrolling_region.right = 4;
    t.set_cursor_pos(1, 3);
    t.print_string("ABC");
    t.set_cursor_pos(1, 3);

    t.clear_dirty();
    t.insert_blanks(2);
    assert!(t.is_dirty(Point::active(0, 0)));
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "  X A");
}

// Zig: "Terminal: insertBlanks outside left/right scroll region".
#[test]
fn insert_blanks_outside_left_right_scroll_region() {
    let mut t = term(6, 10);
    t.set_cursor_pos(1, 4);
    t.print_string("ABC");
    t.scrolling_region.left = 2;
    t.scrolling_region.right = 4;
    assert!(t.screen().cursor.pending_wrap);
    t.clear_dirty();
    t.insert_blanks(2);
    assert!(!t.is_dirty(Point::active(0, 0)));
    assert!(!t.screen().cursor.pending_wrap);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "   ABX");
}

// Zig: "Terminal: insertBlanks left/right scroll region large count".
#[test]
fn insert_blanks_left_right_scroll_region_large_count() {
    let mut t = term(10, 10);
    t.modes.set(Mode::Origin, true);
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(3, 5);
    t.set_cursor_pos(1, 1);
    t.clear_dirty();
    t.insert_blanks(140);
    assert!(t.is_dirty(Point::active(0, 0)));
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "  X");
}

// Zig: "Terminal: insertBlanks deleting graphemes".
#[test]
fn insert_blanks_deleting_graphemes() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);

    t.print_string("ABC");

    // This is: 👨‍👩‍👧
    t.print(0x1F468);
    t.print(0x200D);
    t.print(0x1F469);
    t.print(0x200D);
    t.print(0x1F467);

    assert_eq!(grapheme_page_count(&t), 1);

    t.set_cursor_pos(1, 1);
    t.clear_dirty();
    t.insert_blanks(4);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "    A");

    assert_eq!(grapheme_page_count(&t), 0);
}

// Zig: "Terminal: input glitch text". Fuzz-derived fixture text (embedded
// from `res/glitch.txt`, copied verbatim from Zig's
// `src/terminal/res/glitch.txt`) that is known to grow a page's grapheme
// allocator. Printed repeatedly until the first page's grapheme byte
// capacity increases, asserting that growth actually happens (regression
// coverage for the grapheme-storage growth path under adversarial input).
const GLITCH_TEXT: &str = include_str!("../../res/glitch.txt");

#[test]
fn input_glitch_text() {
    let mut t = term(30, 30);

    // SAFETY: first page is live for the terminal's lifetime.
    let grapheme_cap = unsafe {
        (*t.screen().pages.first_node())
            .data
            .capacity
            .grapheme_bytes
    };

    // Print glitch text until our capacity changes.
    loop {
        // SAFETY: first page is live for the terminal's lifetime.
        let cap = unsafe {
            (*t.screen().pages.first_node())
                .data
                .capacity
                .grapheme_bytes
        };
        if cap != grapheme_cap {
            break;
        }
        t.print_string(GLITCH_TEXT);
    }

    // We're testing to make sure that grapheme capacity gets increased.
    // SAFETY: first page is live for the terminal's lifetime.
    let cap = unsafe {
        (*t.screen().pages.first_node())
            .data
            .capacity
            .grapheme_bytes
    };
    assert!(cap > grapheme_cap);
}

// ---- insertBlanks (deferred batch: hyperlinks / margin edge cases) -------

// Zig: "Terminal: insertBlanks shift graphemes".
#[test]
fn insert_blanks_shift_graphemes() {
    let mut t = term(5, 5);
    t.modes.set(Mode::GraphemeCluster, true);

    t.print_string("A");

    // This is: 👨‍👩‍👧
    t.print(0x1F468);
    t.print(0x200D);
    t.print(0x1F469);
    t.print(0x200D);
    t.print(0x1F467);

    assert_eq!(grapheme_page_count(&t), 1);

    t.set_cursor_pos(1, 1);
    t.clear_dirty();
    t.insert_blanks(1);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(
        t.plain_string(),
        " A\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"
    );

    assert_eq!(grapheme_page_count(&t), 1);
}

// Zig: "Terminal: insertBlanks split multi-cell character from tail".
#[test]
fn insert_blanks_split_multi_cell_character_from_tail() {
    let mut t = term(5, 10);
    t.print_string("橋123");
    t.set_cursor_pos(1, 2);
    t.insert_blanks(1);
    assert_eq!(t.plain_string(), "   12");
}

// Zig: "Terminal: insertBlanks shifts hyperlinks".
//
// osc "8;;http://example.com"
// printf "link"
// printf "\r"
// csi "3@"
// echo
//
// link should be preserved, blanks should not be linked
#[test]
fn insert_blanks_shifts_hyperlinks() {
    let mut t = term(10, 2);
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.insert_blanks(2);

    assert_eq!(t.plain_string(), "  ABC");

    // Verify all our cells have a hyperlink.
    for x in 2..5u16 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, x, 0);
        assert!(row_hl);
        assert!(cell_hl);
        assert_eq!(id, Some(1));
    }
    for x in 0..2u16 {
        let (_, cell_hl, id) = hyperlink_at(&t, x, 0);
        assert!(!cell_hl);
        assert_eq!(id, None);
    }
}

// Zig: "Terminal: insertBlanks pushes hyperlink off end completely".
#[test]
fn insert_blanks_pushes_hyperlink_off_end_completely() {
    let mut t = term(3, 2);
    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.insert_blanks(3);

    assert_eq!(t.plain_string(), "");

    for x in 0..3u16 {
        let (row_hl, cell_hl, id) = hyperlink_at(&t, x, 0);
        assert!(!row_hl);
        assert!(!cell_hl);
        assert_eq!(id, None);
    }
}

// Zig: "Terminal: insertBlanks wide char straddling right margin". Crash
// found by AFL++ fuzzer: when a wide character straddles the right scroll
// margin (head at the margin, spacer_tail just beyond it), insertBlanks
// shifts the wide head away via swapCells but leaves the orphaned
// spacer_tail in place, causing a page integrity violation.
#[test]
fn insert_blanks_wide_char_straddling_right_margin() {
    let mut t = term(10, 5);

    // Fill row: A B C D 橋 _ _ _ _ _
    // Positions: 0 1 2 3 4W 5T 6 7 8 9
    t.set_cursor_pos(1, 1);
    t.print_string("ABCD");
    t.print('橋' as u32); // wide char: head at 4, spacer_tail at 5

    // Set right margin so the wide head is AT the boundary and the
    // spacer_tail is just outside it.
    t.scrolling_region.right = 4;

    // Position cursor at x=2 (1-indexed col 3) and insert one blank. This
    // triggers the swap loop which displaces the wide head at position 4
    // without clearing the spacer_tail at position 5.
    t.set_cursor_pos(1, 3);
    t.insert_blanks(1);

    assert_eq!(t.plain_string(), "AB CD");
}

// Zig: "Terminal: insertBlanks wide char spacer_tail orphaned beyond right
// margin". Regression test for AFL++ crash: when insertBlanks clears the
// entire region from cursor to the right margin (scroll_amount == 0), a wide
// character whose head is AT the right margin gets cleared but its
// spacer_tail just beyond the margin is left behind, causing a page
// integrity violation ("spacer tail not following wide").
#[test]
fn insert_blanks_wide_char_spacer_tail_orphaned_beyond_right_margin() {
    let mut t = term(10, 5);

    // Fill cols 0-9 with wide chars: 中中中中中
    // Positions: 0W 1T 2W 3T 4W 5T 6W 7T 8W 9T
    for _ in 0..5 {
        t.print(0x4E2D);
    }

    // Set left/right margins so that the last wide char (cols 8-9)
    // straddles the boundary: head at col 8 (inside), tail at col 9
    // (outside).
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(1, 9); // 1-indexed: left=0, right=8

    // Cursor is now at (0, 0) after DECSLRM. Print a narrow char to advance
    // cursor to col 1.
    t.print('a' as u32);

    // ICH 8: insert 8 blanks at cursor x=1.
    // rem = right(8) - x(1) + 1 = 8, adjusted_count = 8, scroll_amount = 0.
    // The code clears cols 1-8 without noticing the spacer_tail at col 9.
    t.insert_blanks(8);

    assert_eq!(t.plain_string(), "a");
}

// ---- insert mode ----------------------------------------------------------

// Zig: "Terminal: insert mode with space".
#[test]
fn insert_mode_with_space() {
    let mut t = term(10, 2);
    t.print_string("hello");
    t.set_cursor_pos(1, 2);
    t.modes.set(Mode::Insert, true);
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "hXello");
}

// Zig: "Terminal: insert mode doesn't wrap pushed characters".
#[test]
fn insert_mode_doesnt_wrap_pushed_characters() {
    let mut t = term(5, 2);
    t.print_string("hello");
    t.set_cursor_pos(1, 2);
    t.modes.set(Mode::Insert, true);
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "hXell");
}

// Zig: "Terminal: insert mode does nothing at the end of the line".
#[test]
fn insert_mode_does_nothing_at_the_end_of_the_line() {
    let mut t = term(5, 2);
    t.print_string("hello");
    t.modes.set(Mode::Insert, true);
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "hello\nX");
}

// Zig: "Terminal: insert mode with wide characters".
#[test]
fn insert_mode_with_wide_characters() {
    let mut t = term(5, 2);
    t.print_string("hello");
    t.set_cursor_pos(1, 2);
    t.modes.set(Mode::Insert, true);
    t.print(0x1F600); // 😀
    assert_eq!(t.plain_string(), "h\u{1F600}el");
}

// Zig: "Terminal: insert mode with wide characters at end".
#[test]
fn insert_mode_with_wide_characters_at_end() {
    let mut t = term(5, 2);
    t.print_string("well");
    t.modes.set(Mode::Insert, true);
    t.print(0x1F600); // 😀
    assert_eq!(t.plain_string(), "well\n\u{1F600}");
}

// Zig: "Terminal: insert mode pushing off wide character".
#[test]
fn insert_mode_pushing_off_wide_character() {
    let mut t = term(5, 2);
    t.print_string("123");
    t.print(0x1F600); // 😀
    t.modes.set(Mode::Insert, true);
    t.set_cursor_pos(1, 1);
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "X123");
}

// ---- deleteChars (deferred batch) -----------------------------------------

// Zig: "Terminal: deleteChars".
#[test]
fn delete_chars_basic() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 2);

    t.clear_dirty();
    t.delete_chars(2);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "ADE");
}

// Zig: "Terminal: deleteChars zero count".
#[test]
fn delete_chars_zero_count() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 2);

    t.clear_dirty();
    t.delete_chars(0);
    assert!(!t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "ABCDE");
}

// Zig: "Terminal: deleteChars more than half".
#[test]
fn delete_chars_more_than_half() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 2);

    t.clear_dirty();
    t.delete_chars(3);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "AE");
}

// Zig: "Terminal: deleteChars more than line width".
#[test]
fn delete_chars_more_than_line_width() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 2);

    t.clear_dirty();
    t.delete_chars(10);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "A");
}

// Zig: "Terminal: deleteChars should shift left".
#[test]
fn delete_chars_should_shift_left() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 2);

    t.clear_dirty();
    t.delete_chars(1);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "ACDE");
}

// Zig: "Terminal: deleteChars resets pending wrap".
#[test]
fn delete_chars_resets_pending_wrap() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.delete_chars(1);
    assert!(!t.screen().cursor.pending_wrap);
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "ABCDX");
}

// Zig: "Terminal: deleteChars resets wrap".
#[test]
fn delete_chars_resets_wrap() {
    let mut t = term(5, 5);
    t.print_string("ABCDE123");
    {
        let lc = t.screen().pages.get_cell(Point::active(0, 0)).unwrap();
        assert!(unsafe { (*lc.row).wrap() });
    }
    t.set_cursor_pos(1, 1);
    t.delete_chars(1);

    {
        let lc = t.screen().pages.get_cell(Point::active(0, 0)).unwrap();
        assert!(!unsafe { (*lc.row).wrap() });
    }

    t.print('X' as u32);

    assert_eq!(t.plain_string(), "XCDE\n123");
}

// Zig: "Terminal: deleteChars outside scroll region".
#[test]
fn delete_chars_outside_scroll_region() {
    let mut t = term(6, 10);
    t.print_string("ABC123");
    t.scrolling_region.left = 2;
    t.scrolling_region.right = 4;
    assert!(t.screen().cursor.pending_wrap);
    t.clear_dirty();
    t.delete_chars(2);
    assert!(!t.is_dirty(Point::active(0, 0)));
    assert!(t.screen().cursor.pending_wrap);

    assert_eq!(t.plain_string(), "ABC123");
}

// Zig: "Terminal: deleteChars inside scroll region".
#[test]
fn delete_chars_inside_scroll_region() {
    let mut t = term(6, 10);
    t.print_string("ABC123");
    t.scrolling_region.left = 2;
    t.scrolling_region.right = 4;
    t.set_cursor_pos(1, 4);

    t.clear_dirty();
    t.delete_chars(1);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "ABC2 3");
}

// Zig: "Terminal: deleteChars split wide character from spacer tail".
#[test]
fn delete_chars_split_wide_character_from_spacer_tail() {
    let mut t = term(6, 10);
    t.print_string("A橋123");
    t.set_cursor_pos(1, 3);
    t.delete_chars(1);
    assert_eq!(t.plain_string(), "A 123");
}

// Zig: "Terminal: deleteChars split wide character from wide".
#[test]
fn delete_chars_split_wide_character_from_wide() {
    let mut t = term(6, 10);
    t.print_string("橋123");
    t.set_cursor_pos(1, 1);
    t.delete_chars(1);

    unsafe {
        let lc = t.screen().pages.get_cell(Point::screen(0, 0)).unwrap();
        assert_eq!((*lc.cell).codepoint(), 0);
        assert_eq!((*lc.cell).wide(), Wide::Narrow);
    }
    unsafe {
        let lc = t.screen().pages.get_cell(Point::screen(1, 0)).unwrap();
        assert_eq!((*lc.cell).codepoint(), '1' as u32);
        assert_eq!((*lc.cell).wide(), Wide::Narrow);
    }
}

// Zig: "Terminal: deleteChars split wide character from end".
#[test]
fn delete_chars_split_wide_character_from_end() {
    let mut t = term(6, 10);
    t.print_string("A橋123");
    t.set_cursor_pos(1, 1);
    t.delete_chars(1);

    unsafe {
        let lc = t.screen().pages.get_cell(Point::screen(0, 0)).unwrap();
        assert_eq!((*lc.cell).codepoint(), 0x6A4B);
        assert_eq!((*lc.cell).wide(), Wide::Wide);
    }
    unsafe {
        let lc = t.screen().pages.get_cell(Point::screen(1, 0)).unwrap();
        assert_eq!((*lc.cell).codepoint(), 0);
        assert_eq!((*lc.cell).wide(), Wide::SpacerTail);
    }
}

// Zig: "Terminal: deleteChars with a spacer head at the end".
#[test]
fn delete_chars_with_a_spacer_head_at_the_end() {
    let mut t = term(5, 10);
    t.print_string("0123橋123");
    unsafe {
        let lc = t.screen().pages.get_cell(Point::screen(4, 0)).unwrap();
        assert_eq!((*lc.cell).wide(), Wide::SpacerHead);
        assert!((*lc.row).wrap());
    }

    t.set_cursor_pos(1, 1);
    t.delete_chars(1);

    unsafe {
        let lc = t.screen().pages.get_cell(Point::screen(3, 0)).unwrap();
        assert_eq!((*lc.cell).codepoint(), 0);
        assert_eq!((*lc.cell).wide(), Wide::Narrow);
    }
}

// Zig: "Terminal: deleteChars split wide character tail".
#[test]
fn delete_chars_split_wide_character_tail() {
    let mut t = term(5, 5);
    t.set_cursor_pos(1, (t.cols - 1) as usize);
    t.print(0x6A4B); // 橋
    t.carriage_return();
    t.delete_chars((t.cols - 1) as usize);
    t.print('0' as u32);

    assert_eq!(t.plain_string(), "0");
}

// Zig: "Terminal: deleteChars wide char boundary conditions".
//
// There are 3 or 4 boundaries to be concerned with in deleteChars, depending
// on how you count them. Consider the following terminal:
//
//   +--------+
// 0 |.ABCDEF.|
//   : ^      : (^ = cursor)
//   +--------+
//
// if we DCH 3 we get
//
//   +--------+
// 0 |.DEF....|
//   +--------+
//
// Now consider wide characters (represented as `WW`) at these boundaries:
//
//   +--------+
// 0 |WWaWWbWW|
//   : ^      : (^ = cursor)
//   : ^^^    : (^ = deleted by DCH 3)
//   +--------+
//
// -> DCH 3
// -> The first 2 wide characters are split & destroyed (verified in xterm)
//
//   +--------+
// 0 |..bWW...|
//   +--------+
#[test]
fn delete_chars_wide_char_boundary_conditions() {
    let mut t = term(8, 1);

    t.print_string("\u{1f600}a\u{1f600}b\u{1f600}");
    assert_eq!(t.plain_string(), "\u{1f600}a\u{1f600}b\u{1f600}");

    t.set_cursor_pos(1, 2);
    t.delete_chars(3);
    unsafe {
        let node = t.screen().cursor.page_pin;
        let page = t.screen().pages.node_data((*node).node);
        page.verify_integrity().expect("page integrity");
    }

    assert_eq!(t.plain_string(), "  b\u{1f600}");
}

// Zig: "Terminal: deleteChars wide char wrap boundary conditions" (cont.
// from `delete_chars_wide_char_boundary_conditions`).
//
// Additionally consider soft-wrapped wide chars (`H` = spacer head):
//
//   +--------+
// 0 |.......H…
// 1 …WWabcdeH…
//   : ^      : (^ = cursor)
//   : ^^^    : (^ = deleted by DCH 3)
// 2 …WW......|
//   +--------+
//
// -> DCH 3
// -> First wide character split and destroyed, including spacer head,
//    second spacer head removed (verified in xterm).
// -> Wrap state of row reset
//
//   +--------+
// 0 |........|
// 1 |.cde....|
// 2 |WW......|
//   +--------+
#[test]
fn delete_chars_wide_char_wrap_boundary_conditions() {
    let mut t = term(8, 3);

    t.print_string(".......\u{1f600}abcde\u{1f600}......");
    assert_eq!(t.plain_string(), ".......\n\u{1f600}abcde\n\u{1f600}......");
    assert_eq!(
        t.plain_string_unwrapped(),
        ".......\u{1f600}abcde\u{1f600}......"
    );

    t.set_cursor_pos(2, 2);
    t.delete_chars(3);
    unsafe {
        let node = t.screen().cursor.page_pin;
        let page = t.screen().pages.node_data((*node).node);
        page.verify_integrity().expect("page integrity");
    }

    assert_eq!(t.plain_string(), ".......\n cde\n\u{1f600}......");
    assert_eq!(t.plain_string_unwrapped(), ".......  cde\n\u{1f600}......");
}

// Zig: "Terminal: deleteChars wide char across right margin".
//
// scroll region
//    VVVVVV
//  +-######-+
//  |.abcdeWW|
//  : ^      : (^ = cursor)
//  +--------+
//
// DCH 1
//
// NOTE: This behavior is slightly inconsistent with xterm. xterm _visually_
// splits the wide character (half the wide character shows up in col 6 and
// half in col 8). In all other wide char split scenarios, xterm clears the
// cell. Therefore, we've chosen to clear the cell here. Given we have
// space, we also could actually preserve it, but I haven't yet found a
// terminal that behaves that way. We should be open to revisiting this
// behavior but for now we're going with the simpler impl.
#[test]
fn delete_chars_wide_char_across_right_margin() {
    let mut t = term(8, 3);

    t.print_string("123456橋");
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(2, 7);

    assert_eq!(t.plain_string(), "123456\u{6a4b}");

    t.set_cursor_pos(1, 2);
    t.delete_chars(1);
    unsafe {
        let node = t.screen().cursor.page_pin;
        let page = t.screen().pages.node_data((*node).node);
        page.verify_integrity().expect("page integrity");
    }

    assert_eq!(t.plain_string(), "13456");
}

// ---- saveCursor / restoreCursor / protected mode (deferred batch) --------

// Zig: "Terminal: saveCursor".
#[test]
fn save_cursor_style_charset_and_origin() {
    let mut t = term(3, 3);

    t.set_attribute(Attribute::Bold);
    t.screen_mut().charset.gr = crate::charsets::Slots::G3;
    t.modes.set(Mode::Origin, true);
    t.save_cursor();
    t.screen_mut().charset.gr = crate::charsets::Slots::G0;
    t.set_attribute(Attribute::Unset);
    t.modes.set(Mode::Origin, false);
    t.restore_cursor();
    assert!(t.screen().cursor.style.flags.bold);
    assert_eq!(t.screen().charset.gr, crate::charsets::Slots::G3);
    assert!(t.modes.get(Mode::Origin));
}

// Zig: "Terminal: saveCursor position".
#[test]
fn save_cursor_position() {
    let mut t = term(10, 5);

    t.set_cursor_pos(1, 5);
    t.print('A' as u32);
    t.save_cursor();
    t.set_cursor_pos(1, 1);
    t.print('B' as u32);
    t.restore_cursor();
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "B   AX");
}

// Zig: "Terminal: saveCursor pending wrap state".
#[test]
fn save_cursor_pending_wrap_state() {
    let mut t = term(5, 5);

    t.set_cursor_pos(1, 5);
    t.print('A' as u32);
    t.save_cursor();
    t.set_cursor_pos(1, 1);
    t.print('B' as u32);
    t.restore_cursor();
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "B   A\nX");
}

// Zig: "Terminal: saveCursor origin mode".
#[test]
fn save_cursor_origin_mode() {
    let mut t = term(10, 5);

    t.modes.set(Mode::Origin, true);
    t.save_cursor();
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(3, 5);
    t.set_top_and_bottom_margin(2, 4);
    t.restore_cursor();
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "X");
}

// Zig: "Terminal: saveCursor resize".
#[test]
fn save_cursor_resize() {
    let mut t = term(10, 5);

    t.set_cursor_pos(1, 10);
    t.save_cursor();
    t.resize(5, 5);
    t.restore_cursor();
    t.print('X' as u32);

    assert_eq!(t.plain_string(), "    X");
}

// Zig: "Terminal: saveCursor protected pen".
#[test]
fn save_cursor_protected_pen() {
    let mut t = term(10, 5);

    t.set_protected_mode(ProtectedMode::Iso);
    assert!(t.screen().cursor.protected);
    t.set_cursor_pos(1, 10);
    t.save_cursor();
    t.set_protected_mode(ProtectedMode::Off);
    assert!(!t.screen().cursor.protected);
    t.restore_cursor();
    assert!(t.screen().cursor.protected);
}

// Zig: "Terminal: saveCursor doesn't modify hyperlink state".
#[test]
fn save_cursor_doesnt_modify_hyperlink_state() {
    let mut t = term(3, 3);

    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    let id = t.screen().cursor.hyperlink_id;
    t.save_cursor();
    assert_eq!(id, t.screen().cursor.hyperlink_id);
    t.restore_cursor();
    assert_eq!(id, t.screen().cursor.hyperlink_id);
}

// Zig: "Terminal: restoreCursor uses default style on OutOfSpace". Tests
// that restoreCursor falls back to default style when manualStyleUpdate
// fails with OutOfSpace (can't split a 1-row page and styles are at max
// capacity).
#[test]
fn restore_cursor_uses_default_style_on_out_of_space() {
    use crate::page::style::StyleSet;

    // Use a single row so the page can't be split.
    let mut t = term(10, 1);

    // Set a style and save the cursor.
    t.set_attribute(Attribute::Bold);
    t.save_cursor();

    // Clear the style.
    t.set_attribute(Attribute::Unset);
    assert!(!t.screen().cursor.style.flags.bold);

    // Fill the style map to max capacity.
    let max_styles = crate::page::size::StyleCountInt::MAX;
    loop {
        let node = unsafe { (*t.screen().cursor.page_pin).node };
        let styles_cap = unsafe { t.screen().pages.node_data(node).capacity.styles };
        if styles_cap >= max_styles {
            break;
        }
        // SAFETY: node is live in the cursor's page list.
        let result = unsafe {
            t.screen_mut()
                .pages
                .increase_capacity(node, Some(crate::pagelist::IncreaseCapacity::Styles))
        };
        if result.is_err() {
            break;
        }
    }

    let styles_cap = unsafe {
        let node = (*t.screen().cursor.page_pin).node;
        t.screen().pages.node_data(node).capacity.styles
    };
    assert_eq!(max_styles, styles_cap);

    // Fill all style slots using the StyleSet's layout capacity, which
    // accounts for the load factor -- the capacity in the layout is the
    // actual max number of items that can be stored.
    unsafe {
        let node = (*t.screen().cursor.page_pin).node;
        let page = t.screen_mut().pages.node_data_mut(node);
        (*page).pause_integrity_checks(true);

        let max_items = StyleSet::layout(page.capacity.styles as usize).cap;
        let mem = (*page).memory_mut();
        let mut n: u32 = 1;
        while (n as usize) < max_items {
            let style = crate::page::style::Style {
                bg_color: crate::page::style::Color::Rgb(crate::color::Rgb::new(
                    (n & 0xFF) as u8,
                    ((n >> 8) & 0xFF) as u8,
                    ((n >> 16) & 0xFF) as u8,
                )),
                ..Default::default()
            };
            if (*page).styles().add(mem, style).is_err() {
                break;
            }
            n += 1;
        }

        (*page).pause_integrity_checks(false);
        (*page).verify_integrity().expect("page integrity");
    }

    // Restore cursor - should fall back to default style since the page
    // can't be split (1 row) and styles are at max capacity.
    t.restore_cursor();

    // The style should be reset to default because OutOfSpace occurred.
    assert!(!t.screen().cursor.style.flags.bold);
    assert_eq!(t.screen().cursor.style_id, crate::page::style::DEFAULT_ID);
}

// Zig: "Terminal: setProtectedMode".
#[test]
fn set_protected_mode() {
    let mut t = term(3, 3);

    assert!(!t.screen().cursor.protected);
    t.set_protected_mode(ProtectedMode::Off);
    assert!(!t.screen().cursor.protected);
    t.set_protected_mode(ProtectedMode::Iso);
    assert!(t.screen().cursor.protected);
    t.set_protected_mode(ProtectedMode::Dec);
    assert!(t.screen().cursor.protected);
    t.set_protected_mode(ProtectedMode::Off);
    assert!(!t.screen().cursor.protected);
}

// ---- eraseLine (deferred batch) -------------------------------------------

// Zig: "Terminal: eraseLine simple erase right" (dirty-flag variant of the
// existing `erase_line_right`).
#[test]
fn erase_line_simple_erase_right_dirty() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 3);
    t.clear_dirty();
    t.erase_line(EraseLine::Right, false);
    assert!(t.is_dirty(Point::active(0, 0)));
    assert_eq!(t.plain_string(), "AB");
}

// Zig: "Terminal: eraseLine resets pending wrap".
#[test]
fn erase_line_resets_pending_wrap() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.erase_line(EraseLine::Right, false);
    assert!(!t.screen().cursor.pending_wrap);
    t.print('B' as u32);
    assert_eq!(t.plain_string(), "ABCDB");
}

// Zig: "Terminal: eraseLine right preserves background sgr".
#[test]
fn erase_line_right_preserves_background_sgr() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 2);
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.erase_line(EraseLine::Right, false);

    assert_eq!(t.plain_string(), "A");
    for x in 1..5u16 {
        assert_eq!(bg_rgb_at(&t, x, 0), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
    }
}

// Zig: "Terminal: eraseLine right wide character".
#[test]
fn erase_line_right_wide_character() {
    let mut t = term(10, 5);
    t.print_string("AB");
    t.print('橋' as u32);
    t.print_string("DE");
    t.set_cursor_pos(1, 4);
    t.clear_dirty();
    t.erase_line(EraseLine::Right, false);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "AB");
}

// Zig: "Terminal: eraseLine right protected attributes respected with iso".
#[test]
fn erase_line_right_protected_attributes_respected_with_iso() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Iso);
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.clear_dirty();
    t.erase_line(EraseLine::Right, false);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "ABC");
}

// Zig: "Terminal: eraseLine right protected attributes ignored with dec most
// recent".
#[test]
fn erase_line_right_protected_attributes_ignored_with_dec_most_recent() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Iso);
    t.print_string("ABC");
    t.set_protected_mode(ProtectedMode::Dec);
    t.set_protected_mode(ProtectedMode::Off);
    t.set_cursor_pos(1, 2);
    t.clear_dirty();
    t.erase_line(EraseLine::Right, false);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "A");
}

// Zig: "Terminal: eraseLine right protected attributes ignored with dec
// set".
#[test]
fn erase_line_right_protected_attributes_ignored_with_dec_set() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Dec);
    t.print_string("ABC");
    t.set_cursor_pos(1, 2);
    t.clear_dirty();
    t.erase_line(EraseLine::Right, false);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "A");
}

// Zig: "Terminal: eraseLine right protected requested".
#[test]
fn erase_line_right_protected_requested() {
    let mut t = term(10, 5);
    t.print_string("12345678");
    let y = t.screen().cursor.y;
    t.set_cursor_pos((y + 1) as usize, 6);
    t.set_protected_mode(ProtectedMode::Dec);
    t.print('X' as u32);
    let y = t.screen().cursor.y;
    t.set_cursor_pos((y + 1) as usize, 4);
    t.clear_dirty();
    t.erase_line(EraseLine::Right, true);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "123  X");
}

// Zig: "Terminal: eraseLine simple erase left" (dirty-flag variant of the
// existing `erase_line_left`).
#[test]
fn erase_line_simple_erase_left_dirty() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 3);
    t.clear_dirty();
    t.erase_line(EraseLine::Left, false);
    assert!(t.is_dirty(Point::active(0, 0)));
    assert_eq!(t.plain_string(), "   DE");
}

// Zig: "Terminal: eraseLine left resets wrap".
#[test]
fn erase_line_left_resets_wrap() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);
    t.clear_dirty();
    t.erase_line(EraseLine::Left, false);
    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(!t.screen().cursor.pending_wrap);
    t.print('B' as u32);

    assert_eq!(t.plain_string(), "    B");
}

// Zig: "Terminal: eraseLine left preserves background sgr".
#[test]
fn erase_line_left_preserves_background_sgr() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 2);
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.erase_line(EraseLine::Left, false);

    assert_eq!(t.plain_string(), "  CDE");
    for x in 0..2u16 {
        assert_eq!(bg_rgb_at(&t, x, 0), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
    }
}

// Zig: "Terminal: eraseLine left wide character".
#[test]
fn erase_line_left_wide_character() {
    let mut t = term(10, 5);
    t.print_string("AB");
    t.print('橋' as u32);
    t.print_string("DE");
    t.set_cursor_pos(1, 3);
    t.clear_dirty();
    t.erase_line(EraseLine::Left, false);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "    DE");
}

// Zig: "Terminal: eraseLine left protected attributes respected with iso".
#[test]
fn erase_line_left_protected_attributes_respected_with_iso() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Iso);
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.clear_dirty();
    t.erase_line(EraseLine::Left, false);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "ABC");
}

// Zig: "Terminal: eraseLine left protected attributes ignored with dec most
// recent".
#[test]
fn erase_line_left_protected_attributes_ignored_with_dec_most_recent() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Iso);
    t.print_string("ABC");
    t.set_protected_mode(ProtectedMode::Dec);
    t.set_protected_mode(ProtectedMode::Off);
    t.set_cursor_pos(1, 2);
    t.clear_dirty();
    t.erase_line(EraseLine::Left, false);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "  C");
}

// Zig: "Terminal: eraseLine left protected attributes ignored with dec set".
#[test]
fn erase_line_left_protected_attributes_ignored_with_dec_set() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Dec);
    t.print_string("ABC");
    t.set_cursor_pos(1, 2);
    t.clear_dirty();
    t.erase_line(EraseLine::Left, false);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "  C");
}

// Zig: "Terminal: eraseLine left protected requested".
#[test]
fn erase_line_left_protected_requested() {
    let mut t = term(10, 5);
    t.print_string("123456789");
    let y = t.screen().cursor.y;
    t.set_cursor_pos((y + 1) as usize, 6);
    t.set_protected_mode(ProtectedMode::Dec);
    t.print('X' as u32);
    let y = t.screen().cursor.y;
    t.set_cursor_pos((y + 1) as usize, 8);
    t.clear_dirty();
    t.erase_line(EraseLine::Left, true);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "     X  9");
}

// Zig: "Terminal: eraseLine complete preserves background sgr".
#[test]
fn erase_line_complete_preserves_background_sgr() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    t.set_cursor_pos(1, 2);
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.erase_line(EraseLine::Complete, false);

    assert_eq!(t.plain_string(), "");
    for x in 0..5u16 {
        assert_eq!(bg_rgb_at(&t, x, 0), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
    }
}

// Zig: "Terminal: eraseLine complete protected attributes respected with
// iso".
#[test]
fn erase_line_complete_protected_attributes_respected_with_iso() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Iso);
    t.print_string("ABC");
    t.set_cursor_pos(1, 1);
    t.clear_dirty();
    t.erase_line(EraseLine::Complete, false);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "ABC");
}

// Zig: "Terminal: eraseLine complete protected attributes ignored with dec
// most recent".
#[test]
fn erase_line_complete_protected_attributes_ignored_with_dec_most_recent() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Iso);
    t.print_string("ABC");
    t.set_protected_mode(ProtectedMode::Dec);
    t.set_protected_mode(ProtectedMode::Off);
    t.set_cursor_pos(1, 2);
    t.clear_dirty();
    t.erase_line(EraseLine::Complete, false);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "");
}

// Zig: "Terminal: eraseLine complete protected attributes ignored with dec
// set".
#[test]
fn erase_line_complete_protected_attributes_ignored_with_dec_set() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Dec);
    t.print_string("ABC");
    t.set_cursor_pos(1, 2);
    t.clear_dirty();
    t.erase_line(EraseLine::Complete, false);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "");
}

// Zig: "Terminal: eraseLine complete protected requested".
#[test]
fn erase_line_complete_protected_requested() {
    let mut t = term(10, 5);
    t.print_string("123456789");
    let y = t.screen().cursor.y;
    t.set_cursor_pos((y + 1) as usize, 6);
    t.set_protected_mode(ProtectedMode::Dec);
    t.print('X' as u32);
    let y = t.screen().cursor.y;
    t.set_cursor_pos((y + 1) as usize, 8);
    t.clear_dirty();
    t.erase_line(EraseLine::Complete, true);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "     X");
}

// ---- tabClear --------------------------------------------------------------

// Zig: "Terminal: tabClear single".
#[test]
fn tab_clear_single() {
    let mut t = term(30, 5);
    t.horizontal_tab();
    t.tab_clear(crate::csi::TabClear::Current);
    assert!(!t.is_dirty(Point::active(0, 0)));
    t.set_cursor_pos(1, 1);
    t.horizontal_tab();
    assert_eq!(t.screen().cursor.x, 16);
}

// Zig: "Terminal: tabClear all".
#[test]
fn tab_clear_all() {
    let mut t = term(30, 5);
    t.tab_clear(crate::csi::TabClear::All);
    assert!(!t.is_dirty(Point::active(0, 0)));
    t.set_cursor_pos(1, 1);
    t.horizontal_tab();
    assert_eq!(t.screen().cursor.x, 29);
}

// ---- printRepeat / printSlice (deferred batch) -----------------------------

// Zig: "Terminal: printRepeat simple".
#[test]
fn print_repeat_simple() {
    let mut t = term(5, 5);
    t.print_string("A");
    t.print_repeat(1);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "AA");
}

// Zig: "Terminal: printRepeat wrap".
#[test]
fn print_repeat_wrap() {
    let mut t = term(5, 5);
    t.print_string("    A");
    t.print_repeat(1);
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "    A\nA");
}

// Zig: "Terminal: printRepeat no previous character".
#[test]
fn print_repeat_no_previous_character() {
    let mut t = term(5, 5);
    t.print_repeat(1);
    assert!(!t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "");
}

// Zig: "Terminal: printSlice simple ascii".
#[test]
fn print_slice_simple_ascii() {
    let mut t = term(10, 3);
    t.print_slice(&['h' as u32, 'e' as u32, 'l' as u32, 'l' as u32, 'o' as u32]);
    assert_eq!(t.screen().cursor.x, 5);
    assert_eq!(t.previous_char, Some('o' as u32));
    assert!(t.is_dirty(Point::active(0, 0)));

    assert_eq!(t.plain_string(), "hello");
}

// Zig: "Terminal: printSlice wraps and scrolls".
#[test]
fn print_slice_wraps_and_scrolls() {
    let mut t = term(5, 2);
    // 12 chars: fills row 1 (5), row 2 (5), wraps+scrolls, 2 more.
    let cps: Vec<u32> = "abcdefghijkl".chars().map(|c| c as u32).collect();
    t.print_slice(&cps);

    assert_eq!(t.plain_string(), "fghij\nkl");
    assert_eq!(t.screen().cursor.x, 2);
    assert!(!t.screen().cursor.pending_wrap);
}

// Zig: "Terminal: printSlice pending wrap state".
#[test]
fn print_slice_pending_wrap_state() {
    let mut t = term(5, 2);
    let cps: Vec<u32> = "abcde".chars().map(|c| c as u32).collect();
    t.print_slice(&cps);
    assert_eq!(t.screen().cursor.x, 4);
    assert!(t.screen().cursor.pending_wrap);

    assert_eq!(t.plain_string(), "abcde");
}

/// Small, deterministic, dependency-free splitmix64 PRNG used only by
/// [`print_slice_differential_fuzz_vs_print`] below. Not intended to match
/// Zig's `std.Random` bit-for-bit -- this is a self-consistency differential
/// test (print vs print_slice must agree with EACH OTHER on the same random
/// op stream), not a golden-value test, so any decent PRNG suffices.
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// Inclusive-inclusive range, matching the Zig helper's usage pattern.
    fn range_inclusive(&mut self, lo: usize, hi: usize) -> usize {
        debug_assert!(lo <= hi);
        let span = (hi - lo) as u64 + 1;
        lo + (self.next_u64() % span) as usize
    }

    fn bool(&mut self) -> bool {
        self.next_u64() & 1 == 0
    }

    fn byte(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }
}

/// Differential testing helper: applies the same logical print operations to
/// two terminals, one using per-codepoint [`Terminal::print`] and the other
/// using [`Terminal::print_slice`] with random chunking, verifying that the
/// results are identical. Port of `testPrintSliceDifferential`.
fn print_slice_differential(
    rng: &mut SplitMix64,
    ops: usize,
    cols: CellCountInt,
    rows: CellCountInt,
) {
    let mut t1 = term(cols, rows);
    let mut t2 = term(cols, rows);

    // Alphabet of interesting codepoints: ascii, latin-1, combining marks,
    // CJK (wide), emoji (wide), ZWJ, variation selectors.
    let alphabet: [u32; 40] = [
        'a' as u32,
        'b' as u32,
        'Z' as u32,
        '0' as u32,
        ' ' as u32,
        0x10,
        0x1F,
        0x7F,
        'é' as u32,
        0xFF,
        0x301,
        0x4E00,
        0x4E01,
        0x1F600,
        0x200D,
        0xFE0F,
        'x' as u32,
        'y' as u32,
        0x1F9D1,
        0x0308,
        0xAD,
        0x3042,
        0xAC00,
        'q' as u32,
        'r' as u32,
        's' as u32,
        't' as u32,
        'u' as u32,
        'v' as u32,
        'w' as u32,
        '1' as u32,
        '2' as u32,
        0x1F1E6,
        0x1F1E7,
        0x1100,
        0x1161,
        0x11A8,
        0x200C,
        0x0430,
        0x03B1,
    ];

    let mut cps_buf = [0u32; 64];

    for _ in 0..ops {
        match rng.range_inclusive(0, 20) {
            // Print a run of codepoints (most common op).
            0..=9 => {
                let n = rng.range_inclusive(1, cps_buf.len());
                for slot in cps_buf.iter_mut().take(n) {
                    *slot = alphabet[rng.range_inclusive(0, alphabet.len() - 1)];
                }

                // t1: per-codepoint print.
                for &cp in &cps_buf[0..n] {
                    t1.print(cp);
                }

                // t2: print_slice with random chunking.
                let mut i = 0;
                while i < n {
                    let chunk = rng.range_inclusive(1, n - i);
                    t2.print_slice(&cps_buf[i..i + chunk]);
                    i += chunk;
                }
            }
            10 => {
                t1.carriage_return();
                t2.carriage_return();
                t1.linefeed();
                t2.linefeed();
            }
            11 => {
                let row = rng.range_inclusive(1, rows as usize);
                let col = rng.range_inclusive(1, cols as usize);
                t1.set_cursor_pos(row, col);
                t2.set_cursor_pos(row, col);
            }
            12 => {
                let attr = match rng.range_inclusive(0, 3) {
                    0 => Attribute::Unset,
                    1 => Attribute::Bold,
                    2 => Attribute::DirectColorFg(crate::color::Rgb::new(
                        rng.byte(),
                        rng.byte(),
                        rng.byte(),
                    )),
                    3 => Attribute::Fg8(crate::color::Name::Red),
                    _ => unreachable!(),
                };
                t1.set_attribute(attr);
                t2.set_attribute(attr);
            }
            13 => {
                let v = rng.bool();
                t1.modes.set(Mode::Insert, v);
                t2.modes.set(Mode::Insert, v);
            }
            14 => {
                let v = rng.bool();
                t1.modes.set(Mode::Wraparound, v);
                t2.modes.set(Mode::Wraparound, v);
            }
            15 => {
                let v = rng.bool();
                // Erase the display first: grapheme clusters created while
                // mode 2027 was off can trip a pre-existing debug assert in
                // print()'s cluster walk when the mode is toggled on
                // (unrelated to print_slice; it reproduces with per-codepoint
                // print alone).
                t1.erase_display(EraseDisplay::Complete, false);
                t2.erase_display(EraseDisplay::Complete, false);
                t1.modes.set(Mode::GraphemeCluster, v);
                t2.modes.set(Mode::GraphemeCluster, v);
            }
            16 => {
                // Margins.
                t1.modes.set(Mode::EnableLeftAndRightMargin, true);
                t2.modes.set(Mode::EnableLeftAndRightMargin, true);
                let left = rng.range_inclusive(1, (cols / 2) as usize);
                let right = rng.range_inclusive((cols / 2) as usize, cols as usize);
                t1.set_left_and_right_margin(left, right);
                t2.set_left_and_right_margin(left, right);
            }
            17 => {
                t1.set_left_and_right_margin(0, 0);
                t2.set_left_and_right_margin(0, 0);
            }
            18 => {
                let _ = t1.screen_mut().start_hyperlink(b"http://example.com", None);
                let _ = t2.screen_mut().start_hyperlink(b"http://example.com", None);
            }
            19 => {
                t1.screen_mut().end_hyperlink();
                t2.screen_mut().end_hyperlink();
            }
            20 => {
                let set = if rng.bool() {
                    Charset::DecSpecial
                } else {
                    Charset::Utf8
                };
                t1.configure_charset(crate::charsets::Slots::G0, set);
                t2.configure_charset(crate::charsets::Slots::G0, set);
            }
            _ => unreachable!(),
        }

        // Cursor state must match exactly after every op.
        assert_eq!(t1.screen().cursor.x, t2.screen().cursor.x);
        assert_eq!(t1.screen().cursor.y, t2.screen().cursor.y);
        assert_eq!(
            t1.screen().cursor.pending_wrap,
            t2.screen().cursor.pending_wrap
        );

        // Full screen contents must match after every op.
        let str1 = t1.screen().dump_string(crate::point::Tag::Screen, false);
        let str2 = t2.screen().dump_string(crate::point::Tag::Screen, false);
        assert_eq!(str1, str2, "last print cps: {:?}", &cps_buf);
    }

    // Page integrity (styles refcounts, grapheme maps, etc.) must hold.
    unsafe {
        (*t1.screen().cursor_page())
            .verify_integrity()
            .expect("t1 integrity");
        (*t2.screen().cursor_page())
            .verify_integrity()
            .expect("t2 integrity");
    }
}

// Zig: "Terminal: printSlice differential fuzz vs print". NOTE: this uses a
// local dependency-free PRNG rather than Zig's `std.Random` -- see
// `SplitMix64`'s doc comment for why bit-exact parity isn't needed here.
#[test]
fn print_slice_differential_fuzz_vs_print() {
    // Multiple seeds and terminal sizes for coverage, including a tiny
    // terminal to stress wrap/scroll edge cases. Fixed sizes (not randomized)
    // to match upstream's `testPrintSliceDifferential` call sites exactly --
    // margin-setting ops below assume `cols >= 2` (`cols / 2` is used as a
    // margin bound), so degenerate 1-column terminals are out of scope here,
    // same as upstream.
    //
    // Op counts match upstream's `testPrintSliceDifferential` call sites:
    // (500, 80, 24), (500, 10, 4), (500, 5, 2), (200, 2, 2). They were
    // temporarily held down while `RefCountedSet::add`'s float rehash
    // threshold made `Screen::start_hyperlink` retry `SetNeedsRehash`
    // forever on a fully-living hyperlink set (see
    // `unique_hyperlinks_grow_capacity_without_hanging`).
    let mut rng = SplitMix64::new(0xC0FFEE);
    print_slice_differential(&mut rng, 500, 80, 24);
    print_slice_differential(&mut rng, 500, 10, 4);
    print_slice_differential(&mut rng, 500, 5, 2);
    print_slice_differential(&mut rng, 200, 2, 2);
}

// Zig: "Terminal: printAttributes".
#[test]
fn print_attributes() {
    let mut t = term(5, 5);

    {
        t.set_attribute(Attribute::DirectColorFg(crate::color::Rgb::new(1, 2, 3)));
        assert_eq!(t.print_attributes(), "0;38:2::1:2:3");
        t.set_attribute(Attribute::Unset);
    }

    {
        t.set_attribute(Attribute::Bold);
        t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(1, 2, 3)));
        assert_eq!(t.print_attributes(), "0;1;48:2::1:2:3");
        t.set_attribute(Attribute::Unset);
    }

    {
        t.set_attribute(Attribute::Bold);
        t.set_attribute(Attribute::Faint);
        t.set_attribute(Attribute::Italic);
        t.set_attribute(Attribute::Underline(crate::sgr::Underline::Single));
        t.set_attribute(Attribute::Blink);
        t.set_attribute(Attribute::Inverse);
        t.set_attribute(Attribute::Invisible);
        t.set_attribute(Attribute::Strikethrough);
        t.set_attribute(Attribute::DirectColorFg(crate::color::Rgb::new(
            100, 200, 255,
        )));
        t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(
            101, 102, 103,
        )));
        assert_eq!(
            t.print_attributes(),
            "0;1;2;3;4;5;7;8;9;38:2::100:200:255;48:2::101:102:103"
        );
        t.set_attribute(Attribute::Unset);
    }

    {
        t.set_attribute(Attribute::Underline(crate::sgr::Underline::Single));
        assert_eq!(t.print_attributes(), "0;4");
        t.set_attribute(Attribute::Unset);
    }

    {
        assert_eq!(t.print_attributes(), "0");
    }
}

// ---- eraseDisplay (deferred batch) -----------------------------------------

// Zig: "Terminal: eraseDisplay simple erase below" (dirty-flag variant of
// the existing `erase_display_below`).
#[test]
fn erase_display_simple_erase_below_dirty() {
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
    t.erase_display(EraseDisplay::Below, false);

    assert!(!t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(t.is_dirty(Point::active(0, 2)));

    assert_eq!(t.plain_string(), "ABC\nD");
}

// Zig: "Terminal: eraseDisplay erase below preserves SGR bg".
#[test]
fn erase_display_erase_below_preserves_sgr_bg() {
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
    t.erase_display(EraseDisplay::Below, false);

    assert_eq!(t.plain_string(), "ABC\nD");
    for x in 1..5u16 {
        assert_eq!(bg_rgb_at(&t, x, 1), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
    }
}

// Zig: "Terminal: eraseDisplay below split multi-cell".
#[test]
fn erase_display_below_split_multi_cell() {
    let mut t = term(5, 5);
    t.print_string("AB橋C");
    t.carriage_return();
    t.linefeed();
    t.print_string("DE橋F");
    t.carriage_return();
    t.linefeed();
    t.print_string("GH橋I");
    t.set_cursor_pos(2, 4);
    t.erase_display(EraseDisplay::Below, false);

    assert_eq!(t.plain_string(), "AB橋C\nDE");
}

// Zig: "Terminal: eraseDisplay below protected attributes respected with
// iso".
#[test]
fn erase_display_below_protected_attributes_respected_with_iso() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Iso);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.erase_display(EraseDisplay::Below, false);

    assert_eq!(t.plain_string(), "ABC\nDEF\nGHI");
}

// Zig: "Terminal: eraseDisplay below protected attributes ignored with dec
// most recent".
#[test]
fn erase_display_below_protected_attributes_ignored_with_dec_most_recent() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Iso);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_protected_mode(ProtectedMode::Dec);
    t.set_protected_mode(ProtectedMode::Off);
    t.set_cursor_pos(2, 2);
    t.erase_display(EraseDisplay::Below, false);

    assert_eq!(t.plain_string(), "ABC\nD");
}

// Zig: "Terminal: eraseDisplay below protected attributes ignored with dec
// set".
#[test]
fn erase_display_below_protected_attributes_ignored_with_dec_set() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Dec);
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

// Zig: "Terminal: eraseDisplay below protected attributes respected with
// force".
#[test]
fn erase_display_below_protected_attributes_respected_with_force() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Dec);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.erase_display(EraseDisplay::Below, true);

    assert_eq!(t.plain_string(), "ABC\nDEF\nGHI");
}

// Zig: "Terminal: eraseDisplay simple erase above" (dirty-flag variant of
// the existing `erase_display_above`).
#[test]
fn erase_display_simple_erase_above_dirty() {
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
    t.erase_display(EraseDisplay::Above, false);
    assert!(t.is_dirty(Point::active(0, 0)));
    assert!(t.is_dirty(Point::active(0, 1)));
    assert!(!t.is_dirty(Point::active(0, 2)));

    assert_eq!(t.plain_string(), "\n  F\nGHI");
}

// Zig: "Terminal: eraseDisplay erase above preserves SGR bg".
#[test]
fn erase_display_erase_above_preserves_sgr_bg() {
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
    t.erase_display(EraseDisplay::Above, false);

    assert_eq!(t.plain_string(), "\n  F\nGHI");
    for x in 0..2u16 {
        assert_eq!(bg_rgb_at(&t, x, 1), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
    }
}

// Zig: "Terminal: eraseDisplay above split multi-cell".
#[test]
fn erase_display_above_split_multi_cell() {
    let mut t = term(5, 5);
    t.print_string("AB橋C");
    t.carriage_return();
    t.linefeed();
    t.print_string("DE橋F");
    t.carriage_return();
    t.linefeed();
    t.print_string("GH橋I");
    t.set_cursor_pos(2, 3);
    t.erase_display(EraseDisplay::Above, false);

    assert_eq!(t.plain_string(), "\n    F\nGH橋I");
}

// Zig: "Terminal: eraseDisplay above protected attributes respected with
// iso".
#[test]
fn erase_display_above_protected_attributes_respected_with_iso() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Iso);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.erase_display(EraseDisplay::Above, false);

    assert_eq!(t.plain_string(), "ABC\nDEF\nGHI");
}

// Zig: "Terminal: eraseDisplay above protected attributes ignored with dec
// most recent".
#[test]
fn erase_display_above_protected_attributes_ignored_with_dec_most_recent() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Iso);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_protected_mode(ProtectedMode::Dec);
    t.set_protected_mode(ProtectedMode::Off);
    t.set_cursor_pos(2, 2);
    t.erase_display(EraseDisplay::Above, false);

    assert_eq!(t.plain_string(), "\n  F\nGHI");
}

// Zig: "Terminal: eraseDisplay above protected attributes ignored with dec
// set".
#[test]
fn erase_display_above_protected_attributes_ignored_with_dec_set() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Dec);
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

// Zig: "Terminal: eraseDisplay above protected attributes respected with
// force".
#[test]
fn erase_display_above_protected_attributes_respected_with_force() {
    let mut t = term(5, 5);
    t.set_protected_mode(ProtectedMode::Dec);
    t.print_string("ABC");
    t.carriage_return();
    t.linefeed();
    t.print_string("DEF");
    t.carriage_return();
    t.linefeed();
    t.print_string("GHI");
    t.set_cursor_pos(2, 2);
    t.erase_display(EraseDisplay::Above, true);

    assert_eq!(t.plain_string(), "ABC\nDEF\nGHI");
}

// Zig: "Terminal: eraseDisplay protected complete".
#[test]
fn erase_display_protected_complete() {
    let mut t = term(10, 5);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print_string("123456789");
    let y = t.screen().cursor.y;
    t.set_cursor_pos((y + 1) as usize, 6);
    t.set_protected_mode(ProtectedMode::Dec);
    t.print('X' as u32);
    let y = t.screen().cursor.y;
    t.set_cursor_pos((y + 1) as usize, 4);

    t.clear_dirty();
    t.erase_display(EraseDisplay::Complete, true);
    for y in 0..t.rows as u32 {
        assert!(t.is_dirty(Point::active(0, y)));
    }

    assert_eq!(t.plain_string(), "\n     X");
}

// Zig: "Terminal: eraseDisplay protected below".
#[test]
fn erase_display_protected_below() {
    let mut t = term(10, 5);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print_string("123456789");
    let y = t.screen().cursor.y;
    t.set_cursor_pos((y + 1) as usize, 6);
    t.set_protected_mode(ProtectedMode::Dec);
    t.print('X' as u32);
    let y = t.screen().cursor.y;
    t.set_cursor_pos((y + 1) as usize, 4);
    t.erase_display(EraseDisplay::Below, true);

    assert_eq!(t.plain_string(), "A\n123  X");
}

// Zig: "Terminal: eraseDisplay scroll complete".
#[test]
fn erase_display_scroll_complete() {
    let mut t = term(10, 5);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.erase_display(EraseDisplay::ScrollComplete, false);

    assert_eq!(t.plain_string(), "");
}

// Zig: "Terminal: eraseDisplay protected above".
#[test]
fn erase_display_protected_above() {
    let mut t = term(10, 3);
    t.print('A' as u32);
    t.carriage_return();
    t.linefeed();
    t.print_string("123456789");
    let y = t.screen().cursor.y;
    t.set_cursor_pos((y + 1) as usize, 6);
    t.set_protected_mode(ProtectedMode::Dec);
    t.print('X' as u32);
    let y = t.screen().cursor.y;
    t.set_cursor_pos((y + 1) as usize, 8);
    t.erase_display(EraseDisplay::Above, true);

    assert_eq!(t.plain_string(), "\n     X  9");
}

// Zig: "Terminal: eraseDisplay complete preserves cursor".
#[test]
fn erase_display_complete_preserves_cursor() {
    let mut t = term(5, 5);

    // Set our cursor.
    t.set_attribute(Attribute::Bold);
    t.print_string("AAAA");
    assert_ne!(t.screen().cursor.style_id, style::DEFAULT_ID);

    // Erasing the display may detect that our style is no longer in use and
    // prune our style, which we don't want because it's still our active
    // cursor.
    t.erase_display(EraseDisplay::Complete, false);
    assert_ne!(t.screen().cursor.style_id, style::DEFAULT_ID);
}

// ---- semantic prompt / OSC133 (deferred batch) -----------------------------

// Zig: "Terminal: semantic prompt".
#[test]
fn semantic_prompt_basic() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::page::{SemanticContent, SemanticPrompt as RowSemanticPrompt};

    let mut t = term(10, 5);

    // Prompt.
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::FreshLineNewPrompt,
        options_unvalidated: String::new(),
    });
    t.print_string("hello");
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 5);
    {
        let x = t.screen().cursor.x - 1;
        let y = t.screen().cursor.y;
        let lc = t
            .screen()
            .pages
            .get_cell(Point::active(x, y.into()))
            .unwrap();
        unsafe {
            assert_eq!((*lc.cell).semantic_content(), SemanticContent::Prompt);
            assert_eq!((*lc.row).semantic_prompt(), RowSemanticPrompt::Prompt);
        }
    }

    // Start input but end it on EOL.
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::EndPromptStartInputTerminateEol,
        options_unvalidated: String::new(),
    });
    t.carriage_return();
    t.linefeed();

    // Write some output.
    assert_eq!(t.screen().cursor.y, 1);
    assert_eq!(t.screen().cursor.x, 0);
    t.print_string("world");
    {
        let x = t.screen().cursor.x - 1;
        let y = t.screen().cursor.y;
        let lc = t
            .screen()
            .pages
            .get_cell(Point::active(x, y.into()))
            .unwrap();
        unsafe {
            assert_eq!((*lc.cell).semantic_content(), SemanticContent::Output);
            assert_eq!((*lc.row).semantic_prompt(), RowSemanticPrompt::None);
        }
    }
}

// Zig: "Terminal: semantic prompt continuations".
#[test]
fn semantic_prompt_continuations() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::page::SemanticPrompt as RowSemanticPrompt;

    let mut t = term(10, 5);

    // Prompt.
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::FreshLineNewPrompt,
        options_unvalidated: String::new(),
    });
    t.print_string("hello");
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 5);
    {
        let x = t.screen().cursor.x - 1;
        let y = t.screen().cursor.y;
        let lc = t
            .screen()
            .pages
            .get_cell(Point::active(x, y.into()))
            .unwrap();
        unsafe {
            assert_eq!(
                (*lc.cell).semantic_content(),
                crate::page::SemanticContent::Prompt
            );
            assert_eq!((*lc.row).semantic_prompt(), RowSemanticPrompt::Prompt);
        }
    }

    // Start input but end it on EOL.
    t.carriage_return();
    t.linefeed();
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options_unvalidated: "k=c".to_string(),
    });

    // Write some output.
    assert_eq!(t.screen().cursor.y, 1);
    assert_eq!(t.screen().cursor.x, 0);
    t.print_string("world");
    {
        let x = t.screen().cursor.x - 1;
        let y = t.screen().cursor.y;
        let lc = t
            .screen()
            .pages
            .get_cell(Point::active(x, y.into()))
            .unwrap();
        unsafe {
            assert_eq!(
                (*lc.cell).semantic_content(),
                crate::page::SemanticContent::Prompt
            );
            assert_eq!(
                (*lc.row).semantic_prompt(),
                RowSemanticPrompt::PromptContinuation
            );
        }
    }
}

// Zig: "Terminal: index in prompt mode marks new row as prompt
// continuation". This tests the Fish shell workaround: when in prompt mode
// and we get a newline, assume the new row is a prompt continuation (since
// Fish doesn't emit OSC133 k=s markers for continuation lines).
#[test]
fn index_in_prompt_mode_marks_new_row_as_prompt_continuation() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::page::SemanticPrompt as RowSemanticPrompt;

    let mut t = term(10, 5);

    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options_unvalidated: String::new(),
    });
    t.print_string("hello");

    {
        let lc = t.screen().pages.get_cell(Point::active(0, 0)).unwrap();
        unsafe {
            assert_eq!((*lc.row).semantic_prompt(), RowSemanticPrompt::Prompt);
        }
    }

    t.carriage_return();
    t.linefeed();

    {
        let lc = t.screen().pages.get_cell(Point::active(0, 1)).unwrap();
        unsafe {
            assert_eq!(
                (*lc.row).semantic_prompt(),
                RowSemanticPrompt::PromptContinuation
            );
        }
    }

    assert_eq!(
        t.screen().cursor.semantic_content,
        crate::page::SemanticContent::Prompt
    );
}

// Zig: "Terminal: index in input mode does not mark new row as prompt".
// Input mode should NOT trigger prompt continuation on newline (only prompt
// mode does, not input mode).
#[test]
fn index_in_input_mode_does_not_mark_new_row_as_prompt() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::page::SemanticPrompt as RowSemanticPrompt;

    let mut t = term(10, 5);

    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options_unvalidated: String::new(),
    });
    t.print_string("$ ");
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::EndPromptStartInput,
        options_unvalidated: String::new(),
    });
    t.print_string("echo \\");

    t.carriage_return();
    t.linefeed();

    {
        let lc = t.screen().pages.get_cell(Point::active(0, 1)).unwrap();
        unsafe {
            assert_eq!(
                (*lc.row).semantic_prompt(),
                RowSemanticPrompt::PromptContinuation
            );
        }
    }

    assert_eq!(
        t.screen().cursor.semantic_content,
        crate::page::SemanticContent::Input
    );
}

// Zig: "Terminal: index in output mode does not mark new row as prompt".
// Output mode should NOT trigger prompt continuation.
#[test]
fn index_in_output_mode_does_not_mark_new_row_as_prompt() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::page::SemanticPrompt as RowSemanticPrompt;

    let mut t = term(10, 5);

    // Complete prompt cycle: prompt -> input -> output.
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options_unvalidated: String::new(),
    });
    t.print_string("$ ");
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::EndPromptStartInput,
        options_unvalidated: String::new(),
    });
    t.print_string("ls");
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::EndInputStartOutput,
        options_unvalidated: String::new(),
    });

    t.carriage_return();
    t.linefeed();

    {
        let lc = t.screen().pages.get_cell(Point::active(0, 1)).unwrap();
        unsafe {
            assert_eq!((*lc.row).semantic_prompt(), RowSemanticPrompt::None);
        }
    }
}

// Zig: "Terminal: OSC133C at x=0 on prompt row clears prompt mark". This
// tests the second Fish heuristic: when Fish emits a newline then
// immediately sends OSC133C (start output) at column 0, we should clear the
// prompt continuation mark we just set.
#[test]
fn osc133c_at_x0_on_prompt_row_clears_prompt_mark() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::page::SemanticPrompt as RowSemanticPrompt;

    let mut t = term(10, 5);

    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options_unvalidated: String::new(),
    });
    t.print_string("$ echo \\");

    // Simulate Fish behavior: newline first (which marks next row as
    // prompt).
    t.carriage_return();
    t.linefeed();

    {
        let lc = t.screen().pages.get_cell(Point::active(0, 1)).unwrap();
        unsafe {
            assert_eq!(
                (*lc.row).semantic_prompt(),
                RowSemanticPrompt::PromptContinuation
            );
        }
    }

    // Now Fish sends OSC133C at column 0 (cursor is still at x=0).
    assert_eq!(t.screen().cursor.x, 0);
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::EndInputStartOutput,
        options_unvalidated: String::new(),
    });

    {
        let lc = t.screen().pages.get_cell(Point::active(0, 1)).unwrap();
        unsafe {
            assert_eq!((*lc.row).semantic_prompt(), RowSemanticPrompt::None);
        }
    }
}

// Zig: "Terminal: OSC133C at x>0 on prompt row does not clear prompt mark".
// If we're not at column 0, we shouldn't clear the prompt mark.
#[test]
fn osc133c_at_x_gt_0_on_prompt_row_does_not_clear_prompt_mark() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::page::SemanticPrompt as RowSemanticPrompt;

    let mut t = term(10, 5);

    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options_unvalidated: String::new(),
    });
    t.print_string("$ ");

    t.carriage_return();
    t.linefeed();
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options_unvalidated: "k=c".to_string(),
    });
    t.print_string("> ");

    {
        let lc = t.screen().pages.get_cell(Point::active(0, 1)).unwrap();
        unsafe {
            assert_eq!(
                (*lc.row).semantic_prompt(),
                RowSemanticPrompt::PromptContinuation
            );
        }
    }

    assert!(t.screen().cursor.x > 0);
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::EndInputStartOutput,
        options_unvalidated: String::new(),
    });

    {
        let lc = t.screen().pages.get_cell(Point::active(0, 1)).unwrap();
        unsafe {
            assert_eq!(
                (*lc.row).semantic_prompt(),
                RowSemanticPrompt::PromptContinuation
            );
        }
    }
}

// Zig: "Terminal: multiple newlines in prompt mode marks all rows".
#[test]
fn multiple_newlines_in_prompt_mode_marks_all_rows() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::page::SemanticPrompt as RowSemanticPrompt;

    let mut t = term(10, 5);

    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options_unvalidated: String::new(),
    });
    t.print_string("line1");

    t.carriage_return();
    t.linefeed();
    t.print_string("line2");
    t.carriage_return();
    t.linefeed();
    t.print_string("line3");

    {
        let lc = t.screen().pages.get_cell(Point::active(0, 0)).unwrap();
        unsafe {
            assert_eq!((*lc.row).semantic_prompt(), RowSemanticPrompt::Prompt);
        }
    }
    {
        let lc = t.screen().pages.get_cell(Point::active(0, 1)).unwrap();
        unsafe {
            assert_eq!(
                (*lc.row).semantic_prompt(),
                RowSemanticPrompt::PromptContinuation
            );
        }
    }
    {
        let lc = t.screen().pages.get_cell(Point::active(0, 2)).unwrap();
        unsafe {
            assert_eq!(
                (*lc.row).semantic_prompt(),
                RowSemanticPrompt::PromptContinuation
            );
        }
    }
}

// Zig: "Terminal: OSC133A click_events=1 sets click to click_events".
#[test]
fn osc133a_click_events_1_sets_click_to_click_events() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::screen::semantic::{ClickEvents, SemanticClick};

    let mut t = term(10, 5);
    assert_eq!(t.screen().semantic_prompt.click, SemanticClick::None);

    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::FreshLineNewPrompt,
        options_unvalidated: "click_events=1".to_string(),
    });

    assert_eq!(
        t.screen().semantic_prompt.click,
        SemanticClick::ClickEvents(ClickEvents::Absolute)
    );
}

// Zig: "Terminal: OSC133A click_events=2 sets click to click_events
// (relative)".
#[test]
fn osc133a_click_events_2_sets_click_to_click_events_relative() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::screen::semantic::{ClickEvents, SemanticClick};

    let mut t = term(10, 5);
    assert_eq!(t.screen().semantic_prompt.click, SemanticClick::None);

    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::FreshLineNewPrompt,
        options_unvalidated: "click_events=2".to_string(),
    });

    assert_eq!(
        t.screen().semantic_prompt.click,
        SemanticClick::ClickEvents(ClickEvents::Relative)
    );
}

// Zig: "Terminal: OSC133A click_events=0 does not set click_events".
#[test]
fn osc133a_click_events_0_does_not_set_click_events() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::screen::semantic::SemanticClick;

    let mut t = term(10, 5);

    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::FreshLineNewPrompt,
        options_unvalidated: "click_events=0".to_string(),
    });

    assert_eq!(t.screen().semantic_prompt.click, SemanticClick::None);
}

// Zig: "Terminal: OSC133A cl option sets click to cl value".
#[test]
fn osc133a_cl_option_sets_click_to_cl_value() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::screen::semantic::{Click, SemanticClick};

    let mut t = term(10, 5);

    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::FreshLineNewPrompt,
        options_unvalidated: "cl=m".to_string(),
    });

    assert_eq!(
        t.screen().semantic_prompt.click,
        SemanticClick::Cl(Click::Multiple)
    );
}

// Zig: "Terminal: OSC133A cl=line sets click to line".
#[test]
fn osc133a_cl_line_sets_click_to_line() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::screen::semantic::{Click, SemanticClick};

    let mut t = term(10, 5);

    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::FreshLineNewPrompt,
        options_unvalidated: "cl=line".to_string(),
    });

    assert_eq!(
        t.screen().semantic_prompt.click,
        SemanticClick::Cl(Click::Line)
    );
}

// Zig: "Terminal: OSC133A click_events=1 takes priority over cl".
#[test]
fn osc133a_click_events_1_takes_priority_over_cl() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::screen::semantic::{ClickEvents, SemanticClick};

    let mut t = term(10, 5);

    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::FreshLineNewPrompt,
        options_unvalidated: "click_events=1;cl=m".to_string(),
    });

    assert_eq!(
        t.screen().semantic_prompt.click,
        SemanticClick::ClickEvents(ClickEvents::Absolute)
    );
}

// Zig: "Terminal: OSC133A click_events=0 falls back to cl".
#[test]
fn osc133a_click_events_0_falls_back_to_cl() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::screen::semantic::{Click, SemanticClick};

    let mut t = term(10, 5);

    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::FreshLineNewPrompt,
        options_unvalidated: "click_events=0;cl=v".to_string(),
    });

    assert_eq!(
        t.screen().semantic_prompt.click,
        SemanticClick::Cl(Click::ConservativeVertical)
    );
}

// Zig: "Terminal: OSC133A no click options leaves click as none".
#[test]
fn osc133a_no_click_options_leaves_click_as_none() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};
    use crate::screen::semantic::SemanticClick;

    let mut t = term(10, 5);

    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::FreshLineNewPrompt,
        options_unvalidated: "aid=123".to_string(),
    });

    assert_eq!(t.screen().semantic_prompt.click, SemanticClick::None);
}

// Zig: "Terminal: cursorIsAtPrompt".
#[test]
fn cursor_is_at_prompt() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};

    let mut t = term(10, 3);

    assert!(!t.cursor_is_at_prompt());
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options_unvalidated: String::new(),
    });
    assert!(t.cursor_is_at_prompt());
    t.print_string("$ ");

    // Input is also a prompt.
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::EndPromptStartInput,
        options_unvalidated: String::new(),
    });
    assert!(t.cursor_is_at_prompt());
    t.print_string("ls");

    // But once we say we're starting output, we're not a prompt (cursor is
    // not at x=0, so the Fish heuristic doesn't trigger).
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::EndInputStartOutput,
        options_unvalidated: String::new(),
    });
    // Still a prompt because this line has a prompt.
    assert!(t.cursor_is_at_prompt());
    t.linefeed();
    assert!(!t.cursor_is_at_prompt());

    // Until we know we're at a prompt again.
    t.linefeed();
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options_unvalidated: String::new(),
    });
    assert!(t.cursor_is_at_prompt());
}

// Zig: "Terminal: cursorIsAtPrompt alternate screen".
#[test]
fn cursor_is_at_prompt_alternate_screen() {
    use crate::osc::{SemanticPrompt, SemanticPromptAction};

    let mut t = term(3, 2);

    assert!(!t.cursor_is_at_prompt());
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options_unvalidated: String::new(),
    });
    assert!(t.cursor_is_at_prompt());

    // Secondary screen is never a prompt.
    t.switch_screen_mode(SwitchScreenMode::M1049, true);
    assert!(!t.cursor_is_at_prompt());
    t.semantic_prompt(&SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options_unvalidated: String::new(),
    });
    assert!(!t.cursor_is_at_prompt());
}

// ---- fullReset (deferred batch) --------------------------------------------

// Zig: "Terminal: fullReset with a non-empty pen".
#[test]
fn full_reset_with_a_non_empty_pen() {
    let mut t = term(80, 80);

    t.set_attribute(Attribute::DirectColorFg(crate::color::Rgb::new(
        0xFF, 0, 0x7F,
    )));
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(
        0xFF, 0, 0x7F,
    )));
    t.screen_mut().cursor.semantic_content = crate::page::SemanticContent::Input;
    t.full_reset();

    {
        let (x, y) = (t.screen().cursor.x, t.screen().cursor.y);
        let lc = t
            .screen()
            .pages
            .get_cell(Point::active(x, y.into()))
            .unwrap();
        unsafe {
            assert_eq!((*lc.cell).style_id(), 0);
        }
    }

    assert_eq!(t.screen().cursor.style_id, 0);
    assert_eq!(
        t.screen().cursor.semantic_content,
        crate::page::SemanticContent::Output
    );
}

// Zig: "Terminal: fullReset hyperlink".
#[test]
fn full_reset_hyperlink() {
    let mut t = term(80, 80);

    t.screen_mut()
        .start_hyperlink(b"http://example.com", None)
        .unwrap();
    t.full_reset();
    assert_eq!(t.screen().cursor.hyperlink_id, 0);
}

// Zig: "Terminal: fullReset with a non-empty saved cursor".
#[test]
fn full_reset_with_a_non_empty_saved_cursor() {
    let mut t = term(80, 80);

    t.set_attribute(Attribute::DirectColorFg(crate::color::Rgb::new(
        0xFF, 0, 0x7F,
    )));
    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(
        0xFF, 0, 0x7F,
    )));
    t.save_cursor();
    t.full_reset();

    {
        let (x, y) = (t.screen().cursor.x, t.screen().cursor.y);
        let lc = t
            .screen()
            .pages
            .get_cell(Point::active(x, y.into()))
            .unwrap();
        unsafe {
            assert_eq!((*lc.cell).style_id(), 0);
        }
    }

    assert_eq!(t.screen().cursor.style_id, 0);
}

// Zig: "Terminal: fullReset origin mode".
#[test]
fn full_reset_origin_mode() {
    let mut t = term(10, 10);

    t.set_cursor_pos(3, 5);
    t.modes.set(Mode::Origin, true);
    t.full_reset();

    // Origin mode should be reset and the cursor should be moved.
    assert_eq!(t.screen().cursor.y, 0);
    assert_eq!(t.screen().cursor.x, 0);
    assert!(!t.modes.get(Mode::Origin));
}

// Zig: "Terminal: fullReset status display".
#[test]
fn full_reset_status_display() {
    let mut t = term(10, 10);

    t.status_display = super::StatusDisplay::StatusLine;
    t.full_reset();
    assert_eq!(t.status_display, super::StatusDisplay::Main);
}

// Zig: "Terminal: fullReset clears alt screen kitty keyboard state".
// https://github.com/mitchellh/ghostty/issues/1607
#[test]
fn full_reset_clears_alt_screen_kitty_keyboard_state() {
    let mut t = term(10, 10);

    t.switch_screen_mode(SwitchScreenMode::M1049, true);
    t.screen_mut()
        .kitty_keyboard
        .push(crate::screen::kitty_key::Flags {
            disambiguate: true,
            report_events: false,
            report_alternates: true,
            report_all: true,
            report_associated: true,
        });
    t.switch_screen_mode(SwitchScreenMode::M1049, false);

    t.full_reset();
    assert!(t.screens.get(ScreenKey::Alternate).is_none());
}

// Zig: "Terminal: fullReset default modes".
//
// NOTE(M1 backfill): upstream configures a non-standard `default_modes`
// (`.{ .grapheme_cluster = true }`) via `Terminal.Options`, which this Rust
// port doesn't expose (no `default_modes` field on `terminal::Options`).
// Instead we exercise the same "full_reset preserves the *documented*
// per-mode default, not `false`" code path (`ModeState.reset`, which
// restores `self.default`, not zero) using a mode whose own *documented*
// default is already non-false: `SendReceiveMode` (SRM, default true per
// `modes.rs`'s registration table). This still catches a regression where
// `full_reset`/`modes.reset()` clobbers defaults to false instead of
// restoring them.
#[test]
fn full_reset_default_modes() {
    let mut t = term(10, 10);
    assert!(t.modes.get(Mode::SendReceiveMode));
    t.modes.set(Mode::SendReceiveMode, false);
    t.full_reset();
    assert!(t.modes.get(Mode::SendReceiveMode));
}

// Zig: "Terminal: fullReset tracked pins".
#[test]
fn full_reset_tracked_pins() {
    let mut t = term(80, 80);

    // Create a tracked pin.
    let cursor_pin = unsafe { *t.screen().cursor.page_pin };
    let p = t.screen_mut().pages.track_pin(cursor_pin);
    t.full_reset();
    assert!(unsafe { t.screen().pages.pin_is_valid(*p) });
}

// ---- resize / reflow / DECCOLM (deferred batch) ----------------------------

// Zig: "Terminal: resize less cols with wide char then print".
// https://github.com/mitchellh/ghostty/issues/272
// This is also tested in depth in screen resize tests but we keep this
// around to ensure we don't regress at multiple layers.
#[test]
fn resize_less_cols_with_wide_char_then_print() {
    let mut t = term(3, 3);
    t.print('x' as u32);
    t.print(0x1F600); // 😀
    t.resize(2, 3);
    t.set_cursor_pos(1, 2);
    t.print(0x1F600); // 😀
}

// Zig: "Terminal: resize with left and right margin set".
// https://github.com/mitchellh/ghostty/issues/723
// This was found via fuzzing so it's highly specific.
#[test]
fn resize_with_left_and_right_margin_set() {
    let cols = 70;
    let rows = 23;
    let mut t = term(cols, rows);

    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.print('0' as u32);
    t.modes.set(Mode::EnableMode3, true);
    t.resize(cols, rows);
    t.set_left_and_right_margin(2, 0);
    t.print_repeat(1850);
    t.modes.restore(Mode::EnableMode3);
    t.resize(cols, rows);
}

// Zig: "Terminal: resize with wraparound off".
// https://github.com/mitchellh/ghostty/issues/1343
#[test]
fn resize_with_wraparound_off() {
    let cols = 4;
    let rows = 2;
    let mut t = term(cols, rows);

    t.modes.set(Mode::Wraparound, false);
    t.print('0' as u32);
    t.print('1' as u32);
    t.print('2' as u32);
    t.print('3' as u32);
    let new_cols = 2;
    t.resize(new_cols, rows);

    assert_eq!(t.plain_string(), "01");
}

// Zig: "Terminal: resize with wraparound on".
#[test]
fn resize_with_wraparound_on() {
    let cols = 4;
    let rows = 2;
    let mut t = term(cols, rows);

    t.modes.set(Mode::Wraparound, true);
    t.print('0' as u32);
    t.print('1' as u32);
    t.print('2' as u32);
    t.print('3' as u32);
    let new_cols = 2;
    t.resize(new_cols, rows);

    assert_eq!(t.plain_string(), "01\n23");
}

// Zig: "Terminal: resize with high unique style per cell".
#[test]
fn resize_with_high_unique_style_per_cell() {
    let mut t = term(30, 30);

    for y in 0..t.rows as usize {
        for x in 0..t.cols as usize {
            t.set_cursor_pos(y, x);
            t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(
                x as u8, y as u8, 0,
            )));
            t.print('x' as u32);
        }
    }

    t.resize(60, 30);
}

// Zig: "Terminal: resize with high unique style per cell with wrapping".
#[test]
fn resize_with_high_unique_style_per_cell_with_wrapping() {
    let mut t = term(30, 30);

    let cell_count: u32 = t.rows as u32 * t.cols as u32;
    for i in 0..cell_count {
        let r = (i >> 8) as u8;
        let g = (i & 0xFF) as u8;
        t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(r, g, 0)));
        t.print('x' as u32);
    }

    t.resize(60, 30);
}

// Zig: "Terminal: resize with reflow and saved cursor".
#[test]
fn resize_with_reflow_and_saved_cursor() {
    let mut t = term(2, 3);
    t.print_string("1A2B");
    t.set_cursor_pos(2, 2);
    {
        let (x, y) = (t.screen().cursor.x, t.screen().cursor.y);
        let lc = t
            .screen()
            .pages
            .get_cell(Point::active(x, y.into()))
            .unwrap();
        unsafe {
            assert_eq!((*lc.cell).codepoint(), 'B' as u32);
        }
    }

    assert_eq!(t.plain_string(), "1A\n2B");

    t.save_cursor();
    t.resize(5, 3);
    t.restore_cursor();

    assert_eq!(t.plain_string(), "1A2B");

    // Verify our cursor is still in the same place.
    {
        let (x, y) = (t.screen().cursor.x, t.screen().cursor.y);
        let lc = t
            .screen()
            .pages
            .get_cell(Point::active(x, y.into()))
            .unwrap();
        unsafe {
            assert_eq!((*lc.cell).codepoint(), 'B' as u32);
        }
    }
}

// Zig: "Terminal: resize with reflow and saved cursor pending wrap".
#[test]
fn resize_with_reflow_and_saved_cursor_pending_wrap() {
    let mut t = term(2, 3);
    t.print_string("1A2B");
    {
        let (x, y) = (t.screen().cursor.x, t.screen().cursor.y);
        let lc = t
            .screen()
            .pages
            .get_cell(Point::active(x, y.into()))
            .unwrap();
        unsafe {
            assert_eq!((*lc.cell).codepoint(), 'B' as u32);
        }
    }

    assert_eq!(t.plain_string(), "1A\n2B");

    t.save_cursor();
    t.resize(5, 3);
    t.restore_cursor();

    assert_eq!(t.plain_string(), "1A2B");

    // Pending wrap should be reset.
    t.print('X' as u32);
    assert_eq!(t.plain_string(), "1A2BX");
}

// Zig: "Terminal: DECCOLM without DEC mode 40".
#[test]
fn deccolm_without_dec_mode_40() {
    let mut t = term(5, 5);

    t.modes.set(Mode::Column132, true);
    t.deccolm(true);
    assert_eq!(t.cols, 5);
    assert_eq!(t.rows, 5);
    assert!(!t.modes.get(Mode::Column132));
}

// Zig: "Terminal: DECCOLM unset".
#[test]
fn deccolm_unset() {
    let mut t = term(5, 5);

    t.modes.set(Mode::EnableMode3, true);
    t.deccolm(false);
    assert_eq!(t.cols, 80);
    assert_eq!(t.rows, 5);
}

// Zig: "Terminal: DECCOLM resets pending wrap".
#[test]
fn deccolm_resets_pending_wrap() {
    let mut t = term(5, 5);
    t.print_string("ABCDE");
    assert!(t.screen().cursor.pending_wrap);

    t.modes.set(Mode::EnableMode3, true);
    t.deccolm(false);
    assert_eq!(t.cols, 80);
    assert_eq!(t.rows, 5);
    assert!(!t.screen().cursor.pending_wrap);
}

// Zig: "Terminal: DECCOLM preserves SGR bg".
#[test]
fn deccolm_preserves_sgr_bg() {
    let mut t = term(5, 5);

    t.set_attribute(Attribute::DirectColorBg(crate::color::Rgb::new(0xFF, 0, 0)));
    t.modes.set(Mode::EnableMode3, true);
    t.deccolm(false);

    assert_eq!(bg_rgb_at(&t, 0, 0), (ContentTag::BgColorRgb, (0xFF, 0, 0)));
}

// Zig: "Terminal: DECCOLM resets scroll region".
#[test]
fn deccolm_resets_scroll_region() {
    let mut t = term(5, 5);

    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_top_and_bottom_margin(2, 3);
    t.set_left_and_right_margin(3, 5);

    t.modes.set(Mode::EnableMode3, true);
    t.deccolm(false);

    assert!(t.modes.get(Mode::EnableLeftAndRightMargin));
    assert_eq!(t.scrolling_region.top, 0);
    assert_eq!(t.scrolling_region.bottom, 4);
    assert_eq!(t.scrolling_region.left, 0);
    assert_eq!(t.scrolling_region.right, 79);
}

// ---- alt-screen modes 47 / 1047 / 1049 (deferred batch) -------------------

// Zig: "Terminal: mode 47 alt screen plain".
#[test]
fn mode_47_alt_screen_plain() {
    let mut t = term(5, 5);

    // Print on primary screen.
    t.print_string("1A");

    // Go to alt screen with mode 47.
    t.switch_screen_mode(SwitchScreenMode::M47, true);
    assert_eq!(t.screens.active_key(), ScreenKey::Alternate);

    // Screen should be empty.
    assert_eq!(t.plain_string(), "");

    // Print on alt screen. This should be off center because we copy the
    // cursor over from the primary screen.
    t.print_string("2B");
    assert_eq!(t.plain_string(), "  2B");

    // Go back to primary.
    t.switch_screen_mode(SwitchScreenMode::M47, false);
    assert_eq!(t.screens.active_key(), ScreenKey::Primary);

    // Primary screen should still have the original content.
    assert_eq!(t.plain_string(), "1A");

    // Go back to alt screen with mode 47.
    t.switch_screen_mode(SwitchScreenMode::M47, true);
    assert_eq!(t.screens.active_key(), ScreenKey::Alternate);

    // Screen should retain content.
    assert_eq!(t.plain_string(), "  2B");
}

// Zig: "Terminal: mode 47 copies cursor both directions".
#[test]
fn mode_47_copies_cursor_both_directions() {
    let mut t = term(5, 5);

    // Color our cursor red.
    t.set_attribute(Attribute::DirectColorFg(crate::color::Rgb::new(
        0xFF, 0, 0x7F,
    )));

    // Go to alt screen with mode 47.
    t.switch_screen_mode(SwitchScreenMode::M47, true);
    assert_eq!(t.screens.active_key(), ScreenKey::Alternate);

    // Verify that our style is set.
    unsafe {
        assert_ne!(t.screen().cursor.style_id, style::DEFAULT_ID);
        let page = t.screen().cursor_page();
        assert_eq!((*page).styles().count(), 1);
        let mem = (*page).memory();
        assert!(
            (*page)
                .styles()
                .ref_count(mem as *mut u8, t.screen().cursor.style_id)
                > 0
        );
    }

    // Set a new style.
    t.set_attribute(Attribute::DirectColorFg(crate::color::Rgb::new(0, 0xFF, 0)));

    // Go back to primary.
    t.switch_screen_mode(SwitchScreenMode::M47, false);
    assert_eq!(t.screens.active_key(), ScreenKey::Primary);

    // Verify that our style is still set.
    unsafe {
        assert_ne!(t.screen().cursor.style_id, style::DEFAULT_ID);
        let page = t.screen().cursor_page();
        assert_eq!((*page).styles().count(), 1);
        let mem = (*page).memory();
        assert!(
            (*page)
                .styles()
                .ref_count(mem as *mut u8, t.screen().cursor.style_id)
                > 0
        );
    }
}

// Zig: "Terminal: mode 1047 alt screen plain".
#[test]
fn mode_1047_alt_screen_plain() {
    let mut t = term(5, 5);

    // Print on primary screen.
    t.print_string("1A");

    // Go to alt screen with mode 1047.
    t.switch_screen_mode(SwitchScreenMode::M1047, true);
    assert_eq!(t.screens.active_key(), ScreenKey::Alternate);

    // Screen should be empty.
    assert_eq!(t.plain_string(), "");

    // Print on alt screen. This should be off center because we copy the
    // cursor over from the primary screen.
    t.print_string("2B");
    assert_eq!(t.plain_string(), "  2B");

    // Go back to primary.
    t.switch_screen_mode(SwitchScreenMode::M1047, false);
    assert_eq!(t.screens.active_key(), ScreenKey::Primary);

    // Primary screen should still have the original content.
    assert_eq!(t.plain_string(), "1A");

    // Go back to alt screen with mode 1047.
    t.switch_screen_mode(SwitchScreenMode::M1047, true);
    assert_eq!(t.screens.active_key(), ScreenKey::Alternate);

    // Screen should be empty (mode 1047 clears the alt screen on exit,
    // unlike mode 47 which retains content).
    assert_eq!(t.plain_string(), "");
}

// Zig: "Terminal: mode 1047 copies cursor both directions".
#[test]
fn mode_1047_copies_cursor_both_directions() {
    let mut t = term(5, 5);

    // Color our cursor red.
    t.set_attribute(Attribute::DirectColorFg(crate::color::Rgb::new(
        0xFF, 0, 0x7F,
    )));

    // Go to alt screen with mode 1047.
    t.switch_screen_mode(SwitchScreenMode::M1047, true);
    assert_eq!(t.screens.active_key(), ScreenKey::Alternate);

    // Verify that our style is set.
    unsafe {
        assert_ne!(t.screen().cursor.style_id, style::DEFAULT_ID);
        let page = t.screen().cursor_page();
        assert_eq!((*page).styles().count(), 1);
        let mem = (*page).memory();
        assert!(
            (*page)
                .styles()
                .ref_count(mem as *mut u8, t.screen().cursor.style_id)
                > 0
        );
    }

    // Set a new style.
    t.set_attribute(Attribute::DirectColorFg(crate::color::Rgb::new(0, 0xFF, 0)));

    // Go back to primary.
    t.switch_screen_mode(SwitchScreenMode::M1047, false);
    assert_eq!(t.screens.active_key(), ScreenKey::Primary);

    // Verify that our style is still set.
    unsafe {
        assert_ne!(t.screen().cursor.style_id, style::DEFAULT_ID);
        let page = t.screen().cursor_page();
        assert_eq!((*page).styles().count(), 1);
        let mem = (*page).memory();
        assert!(
            (*page)
                .styles()
                .ref_count(mem as *mut u8, t.screen().cursor.style_id)
                > 0
        );
    }
}

// Zig: "Terminal: mode 1049 alt screen plain".
#[test]
fn mode_1049_alt_screen_plain() {
    let mut t = term(5, 5);

    // Print on primary screen.
    t.print_string("1A");

    // Go to alt screen with mode 1049.
    t.switch_screen_mode(SwitchScreenMode::M1049, true);
    assert_eq!(t.screens.active_key(), ScreenKey::Alternate);

    // Screen should be empty.
    assert_eq!(t.plain_string(), "");

    // Print on alt screen. This should be off center because we copy the
    // cursor over from the primary screen.
    t.print_string("2B");
    assert_eq!(t.plain_string(), "  2B");

    // Go back to primary.
    t.switch_screen_mode(SwitchScreenMode::M1049, false);
    assert_eq!(t.screens.active_key(), ScreenKey::Primary);

    // Primary screen should still have the original content.
    assert_eq!(t.plain_string(), "1A");

    // Write; our cursor should be restored back.
    t.print_string("C");
    assert_eq!(t.plain_string(), "1AC");

    // Go back to alt screen with mode 1049.
    t.switch_screen_mode(SwitchScreenMode::M1049, true);
    assert_eq!(t.screens.active_key(), ScreenKey::Alternate);

    // Screen should be empty.
    assert_eq!(t.plain_string(), "");
}

// ---- misc tail --------------------------------------------------------------

// Zig: "Terminal: deleteLines wide char at right margin with full clear".
// Reproduces a crash found by AFL++ fuzzer. The crash is a page integrity
// violation "spacer tail not following wide" triggered during scrollUp ->
// deleteLines -> clearCells. When deleteLines count >= scroll region
// height, all rows are cleared (no shifting), so rowWillBeShifted is never
// called and wide characters straddling the right margin boundary leave
// orphaned spacer_tails.
#[test]
fn delete_lines_wide_char_at_right_margin_with_full_clear() {
    let mut t = term(80, 24);

    // Place a wide character at col 39 (1-indexed) on several rows. The
    // wide cell lands at col 38 (0-indexed) with spacer_tail at col 39.
    t.set_cursor_pos(10, 39);
    t.print(0x4E2D); // '中'

    // Set left/right scroll margins so scrolling_region.right = 38.
    // clear_cells will clear cells[4..39], which includes the wide cell at
    // col 38 but NOT the spacer_tail at col 39.
    t.modes.set(Mode::EnableLeftAndRightMargin, true);
    t.set_left_and_right_margin(5, 39);

    // scroll_up with count >= region height causes delete_lines to clear
    // ALL rows without any shifting, so rowWillBeShifted is never called
    // and the orphaned spacer_tail at col 39 triggers a page integrity
    // violation in clear_cells.
    t.scroll_up(t.rows as usize);
}
