//! A single highlight region on the screen (port of `src/terminal/Selection.zig`,
//! commit `2da015cd6`).
//!
//! See `docs/analysis/selection.md` for the maintainer-grade map. A [`Selection`]
//! is a pair of pins (start/end, in any order) plus a `rectangle` flag. The pins
//! are either untracked (valid only until the next screen mutation) or tracked
//! (kept valid by [`PageList`]'s pin-fixup machinery, at the cost of an allocation
//! and a fixup walk per mutating op). All ordering (`topLeft`/`bottomRight`/
//! containment) is recovered lazily from screen points via [`Selection::order`].
//!
//! `SelectionGesture.zig` (input-phase drag/click state) is out of scope — it is
//! frontend territory above `Screen`; the engine only exposes the value and the
//! query/adjust primitives it drives.

use crate::page::Cell;
use crate::pagelist::{Direction, PageList, Pin};
use crate::point::Tag;

/// The bounds of a selection. Port of `Selection.Bounds`.
///
/// In all cases `start`/`end` can be in any order (a backwards drag makes `start`
/// after `end`); use the struct functions rather than assuming order.
#[derive(Debug, Clone, Copy)]
enum Bounds {
    /// Plain pins, valid only until the next screen mutation.
    Untracked { start: Pin, end: Pin },
    /// Pins vended by `PageList::track_pin`, kept valid across mutations.
    Tracked { start: *mut Pin, end: *mut Pin },
}

/// The order of a selection. Port of `Selection.Order`.
///
/// - `Forward`: start is before end (top-left → bottom-right).
/// - `Reverse`: end is before start (bottom-right → top-left).
/// - `MirroredForward`/`MirroredReverse`: rectangle-only. A rectangle orientation
///   flips a single axis, so top-right→bottom-left and bottom-left→top-right are
///   *mirrored* rather than plain forward/reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Forward,
    Reverse,
    MirroredForward,
    MirroredReverse,
}

/// Possible adjustments to a selection. Port of `Selection.Adjustment`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adjustment {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    BeginningOfLine,
    EndOfLine,
}

/// A single highlight region. Port of `Selection`.
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    bounds: Bounds,
    /// Whether this selection is a rectangle rather than whole lines. In this
    /// mode start/end are the top-left and bottom-right (or vice versa if
    /// backwards). Port of `Selection.rectangle`.
    pub rectangle: bool,
}

impl Selection {
    /// A new **untracked** selection. Port of `init`.
    pub fn init(start_pin: Pin, end_pin: Pin, rect: bool) -> Selection {
        Selection {
            bounds: Bounds::Untracked {
                start: start_pin,
                end: end_pin,
            },
            rectangle: rect,
        }
    }

    /// Untrack this selection's pins if it is tracked. Port of `deinit`.
    pub fn deinit(&self, pages: &mut PageList) {
        if let Bounds::Tracked { start, end } = self.bounds {
            pages.untrack_pin(start);
            pages.untrack_pin(end);
        }
    }

    /// True if this is a tracked selection. Port of `tracked`.
    pub fn tracked(&self) -> bool {
        matches!(self.bounds, Bounds::Tracked { .. })
    }

    /// Convert an untracked selection into a tracked one. Asserts untracked.
    /// Port of `track`.
    ///
    /// The Zig version threads `Allocator.Error`; the Rust pin model is
    /// infallible-alloc (matching the PageList port), so this cannot fail.
    pub fn track(&self, pages: &mut PageList) -> Selection {
        debug_assert!(!self.tracked());
        let (start_pin, end_pin) = match self.bounds {
            Bounds::Untracked { start, end } => (start, end),
            Bounds::Tracked { .. } => unreachable!("track called on tracked selection"),
        };
        let tracked_start = pages.track_pin(start_pin);
        let tracked_end = pages.track_pin(end_pin);
        Selection {
            bounds: Bounds::Tracked {
                start: tracked_start,
                end: tracked_end,
            },
            rectangle: self.rectangle,
        }
    }

    /// The starting pin (NOT ordered). Port of `start`.
    pub fn start(&self) -> Pin {
        match self.bounds {
            Bounds::Untracked { start, .. } => start,
            // SAFETY: tracked pin pointers are live for the selection's lifetime.
            Bounds::Tracked { start, .. } => unsafe { *start },
        }
    }

    /// The ending pin (NOT ordered). Port of `end`.
    pub fn end(&self) -> Pin {
        match self.bounds {
            Bounds::Untracked { end, .. } => end,
            // SAFETY: tracked pin pointers are live for the selection's lifetime.
            Bounds::Tracked { end, .. } => unsafe { *end },
        }
    }

    /// The tracked start pin pointer, if tracked. Used by `Screen::clone`.
    pub(crate) fn tracked_start(&self) -> Option<*mut Pin> {
        match self.bounds {
            Bounds::Tracked { start, .. } => Some(start),
            Bounds::Untracked { .. } => None,
        }
    }

    /// The tracked end pin pointer, if tracked. Used by `Screen::clone`.
    pub(crate) fn tracked_end(&self) -> Option<*mut Pin> {
        match self.bounds {
            Bounds::Tracked { end, .. } => Some(end),
            Bounds::Untracked { .. } => None,
        }
    }

    /// Build a tracked selection from two already-tracked pin pointers. Used by
    /// `Screen::clone`, which remaps the pins into the cloned pagelist itself.
    pub(crate) fn from_tracked(start: *mut Pin, end: *mut Pin, rectangle: bool) -> Selection {
        Selection {
            bounds: Bounds::Tracked { start, end },
            rectangle,
        }
    }

    /// Selection equality. Port of `eql`.
    pub fn eql(&self, other: &Selection) -> bool {
        self.start().eql(other.start())
            && self.end().eql(other.end())
            && self.rectangle == other.rectangle
    }

    /// The order of the selection. Port of `order`.
    pub fn order(&self, pages: &PageList) -> Order {
        let start_pt = pages
            .point_from_pin(Tag::Screen, self.start())
            .expect("selection start pin not on screen")
            .coord;
        let end_pt = pages
            .point_from_pin(Tag::Screen, self.end())
            .expect("selection end pin not on screen")
            .coord;

        if self.rectangle {
            // Reverse (also handles single-column)
            if start_pt.y > end_pt.y && start_pt.x >= end_pt.x {
                return Order::Reverse;
            }
            if start_pt.y >= end_pt.y && start_pt.x > end_pt.x {
                return Order::Reverse;
            }
            // Mirror, bottom-left to top-right
            if start_pt.y > end_pt.y && start_pt.x < end_pt.x {
                return Order::MirroredReverse;
            }
            // Mirror, top-right to bottom-left
            if start_pt.y < end_pt.y && start_pt.x > end_pt.x {
                return Order::MirroredForward;
            }
            // Forward
            return Order::Forward;
        }

        if start_pt.y < end_pt.y {
            return Order::Forward;
        }
        if start_pt.y > end_pt.y {
            return Order::Reverse;
        }
        if start_pt.x <= end_pt.x {
            return Order::Forward;
        }
        Order::Reverse
    }

    /// The top-left pin of the selection. Port of `topLeft`.
    pub fn top_left(&self, pages: &PageList) -> Pin {
        match self.order(pages) {
            Order::Forward => self.start(),
            Order::Reverse => self.end(),
            Order::MirroredForward => {
                let mut p = self.start();
                p.x = self.end().x();
                p
            }
            Order::MirroredReverse => {
                let mut p = self.end();
                p.x = self.start().x();
                p
            }
        }
    }

    /// The bottom-right pin of the selection. Port of `bottomRight`.
    pub fn bottom_right(&self, pages: &PageList) -> Pin {
        match self.order(pages) {
            Order::Forward => self.end(),
            Order::Reverse => self.start(),
            Order::MirroredForward => {
                let mut p = self.end();
                p.x = self.start().x();
                p
            }
            Order::MirroredReverse => {
                let mut p = self.start();
                p.x = self.end().x();
                p
            }
        }
    }

    /// Return the selection reordered as `desired` (a new untracked selection).
    /// Only `Forward`/`Reverse` are useful; any other desired order acts as
    /// forward. Port of `ordered`.
    pub fn ordered(&self, pages: &PageList, desired: Order) -> Selection {
        if self.order(pages) == desired {
            return Selection::init(self.start(), self.end(), self.rectangle);
        }
        let tl = self.top_left(pages);
        let br = self.bottom_right(pages);
        match desired {
            Order::Reverse => Selection::init(br, tl, self.rectangle),
            // forward and all mirrored/other orders act as forward
            _ => Selection::init(tl, br, self.rectangle),
        }
    }

    /// True if the selection contains `pin`. Port of `contains`.
    pub fn contains(&self, pages: &PageList, pin: Pin) -> bool {
        let tl_pin = self.top_left(pages);
        let br_pin = self.bottom_right(pages);

        let tl = pages.point_from_pin(Tag::Screen, tl_pin).unwrap().coord;
        let br = pages.point_from_pin(Tag::Screen, br_pin).unwrap().coord;
        let p = pages.point_from_pin(Tag::Screen, pin).unwrap().coord;

        if self.rectangle {
            return p.y >= tl.y && p.y <= br.y && p.x >= tl.x && p.x <= br.x;
        }

        // Same line
        if tl.y == br.y {
            return p.y == tl.y && p.x >= tl.x && p.x <= br.x;
        }
        // Top line: left of X
        if p.y == tl.y {
            return p.x >= tl.x;
        }
        // Bottom line: right of X
        if p.y == br.y {
            return p.x <= br.x;
        }
        // Between: always good
        p.y > tl.y && p.y < br.y
    }

    /// The single-row sub-selection for `pin`'s row, or `None` if that row is
    /// outside the selection. Port of `containedRow`.
    pub fn contained_row(&self, pages: &PageList, pin: Pin) -> Option<Selection> {
        let tl_pin = self.top_left(pages);
        let br_pin = self.bottom_right(pages);
        let tl = pages.point_from_pin(Tag::Screen, tl_pin).unwrap().coord;
        let br = pages.point_from_pin(Tag::Screen, br_pin).unwrap().coord;
        let p = pages.point_from_pin(Tag::Screen, pin).unwrap().coord;
        self.contained_row_cached(pages, tl_pin, br_pin, pin, tl, br, p)
    }

    /// Same as `contained_row` but with pre-computed pins/points cached across
    /// calls. Port of `containedRowCached`.
    #[allow(clippy::too_many_arguments)]
    pub fn contained_row_cached(
        &self,
        pages: &PageList,
        tl_pin: Pin,
        br_pin: Pin,
        pin: Pin,
        tl: crate::point::Coordinate,
        br: crate::point::Coordinate,
        p: crate::point::Coordinate,
    ) -> Option<Selection> {
        if p.y < tl.y || p.y > br.y {
            return None;
        }

        // Rectangle: the x range is always the same for a contained row.
        if self.rectangle {
            let mut start = pin;
            start.x = tl.x;
            let mut end = pin;
            end.x = br.x;
            return Some(Selection::init(start, end, true));
        }

        let cols = pages.cols();

        if p.y == tl.y {
            // If the selection is JUST this line, return it as-is.
            if p.y == br.y {
                return Some(Selection::init(tl_pin, br_pin, false));
            }
            // Selection top-left line matches only.
            let mut end = pin;
            end.x = cols - 1;
            return Some(Selection::init(tl_pin, end, false));
        }

        // Bottom selection row (selection is multi-line by the above).
        if p.y == br.y {
            debug_assert!(p.y != tl.y);
            let mut start = pin;
            start.x = 0;
            return Some(Selection::init(start, br_pin, false));
        }

        // A middle row: the full line.
        let mut start = pin;
        start.x = 0;
        let mut end = pin;
        end.x = cols - 1;
        Some(Selection::init(start, end, false))
    }

    /// Adjust the selection by `adjustment`. Always moves the `end` pin (end is
    /// the last mouse point, so up/down drags both behave). Port of `adjust`.
    pub fn adjust(&mut self, pages: &PageList, adjustment: Adjustment) {
        match adjustment {
            Adjustment::Up => {
                // SAFETY: pins' nodes are live.
                match unsafe { self.end().up(1) } {
                    Some(new_end) => self.set_end(new_end),
                    None => self.adjust(pages, Adjustment::BeginningOfLine),
                }
            }

            Adjustment::Down => {
                // Find the next non-blank row.
                let mut current = self.end();
                let mut found = false;
                // SAFETY: pins' nodes are live.
                unsafe {
                    while let Some(next) = current.down(1) {
                        let (row, _) = next.row_and_cell();
                        let cells = &*(*next.node).data.get_cells(row);
                        if Cell::has_text_any(cells) {
                            self.set_end(next);
                            found = true;
                            break;
                        }
                        current = next;
                    }
                }
                if !found {
                    self.adjust(pages, Adjustment::EndOfLine);
                }
            }

            Adjustment::Left => {
                // SAFETY: pins' nodes are live.
                unsafe {
                    let mut it = self.end().cell_iterator(Direction::LeftUp, None);
                    let _ = it.next(); // skip self
                    while let Some(next) = it.next() {
                        let (_, cell) = next.row_and_cell();
                        if (*cell).has_text() {
                            self.set_end(next);
                            break;
                        }
                    }
                }
            }

            Adjustment::Right => {
                // SAFETY: pins' nodes are live.
                unsafe {
                    let mut it = self.end().cell_iterator(Direction::RightDown, None);
                    let _ = it.next(); // skip self
                    while let Some(next) = it.next() {
                        let (_, cell) = next.row_and_cell();
                        if (*cell).has_text() {
                            self.set_end(next);
                            break;
                        }
                    }
                }
            }

            Adjustment::PageUp => {
                // SAFETY: pins' nodes are live.
                match unsafe { self.end().up(pages.rows() as usize) } {
                    Some(new_end) => self.set_end(new_end),
                    None => self.adjust(pages, Adjustment::Home),
                }
            }

            Adjustment::PageDown => {
                // SAFETY: pins' nodes are live.
                match unsafe { self.end().down(pages.rows() as usize) } {
                    Some(new_end) => self.set_end(new_end),
                    None => self.adjust(pages, Adjustment::End),
                }
            }

            Adjustment::Home => {
                let new_end = pages
                    .pin(crate::point::Point::screen(0, 0))
                    .expect("screen origin pin");
                self.set_end(new_end);
            }

            Adjustment::End => {
                let mut it =
                    pages.row_iterator(Direction::LeftUp, crate::point::Point::screen(0, 0), None);
                // SAFETY: iterator yields valid pins.
                unsafe {
                    while let Some(next) = it.next() {
                        let (row, _) = next.row_and_cell();
                        let cells = &*(*next.node).data.get_cells(row);
                        if Cell::has_text_any(cells) {
                            let mut e = next;
                            e.x = (cells.len() - 1) as crate::page::size::CellCountInt;
                            self.set_end(e);
                            break;
                        }
                    }
                }
            }

            Adjustment::BeginningOfLine => {
                let mut e = self.end();
                e.x = 0;
                self.set_end(e);
            }

            Adjustment::EndOfLine => {
                let mut e = self.end();
                // SAFETY: pin node live.
                let cols = unsafe { (*e.node).data.size.cols };
                e.x = cols - 1;
                self.set_end(e);
            }
        }
    }

    /// Set the `end` pin, writing through the tracked pointer when tracked.
    fn set_end(&mut self, new_end: Pin) {
        match self.bounds {
            Bounds::Untracked { ref mut end, .. } => *end = new_end,
            // SAFETY: tracked pin pointer is live.
            Bounds::Tracked { end, .. } => unsafe { *end = new_end },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::size::CellCountInt;
    use crate::point::Point;
    use crate::screen::{Options, Screen};

    fn init(cols: CellCountInt, rows: CellCountInt, max_scrollback: usize) -> Screen {
        Screen::init(Options {
            cols,
            rows,
            max_scrollback,
        })
    }

    fn pin(s: &Screen, pt: Point) -> Pin {
        s.pages.pin(pt).unwrap()
    }

    /// Screen (x, y) of a pin.
    fn screen_pt(s: &Screen, p: Pin) -> (CellCountInt, u32) {
        let c = s.pages.point_from_pin(Tag::Screen, p).unwrap().coord;
        (c.x, c.y)
    }

    // ---- adjust ----------------------------------------------------------

    // Port of `test "Selection: adjust right"`.
    #[test]
    fn adjust_right() {
        let mut s = init(10, 10, 0);
        s.test_write_string("A1234\nB5678\nC1234\nD5678");

        // Simple movement right
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(3, 3)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::Right);
            assert_eq!(screen_pt(&s, sel.start()), (5, 1));
            assert_eq!(screen_pt(&s, sel.end()), (4, 3));
        }

        // Already at end of the line.
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(4, 1)),
                pin(&s, Point::screen(4, 2)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::Right);
            assert_eq!(screen_pt(&s, sel.start()), (4, 1));
            assert_eq!(screen_pt(&s, sel.end()), (0, 3));
        }

        // Already at end of the screen
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(4, 3)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::Right);
            assert_eq!(screen_pt(&s, sel.start()), (5, 1));
            assert_eq!(screen_pt(&s, sel.end()), (4, 3));
        }
    }

    // Port of `test "Selection: adjust left"`.
    #[test]
    fn adjust_left() {
        let mut s = init(10, 10, 0);
        s.test_write_string("A1234\nB5678\nC1234\nD5678");

        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(3, 3)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::Left);
            assert_eq!(screen_pt(&s, sel.start()), (5, 1));
            assert_eq!(screen_pt(&s, sel.end()), (2, 3));
        }

        // Already at beginning of the line.
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(0, 3)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::Left);
            assert_eq!(screen_pt(&s, sel.start()), (5, 1));
            assert_eq!(screen_pt(&s, sel.end()), (4, 2));
        }
    }

    // Port of `test "Selection: adjust left skips blanks"`.
    #[test]
    fn adjust_left_skips_blanks() {
        let mut s = init(10, 10, 0);
        s.test_write_string("A1234\nB5678\nC12\nD56");

        // Same line
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(4, 3)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::Left);
            assert_eq!(screen_pt(&s, sel.start()), (5, 1));
            assert_eq!(screen_pt(&s, sel.end()), (2, 3));
        }

        // Edge
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(0, 3)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::Left);
            assert_eq!(screen_pt(&s, sel.start()), (5, 1));
            assert_eq!(screen_pt(&s, sel.end()), (2, 2));
        }
    }

    // Port of `test "Selection: adjust up"`.
    #[test]
    fn adjust_up() {
        let mut s = init(10, 10, 0);
        s.test_write_string("A\nB\nC\nD\nE");

        // Not on the first line
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(3, 3)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::Up);
            assert_eq!(screen_pt(&s, sel.start()), (5, 1));
            assert_eq!(screen_pt(&s, sel.end()), (3, 2));
        }

        // On the first line
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(3, 0)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::Up);
            assert_eq!(screen_pt(&s, sel.start()), (5, 1));
            assert_eq!(screen_pt(&s, sel.end()), (0, 0));
        }
    }

    // Port of `test "Selection: adjust down"`.
    #[test]
    fn adjust_down() {
        let mut s = init(10, 10, 0);
        s.test_write_string("A\nB\nC\nD\nE");

        // Not on the first line
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(3, 3)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::Down);
            assert_eq!(screen_pt(&s, sel.start()), (5, 1));
            assert_eq!(screen_pt(&s, sel.end()), (3, 4));
        }

        // On the last line
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(4, 1)),
                pin(&s, Point::screen(3, 4)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::Down);
            assert_eq!(screen_pt(&s, sel.start()), (4, 1));
            assert_eq!(screen_pt(&s, sel.end()), (9, 4));
        }
    }

    // Port of `test "Selection: adjust down with not full screen"`.
    #[test]
    fn adjust_down_not_full_screen() {
        let mut s = init(10, 10, 0);
        s.test_write_string("A\nB\nC");

        let mut sel = Selection::init(
            pin(&s, Point::screen(4, 1)),
            pin(&s, Point::screen(3, 2)),
            false,
        );
        sel.adjust(&s.pages, Adjustment::Down);
        assert_eq!(screen_pt(&s, sel.start()), (4, 1));
        assert_eq!(screen_pt(&s, sel.end()), (9, 2));
    }

    // Port of `test "Selection: adjust home"`.
    #[test]
    fn adjust_home() {
        let mut s = init(10, 10, 0);
        s.test_write_string("A\nB\nC");

        let mut sel = Selection::init(
            pin(&s, Point::screen(4, 1)),
            pin(&s, Point::screen(1, 2)),
            false,
        );
        sel.adjust(&s.pages, Adjustment::Home);
        assert_eq!(screen_pt(&s, sel.start()), (4, 1));
        assert_eq!(screen_pt(&s, sel.end()), (0, 0));
    }

    // Port of `test "Selection: adjust end with not full screen"`.
    #[test]
    fn adjust_end_not_full_screen() {
        let mut s = init(10, 10, 0);
        s.test_write_string("A\nB\nC");

        let mut sel = Selection::init(
            pin(&s, Point::screen(4, 0)),
            pin(&s, Point::screen(1, 1)),
            false,
        );
        sel.adjust(&s.pages, Adjustment::End);
        assert_eq!(screen_pt(&s, sel.start()), (4, 0));
        assert_eq!(screen_pt(&s, sel.end()), (9, 2));
    }

    // Port of `test "Selection: adjust beginning of line"`.
    #[test]
    fn adjust_beginning_of_line() {
        let mut s = init(8, 10, 0);
        s.test_write_string("A12 B34\nC12 D34");

        // Not at beginning of the line
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(5, 1)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::BeginningOfLine);
            assert_eq!(screen_pt(&s, sel.start()), (5, 1));
            assert_eq!(screen_pt(&s, sel.end()), (0, 1));
        }

        // Already at beginning of the line
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(0, 1)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::BeginningOfLine);
            assert_eq!(screen_pt(&s, sel.start()), (5, 1));
            assert_eq!(screen_pt(&s, sel.end()), (0, 1));
        }

        // End pin moves to start pin
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(0, 1)),
                pin(&s, Point::screen(5, 1)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::BeginningOfLine);
            assert_eq!(screen_pt(&s, sel.start()), (0, 1));
            assert_eq!(screen_pt(&s, sel.end()), (0, 1));
        }
    }

    // Port of `test "Selection: adjust end of line"`.
    #[test]
    fn adjust_end_of_line() {
        let mut s = init(8, 10, 0);
        s.test_write_string("A12 B34\nC12 D34");

        // Not at end of the line
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(1, 0)),
                pin(&s, Point::screen(1, 0)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::EndOfLine);
            assert_eq!(screen_pt(&s, sel.start()), (1, 0));
            assert_eq!(screen_pt(&s, sel.end()), (7, 0));
        }

        // Already at end of the line
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(1, 0)),
                pin(&s, Point::screen(7, 0)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::EndOfLine);
            assert_eq!(screen_pt(&s, sel.start()), (1, 0));
            assert_eq!(screen_pt(&s, sel.end()), (7, 0));
        }

        // End pin moves to start pin
        {
            let mut sel = Selection::init(
                pin(&s, Point::screen(7, 0)),
                pin(&s, Point::screen(1, 0)),
                false,
            );
            sel.adjust(&s.pages, Adjustment::EndOfLine);
            assert_eq!(screen_pt(&s, sel.start()), (7, 0));
            assert_eq!(screen_pt(&s, sel.end()), (7, 0));
        }
    }

    // ---- order / topLeft / bottomRight / ordered -------------------------

    // Port of `test "Selection: order, standard"`.
    #[test]
    fn order_standard() {
        let s = init(100, 100, 1);

        // forward, multi-line
        let sel = Selection::init(
            pin(&s, Point::screen(2, 1)),
            pin(&s, Point::screen(2, 2)),
            false,
        );
        assert_eq!(sel.order(&s.pages), Order::Forward);

        // reverse, multi-line
        let sel = Selection::init(
            pin(&s, Point::screen(2, 2)),
            pin(&s, Point::screen(2, 1)),
            false,
        );
        assert_eq!(sel.order(&s.pages), Order::Reverse);

        // forward, same-line
        let sel = Selection::init(
            pin(&s, Point::screen(2, 1)),
            pin(&s, Point::screen(3, 1)),
            false,
        );
        assert_eq!(sel.order(&s.pages), Order::Forward);

        // forward, single char
        let sel = Selection::init(
            pin(&s, Point::screen(2, 1)),
            pin(&s, Point::screen(2, 1)),
            false,
        );
        assert_eq!(sel.order(&s.pages), Order::Forward);

        // reverse, single line
        let sel = Selection::init(
            pin(&s, Point::screen(2, 1)),
            pin(&s, Point::screen(1, 1)),
            false,
        );
        assert_eq!(sel.order(&s.pages), Order::Reverse);
    }

    // Port of `test "Selection: order, rectangle"`.
    #[test]
    fn order_rectangle() {
        let s = init(100, 100, 1);

        // forward (TL -> BR)
        let sel = Selection::init(
            pin(&s, Point::screen(1, 1)),
            pin(&s, Point::screen(2, 2)),
            true,
        );
        assert_eq!(sel.order(&s.pages), Order::Forward);

        // reverse (BR -> TL)
        let sel = Selection::init(
            pin(&s, Point::screen(2, 2)),
            pin(&s, Point::screen(1, 1)),
            true,
        );
        assert_eq!(sel.order(&s.pages), Order::Reverse);

        // mirrored_forward (TR -> BL)
        let sel = Selection::init(
            pin(&s, Point::screen(3, 1)),
            pin(&s, Point::screen(1, 3)),
            true,
        );
        assert_eq!(sel.order(&s.pages), Order::MirroredForward);

        // mirrored_reverse (BL -> TR)
        let sel = Selection::init(
            pin(&s, Point::screen(1, 3)),
            pin(&s, Point::screen(3, 1)),
            true,
        );
        assert_eq!(sel.order(&s.pages), Order::MirroredReverse);

        // forward, single line (left -> right)
        let sel = Selection::init(
            pin(&s, Point::screen(1, 1)),
            pin(&s, Point::screen(3, 1)),
            true,
        );
        assert_eq!(sel.order(&s.pages), Order::Forward);

        // reverse, single line (right -> left)
        let sel = Selection::init(
            pin(&s, Point::screen(3, 1)),
            pin(&s, Point::screen(1, 1)),
            true,
        );
        assert_eq!(sel.order(&s.pages), Order::Reverse);

        // forward, single column (top -> bottom)
        let sel = Selection::init(
            pin(&s, Point::screen(2, 1)),
            pin(&s, Point::screen(2, 3)),
            true,
        );
        assert_eq!(sel.order(&s.pages), Order::Forward);

        // reverse, single column (bottom -> top)
        let sel = Selection::init(
            pin(&s, Point::screen(2, 3)),
            pin(&s, Point::screen(2, 1)),
            true,
        );
        assert_eq!(sel.order(&s.pages), Order::Reverse);

        // forward, single cell
        let sel = Selection::init(
            pin(&s, Point::screen(1, 1)),
            pin(&s, Point::screen(1, 1)),
            true,
        );
        assert_eq!(sel.order(&s.pages), Order::Forward);
    }

    // Port of `test "topLeft"`.
    #[test]
    fn top_left() {
        let s = init(10, 10, 0);

        // forward
        let sel = Selection::init(
            pin(&s, Point::screen(1, 1)),
            pin(&s, Point::screen(3, 1)),
            true,
        );
        assert_eq!(screen_pt(&s, sel.top_left(&s.pages)), (1, 1));

        // reverse
        let sel = Selection::init(
            pin(&s, Point::screen(3, 1)),
            pin(&s, Point::screen(1, 1)),
            true,
        );
        assert_eq!(screen_pt(&s, sel.top_left(&s.pages)), (1, 1));

        // mirrored_forward
        let sel = Selection::init(
            pin(&s, Point::screen(3, 1)),
            pin(&s, Point::screen(1, 3)),
            true,
        );
        assert_eq!(screen_pt(&s, sel.top_left(&s.pages)), (1, 1));

        // mirrored_reverse
        let sel = Selection::init(
            pin(&s, Point::screen(1, 3)),
            pin(&s, Point::screen(3, 1)),
            true,
        );
        assert_eq!(screen_pt(&s, sel.top_left(&s.pages)), (1, 1));
    }

    // Port of `test "bottomRight"`.
    #[test]
    fn bottom_right() {
        let s = init(10, 10, 0);

        // forward
        let sel = Selection::init(
            pin(&s, Point::screen(1, 1)),
            pin(&s, Point::screen(3, 1)),
            false,
        );
        assert_eq!(screen_pt(&s, sel.bottom_right(&s.pages)), (3, 1));

        // reverse
        let sel = Selection::init(
            pin(&s, Point::screen(3, 1)),
            pin(&s, Point::screen(1, 1)),
            false,
        );
        assert_eq!(screen_pt(&s, sel.bottom_right(&s.pages)), (3, 1));

        // mirrored_forward
        let sel = Selection::init(
            pin(&s, Point::screen(3, 1)),
            pin(&s, Point::screen(1, 3)),
            true,
        );
        assert_eq!(screen_pt(&s, sel.bottom_right(&s.pages)), (3, 3));

        // mirrored_reverse
        let sel = Selection::init(
            pin(&s, Point::screen(1, 3)),
            pin(&s, Point::screen(3, 1)),
            true,
        );
        assert_eq!(screen_pt(&s, sel.bottom_right(&s.pages)), (3, 3));
    }

    // Port of `test "ordered"`.
    #[test]
    fn ordered_test() {
        let s = init(10, 10, 0);

        // forward
        {
            let sel = Selection::init(
                pin(&s, Point::screen(1, 1)),
                pin(&s, Point::screen(3, 1)),
                false,
            );
            let sel_reverse = Selection::init(
                pin(&s, Point::screen(3, 1)),
                pin(&s, Point::screen(1, 1)),
                false,
            );
            assert!(sel.ordered(&s.pages, Order::Forward).eql(&sel));
            assert!(sel.ordered(&s.pages, Order::Reverse).eql(&sel_reverse));
            assert!(sel.ordered(&s.pages, Order::MirroredForward).eql(&sel));
        }

        // reverse
        {
            let sel = Selection::init(
                pin(&s, Point::screen(3, 1)),
                pin(&s, Point::screen(1, 1)),
                false,
            );
            let sel_forward = Selection::init(
                pin(&s, Point::screen(1, 1)),
                pin(&s, Point::screen(3, 1)),
                false,
            );
            assert!(sel.ordered(&s.pages, Order::Forward).eql(&sel_forward));
            assert!(sel.ordered(&s.pages, Order::Reverse).eql(&sel));
            assert!(
                sel.ordered(&s.pages, Order::MirroredForward)
                    .eql(&sel_forward)
            );
        }

        // mirrored_forward
        {
            let sel = Selection::init(
                pin(&s, Point::screen(3, 1)),
                pin(&s, Point::screen(1, 3)),
                true,
            );
            let sel_forward = Selection::init(
                pin(&s, Point::screen(1, 1)),
                pin(&s, Point::screen(3, 3)),
                true,
            );
            let sel_reverse = Selection::init(
                pin(&s, Point::screen(3, 3)),
                pin(&s, Point::screen(1, 1)),
                true,
            );
            assert!(sel.ordered(&s.pages, Order::Forward).eql(&sel_forward));
            assert!(sel.ordered(&s.pages, Order::Reverse).eql(&sel_reverse));
            assert!(
                sel.ordered(&s.pages, Order::MirroredReverse)
                    .eql(&sel_forward)
            );
        }

        // mirrored_reverse
        {
            let sel = Selection::init(
                pin(&s, Point::screen(1, 3)),
                pin(&s, Point::screen(3, 1)),
                true,
            );
            let sel_forward = Selection::init(
                pin(&s, Point::screen(1, 1)),
                pin(&s, Point::screen(3, 3)),
                true,
            );
            let sel_reverse = Selection::init(
                pin(&s, Point::screen(3, 3)),
                pin(&s, Point::screen(1, 1)),
                true,
            );
            assert!(sel.ordered(&s.pages, Order::Forward).eql(&sel_forward));
            assert!(sel.ordered(&s.pages, Order::Reverse).eql(&sel_reverse));
            assert!(
                sel.ordered(&s.pages, Order::MirroredForward)
                    .eql(&sel_forward)
            );
        }
    }

    // ---- contains / containedRow -----------------------------------------

    // Port of `test "Selection: contains"`.
    #[test]
    fn contains() {
        let s = init(10, 10, 0);

        {
            let sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(3, 2)),
                false,
            );
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(6, 1))));
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(1, 2))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(1, 1))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(5, 2))));
        }

        // Reverse
        {
            let sel = Selection::init(
                pin(&s, Point::screen(3, 2)),
                pin(&s, Point::screen(5, 1)),
                false,
            );
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(6, 1))));
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(1, 2))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(1, 1))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(5, 2))));
        }

        // Single line
        {
            let sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(8, 1)),
                false,
            );
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(6, 1))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(2, 1))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(9, 1))));
        }
    }

    // Port of `test "Selection: contains, rectangle"`.
    #[test]
    fn contains_rectangle() {
        let s = init(15, 15, 0);

        {
            let sel = Selection::init(
                pin(&s, Point::screen(3, 3)),
                pin(&s, Point::screen(7, 9)),
                true,
            );
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(5, 6)))); // Center
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(3, 6)))); // Left border
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(7, 6)))); // Right border
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(5, 3)))); // Top border
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(5, 9)))); // Bottom border

            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(5, 2))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(5, 10))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(2, 6))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(8, 6))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(8, 3))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(2, 9))));
        }

        // Reverse
        {
            let sel = Selection::init(
                pin(&s, Point::screen(7, 9)),
                pin(&s, Point::screen(3, 3)),
                true,
            );
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(5, 6))));
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(3, 6))));
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(7, 6))));
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(5, 3))));
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(5, 9))));

            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(5, 2))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(5, 10))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(2, 6))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(8, 6))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(8, 3))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(2, 9))));
        }

        // Single line
        {
            let sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(10, 1)),
                true,
            );
            assert!(sel.contains(&s.pages, pin(&s, Point::screen(6, 1))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(2, 1))));
            assert!(!sel.contains(&s.pages, pin(&s, Point::screen(12, 1))));
        }
    }

    // Port of `test "Selection: containedRow"`.
    #[test]
    fn contained_row() {
        let s = init(10, 5, 0);
        let cols = s.pages.cols();

        {
            let sel = Selection::init(
                pin(&s, Point::screen(5, 1)),
                pin(&s, Point::screen(3, 3)),
                false,
            );

            // Not contained
            assert!(
                sel.contained_row(&s.pages, pin(&s, Point::screen(1, 4)))
                    .is_none()
            );

            // Start line
            let r = sel
                .contained_row(&s.pages, pin(&s, Point::screen(1, 1)))
                .unwrap();
            assert_eq!(screen_pt(&s, r.start()), (5, 1));
            assert_eq!(screen_pt(&s, r.end()), (cols - 1, 1));

            // End line
            let r = sel
                .contained_row(&s.pages, pin(&s, Point::screen(2, 3)))
                .unwrap();
            assert_eq!(screen_pt(&s, r.start()), (0, 3));
            assert_eq!(screen_pt(&s, r.end()), (3, 3));

            // Middle line
            let r = sel
                .contained_row(&s.pages, pin(&s, Point::screen(2, 2)))
                .unwrap();
            assert_eq!(screen_pt(&s, r.start()), (0, 2));
            assert_eq!(screen_pt(&s, r.end()), (cols - 1, 2));
        }

        // Rectangle
        {
            let sel = Selection::init(
                pin(&s, Point::screen(3, 1)),
                pin(&s, Point::screen(6, 3)),
                true,
            );

            assert!(
                sel.contained_row(&s.pages, pin(&s, Point::screen(1, 4)))
                    .is_none()
            );

            let r = sel
                .contained_row(&s.pages, pin(&s, Point::screen(1, 1)))
                .unwrap();
            assert_eq!(screen_pt(&s, r.start()), (3, 1));
            assert_eq!(screen_pt(&s, r.end()), (6, 1));
            assert!(r.rectangle);

            let r = sel
                .contained_row(&s.pages, pin(&s, Point::screen(2, 3)))
                .unwrap();
            assert_eq!(screen_pt(&s, r.start()), (3, 3));
            assert_eq!(screen_pt(&s, r.end()), (6, 3));

            let r = sel
                .contained_row(&s.pages, pin(&s, Point::screen(2, 2)))
                .unwrap();
            assert_eq!(screen_pt(&s, r.start()), (3, 2));
            assert_eq!(screen_pt(&s, r.end()), (6, 2));
        }

        // Single-line selection
        {
            let sel = Selection::init(
                pin(&s, Point::screen(2, 1)),
                pin(&s, Point::screen(6, 1)),
                false,
            );

            assert!(
                sel.contained_row(&s.pages, pin(&s, Point::screen(1, 0)))
                    .is_none()
            );
            assert!(
                sel.contained_row(&s.pages, pin(&s, Point::screen(1, 2)))
                    .is_none()
            );

            let r = sel
                .contained_row(&s.pages, pin(&s, Point::screen(1, 1)))
                .unwrap();
            assert_eq!(screen_pt(&s, r.start()), (2, 1));
            assert_eq!(screen_pt(&s, r.end()), (6, 1));
        }
    }
}
