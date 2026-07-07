//! Inline tests ported from `Screen.zig` (commit `2da015cd6`).
//!
//! See `docs/analysis/screen.md` for the Zig-vs-Rust test accounting and the
//! deferred list.

#![allow(clippy::bool_assert_comparison)]

use super::*;

fn init(cols: CellCountInt, rows: CellCountInt, max_scrollback: usize) -> Screen {
    Screen::init(Options {
        cols,
        rows,
        max_scrollback,
    })
}

/// A no-reflow resize with the default (no) prompt redraw.
fn resize(s: &mut Screen, cols: CellCountInt, rows: CellCountInt, reflow: bool) {
    s.resize(Resize {
        cols,
        rows,
        reflow,
        prompt_redraw: Redraw::False,
    });
}

/// Read the codepoint of the cell at an active point.
fn active_codepoint(s: &Screen, x: CellCountInt, y: CellCountInt) -> u32 {
    let c = s.pages.get_cell(Point::active(x, y as u32)).unwrap();
    unsafe { (*c.cell).codepoint() }
}

/// Read the `wrap` flag of the row at a screen point.
fn screen_row_wrap(s: &Screen, y: u32) -> bool {
    let c = s.pages.get_cell(Point::screen(0, y)).unwrap();
    unsafe { (*c.row).wrap() }
}

/// Read the codepoint of the cell at a screen point.
fn screen_codepoint(s: &Screen, x: CellCountInt, y: u32) -> u32 {
    let c = s.pages.get_cell(Point::screen(x, y)).unwrap();
    unsafe { (*c.cell).codepoint() }
}

/// Read the `wide` state of the cell at a screen point.
fn screen_wide(s: &Screen, x: CellCountInt, y: u32) -> crate::page::Wide {
    let c = s.pages.get_cell(Point::screen(x, y)).unwrap();
    unsafe { (*c.cell).wide() }
}

/// Read the semantic-prompt state of the row at an active point.
fn active_row_semantic_prompt(s: &Screen, y: u32) -> crate::page::SemanticPrompt {
    let c = s.pages.get_cell(Point::active(0, y)).unwrap();
    unsafe { (*c.row).semantic_prompt() }
}

/// Write a bg-color-rgb cell (non-text) at an active point, used by the
/// "trims blank lines" resize tests. Mirrors `list_cell.cell.* = .{ .content_tag
/// = .bg_color_rgb, ... }`.
fn write_bg_rgb(s: &mut Screen, x: CellCountInt, y: CellCountInt) {
    use crate::page::ContentTag;
    let c = s.pages.get_cell(Point::active(x, y as u32)).unwrap();
    unsafe {
        let mut cell = crate::page::Cell::default();
        cell.set_content_tag(ContentTag::BgColorRgb);
        *c.cell = cell;
    }
}

// ---- read and write -----------------------------------------------------

// Port of `test "Screen read and write"`.
#[test]
fn read_and_write() {
    let mut s = init(80, 24, 1000);
    assert_eq!(s.cursor.style_id, 0);
    s.test_write_string("hello, world");
    assert_eq!(s.dump_string(Tag::Screen, false), "hello, world");
}

// Port of `test "Screen read and write newline"`.
#[test]
fn read_and_write_newline() {
    let mut s = init(80, 24, 1000);
    assert_eq!(s.cursor.style_id, 0);
    s.test_write_string("hello\nworld");
    assert_eq!(s.dump_string(Tag::Screen, false), "hello\nworld");
}

// Port of `test "Screen read and write scrollback"`.
#[test]
fn read_and_write_scrollback() {
    let mut s = init(80, 2, 1000);
    s.test_write_string("hello\nworld\ntest");
    assert_eq!(s.dump_string(Tag::Screen, false), "hello\nworld\ntest");
    assert_eq!(s.dump_string(Tag::Active, false), "world\ntest");
}

// Port of `test "Screen read and write no scrollback small"`.
#[test]
fn read_and_write_no_scrollback_small() {
    let mut s = init(80, 2, 0);
    s.test_write_string("hello\nworld\ntest");
    assert_eq!(s.dump_string(Tag::Screen, false), "world\ntest");
    assert_eq!(s.dump_string(Tag::Active, false), "world\ntest");
}

// Port of `test "Screen read and write no scrollback large"`.
#[test]
fn read_and_write_no_scrollback_large() {
    let mut s = init(80, 2, 0);
    for i in 0..1_000 {
        s.test_write_string(&format!("{i}\n"));
    }
    s.test_write_string("1000");
    assert_eq!(s.dump_string(Tag::Screen, false), "999\n1000");
}

// ---- clearRows / eraseRows ----------------------------------------------

// Port of `test "Screen clearRows active one line"`.
#[test]
fn clear_rows_active_one_line() {
    let mut s = init(80, 24, 1000);
    s.test_write_string("hello, world");
    s.clear_rows(Point::active(0, 0), None, false);
    assert!(s.pages.is_dirty(Point::active(0, 0)));
    assert_eq!(s.dump_string(Tag::Screen, false), "");
}

// Port of `test "Screen clearRows active multi line"`.
#[test]
fn clear_rows_active_multi_line() {
    let mut s = init(80, 24, 1000);
    s.test_write_string("hello\nworld");
    s.clear_rows(Point::active(0, 0), None, false);
    assert!(s.pages.is_dirty(Point::active(0, 0)));
    assert!(s.pages.is_dirty(Point::active(0, 1)));
    assert_eq!(s.dump_string(Tag::Screen, false), "");
}

// Port of `test "Screen clearRows protected"`.
#[test]
fn clear_rows_protected() {
    let mut s = init(80, 24, 1000);
    s.test_write_string("UNPROTECTED");
    s.cursor.protected = true;
    s.test_write_string("PROTECTED");
    s.cursor.protected = false;
    s.test_write_string("UNPROTECTED");
    s.test_write_string("\n");
    s.cursor.protected = true;
    s.test_write_string("PROTECTED");
    s.cursor.protected = false;
    s.test_write_string("UNPROTECTED");
    s.cursor.protected = true;
    s.test_write_string("PROTECTED");
    s.cursor.protected = false;

    s.clear_rows(Point::active(0, 0), None, true);

    assert_eq!(
        s.dump_string(Tag::Screen, false),
        "           PROTECTED\nPROTECTED           PROTECTED"
    );
}

// Port of `test "Screen eraseRows history"`.
#[test]
fn erase_rows_history() {
    let mut s = init(5, 5, 1000);
    s.test_write_string("1\n2\n3\n4\n5\n6");
    assert_eq!(s.dump_string(Tag::Active, false), "2\n3\n4\n5\n6");
    assert_eq!(s.dump_string(Tag::Screen, false), "1\n2\n3\n4\n5\n6");

    s.erase_history(None);

    assert_eq!(s.dump_string(Tag::Active, false), "2\n3\n4\n5\n6");
    assert_eq!(s.dump_string(Tag::Screen, false), "2\n3\n4\n5\n6");
}

// Port of `test "Screen eraseRows history with more lines"`.
#[test]
fn erase_rows_history_with_more_lines() {
    let mut s = init(5, 5, 1000);
    s.test_write_string("A\nB\nC\n1\n2\n3\n4\n5\n6");
    assert_eq!(s.dump_string(Tag::Active, false), "2\n3\n4\n5\n6");
    assert_eq!(
        s.dump_string(Tag::Screen, false),
        "A\nB\nC\n1\n2\n3\n4\n5\n6"
    );

    s.erase_history(None);

    assert_eq!(s.dump_string(Tag::Active, false), "2\n3\n4\n5\n6");
    assert_eq!(s.dump_string(Tag::Screen, false), "2\n3\n4\n5\n6");
}

// Port of `test "Screen eraseRows active partial"`.
#[test]
fn erase_rows_active_partial() {
    let mut s = init(5, 5, 0);
    s.test_write_string("1\n2\n3");
    assert_eq!(s.dump_string(Tag::Active, false), "1\n2\n3");

    s.erase_active(1);

    assert_eq!(s.dump_string(Tag::Active, false), "3");
    assert_eq!(s.dump_string(Tag::Screen, false), "3");
}

// ---- scrolling ----------------------------------------------------------

// Port of `test "Screen: scrolling with a single-row screen no scrollback"`.
#[test]
fn scrolling_single_row_no_scrollback() {
    let mut s = init(10, 1, 0);
    s.test_write_string("1ABCD");
    s.cursor_down_scroll();
    assert_eq!(s.dump_string(Tag::Viewport, false), "");
    assert!(s.pages.is_dirty(Point::active(0, 0)));
}

// Port of `test "Screen: scrolling with a single-row screen with scrollback"`.
#[test]
fn scrolling_single_row_with_scrollback() {
    let mut s = init(10, 1, 1);
    s.test_write_string("1ABCD");
    s.cursor_down_scroll();
    assert_eq!(s.dump_string(Tag::Viewport, false), "");
    assert!(s.pages.is_dirty(Point::active(0, 0)));
    assert!(s.pages.is_dirty(Point::screen(0, 0)));
    s.scroll(Scroll::DeltaRow(-1));
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD");
}

// Port of `test "Screen: scroll down from 0"`.
#[test]
fn scroll_down_from_0() {
    let mut s = init(10, 3, 0);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL");
    // Scrolling up does nothing but is allowed.
    s.scroll(Scroll::DeltaRow(-1));
    assert!(s.viewport_is_bottom());
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD\n2EFGH\n3IJKL");
}

// Port of `test "Screen: scrollback various cases"`.
#[test]
fn scrollback_various_cases() {
    let mut s = init(10, 3, 1);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL");
    s.cursor_down_scroll();
    assert_eq!(s.dump_string(Tag::Viewport, false), "2EFGH\n3IJKL");

    s.scroll(Scroll::Active);
    assert_eq!(s.dump_string(Tag::Viewport, false), "2EFGH\n3IJKL");

    s.scroll(Scroll::DeltaRow(-1));
    assert!(!s.viewport_is_bottom());
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD\n2EFGH\n3IJKL");

    s.scroll(Scroll::DeltaRow(-1));
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD\n2EFGH\n3IJKL");

    s.scroll(Scroll::Active);
    assert_eq!(s.dump_string(Tag::Viewport, false), "2EFGH\n3IJKL");

    s.scroll(Scroll::DeltaRow(1));
    assert_eq!(s.dump_string(Tag::Viewport, false), "2EFGH\n3IJKL");

    s.scroll(Scroll::Top);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD\n2EFGH\n3IJKL");

    s.clear_rows(Point::active(0, 0), None, false);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD");

    s.scroll(Scroll::Active);
    assert_eq!(s.dump_string(Tag::Viewport, false), "");
}

// Port of `test "Screen: scrollback with multi-row delta"`.
#[test]
fn scrollback_with_multi_row_delta() {
    let mut s = init(10, 3, 3);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL\n4ABCD\n5EFGH\n6IJKL");
    s.scroll(Scroll::Top);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD\n2EFGH\n3IJKL");
    s.scroll(Scroll::DeltaRow(5));
    assert!(s.viewport_is_bottom());
    assert_eq!(s.dump_string(Tag::Viewport, false), "4ABCD\n5EFGH\n6IJKL");
}

// Port of `test "Screen: scrollback empty"`.
#[test]
fn scrollback_empty() {
    let mut s = init(10, 3, 50);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL");
    s.scroll(Scroll::DeltaRow(1));
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD\n2EFGH\n3IJKL");
}

// Port of `test "Screen: scrollback doesn't move viewport if not at bottom"`.
#[test]
fn scrollback_doesnt_move_viewport_if_not_at_bottom() {
    let mut s = init(10, 3, 3);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL\n4ABCD\n5EFGH");
    s.scroll(Scroll::DeltaRow(-1));
    assert_eq!(s.dump_string(Tag::Viewport, false), "2EFGH\n3IJKL\n4ABCD");
    s.cursor_down_scroll();
    assert_eq!(s.dump_string(Tag::Viewport, false), "2EFGH\n3IJKL\n4ABCD");
    s.cursor_down_scroll();
    assert_eq!(s.dump_string(Tag::Viewport, false), "2EFGH\n3IJKL\n4ABCD");
}

// Port of `test "Screen: scrolling moves viewport"`.
#[test]
fn scrolling_moves_viewport() {
    let mut s = init(10, 3, 1);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL\n");
    s.test_write_string("1ABCD\n2EFGH\n3IJKL");
    s.scroll(Scroll::DeltaRow(-2));
    assert_eq!(s.dump_string(Tag::Viewport, false), "2EFGH\n3IJKL\n1ABCD");
    let tl = s.pages.get_top_left(Tag::Viewport);
    assert_eq!(
        s.pages.point_from_pin(Tag::Screen, tl).unwrap(),
        Point::screen(0, 1)
    );
}

// Port of `test "Screen: scrolling when viewport is pruned"`.
#[test]
fn scrolling_when_viewport_is_pruned() {
    let mut s = init(215, 3, 1);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL\n");
    s.test_write_string("1ABCD\n2EFGH\n3IJKL");
    s.scroll(Scroll::DeltaRow(-2));

    s.test_write_string("\n");
    for _ in 0..1000 {
        s.test_write_string("1ABCD\n2EFGH\n3IJKL\n");
    }
    s.test_write_string("1ABCD\n2EFGH\n3IJKL");

    let tl = s.pages.get_top_left(Tag::Viewport);
    assert_eq!(
        s.pages.point_from_pin(Tag::Screen, tl).unwrap(),
        Point::screen(0, 0)
    );
}

// Port of `test "Screen: scroll and clear full screen"`.
#[test]
fn scroll_and_clear_full_screen() {
    let mut s = init(10, 3, 5);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL");
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD\n2EFGH\n3IJKL");
    s.scroll_clear();
    assert_eq!(s.dump_string(Tag::Viewport, false), "");
    assert_eq!(s.dump_string(Tag::Screen, false), "1ABCD\n2EFGH\n3IJKL");
}

// Port of `test "Screen: scroll and clear partial screen"`.
#[test]
fn scroll_and_clear_partial_screen() {
    let mut s = init(10, 3, 5);
    s.test_write_string("1ABCD\n2EFGH");
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD\n2EFGH");
    s.scroll_clear();
    assert_eq!(s.dump_string(Tag::Viewport, false), "");
    assert_eq!(s.dump_string(Tag::Screen, false), "1ABCD\n2EFGH");
}

// Port of `test "Screen: scroll and clear empty screen"`.
#[test]
fn scroll_and_clear_empty_screen() {
    let mut s = init(10, 3, 5);
    s.scroll_clear();
    assert_eq!(s.dump_string(Tag::Viewport, false), "");
    assert_eq!(s.dump_string(Tag::Screen, false), "");
}

// Port of `test "Screen: scroll and clear ignore blank lines"`.
#[test]
fn scroll_and_clear_ignore_blank_lines() {
    let mut s = init(10, 3, 10);
    s.test_write_string("1ABCD\n2EFGH");
    s.scroll_clear();
    assert_eq!(s.dump_string(Tag::Viewport, false), "");

    s.cursor_absolute(0, 0);
    s.test_write_string("3ABCD\n");
    assert_eq!(s.dump_string(Tag::Active, false), "3ABCD");

    s.scroll_clear();
    assert_eq!(s.dump_string(Tag::Viewport, false), "");

    s.cursor_absolute(0, 0);
    s.test_write_string("X");
    assert_eq!(s.dump_string(Tag::Screen, false), "1ABCD\n2EFGH\n3ABCD\nX");
}

// ---- clone --------------------------------------------------------------

// Port of `test "Screen: clone"`.
#[test]
fn clone() {
    let mut s = init(10, 3, 10);
    s.test_write_string("1ABCD\n2EFGH");
    assert_eq!(s.dump_string(Tag::Active, false), "1ABCD\n2EFGH");
    assert_eq!(s.cursor.x, 5);
    assert_eq!(s.cursor.y, 1);

    let mut s2 = s.clone(Point::active(0, 0), None);
    assert_eq!(s2.dump_string(Tag::Active, false), "1ABCD\n2EFGH");
    assert_eq!(s2.cursor.x, 5);
    assert_eq!(s2.cursor.y, 1);

    // Write to s, should not appear in s2.
    s.test_write_string("\n34567");
    assert_eq!(s.dump_string(Tag::Active, false), "1ABCD\n2EFGH\n34567");
    assert_eq!(s2.dump_string(Tag::Active, false), "1ABCD\n2EFGH");
    assert_eq!(s2.cursor.x, 5);
    assert_eq!(s2.cursor.y, 1);
    let _ = &mut s2;
}

// Port of `test "Screen: clone partial"`.
#[test]
fn clone_partial() {
    let mut s = init(10, 3, 10);
    s.test_write_string("1ABCD\n2EFGH");
    assert_eq!(s.dump_string(Tag::Active, false), "1ABCD\n2EFGH");
    assert_eq!(s.cursor.x, 5);
    assert_eq!(s.cursor.y, 1);

    let s2 = s.clone(Point::active(0, 1), None);
    assert_eq!(s2.dump_string(Tag::Active, false), "2EFGH");
    // Cursor is shifted since we cloned partial.
    assert_eq!(s2.cursor.x, 5);
    assert_eq!(s2.cursor.y, 0);
}

// Port of `test "Screen: clone partial cursor out of bounds"`.
#[test]
fn clone_partial_cursor_out_of_bounds() {
    let mut s = init(10, 3, 10);
    s.test_write_string("1ABCD\n2EFGH");
    assert_eq!(s.dump_string(Tag::Active, false), "1ABCD\n2EFGH");
    assert_eq!(s.cursor.x, 5);
    assert_eq!(s.cursor.y, 1);

    let s2 = s.clone(Point::active(0, 0), Some(Point::active(0, 0)));
    assert_eq!(s2.dump_string(Tag::Active, false), "1ABCD");
    // Cursor is shifted since we cloned partial (out of bounds -> top-left).
    assert_eq!(s2.cursor.x, 0);
    assert_eq!(s2.cursor.y, 0);
}

// Port of `test "Screen: clone basic"`.
#[test]
fn clone_basic() {
    let mut s = init(10, 3, 0);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL");

    let s2 = s.clone(Point::active(0, 1), Some(Point::active(0, 1)));
    assert_eq!(s2.dump_string(Tag::Active, false), "2EFGH");

    let s3 = s.clone(Point::active(0, 1), Some(Point::active(0, 2)));
    assert_eq!(s3.dump_string(Tag::Active, false), "2EFGH\n3IJKL");
}

// Port of `test "Screen: clone empty viewport"`.
#[test]
fn clone_empty_viewport() {
    let s = init(10, 3, 0);
    let s2 = s.clone(Point::viewport(0, 0), Some(Point::viewport(0, 0)));
    assert_eq!(s2.dump_string(Tag::Viewport, false), "");
}

// Port of `test "Screen: clone one line viewport"`.
#[test]
fn clone_one_line_viewport() {
    let mut s = init(10, 3, 0);
    s.test_write_string("1ABC");
    let s2 = s.clone(Point::viewport(0, 0), Some(Point::viewport(0, 0)));
    assert_eq!(s2.dump_string(Tag::Viewport, false), "1ABC");
}

// Port of `test "Screen: clone empty active"`.
#[test]
fn clone_empty_active() {
    let s = init(10, 3, 0);
    let s2 = s.clone(Point::active(0, 0), Some(Point::active(0, 0)));
    assert_eq!(s2.dump_string(Tag::Active, false), "");
}

// Port of `test "Screen: clone one line active with extra space"`.
#[test]
fn clone_one_line_active_with_extra_space() {
    let mut s = init(10, 3, 0);
    s.test_write_string("1ABC");
    let s2 = s.clone(Point::active(0, 0), None);
    assert_eq!(s2.dump_string(Tag::Active, false), "1ABC");
}

// ---- clear history / clear above cursor ---------------------------------

// Port of `test "Screen: clear history with no history"`.
#[test]
fn clear_history_with_no_history() {
    let mut s = init(10, 3, 3);
    s.test_write_string("4ABCD\n5EFGH\n6IJKL");
    assert!(s.viewport_is_bottom());
    s.erase_history(None);
    assert!(s.viewport_is_bottom());
    assert_eq!(s.dump_string(Tag::Viewport, false), "4ABCD\n5EFGH\n6IJKL");
    assert_eq!(s.dump_string(Tag::Screen, false), "4ABCD\n5EFGH\n6IJKL");
}

// Port of `test "Screen: clear history"`.
#[test]
fn clear_history() {
    let mut s = init(10, 3, 3);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL\n4ABCD\n5EFGH\n6IJKL");
    assert!(s.viewport_is_bottom());

    s.scroll(Scroll::Top);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD\n2EFGH\n3IJKL");

    s.erase_history(None);
    assert!(s.viewport_is_bottom());
    assert_eq!(s.dump_string(Tag::Viewport, false), "4ABCD\n5EFGH\n6IJKL");
    assert_eq!(s.dump_string(Tag::Screen, false), "4ABCD\n5EFGH\n6IJKL");
}

// Port of `test "Screen: clear above cursor"`.
#[test]
fn clear_above_cursor() {
    let mut s = init(10, 10, 3);
    s.test_write_string("4ABCD\n5EFGH\n6IJKL");
    let y = s.cursor.y;
    s.clear_rows(
        Point::active(0, 0),
        Some(Point::active(0, (y - 1) as u32)),
        false,
    );
    assert_eq!(s.dump_string(Tag::Viewport, false), "\n\n6IJKL");
    assert_eq!(s.dump_string(Tag::Screen, false), "\n\n6IJKL");
    assert_eq!(s.cursor.x, 5);
    assert_eq!(s.cursor.y, 2);
}

// Port of `test "Screen: clear above cursor with history"`.
#[test]
fn clear_above_cursor_with_history() {
    let mut s = init(10, 3, 3);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL\n");
    s.test_write_string("4ABCD\n5EFGH\n6IJKL");
    let y = s.cursor.y;
    s.clear_rows(
        Point::active(0, 0),
        Some(Point::active(0, (y - 1) as u32)),
        false,
    );
    assert_eq!(s.dump_string(Tag::Viewport, false), "\n\n6IJKL");
    assert_eq!(
        s.dump_string(Tag::Screen, false),
        "1ABCD\n2EFGH\n3IJKL\n\n\n6IJKL"
    );
    assert_eq!(s.cursor.x, 5);
    assert_eq!(s.cursor.y, 2);
}

// ---- resize (no reflow) -------------------------------------------------

// Port of `test "Screen: resize (no reflow) more rows"`.
#[test]
fn resize_no_reflow_more_rows() {
    let mut s = init(10, 3, 0);
    let str = "1ABCD\n2EFGH\n3IJKL";
    s.test_write_string(str);
    resize(&mut s, 10, 10, false);
    assert_eq!(s.dump_string(Tag::Viewport, false), str);
}

// Port of `test "Screen: resize (no reflow) less rows"`.
#[test]
fn resize_no_reflow_less_rows() {
    let mut s = init(10, 3, 0);
    let str = "1ABCD\n2EFGH\n3IJKL";
    s.test_write_string(str);
    assert_eq!(s.cursor.x, 5);
    assert_eq!(s.cursor.y, 2);
    resize(&mut s, 10, 2, false);
    assert_eq!(s.cursor.x, 5);
    assert_eq!(s.cursor.y, 1);
    assert_eq!(s.dump_string(Tag::Viewport, false), "2EFGH\n3IJKL");
}

// Port of `test "Screen: resize (no reflow) less rows trims blank lines"`.
#[test]
fn resize_no_reflow_less_rows_trims_blank_lines() {
    let mut s = init(10, 3, 0);
    s.test_write_string("1ABCD");
    for y in 1..s.pages.rows() {
        write_bg_rgb(&mut s, 0, y);
    }
    let (cx, cy) = (s.cursor.x, s.cursor.y);
    resize(&mut s, 6, 2, false);
    assert_eq!(s.cursor.x, cx);
    assert_eq!(s.cursor.y, cy);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD");
}

// Port of `test "Screen: resize (no reflow) more rows trims blank lines"`.
#[test]
fn resize_no_reflow_more_rows_trims_blank_lines() {
    let mut s = init(10, 3, 0);
    s.test_write_string("1ABCD");
    for y in 1..s.pages.rows() {
        write_bg_rgb(&mut s, 0, y);
    }
    let (cx, cy) = (s.cursor.x, s.cursor.y);
    resize(&mut s, 10, 7, false);
    assert_eq!(s.cursor.x, cx);
    assert_eq!(s.cursor.y, cy);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD");
}

// Port of `test "Screen: resize (no reflow) more cols"`.
#[test]
fn resize_no_reflow_more_cols() {
    let mut s = init(10, 3, 0);
    let str = "1ABCD\n2EFGH\n3IJKL";
    s.test_write_string(str);
    resize(&mut s, 20, 3, false);
    assert_eq!(s.dump_string(Tag::Viewport, false), str);
}

// Port of `test "Screen: resize (no reflow) less cols"`.
#[test]
fn resize_no_reflow_less_cols() {
    let mut s = init(10, 3, 0);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL");
    resize(&mut s, 4, 3, false);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABC\n2EFG\n3IJK");
}

// Port of `test "Screen: resize (no reflow) more rows with scrollback cursor end"`.
#[test]
fn resize_no_reflow_more_rows_with_scrollback_cursor_end() {
    let mut s = init(7, 3, 2);
    let str = "1ABCD\n2EFGH\n3IJKL\n4ABCD\n5EFGH";
    s.test_write_string(str);
    resize(&mut s, 7, 10, false);
    assert_eq!(s.dump_string(Tag::Viewport, false), str);
}

// Port of `test "Screen: resize (no reflow) less rows with scrollback"`.
#[test]
fn resize_no_reflow_less_rows_with_scrollback() {
    let mut s = init(7, 3, 2);
    let str = "1ABCD\n2EFGH\n3IJKL\n4ABCD\n5EFGH";
    s.test_write_string(str);
    resize(&mut s, 7, 2, false);
    assert_eq!(s.dump_string(Tag::Viewport, false), "4ABCD\n5EFGH");
}

// Port of `test "Screen: resize (no reflow) less rows with empty trailing"`.
#[test]
fn resize_no_reflow_less_rows_with_empty_trailing() {
    let mut s = init(5, 3, 5);
    s.test_write_string("1\n2\n3\n4\n5\n6\n7\n8");
    s.scroll_clear();
    s.cursor_absolute(0, 0);
    s.test_write_string("A\nB");
    let (cx, cy) = (s.cursor.x, s.cursor.y);
    resize(&mut s, 5, 2, false);
    assert_eq!(s.cursor.x, cx);
    assert_eq!(s.cursor.y, cy);
    assert_eq!(s.dump_string(Tag::Viewport, false), "A\nB");
}

// Port of `test "Screen: resize (no reflow) more rows with soft wrapping"`.
#[test]
fn resize_no_reflow_more_rows_with_soft_wrapping() {
    let mut s = init(2, 3, 3);
    s.test_write_string("1A2B\n3C4E\n5F6G");
    for y in 0..6u32 {
        assert_eq!(screen_row_wrap(&s, y), y % 2 == 0);
    }
    resize(&mut s, 2, 10, false);
    assert_eq!(
        s.dump_string(Tag::Viewport, false),
        "1A\n2B\n3C\n4E\n5F\n6G"
    );
    for y in 0..6u32 {
        assert_eq!(screen_row_wrap(&s, y), y % 2 == 0);
    }
}

// ---- resize (reflow) ----------------------------------------------------

// Port of `test "Screen: resize more rows no scrollback"`.
#[test]
fn resize_more_rows_no_scrollback() {
    let mut s = init(5, 3, 0);
    let str = "1ABCD\n2EFGH\n3IJKL";
    s.test_write_string(str);
    let (cx, cy) = (s.cursor.x, s.cursor.y);
    resize(&mut s, 5, 10, true);
    assert_eq!(s.cursor.x, cx);
    assert_eq!(s.cursor.y, cy);
    assert_eq!(s.dump_string(Tag::Viewport, false), str);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
}

// Port of `test "Screen: resize more rows with empty scrollback"`.
#[test]
fn resize_more_rows_with_empty_scrollback() {
    let mut s = init(5, 3, 10);
    let str = "1ABCD\n2EFGH\n3IJKL";
    s.test_write_string(str);
    let (cx, cy) = (s.cursor.x, s.cursor.y);
    resize(&mut s, 5, 10, true);
    assert_eq!(s.cursor.x, cx);
    assert_eq!(s.cursor.y, cy);
    assert_eq!(s.dump_string(Tag::Viewport, false), str);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
}

// Port of `test "Screen: resize more rows with populated scrollback"`.
#[test]
fn resize_more_rows_with_populated_scrollback() {
    let mut s = init(5, 3, 5);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL\n4ABCD\n5EFGH");
    assert_eq!(s.dump_string(Tag::Viewport, false), "3IJKL\n4ABCD\n5EFGH");
    s.cursor_absolute(0, 1);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), '4' as u32);
    resize(&mut s, 5, 10, true);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), '4' as u32);
    assert_eq!(s.dump_string(Tag::Viewport, false), "3IJKL\n4ABCD\n5EFGH");
}

// Port of `test "Screen: resize more cols no reflow"`.
#[test]
fn resize_more_cols_no_reflow() {
    let mut s = init(5, 3, 0);
    let str = "1ABCD\n2EFGH\n3IJKL";
    s.test_write_string(str);
    let (cx, cy) = (s.cursor.x, s.cursor.y);
    resize(&mut s, 10, 3, true);
    assert_eq!(s.cursor.x, cx);
    assert_eq!(s.cursor.y, cy);
    assert_eq!(s.dump_string(Tag::Viewport, false), str);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
}

// Port of `test "Screen: resize more cols perfect split"`.
#[test]
fn resize_more_cols_perfect_split() {
    let mut s = init(5, 3, 0);
    s.test_write_string("1ABCD2EFGH3IJKL");
    resize(&mut s, 10, 3, true);
    assert_eq!(s.dump_string(Tag::Screen, false), "1ABCD2EFGH\n3IJKL");
}

// Port of `test "Screen: resize more cols no reflow preserves semantic prompt"`.
#[test]
fn resize_more_cols_no_reflow_preserves_semantic_prompt() {
    let mut s = init(5, 3, 0);
    s.cursor_set_semantic_content(SemanticContentSet::Output);
    s.test_write_string("1ABCD\n");
    s.cursor_set_semantic_content(SemanticContentSet::Prompt(PromptKind::Initial));
    s.test_write_string("2EFGH");
    s.cursor_set_semantic_content(SemanticContentSet::Output);
    s.test_write_string("\n3IJKL");

    resize(&mut s, 10, 3, false);

    let expected = "1ABCD\n2EFGH\n3IJKL";
    assert_eq!(s.dump_string(Tag::Viewport, false), expected);
    assert_eq!(s.dump_string(Tag::Screen, false), expected);

    use crate::page::SemanticPrompt as SP;
    assert_eq!(active_row_semantic_prompt(&s, 0), SP::None);
    assert_eq!(active_row_semantic_prompt(&s, 1), SP::Prompt);
    assert_eq!(active_row_semantic_prompt(&s, 2), SP::None);
}

// Port of `test "Screen: resize (no reflow) more cols with scrollback scrolled up"`.
#[test]
fn resize_no_reflow_more_cols_with_scrollback_scrolled_up() {
    let mut s = init(5, 3, 5);
    let str = "1\n2\n3\n4\n5\n6\n7\n8";
    s.test_write_string(str);
    assert_eq!(s.cursor.x, 1);
    assert_eq!(s.cursor.y, 2);
    s.scroll(Scroll::DeltaRow(-4));
    assert_eq!(s.dump_string(Tag::Viewport, false), "2\n3\n4");
    resize(&mut s, 8, 3, true);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
    assert_eq!(s.cursor.x, 1);
    assert_eq!(s.cursor.y, 2);
}

// Port of `test "Screen: resize (no reflow) less cols with scrollback scrolled up"`.
#[test]
fn resize_no_reflow_less_cols_with_scrollback_scrolled_up() {
    let mut s = init(5, 3, 5);
    let str = "1\n2\n3\n4\n5\n6\n7\n8";
    s.test_write_string(str);
    assert_eq!(s.cursor.x, 1);
    assert_eq!(s.cursor.y, 2);
    s.scroll(Scroll::DeltaRow(-4));
    assert_eq!(s.dump_string(Tag::Viewport, false), "2\n3\n4");
    resize(&mut s, 4, 3, true);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
    assert_eq!(s.dump_string(Tag::Active, false), "6\n7\n8");
    assert_eq!(s.cursor.x, 1);
    assert_eq!(s.cursor.y, 2);
}

// Port of `test "Screen: resize more cols with reflow that fits full width"`.
#[test]
fn resize_more_cols_with_reflow_that_fits_full_width() {
    let mut s = init(5, 3, 0);
    let str = "1ABCD2EFGH\n3IJKL";
    s.test_write_string(str);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD\n2EFGH\n3IJKL");
    s.cursor_absolute(0, 1);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), '2' as u32);
    resize(&mut s, 10, 3, true);
    assert_eq!(s.dump_string(Tag::Viewport, false), str);
    assert_eq!(s.cursor.x, 5);
    assert_eq!(s.cursor.y, 0);
}

// Port of `test "Screen: resize more cols with reflow that ends in newline"`.
#[test]
fn resize_more_cols_with_reflow_that_ends_in_newline() {
    let mut s = init(6, 3, 0);
    let str = "1ABCD2EFGH\n3IJKL";
    s.test_write_string(str);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD2\nEFGH\n3IJKL");
    s.cursor_absolute(0, 2);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), '3' as u32);
    resize(&mut s, 10, 3, true);
    assert_eq!(s.dump_string(Tag::Viewport, false), str);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), '3' as u32);
}

// Port of `test "Screen: resize more cols with reflow that forces more wrapping"`.
#[test]
fn resize_more_cols_with_reflow_that_forces_more_wrapping() {
    let mut s = init(5, 3, 0);
    s.test_write_string("1ABCD2EFGH\n3IJKL");
    s.cursor_absolute(0, 1);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), '2' as u32);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD\n2EFGH\n3IJKL");
    resize(&mut s, 7, 3, true);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD2E\nFGH\n3IJKL");
    assert_eq!(s.cursor.x, 5);
    assert_eq!(s.cursor.y, 0);
}

// Port of `test "Screen: resize more cols with reflow that unwraps multiple times"`.
#[test]
fn resize_more_cols_with_reflow_that_unwraps_multiple_times() {
    let mut s = init(5, 3, 0);
    s.test_write_string("1ABCD2EFGH3IJKL");
    s.cursor_absolute(0, 2);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), '3' as u32);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD\n2EFGH\n3IJKL");
    resize(&mut s, 15, 3, true);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1ABCD2EFGH3IJKL");
    assert_eq!(s.cursor.x, 10);
    assert_eq!(s.cursor.y, 0);
}

// Port of `test "Screen: resize more cols with populated scrollback"`.
#[test]
fn resize_more_cols_with_populated_scrollback() {
    let mut s = init(5, 3, 5);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL\n4ABCD5EFGH");
    assert_eq!(s.dump_string(Tag::Viewport, false), "3IJKL\n4ABCD\n5EFGH");
    s.cursor_absolute(0, 2);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), '5' as u32);
    resize(&mut s, 10, 3, true);
    assert_eq!(
        s.dump_string(Tag::Viewport, false),
        "2EFGH\n3IJKL\n4ABCD5EFGH"
    );
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), '5' as u32);
}

// Port of `test "Screen: resize more cols with reflow"`.
#[test]
fn resize_more_cols_with_reflow() {
    let mut s = init(2, 3, 5);
    s.test_write_string("1ABC\n2DEF\n3ABC\n4DEF");
    s.cursor_absolute(0, 2);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), 'E' as u32);
    assert_eq!(s.dump_string(Tag::Viewport, false), "BC\n4D\nEF");
    resize(&mut s, 7, 3, true);
    assert_eq!(s.dump_string(Tag::Screen, false), "1ABC\n2DEF\n3ABC\n4DEF");
    assert_eq!(s.cursor.x, 2);
    assert_eq!(s.cursor.y, 2);
}

// Port of `test "Screen: resize more rows and cols with wrapping"`.
#[test]
fn resize_more_rows_and_cols_with_wrapping() {
    let mut s = init(2, 4, 0);
    let str = "1A2B\n3C4D";
    s.test_write_string(str);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1A\n2B\n3C\n4D");
    resize(&mut s, 5, 10, true);
    assert_eq!(s.cursor.x, 3);
    assert_eq!(s.cursor.y, 1);
    assert_eq!(s.dump_string(Tag::Viewport, false), str);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
}

// Port of `test "Screen: resize less rows no scrollback"`.
#[test]
fn resize_less_rows_no_scrollback() {
    let mut s = init(5, 3, 0);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL");
    s.cursor_absolute(0, 0);
    let (cx, cy) = (s.cursor.x, s.cursor.y);
    resize(&mut s, 5, 1, true);
    assert_eq!(s.cursor.x, cx);
    assert_eq!(s.cursor.y, cy);
    assert_eq!(s.dump_string(Tag::Viewport, false), "3IJKL");
    assert_eq!(s.dump_string(Tag::Screen, false), "3IJKL");
}

// Port of `test "Screen: resize less rows moving cursor"`.
#[test]
fn resize_less_rows_moving_cursor() {
    let mut s = init(5, 3, 0);
    s.test_write_string("1ABCD\n2EFGH\n3IJKL");
    s.cursor_absolute(1, 2);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), 'I' as u32);
    resize(&mut s, 5, 1, true);
    assert_eq!(s.dump_string(Tag::Viewport, false), "3IJKL");
    assert_eq!(s.dump_string(Tag::Screen, false), "3IJKL");
    assert_eq!(s.cursor.x, 1);
    assert_eq!(s.cursor.y, 0);
}

// Port of `test "Screen: resize less rows with empty scrollback"`.
#[test]
fn resize_less_rows_with_empty_scrollback() {
    let mut s = init(5, 3, 10);
    let str = "1ABCD\n2EFGH\n3IJKL";
    s.test_write_string(str);
    resize(&mut s, 5, 1, true);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
    assert_eq!(s.dump_string(Tag::Viewport, false), "3IJKL");
}

// Port of `test "Screen: resize less rows with populated scrollback"`.
#[test]
fn resize_less_rows_with_populated_scrollback() {
    let mut s = init(5, 3, 5);
    let str = "1ABCD\n2EFGH\n3IJKL\n4ABCD\n5EFGH";
    s.test_write_string(str);
    assert_eq!(s.dump_string(Tag::Viewport, false), "3IJKL\n4ABCD\n5EFGH");
    resize(&mut s, 5, 1, true);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
    assert_eq!(s.dump_string(Tag::Viewport, false), "5EFGH");
}

// Port of `test "Screen: resize less rows with full scrollback"`.
#[test]
fn resize_less_rows_with_full_scrollback() {
    let mut s = init(5, 3, 3);
    let str = "00000\n1ABCD\n2EFGH\n3IJKL\n4ABCD\n5EFGH";
    s.test_write_string(str);
    assert_eq!(s.dump_string(Tag::Viewport, false), "3IJKL\n4ABCD\n5EFGH");
    assert_eq!(s.cursor.x, 4);
    assert_eq!(s.cursor.y, 2);
    resize(&mut s, 5, 2, true);
    assert_eq!(s.cursor.x, 4);
    assert_eq!(s.cursor.y, 1);
    assert_eq!(
        s.dump_string(Tag::Screen, false),
        "00000\n1ABCD\n2EFGH\n3IJKL\n4ABCD\n5EFGH"
    );
    assert_eq!(s.dump_string(Tag::Viewport, false), "4ABCD\n5EFGH");
}

// Port of `test "Screen: resize less cols no reflow"`.
#[test]
fn resize_less_cols_no_reflow() {
    let mut s = init(5, 3, 0);
    let str = "1AB\n2EF\n3IJ";
    s.test_write_string(str);
    s.cursor_absolute(0, 0);
    let (cx, cy) = (s.cursor.x, s.cursor.y);
    resize(&mut s, 3, 3, true);
    assert_eq!(s.cursor.x, cx);
    assert_eq!(s.cursor.y, cy);
    assert_eq!(s.dump_string(Tag::Viewport, false), str);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
}

// Port of `test "Screen: resize less cols with reflow but row space"`.
#[test]
fn resize_less_cols_with_reflow_but_row_space() {
    let mut s = init(5, 3, 1);
    s.test_write_string("1ABCD");
    s.cursor_absolute(4, 0);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), 'D' as u32);
    resize(&mut s, 3, 3, true);
    assert_eq!(s.dump_string(Tag::Viewport, false), "1AB\nCD");
    assert_eq!(s.dump_string(Tag::Screen, false), "1AB\nCD");
    assert_eq!(s.cursor.x, 1);
    assert_eq!(s.cursor.y, 1);
}

// Port of `test "Screen: resize less cols with reflow with trimmed rows"`.
#[test]
fn resize_less_cols_with_reflow_with_trimmed_rows() {
    let mut s = init(5, 3, 0);
    s.test_write_string("3IJKL\n4ABCD\n5EFGH");
    resize(&mut s, 3, 3, true);
    assert_eq!(s.dump_string(Tag::Viewport, false), "CD\n5EF\nGH");
    assert_eq!(s.dump_string(Tag::Screen, false), "CD\n5EF\nGH");
}

// Port of `test "Screen: resize less cols with reflow with trimmed rows and scrollback"`.
#[test]
fn resize_less_cols_with_reflow_with_trimmed_rows_and_scrollback() {
    let mut s = init(5, 3, 1);
    s.test_write_string("3IJKL\n4ABCD\n5EFGH");
    resize(&mut s, 3, 3, true);
    assert_eq!(s.dump_string(Tag::Viewport, false), "CD\n5EF\nGH");
    assert_eq!(
        s.dump_string(Tag::Screen, false),
        "3IJ\nKL\n4AB\nCD\n5EF\nGH"
    );
}

// Port of `test "Screen: resize less cols with reflow previously wrapped"`.
#[test]
fn resize_less_cols_with_reflow_previously_wrapped() {
    let mut s = init(5, 3, 0);
    s.test_write_string("3IJKL4ABCD5EFGH");
    assert_eq!(s.dump_string(Tag::Screen, false), "3IJKL\n4ABCD\n5EFGH");
    resize(&mut s, 3, 3, true);
    assert_eq!(s.dump_string(Tag::Screen, false), "ABC\nD5E\nFGH");
}

// Port of `test "Screen: resize less cols with reflow and scrollback"`.
#[test]
fn resize_less_cols_with_reflow_and_scrollback() {
    let mut s = init(5, 3, 5);
    s.test_write_string("1A\n2B\n3C\n4D\n5E");
    s.cursor_absolute(1, s.pages.rows() - 1);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), 'E' as u32);
    resize(&mut s, 3, 3, true);
    assert_eq!(s.dump_string(Tag::Viewport, false), "3C\n4D\n5E");
    assert_eq!(s.cursor.x, 1);
    assert_eq!(s.cursor.y, 2);
}

// Port of `test "Screen: resize less cols with reflow previously wrapped and scrollback"`.
#[test]
fn resize_less_cols_with_reflow_previously_wrapped_and_scrollback() {
    let mut s = init(5, 3, 2);
    s.test_write_string("1ABCD2EFGH3IJKL4ABCD5EFGH");
    assert_eq!(s.dump_string(Tag::Viewport, false), "3IJKL\n4ABCD\n5EFGH");
    s.cursor_absolute(s.pages.cols() - 1, s.pages.rows() - 1);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), 'H' as u32);
    resize(&mut s, 3, 3, true);
    assert_eq!(s.dump_string(Tag::Viewport, false), "CD5\nEFG\nH");
    assert_eq!(
        s.dump_string(Tag::Screen, false),
        "1AB\nCD2\nEFG\nH3I\nJKL\n4AB\nCD5\nEFG\nH"
    );
    assert_eq!(s.cursor.x, 0);
    assert_eq!(s.cursor.y, 2);
    assert_eq!(active_codepoint(&s, s.cursor.x, s.cursor.y), 'H' as u32);
}

// Port of `test "Screen: resize less cols with scrollback keeps cursor row"`.
#[test]
fn resize_less_cols_with_scrollback_keeps_cursor_row() {
    let mut s = init(5, 3, 5);
    s.test_write_string("1A\n2B\n3C\n4D\n5E");
    s.scroll_clear();
    s.cursor_absolute(0, 0);
    resize(&mut s, 3, 3, true);
    assert_eq!(s.dump_string(Tag::Viewport, false), "");
    assert_eq!(s.cursor.x, 0);
    assert_eq!(s.cursor.y, 0);
}

// Port of `test "Screen: resize more rows, less cols with reflow with scrollback"`.
#[test]
fn resize_more_rows_less_cols_with_reflow_with_scrollback() {
    let mut s = init(5, 3, 3);
    s.test_write_string("1ABCD\n2EFGH3IJKL\n4MNOP");
    assert_eq!(
        s.dump_string(Tag::Screen, false),
        "1ABCD\n2EFGH\n3IJKL\n4MNOP"
    );
    assert_eq!(s.dump_string(Tag::Viewport, false), "2EFGH\n3IJKL\n4MNOP");
    resize(&mut s, 2, 10, true);
    assert_eq!(
        s.dump_string(Tag::Viewport, false),
        "BC\nD\n2E\nFG\nH3\nIJ\nKL\n4M\nNO\nP"
    );
    assert_eq!(
        s.dump_string(Tag::Screen, false),
        "1A\nBC\nD\n2E\nFG\nH3\nIJ\nKL\n4M\nNO\nP"
    );
}

// Port of `test "Screen: resize more rows then shrink again"`.
#[test]
fn resize_more_rows_then_shrink_again() {
    let mut s = init(5, 3, 10);
    let str = "1ABC";
    s.test_write_string(str);
    resize(&mut s, 5, 10, true);
    assert_eq!(s.dump_string(Tag::Viewport, false), str);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
    resize(&mut s, 5, 3, true);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
    assert_eq!(s.dump_string(Tag::Viewport, false), str);
    resize(&mut s, 5, 10, true);
    assert_eq!(s.dump_string(Tag::Viewport, false), str);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
}

// Port of `test "Screen: resize less cols to eliminate wide char"`.
#[test]
fn resize_less_cols_to_eliminate_wide_char() {
    use crate::page::Wide;
    let mut s = init(2, 1, 0);
    let str = "😀";
    s.test_write_string(str);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
    assert_eq!(screen_wide(&s, 0, 0), Wide::Wide);
    assert_eq!(screen_codepoint(&s, 0, 0), '😀' as u32);
    resize(&mut s, 1, 1, true);
    assert_eq!(s.dump_string(Tag::Screen, false), "");
    assert_eq!(screen_codepoint(&s, 0, 0), 0);
    assert_eq!(screen_wide(&s, 0, 0), Wide::Narrow);
}

// Port of `test "Screen: resize less cols to wrap wide char"`.
#[test]
fn resize_less_cols_to_wrap_wide_char() {
    use crate::page::Wide;
    let mut s = init(3, 3, 0);
    let str = "x😀";
    s.test_write_string(str);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
    assert_eq!(screen_wide(&s, 1, 0), Wide::Wide);
    assert_eq!(screen_codepoint(&s, 1, 0), '😀' as u32);
    assert_eq!(screen_wide(&s, 2, 0), Wide::SpacerTail);
    resize(&mut s, 2, 3, true);
    assert_eq!(s.dump_string(Tag::Screen, false), "x\n😀");
    assert_eq!(screen_wide(&s, 1, 0), Wide::SpacerHead);
    assert!(screen_row_wrap(&s, 0));
}

// Port of `test "Screen: resize less cols to eliminate wide char with row space"`.
#[test]
fn resize_less_cols_to_eliminate_wide_char_with_row_space() {
    use crate::page::Wide;
    let mut s = init(2, 2, 0);
    let str = "😀";
    s.test_write_string(str);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
    assert_eq!(screen_wide(&s, 0, 0), Wide::Wide);
    assert_eq!(screen_codepoint(&s, 0, 0), '😀' as u32);
    assert_eq!(screen_wide(&s, 1, 0), Wide::SpacerTail);
    resize(&mut s, 1, 2, true);
    assert_eq!(s.dump_string(Tag::Screen, false), "");
}

// Port of `test "Screen: resize less cols reflows cursor after wrapped text"`.
#[test]
fn resize_less_cols_reflows_cursor_after_wrapped_text() {
    let mut s = init(50, 7, 0);
    for _ in 0..30 {
        s.test_write_string("a");
    }
    assert_eq!(s.cursor.y, 0);
    assert_eq!(s.cursor.x, 30);
    resize(&mut s, 25, 7, true);
    assert_eq!(s.cursor.y, 1);
    assert_eq!(s.cursor.x, 5);
}

// Port of `test "Screen: resize less cols reflows cursor after empty cells"`.
#[test]
fn resize_less_cols_reflows_cursor_after_empty_cells() {
    let mut s = init(10, 3, 0);
    s.test_write_string("abc");
    s.cursor_right(6);
    assert_eq!(s.cursor.y, 0);
    assert_eq!(s.cursor.x, 9);
    resize(&mut s, 5, 3, true);
    assert_eq!(s.cursor.y, 1);
    assert_eq!(s.cursor.x, 4);
}

// Port of `test "Screen: resize more cols with wide spacer head"`.
#[test]
fn resize_more_cols_with_wide_spacer_head() {
    use crate::page::Wide;
    let mut s = init(3, 2, 0);
    let str = "  😀";
    s.test_write_string(str);
    assert_eq!(s.dump_string(Tag::Screen, false), "  \n😀");
    assert_eq!(screen_wide(&s, 2, 0), Wide::SpacerHead);
    assert_eq!(screen_wide(&s, 0, 1), Wide::Wide);
    assert_eq!(screen_wide(&s, 1, 1), Wide::SpacerTail);
    resize(&mut s, 4, 2, true);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
    assert_eq!(screen_wide(&s, 2, 0), Wide::Wide);
    assert_eq!(screen_codepoint(&s, 2, 0), '😀' as u32);
    assert_eq!(screen_wide(&s, 3, 0), Wide::SpacerTail);
}

// Port of `test "Screen: resize more cols with wide spacer head multiple lines"`.
#[test]
fn resize_more_cols_with_wide_spacer_head_multiple_lines() {
    use crate::page::Wide;
    let mut s = init(3, 3, 0);
    let str = "xxxyy😀";
    s.test_write_string(str);
    assert_eq!(s.dump_string(Tag::Screen, false), "xxx\nyy\n😀");
    assert_eq!(screen_wide(&s, 2, 1), Wide::SpacerHead);
    assert_eq!(screen_wide(&s, 0, 2), Wide::Wide);
    assert_eq!(screen_wide(&s, 1, 2), Wide::SpacerTail);
    resize(&mut s, 8, 2, true);
    assert_eq!(s.dump_string(Tag::Screen, false), str);
    assert_eq!(screen_wide(&s, 5, 0), Wide::Wide);
    assert_eq!(screen_codepoint(&s, 5, 0), '😀' as u32);
    assert_eq!(screen_wide(&s, 6, 0), Wide::SpacerTail);
}

// Port of `test "Screen: resize more cols requiring a wide spacer head"`.
#[test]
fn resize_more_cols_requiring_a_wide_spacer_head() {
    use crate::page::Wide;
    let mut s = init(2, 2, 0);
    let str = "xx😀";
    s.test_write_string(str);
    assert_eq!(s.dump_string(Tag::Screen, false), "xx\n😀");
    assert_eq!(screen_wide(&s, 0, 1), Wide::Wide);
    assert_eq!(screen_wide(&s, 1, 1), Wide::SpacerTail);
    resize(&mut s, 3, 2, true);
    assert_eq!(s.dump_string(Tag::Screen, false), "xx\n😀");
    assert_eq!(screen_wide(&s, 2, 0), Wide::SpacerHead);
    assert_eq!(screen_wide(&s, 0, 1), Wide::Wide);
    assert_eq!(screen_codepoint(&s, 0, 1), '😀' as u32);
    assert_eq!(screen_wide(&s, 1, 1), Wide::SpacerTail);
}

// ---- resize with prompt redraw ------------------------------------------

// Port of `test "Screen: resize more cols with cursor at prompt"`.
#[test]
fn resize_more_cols_with_cursor_at_prompt() {
    let mut s = init(10, 3, 5);
    s.test_write_string("ABCDE\n");
    s.cursor_set_semantic_content(SemanticContentSet::Prompt(PromptKind::Initial));
    s.test_write_string("> ");
    s.cursor_set_semantic_content(SemanticContentSet::Input { clear_eol: true });
    s.test_write_string("echo");
    assert_eq!(s.dump_string(Tag::Viewport, false), "ABCDE\n> echo");
    s.resize(Resize {
        cols: 20,
        rows: 3,
        reflow: true,
        prompt_redraw: Redraw::True,
    });
    assert_eq!(s.cursor.x, 6);
    assert_eq!(s.cursor.y, 1);
    assert_eq!(s.dump_string(Tag::Viewport, false), "ABCDE");
}

// Port of `test "Screen: resize more cols with cursor not at prompt"`.
#[test]
fn resize_more_cols_with_cursor_not_at_prompt() {
    let mut s = init(10, 3, 5);
    s.test_write_string("ABCDE\n");
    s.cursor_set_semantic_content(SemanticContentSet::Prompt(PromptKind::Initial));
    s.test_write_string("> ");
    s.cursor_set_semantic_content(SemanticContentSet::Input { clear_eol: true });
    s.test_write_string("echo\n");
    s.test_write_string("output");
    assert_eq!(s.dump_string(Tag::Viewport, false), "ABCDE\n> echo\noutput");
    s.resize(Resize {
        cols: 20,
        rows: 3,
        reflow: true,
        prompt_redraw: Redraw::True,
    });
    assert_eq!(s.cursor.x, 6);
    assert_eq!(s.cursor.y, 2);
    assert_eq!(s.dump_string(Tag::Viewport, false), "ABCDE\n> echo\noutput");
}

// Port of `test "Screen: resize with prompt_redraw last clears only one line"`.
#[test]
fn resize_with_prompt_redraw_last_clears_only_one_line() {
    let mut s = init(10, 4, 5);
    s.test_write_string("ABCDE\n");
    s.cursor_set_semantic_content(SemanticContentSet::Prompt(PromptKind::Initial));
    s.test_write_string("> ");
    s.cursor_set_semantic_content(SemanticContentSet::Input { clear_eol: false });
    s.test_write_string("hello\n");
    s.test_write_string("world");
    assert_eq!(s.dump_string(Tag::Viewport, false), "ABCDE\n> hello\nworld");
    s.resize(Resize {
        cols: 20,
        rows: 4,
        reflow: true,
        prompt_redraw: Redraw::Last,
    });
    assert_eq!(s.dump_string(Tag::Viewport, false), "ABCDE\n> hello");
}

// Port of `test "Screen: resize with prompt_redraw last multiline prompt clears only last line"`.
#[test]
fn resize_with_prompt_redraw_last_multiline_prompt_clears_only_last_line() {
    let mut s = init(20, 5, 5);
    s.cursor_set_semantic_content(SemanticContentSet::Prompt(PromptKind::Initial));
    s.test_write_string("line1\n");
    s.cursor_set_semantic_content(SemanticContentSet::Prompt(PromptKind::Continuation));
    s.test_write_string("line2\n");
    s.cursor_set_semantic_content(SemanticContentSet::Prompt(PromptKind::Continuation));
    s.test_write_string("line3");
    assert_eq!(s.dump_string(Tag::Viewport, false), "line1\nline2\nline3");
    s.resize(Resize {
        cols: 30,
        rows: 5,
        reflow: true,
        prompt_redraw: Redraw::Last,
    });
    assert_eq!(s.dump_string(Tag::Viewport, false), "line1\nline2");
}
