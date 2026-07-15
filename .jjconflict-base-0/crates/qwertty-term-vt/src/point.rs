//! Terminal coordinate points (port of `src/terminal/point.zig`, commit `2da015cd6`).
//!
//! A [`Point`] is an x/y coordinate paired with a [`Tag`] that says *which*
//! coordinate space the point lives in: the editable active area, the visible
//! viewport, the full screen (scrollback + active), or just the history.
//! `PageList` resolves points into pins against those reference frames.

use crate::page::size::CellCountInt;

/// The reference frame a [`Point`] is measured against.
///
/// See `point.zig:12-50` for the full semantics. In short:
/// - `Active`: the editable bottom rows a running program can address. Its
///   bottom-right spans the *full* row height (including unwritten rows).
/// - `Viewport`: the currently visible region (moves as the user scrolls).
/// - `Screen`: from the furthest-back scrollback row to the last written row.
/// - `History`: like `Screen` but bounded above by the row just before active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tag {
    Active,
    Viewport,
    Screen,
    History,
}

/// An x/y coordinate. `x` fits in [`CellCountInt`] (never more than a page's
/// column count); `y` is a `u32` because screen/history can span more rows than
/// fit in a single page (`point.zig:91-104`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Coordinate {
    pub x: CellCountInt,
    pub y: u32,
}

impl Coordinate {
    pub fn new(x: CellCountInt, y: u32) -> Self {
        Self { x, y }
    }
}

/// A tagged coordinate. Port of the `point.Point` tagged union.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub tag: Tag,
    pub coord: Coordinate,
}

impl Point {
    pub fn new(tag: Tag, coord: Coordinate) -> Self {
        Self { tag, coord }
    }

    /// `.active = .{}` — active-area origin.
    pub fn active(x: CellCountInt, y: u32) -> Self {
        Self::new(Tag::Active, Coordinate::new(x, y))
    }

    pub fn viewport(x: CellCountInt, y: u32) -> Self {
        Self::new(Tag::Viewport, Coordinate::new(x, y))
    }

    pub fn screen(x: CellCountInt, y: u32) -> Self {
        Self::new(Tag::Screen, Coordinate::new(x, y))
    }

    pub fn history(x: CellCountInt, y: u32) -> Self {
        Self::new(Tag::History, Coordinate::new(x, y))
    }

    /// The `x`/`y` coordinate regardless of tag.
    pub fn coord(self) -> Coordinate {
        self.coord
    }
}
